"""A tiny JBIG2 encoder for a pattern dictionary and halftone region, to
exercise those decoding paths. Checked against jbig2dec.

Usage: python3 jbig2_halftone_sample.py out.jb2"""
import os, struct, sys, tempfile
from PIL import Image
def g4(bits, w, h):
    # bits: rows of 0/1 with 1 = black. Pillow's 1-bit TIFFs are
    # min-is-black, so libtiff codes Pillow value 1 as a CCITT black run.
    im = Image.new("1", (w, h), 0)
    for y in range(h):
        for x in range(w):
            if bits[y][x]: im.putpixel((x, y), 1)
    path = os.path.join(tempfile.mkdtemp(), "t.tif")
    im.save(path, compression="group4", strip_size=10**8)
    t = Image.open(path)
    off = t.tag_v2[273]; off = off[0] if isinstance(off, tuple) else off
    cnt = t.tag_v2[279]; cnt = cnt[0] if isinstance(cnt, tuple) else cnt
    return open(path, "rb").read()[off:off + cnt]
def seg(num, kind, data, refs=()):
    return struct.pack(">IB", num, kind) + bytes([len(refs) << 5]) + bytes(refs) + bytes([1]) + struct.pack(">I", len(data)) + data
P = 4; N = 17
order = [0,8,2,10,12,4,14,6,3,11,1,9,15,7,13,5]  # Bayer
pats = [[[1 if order[y*4+x] < k else 0 for x in range(P)] for y in range(P)] for k in range(N)]
coll = [[pats[i][y][x] for i in range(N) for x in range(P)] for y in range(P)]
pd = bytes([1, P, P]) + struct.pack(">I", N - 1) + g4(coll, N * P, P)
GW, GH = 100, 40
vals = [[min(16, (x * 17) // GW + ((y // 10) % 2) * 3) % 17 for x in range(GW)] for y in range(GH)]
bits = 5
planes = b""
for j in reversed(range(bits)):
    plane = [[((v >> j) & 1) ^ ((v >> (j + 1)) & 1) for v in row] for row in vals]
    planes += g4(plane, GW, GH)
W_, H_ = GW * P, GH * P
flags = 1  # HMMR, OR
hr = struct.pack(">IIIIB", W_, H_, 0, 0, 0) + bytes([flags]) + struct.pack(">IIiiHH", GW, GH, 0, 0, P * 256, 0) + planes
page = struct.pack(">IIIIBH", W_, H_, 0, 0, 0, 0)
open(sys.argv[1], "wb").write(seg(0, 48, page) + seg(1, 16, pd) + seg(2, 22, hr, (1,)) + seg(3, 49, b""))
