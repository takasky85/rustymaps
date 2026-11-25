#!/usr/bin/env python3
"""Bakes a glTF/GLB aircraft into the compact binary RustyMaps loads at runtime (data/models/<id>.bin).

Textures are baked into vertex colours (triangles are subdivided until they are small enough to carry the
detail), so the app needs no texture support.  Output body axes, metres: x = right, y = forward (nose), z = up.

usage: bake_plane.py MODEL.gltf|glb OUT.bin --length 39.5 --map "right=-x,fwd=z,up=y" [--exclude REGEX] [--preview P.png]

Layout (little endian): u32 vertex count, u32 index count, f32 bounding radius (m), then vertices
(position f32x3, normal i8x4 snorm, colour u8x4 sRGB-encoded rgba), then u32 indices.
"""
import argparse, io, json, re, struct, sys
import numpy as np
from PIL import Image

ap = argparse.ArgumentParser()
ap.add_argument('src'); ap.add_argument('out')
ap.add_argument('--length', type=float, required=True, help='real aircraft length in metres')
ap.add_argument('--map', default='right=-x,fwd=z,up=y', help='which model axis becomes right/fwd/up (sign allowed)')
ap.add_argument('--exclude', default='', help='regex of material names to drop (cabin interiors etc.)')
ap.add_argument('--exclude-node', default='', help='regex of node names to drop (ground planes, shadow catchers)')
ap.add_argument('--edge', type=float, default=0.2, help='start limit (m) for subdividing textured triangles')
ap.add_argument('--max-tris', type=int, default=450000)
ap.add_argument('--preview', default='')
a = ap.parse_args()

CT = {5120: np.int8, 5121: np.uint8, 5122: np.int16, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}
NC = {'SCALAR': 1, 'VEC2': 2, 'VEC3': 3, 'VEC4': 4, 'MAT4': 16}

def load(path):
    if path.lower().endswith('.glb'):
        d = open(path, 'rb').read()
        _, _, length = struct.unpack('<III', d[:12]); off = 12; js = None; binc = b''
        while off < length:
            cl, ct = struct.unpack('<II', d[off:off + 8]); chunk = d[off + 8:off + 8 + cl]; off += 8 + cl
            if ct == 0x4E4F534A: js = json.loads(chunk)
            elif ct == 0x004E4942: binc = chunk
        return js, binc, None
    import os
    js = json.load(open(path))
    base = os.path.dirname(path)
    bins = b''
    return js, open(os.path.join(base, js['buffers'][0]['uri']), 'rb').read(), base

g, B, base = load(a.src)

def acc(i):
    ac = g['accessors'][i]; bv = g['bufferViews'][ac['bufferView']]
    dt = np.dtype(CT[ac['componentType']]); n = NC[ac['type']]
    off = bv.get('byteOffset', 0) + ac.get('byteOffset', 0)
    stride = bv.get('byteStride') or dt.itemsize * n
    if stride == dt.itemsize * n:
        arr = np.frombuffer(B, dtype=dt, count=ac['count'] * n, offset=off).reshape(ac['count'], n)
    else:
        arr = np.stack([np.frombuffer(B, dtype=dt, count=n, offset=off + k * stride) for k in range(ac['count'])])
    if ac.get('normalized'):
        arr = arr.astype(np.float64) / np.iinfo(dt).max
    return arr

def srgb_to_lin(x): return np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)
def lin_to_srgb(x): x = np.clip(x, 0, 1); return np.where(x <= 0.0031308, x * 12.92, 1.055 * x ** (1 / 2.4) - 0.055)

images = {}
def image(ti):
    if ti in images: return images[ti]
    t = g['textures'][ti]; im = g['images'][t['source']]
    if 'uri' in im:
        import os
        data = open(os.path.join(base, im['uri']), 'rb').read()
    else:
        bv = g['bufferViews'][im['bufferView']]; data = B[bv.get('byteOffset', 0):bv.get('byteOffset', 0) + bv['byteLength']]
    img = np.asarray(Image.open(io.BytesIO(data)).convert('RGBA'), dtype=np.float64) / 255.0
    img[..., :3] = srgb_to_lin(img[..., :3])
    images[ti] = img
    return img

