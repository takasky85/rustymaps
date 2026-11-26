#!/usr/bin/env python3
"""Bakes Natural Earth 110m coastlines (public domain) into assets/coast.bin for the mini globe.

    python3 tools/make_coast.py ne_110m_coastline.geojson assets/coast.bin

Format: repeated [u16 point count][count x (i16 lon*100, i16 lat*100)], little endian.
"""
import json, struct, sys, math

def dp(pts, eps):
    if len(pts) < 3:
        return pts
    (x1, y1), (x2, y2) = pts[0], pts[-1]
    dx, dy = x2 - x1, y2 - y1
    n = math.hypot(dx, dy) or 1e-12
    best, bi = -1, 0
    for i in range(1, len(pts) - 1):
        d = abs(dy * (pts[i][0] - x1) - dx * (pts[i][1] - y1)) / n
        if d > best:
            best, bi = d, i
    if best <= eps:
        return [pts[0], pts[-1]]
    return dp(pts[:bi + 1], eps)[:-1] + dp(pts[bi:], eps)

src, dst = sys.argv[1], sys.argv[2]
gj = json.load(open(src))
lines = []
for f in gj["features"]:
    g = f["geometry"]
    parts = g["coordinates"] if g["type"] == "MultiLineString" else [g["coordinates"]]
    for part in parts:
        pts = dp([(p[0], p[1]) for p in part], 0.2)
        if len(pts) >= 2:
            lines.append(pts)
out = bytearray()
total = 0
for pts in lines:
    out += struct.pack("<H", len(pts))
    for lon, lat in pts:
        out += struct.pack("<hh", round(lon * 100), round(lat * 100))
    total += len(pts)
open(dst, "wb").write(out)
print(len(lines), "lines,", total, "points,", len(out), "bytes")
