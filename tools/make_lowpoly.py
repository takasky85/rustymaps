#!/usr/bin/env python3
"""Generates the three low-poly, flat-shaded airliner models (assets/models/{b737,a380,b787}.bin).

The models are built from a handful of simple shapes (a lofted fuselage, swept wing / tail slabs,
engine nacelles, winglets) in real-world metres, so there are no photographed textures and no
licence to carry. Every face has its own vertices and normal, which is what gives the flat-shaded
look. Each is roughly 1.5k triangles.

Usage:  python3 tools/make_lowpoly.py <out-dir> [--preview <png-dir>]

Axes in the model: x = right (starboard), y = forward (nose), z = up; origin at the middle of the
fuselage. File layout (little endian): u32 vertex count, u32 index count, f32 bounding radius (m),
then vertices of 20 bytes (f32 x3 position, i8 x4 normal (snorm), u8 x4 colour, linear light) and
u32 indices.
"""
import math
import struct
import sys
import os

import numpy as np

# ---------------------------------------------------------------------------------------------
# tiny mesh toolkit


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def mid(pts):
    n = len(pts)
    return (sum(p[0] for p in pts) / n, sum(p[1] for p in pts) / n, sum(p[2] for p in pts) / n)


def srgb(hexv):
    """0xRRGGBB (what you would see) -> linear 0..1 triple (what the shader expects)."""
    return tuple(((hexv >> s) & 255) / 255.0 for s in (16, 8, 0))


class Mesh:
    def __init__(self):
        self.tris = []  # (p0, p1, p2, colour)

    def poly(self, pts, colour, inside):
        """A convex polygon, wound so that its normal points away from `inside`."""
        for i in range(1, len(pts) - 1):
            a, b, c = pts[0], pts[i], pts[i + 1]
            n = cross(sub(b, a), sub(c, a))
            if dot(n, n) < 1e-12:
                continue
            if dot(n, sub(mid([a, b, c]), inside)) < 0:
                b, c = c, b
            self.tris.append((a, b, c, colour))

    def hexa(self, bottom, top, colours, skip=()):
        """A hexahedron from a bottom quad and a top quad (top[i] sits above bottom[i]).
        `colours` = (bottom, top, sides). `skip` lists faces to leave out: 'b','t',0..3."""
        inside = mid(list(bottom) + list(top))
        cb, ct, cs = colours
        if 'b' not in skip:
            self.poly(list(bottom), cb, inside)
        if 't' not in skip:
            self.poly(list(top), ct, inside)
        for i in range(4):
            if i in skip:
                continue
            j = (i + 1) % 4
            self.poly([bottom[i], bottom[j], top[j], top[i]], cs, inside)

    def loft(self, rings, colour_fn, centre_fn, cap_first=None, cap_last=None):
        """Quads between consecutive rings (lists of points), flat shaded. colour_fn(ring_idx, seg)."""
        for r in range(len(rings) - 1):
            a, b = rings[r], rings[r + 1]
            n = len(a)
            for k in range(n):
                k2 = (k + 1) % n
                quad = [a[k], a[k2], b[k2], b[k]]
                # drop collapsed corners (a ring of radius 0 at the nose tip)
                pts = []
                for p in quad:
                    if not any(abs(p[0] - q[0]) + abs(p[1] - q[1]) + abs(p[2] - q[2]) < 1e-9 for q in pts):
                        pts.append(p)
                if len(pts) >= 3:
                    self.poly(pts, colour_fn(r, k), centre_fn(r))
        if cap_first is not None:
            self.poly(rings[0], cap_first, mid(rings[1]))
        if cap_last is not None:
            self.poly(rings[-1], cap_last, mid(rings[-2]))

    def mirrored(self):
        """Adds a copy mirrored in x (port side from starboard)."""
        self.tris += [((-a[0], a[1], a[2]), (-c[0], c[1], c[2]), (-b[0], b[1], b[2]), col) for a, b, c, col in self.tris]

    def write(self, path):
        n = len(self.tris)
        verts = np.zeros(n * 3, dtype=[('p', '<f4', 3), ('n', 'i1', 4), ('c', 'u1', 4)])
        for i, (a, b, c, col) in enumerate(self.tris):
            nrm = cross(sub(b, a), sub(c, a))
            ln = math.sqrt(dot(nrm, nrm)) or 1.0
            nrm = (nrm[0] / ln, nrm[1] / ln, nrm[2] / ln)
            lin = [int(round(255 * (ch ** 2.2))) for ch in col]
            for j, p in enumerate((a, b, c)):
                v = verts[i * 3 + j]
                v['p'] = p
                v['n'] = [int(round(nrm[0] * 127)), int(round(nrm[1] * 127)), int(round(nrm[2] * 127)), 0]
                v['c'] = lin + [255]
        idx = np.arange(n * 3, dtype='<u4')
        radius = float(np.linalg.norm(verts['p'].astype(float), axis=1).max())
        with open(path, 'wb') as f:
            f.write(struct.pack('<IIf', len(verts), len(idx), radius))
            f.write(verts.tobytes())
            f.write(idx.tobytes())
        return len(self.tris), radius