def local(n):
    if 'matrix' in n: return np.array(n['matrix'], dtype=np.float64).reshape(4, 4).T
    t = np.array(n.get('translation', [0, 0, 0])); r = n.get('rotation', [0, 0, 0, 1]); s = np.array(n.get('scale', [1, 1, 1]))
    x, y, z, w = r
    R = np.array([[1-2*(y*y+z*z), 2*(x*y-z*w), 2*(x*z+y*w)], [2*(x*y+z*w), 1-2*(x*x+z*z), 2*(y*z-x*w)], [2*(x*z-y*w), 2*(y*z+x*w), 1-2*(x*x+y*y)]])
    M = np.eye(4); M[:3, :3] = R * s; M[:3, 3] = t
    return M

excl = re.compile(a.exclude, re.I) if a.exclude else None
excln = re.compile(a.exclude_node, re.I) if a.exclude_node else None
prims = []   # (P, N, UV or None, F, color(3,) or texture info)
def walk(ni, M):
    n = g['nodes'][ni]; W = M @ local(n)
    if 'mesh' in n and not (excln and excln.search(n.get('name', ''))):
        for p in g['meshes'][n['mesh']]['primitives']:
            if p.get('mode', 4) != 4 or 'indices' not in p: continue
            mat = g['materials'][p['material']] if 'material' in p else {}
            if excl and excl.search(mat.get('name', '')): continue
            pos = acc(p['attributes']['POSITION']).astype(np.float64)
            wp = (np.c_[pos, np.ones(len(pos))] @ W.T)[:, :3]
            if 'NORMAL' in p['attributes']:
                nr = acc(p['attributes']['NORMAL']).astype(np.float64)
                wn = nr @ np.linalg.inv(W[:3, :3]).T
                wn /= np.maximum(np.linalg.norm(wn, axis=1, keepdims=True), 1e-12)
            else:
                wn = np.zeros_like(wp)
            pbr = mat.get('pbrMetallicRoughness', {})
            factor = np.array(pbr.get('baseColorFactor', [1, 1, 1, 1]), dtype=np.float64)
            tex = pbr.get('baseColorTexture'); uv = None; timg = None
            if tex is not None:
                tc = tex.get('texCoord', 0)
                key = 'TEXCOORD_%d' % tc
                if key in p['attributes']:
                    uv = acc(p['attributes'][key]).astype(np.float64); timg = image(tex['index'])
            if mat.get('alphaMode') == 'BLEND':
                factor = factor * np.array([0.25, 0.25, 0.28, 1.0])  # tinted glass drawn opaque and dark
            F = acc(p['indices']).reshape(-1, 3).astype(np.int64)
            prims.append([wp, wn, uv, F, factor[:3], timg])
    for c in n.get('children', []): walk(c, W)
for r in g['scenes'][g.get('scene', 0)]['nodes']: walk(r, np.eye(4))

# --- axes: model -> body (x right, y forward, z up), metres ---
def axis(spec):
    m = re.fullmatch(r'(-?)([xyz])', spec); return ('xyz'.index(m.group(2)), -1.0 if m.group(1) else 1.0)
mp = dict(kv.split('=') for kv in a.map.split(','))
ar, af, au = axis(mp['right']), axis(mp['fwd']), axis(mp['up'])
def to_body(v):
    return np.stack([v[:, ar[0]] * ar[1], v[:, af[0]] * af[1], v[:, au[0]] * au[1]], axis=1)

allp = np.concatenate([to_body(p[0]) for p in prims])
fmin, fmax = allp[:, 1].min(), allp[:, 1].max()
scale = a.length / (fmax - fmin)
centre = np.array([(allp[:, 0].min() + allp[:, 0].max()) / 2, (fmin + fmax) / 2, (allp[:, 2].min() + allp[:, 2].max()) / 2])
print('prims', len(prims), 'scale', scale, 'raw extent', allp.max(0) - allp.min(0))

