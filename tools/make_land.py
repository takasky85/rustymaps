#!/usr/bin/env python3
"""Bakes Natural Earth 110m land polygons (public domain) into assets/land.bin for the mini globe.

    python3 tools/make_land.py ne_110m_land.geojson assets/land.bin

Format: a 720 x 360 bitmap (0.5 degree cells), row 0 = latitude +90, column 0 = longitude -180,
1 bit per cell (most significant bit first), 1 = land.  Needs Pillow.
"""
import json, sys
from PIL import Image, ImageDraw

W, H = 720, 360
S = 4  # draw at 4x and take the majority, for cleaner coasts
src, dst = sys.argv[1], sys.argv[2]
gj = json.load(open(src))
img = Image.new("L", (W * S, H * S), 0)
d = ImageDraw.Draw(img)

def px(ring):
    return [((lon + 180.0) / 360.0 * W * S, (90.0 - lat) / 180.0 * H * S) for lon, lat in ring]

for f in gj["features"]:
    g = f["geometry"]
    polys = g["coordinates"] if g["type"] == "MultiPolygon" else [g["coordinates"]]
    for poly in polys:
        d.polygon(px(poly[0]), fill=255)
        for hole in poly[1:]:
            d.polygon(px(hole), fill=0)
small = img.resize((W, H), Image.BOX)
data = small.load()
out = bytearray()
for y in range(H):
    byte, n = 0, 0
    for x in range(W):
        byte = (byte << 1) | (1 if data[x, y] >= 128 else 0)
        n += 1
        if n == 8:
            out.append(byte)
            byte, n = 0, 0
open(dst, "wb").write(bytes(out))
print(len(out), "bytes")