# ---------------------------------------------------------------------------------------------
# the aircraft

SIDES = 16


def aircraft(p):
    m = Mesh()  # centre-line parts (fuselage, fin)
    sb = Mesh()  # starboard parts, mirrored to port at the end
    L, D = p['length'], p['diameter']
    Dh = p.get('height', D)
    a, b = D / 2, Dh / 2
    lv = p['livery']
    y_nose = L / 2

    # ---- fuselage ---------------------------------------------------------------------------
    stations = [0.0, 0.012, 0.03, 0.055, 0.09, 0.16, 0.70, 0.78, 0.86, 0.93, 1.0]
    rings, info = [], []
    for s in stations:
        if s < 0.09:
            r = math.sqrt(max(0.0, 1.0 - (1.0 - s / 0.09) ** 2))
            zc = -0.12 * Dh * (1.0 - s / 0.09) ** 2
        elif s <= 0.70:
            r, zc = 1.0, 0.0
        else:
            t = (s - 0.70) / 0.30
            r = 1.0 - 0.88 * (t ** 1.5)
            zc = p.get('tail_lift', 0.40) * Dh * t * t
        y = y_nose - s * L
        ring = []
        for k in range(SIDES):
            phi = 2 * math.pi * k / SIDES  # 0 = top, clockwise seen from the front
            ring.append((a * r * math.sin(phi), y, zc + b * r * math.cos(phi)))
        rings.append(ring)
        info.append((s, zc))

    def fus_colour(ri, k):
        s = (stations[ri] + stations[ri + 1]) / 2
        phi = 2 * math.pi * (k + 0.5) / SIDES
        theta = math.degrees(min(phi, 2 * math.pi - phi))
        if s < 0.04 and theta < 75:
            return srgb(lv['cockpit'])
        if theta < 67.5:
            return srgb(lv['upper'])
        if theta < 90:
            return srgb(lv['cheat'])
        if theta < 135:
            return srgb(lv['lower'])
        return srgb(lv['belly'])

    m.loft(rings, fus_colour, lambda r: (0.0, rings[r][0][1], info[r][1]), cap_last=srgb(lv['upper']))

    # ---- wings ------------------------------------------------------------------------------
    w = p['wing']
    half = w['span'] / 2
    y_le = y_nose - w['root_le_s'] * L  # leading edge of the root chord
    z_w = -w['low'] * b
    sweep = math.tan(math.radians(w['sweep']))
    tipx = half
    x_root = a * 0.6  # buried in the fuselage
    dx = tipx - x_root
    dz = dx * math.tan(math.radians(w['dihedral']))
    top_c, bot_c = srgb(w['top']), srgb(w['bottom'])
    # quad order: root LE, root TE, tip TE, tip LE
    rl = (x_root, y_le, z_w)
    rt = (x_root, y_le - w['root_chord'], z_w)
    tl = (tipx, y_le - dx * sweep, z_w + dz)
    tt = (tipx, y_le - dx * sweep - w['tip_chord'], z_w + dz)

    def lower(pt, th):
        return (pt[0], pt[1], pt[2] - th)

    top = [rl, rt, tt, tl]
    bottom = [lower(rl, w['t_root']), lower(rt, w['t_root']), lower(tt, w['t_tip']), lower(tl, w['t_tip'])]
    # sides: i joins corner i and i+1: 0 = root chord (inside the fuselage, left out), 1 = trailing
    # edge, 2 = tip, 3 = leading edge
    sb.hexa(bottom, top, (bot_c, top_c, srgb(w['edge'])), skip=(0,))

    # winglet / wingtip fin
    wl = w.get('winglet')
    if wl:
        h, rake = wl['height'], wl['rake']
        x0 = tipx - 0.05
        top_le = tl[1] - rake
        pts = [
            (tl[1], tl[2]),
            (tt[1], tt[2]),
            (top_le - wl['top_chord'], tt[2] + h),
            (top_le, tl[2] + h),
        ]
        thick = 0.14
        bq = [(x0 - thick, y, z) for y, z in pts]
        tq = [(x0 + thick, y, z) for y, z in pts]
        sb.hexa(bq, tq, (srgb(wl['colour']),) * 3)

    # ---- engines ----------------------------------------------------------------------------
    e = p['engine']
    for ex in e['x']:
        frac = (ex - x_root) / dx
        z_at = z_w + dz * frac
        y_front = y_le - (ex - x_root) * sweep + e['forward']
        ez = z_at - e['drop']
        er = e['diameter'] / 2
        elen = e['length']
        prof = [(0.0, 0.96, 'lip'), (0.06, 1.05, 'body'), (0.55, 1.0, 'body'), (0.85, 0.78, 'body'), (1.0, 0.45, 'body')]
        rr = []
        for t, sc, _ in prof:
            y = y_front - t * elen
            rr.append([(ex + er * sc * math.sin(2 * math.pi * k / 12), y, ez + er * sc * math.cos(2 * math.pi * k / 12)) for k in range(12)])

        def eng_colour(ri, k, rr=rr):
            return srgb(e['lip']) if ri == 0 else srgb(e['body'])

        sb.loft(rr, eng_colour, lambda r, ex=ex, ez=ez, rr=rr: (ex, rr[r][0][1], ez), cap_first=srgb(e['intake']), cap_last=srgb(e['body']))
        # pylon
        pylon_b = [(ex - 0.12, y_front - 0.15 * elen, ez + er * 0.9), (ex + 0.12, y_front - 0.15 * elen, ez + er * 0.9),
                   (ex + 0.12, y_front - 0.75 * elen, ez + er * 0.9), (ex - 0.12, y_front - 0.75 * elen, ez + er * 0.9)]
        pylon_t = [(x, y, z + er * 0.7) for x, y, z in pylon_b]
        sb.hexa(pylon_b, pylon_t, (srgb(w['bottom']),) * 3)

    # ---- tail -------------------------------------------------------------------------------
    t = p['tail']
    s_t = t['root_s']
    y_tr = y_nose - s_t * L
    zc_t = p.get('tail_lift', 0.40) * Dh * ((s_t - 0.70) / 0.30) ** 2
    z0 = zc_t + b * 0.65
    fin = srgb(t['fin'])
    fp = [
        (y_tr, z0 - 0.3),
        (y_tr - t['fin_root'], z0 - 0.3),
        (y_tr - t['fin_sweep'] - t['fin_tip'], z0 + t['fin_height']),
        (y_tr - t['fin_sweep'], z0 + t['fin_height']),
    ]
    th = 0.32
    m.hexa([(-th / 2, y, z) for y, z in fp], [(th / 2, y, z) for y, z in fp], (fin, fin, fin))
    # horizontal stabiliser (mirrored later)
    sp = t['stab_span'] / 2
    ss = math.tan(math.radians(30))
    zs = zc_t + 0.1 * Dh
    r_le = (0.25 * a, y_tr - 0.5, zs)
    r_te = (0.25 * a, y_tr - 0.5 - t['stab_root'], zs)
    t_te = (sp, y_tr - 0.5 - (sp - 0.25 * a) * ss - t['stab_tip'], zs + 0.3)
    t_le = (sp, y_tr - 0.5 - (sp - 0.25 * a) * ss, zs + 0.3)
    st_top = [r_le, r_te, t_te, t_le]
    st_bot = [lower(q, 0.35 if i < 2 else 0.12) for i, q in enumerate(st_top)]
    sc = srgb(t['stab'])
    sb.hexa(st_bot, st_top, (sc, sc, sc))

    sb.mirrored()
    m.tris += sb.tris
    return m