def subdivide(P, N, UV, F, L):
    while True:
        e = np.stack([np.linalg.norm(P[F[:, 0]] - P[F[:, 1]], axis=1), np.linalg.norm(P[F[:, 1]] - P[F[:, 2]], axis=1), np.linalg.norm(P[F[:, 2]] - P[F[:, 0]], axis=1)], axis=1).max(1)
        big = e > L
        if not big.any(): return P, N, UV, F
        keep, sp = F[~big], F[big]
        def mid(i, j):
            nonlocal P, N, UV
            k = len(P)
            P = np.concatenate([P, (P[i] + P[j]) / 2]); N = np.concatenate([N, (N[i] + N[j]) / 2])
            if UV is not None: UV = np.concatenate([UV, (UV[i] + UV[j]) / 2])
            return np.arange(k, k + len(i))
        m01 = mid(sp[:, 0], sp[:, 1]); m12 = mid(sp[:, 1], sp[:, 2]); m20 = mid(sp[:, 2], sp[:, 0])
        new = np.concatenate([np.stack([sp[:, 0], m01, m20], 1), np.stack([m01, sp[:, 1], m12], 1), np.stack([m20, m12, sp[:, 2]], 1), np.stack([m01, m12, m20], 1)])
        F = np.concatenate([keep, new])

def build(L):
    Ps, Ns, Cs, Fs = [], [], [], []; base_i = 0
    for P, N, UV, F, factor, timg in prims:
        P = to_body(P); Nn = to_body(N)
        if timg is not None and UV is not None:
            P, Nn, UV, F = subdivide(P, Nn, UV, F, L / scale)
            h, w = timg.shape[:2]
            u = np.mod(UV[:, 0], 1.0) * (w - 1); v = np.mod(UV[:, 1], 1.0) * (h - 1)
            col = timg[np.rint(v).astype(int), np.rint(u).astype(int), :3] * factor
        else:
            col = np.tile(factor, (len(P), 1))
        Ps.append((P - centre) * scale); Ns.append(Nn); Cs.append(col); Fs.append(F.reshape(-1) + base_i); base_i += len(P)
    return np.concatenate(Ps), np.concatenate(Ns), np.concatenate(Cs), np.concatenate(Fs)

L = a.edge
while True:
    P, N, C, I = build(L)
    print('edge limit', L, 'tris', len(I) // 3)
    if len(I) // 3 <= a.max_tris or L > 1000: break  # (untextured triangles are never subdivided)
    L *= 1.5
N = N / np.maximum(np.linalg.norm(N, axis=1, keepdims=True), 1e-12)
radius = float(np.linalg.norm(P, axis=1).max())
v = np.zeros(len(P), dtype=[('p', '<f4', 3), ('n', 'i1', 4), ('c', 'u1', 4)])
v['p'] = P; v['n'][:, :3] = np.clip(np.round(N * 127), -127, 127)
v['c'][:, :3] = np.clip(np.round(lin_to_srgb(C) * 255), 0, 255); v['c'][:, 3] = 255
with open(a.out, 'wb') as f:
    f.write(struct.pack('<IIf', len(v), len(I), radius)); f.write(v.tobytes()); f.write(I.astype('<u4').tobytes())
print('verts', len(v), 'indices', len(I), 'radius', radius, 'extent', P.min(0), P.max(0))

if a.preview:
    import matplotlib; matplotlib.use('Agg'); import matplotlib.pyplot as plt
    fig, ax = plt.subplots(1, 3, figsize=(21, 7)); s = max(1, len(P) // 60000)
    col = np.clip(lin_to_srgb(C), 0, 1)
    for x, (i, j, t) in zip(ax, [(1, 2, 'side: forward -> right, up'), (0, 1, 'top: right, forward'), (0, 2, 'front: right, up')]):
        x.set_aspect('equal'); x.set_title(t); x.scatter(P[::s, i], P[::s, j], s=0.6, c=col[::s])
    fig.savefig(a.preview, dpi=60)

# Axis mappings used for the shipped models (right-handed: right x forward = up):
#   737  (A Random Modeler):  --length 39.5 --map "right=-x,fwd=z,up=y"          -> b737.bin
#   A380 (Nobilis 2):         --length 72.7 --map "right=-x,fwd=z,up=y" --edge 0.6 --max-tris 260000 -> a380.bin
#   787  (themaker845, ANA):  --length 62.8 --map "right=x,fwd=-z,up=y" --edge 1.5 --max-tris 300000 \
#        --exclude "interior|inetrior|chair|cjair|floor|storage" --exclude-node "^Plane"           -> b787.bin