# Real dimensions (metres). Liveries are simplified: sRGB colours as seen, converted at write time.
MODELS = {
    'b737': dict(
        length=39.5, diameter=3.76, tail_lift=0.40,
        livery=dict(upper=0x2f56d0, cheat=0xffc425, lower=0xe0183a, belly=0x2f56d0, cockpit=0x1a2230),
        wing=dict(span=35.8, root_le_s=0.40, root_chord=6.8, tip_chord=1.5, sweep=25, dihedral=6, low=0.55,
                  t_root=0.9, t_tip=0.25, top=0xeef0f4, bottom=0xb3bac5, edge=0xd2d7df,
                  winglet=dict(height=2.5, rake=1.2, top_chord=0.8, colour=0x2f56d0)),
        engine=dict(x=[5.7], diameter=2.0, length=4.2, forward=1.8, drop=1.25,
                    body=0xffc425, lip=0x2a2f3a, intake=0x14171d),
        tail=dict(root_s=0.80, fin_root=7.2, fin_tip=2.2, fin_height=6.4, fin_sweep=4.6, fin=0x2f56d0,
                  stab_span=14.4, stab_root=3.8, stab_tip=1.3, stab=0x2f56d0),
    ),
    'a380': dict(
        length=72.7, diameter=7.14, height=8.4, tail_lift=0.32,
        livery=dict(upper=0xf4f6f9, cheat=0x7e8cab, lower=0xf4f6f9, belly=0xc4cad4, cockpit=0x1a2230),
        wing=dict(span=79.8, root_le_s=0.36, root_chord=16.5, tip_chord=3.4, sweep=33.5, dihedral=5, low=0.7,
                  t_root=2.1, t_tip=0.45, top=0xe4e8ee, bottom=0xaab2be, edge=0xd0d5dd,
                  winglet=dict(height=4.2, rake=2.0, top_chord=1.4, colour=0xf4f6f9)),
        engine=dict(x=[12.0, 25.5], diameter=3.1, length=6.2, forward=3.2, drop=2.3,
                    body=0xe8ebf0, lip=0x2a2f3a, intake=0x14171d),
        tail=dict(root_s=0.78, fin_root=15.5, fin_tip=5.0, fin_height=14.0, fin_sweep=10.5, fin=0x0a1f78,
                  stab_span=30.4, stab_root=9.5, stab_tip=2.8, stab=0xe4e8ee),
    ),
    'b787': dict(
        length=62.8, diameter=5.77, tail_lift=0.38,
        livery=dict(upper=0xf6f8fb, cheat=0x2a6fd6, lower=0xf6f8fb, belly=0xc9d1dc, cockpit=0x1a2230),
        wing=dict(span=60.1, root_le_s=0.38, root_chord=11.0, tip_chord=2.4, sweep=32, dihedral=7, low=0.6,
                  t_root=1.5, t_tip=0.3, top=0xe6eaf0, bottom=0xaeb6c2, edge=0xd0d6de,
                  winglet=None),
        engine=dict(x=[9.6], diameter=2.9, length=5.6, forward=2.8, drop=1.9,
                    body=0xf6f8fb, lip=0x2a6fd6, intake=0x14171d),
        tail=dict(root_s=0.80, fin_root=10.5, fin_tip=3.2, fin_height=9.6, fin_sweep=7.0, fin=0x2a56c8,
                  stab_span=19.2, stab_root=5.6, stab_tip=1.8, stab=0xe6eaf0),
    ),
}

# ---------------------------------------------------------------------------------------------
# preview: painter's-algorithm software render with the same light as the app's shader


def render(mesh, yaw, pitch, size=(760, 460)):
    """Software z-buffer render with the same light as the app's shader (for checking the models)."""
    from PIL import Image
    W, H = size
    P = np.array([[t[0], t[1], t[2]] for t in mesh.tris], dtype=float)  # (n,3,3)
    cy, sy, cp, sp = math.cos(yaw), math.sin(yaw), math.cos(pitch), math.sin(pitch)
    Rz = np.array([[cy, -sy, 0], [sy, cy, 0], [0, 0, 1]])
    Rx = np.array([[1, 0, 0], [0, cp, -sp], [0, sp, cp]])
    R = Rx @ Rz
    Q = P @ R.T  # x right, y depth (away from viewer), z up
    ext = max(np.abs(Q[:, :, 0]).max(), np.abs(Q[:, :, 2]).max())
    scale = 0.5 * min(W, H) / (ext * 1.08) if ext * 2 * 0.0 == 0 else 1.0
    scale = min(0.5 * W / (np.abs(Q[:, :, 0]).max() * 1.08), 0.5 * H / (np.abs(Q[:, :, 2]).max() * 1.08))
    img = np.empty((H, W, 3), dtype=np.float32)
    img[:] = (190, 205, 220)
    zbuf = np.full((H, W), np.inf, dtype=np.float32)
    light = np.array([-0.45, 0.65, 0.75])
    light /= np.linalg.norm(light)
    hv = light + np.array([0, 0, 1.0])
    hv /= np.linalg.norm(hv)
    for i, (a, b, c, col) in enumerate(mesh.tris):
        n = np.cross(np.array(b) - np.array(a), np.array(c) - np.array(a))
        n = n / (np.linalg.norm(n) + 1e-12)
        nv = R @ n
        ns = np.array([nv[0], nv[2], -nv[1]])
        if ns[2] < 0:
            ns = -ns
        diff = max(float(ns @ light), 0.0)
        spec = max(float(ns @ hv), 0.0) ** 40 * 0.25
        k = 0.38 + 0.78 * diff
        rgb = np.array([min(max(((col[j] ** 2.2) * k + spec), 0.0), 1.0) ** (1 / 2.2) for j in range(3)]) * 255
        xs = W / 2 + Q[i, :, 0] * scale
        ys = H / 2 - Q[i, :, 2] * scale
        zs = Q[i, :, 1]
        x0, x1 = int(max(math.floor(xs.min()), 0)), int(min(math.ceil(xs.max()), W - 1))
        y0, y1 = int(max(math.floor(ys.min()), 0)), int(min(math.ceil(ys.max()), H - 1))
        if x1 < x0 or y1 < y0:
            continue
        gx, gy = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
        d = (ys[1] - ys[2]) * (xs[0] - xs[2]) + (xs[2] - xs[1]) * (ys[0] - ys[2])
        if abs(d) < 1e-9:
            continue
        l0 = ((ys[1] - ys[2]) * (gx - xs[2]) + (xs[2] - xs[1]) * (gy - ys[2])) / d
        l1 = ((ys[2] - ys[0]) * (gx - xs[2]) + (xs[0] - xs[2]) * (gy - ys[2])) / d
        l2 = 1 - l0 - l1
        inside = (l0 >= -0.02) & (l1 >= -0.02) & (l2 >= -0.02)
        z = l0 * zs[0] + l1 * zs[1] + l2 * zs[2]
        sub_z = zbuf[y0:y1 + 1, x0:x1 + 1]
        upd = inside & (z < sub_z)
        sub_z[upd] = z[upd]
        img[y0:y1 + 1, x0:x1 + 1][upd] = rgb
    return Image.fromarray(img.astype(np.uint8))


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(1)
    out = args[0]
    prev = args[args.index('--preview') + 1] if '--preview' in args else None
    os.makedirs(out, exist_ok=True)
    for name, params in MODELS.items():
        m = aircraft(params)
        tris, radius = m.write(os.path.join(out, name + '.bin'))
        print(f'{name}: {tris} triangles, radius {radius:.1f} m')
        if prev:
            from PIL import Image
            os.makedirs(prev, exist_ok=True)
            views = [render(m, math.radians(a), math.radians(p)) for a, p in ((30, 25), (0, 90 - 1e-3), (90, 0))]
            sheet = Image.new('RGB', (views[0].width * len(views), views[0].height))
            for i, v in enumerate(views):
                sheet.paste(v, (i * v.width, 0))
            sheet.save(os.path.join(prev, name + '.png'))


if __name__ == '__main__':
    main()
