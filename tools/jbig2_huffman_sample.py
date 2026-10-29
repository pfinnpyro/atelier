"""A tiny JBIG2 encoder for Huffman-coded symbol dictionaries and text
regions, to exercise those decoding paths. Checked against jbig2dec."""
import struct, sys
from PIL import Image, ImageDraw, ImageFont

TABLES = {
 1: ([(0,1,4),(16,2,8),(272,3,16)], None, (65808,3), None),
 2: ([(0,1,0),(1,2,0),(2,3,0),(3,4,3),(11,5,6)], None, (75,6), 6),
 4: ([(1,1,0),(2,2,0),(3,3,0),(4,4,3),(12,5,6)], None, (76,5), None),
 6: ([(-2048,5,10),(-1024,4,9),(-512,4,8),(-256,4,7),(-128,5,6),(-64,5,5),(-32,4,5),(0,2,7),(128,3,7),(256,3,8),(512,4,9),(1024,4,10)], (-2049,6), (2048,6), None),
 8: ([(-15,8,3),(-7,9,1),(-5,8,1),(-3,9,0),(-2,7,0),(-1,4,0),(0,2,1),(2,5,0),(3,6,0),(4,3,4),(20,6,1),(22,4,4),(38,4,5),(70,5,6),(134,5,7),(262,6,7),(390,7,8),(646,6,10)], (-16,9), (1670,9), 2),
 11: ([(1,1,0),(2,2,1),(4,4,0),(5,4,1),(7,5,1),(9,5,2),(13,6,2),(17,7,2),(21,7,3),(29,7,4),(45,7,5),(77,7,6)], None, (141,7), None),
}

def canonical(lines):
    # lines: list of [preflen, ...]; returns codes in order
    mx = max(l[0] for l in lines)
    count = [0]*(mx+2)
    for l in lines: count[l[0]] += 1
    count[0] = 0
    codes = [None]*len(lines); first = 0
    for ln in range(1, mx+1):
        first = (first + count[ln-1]) << 1; c = first
        for i, l in enumerate(lines):
            if l[0] == ln: codes[i] = c; c += 1
    return codes

class Huff:
    def __init__(self, n):
        normal, lower, upper, oob = TABLES[n]
        self.lines = [(p, r, low, 'n') for (low, p, r) in normal]
        if lower: self.lines.append((lower[1], 32, lower[0], 'l'))
        self.lines.append((upper[1], 32, upper[0], 'u'))
        if oob: self.lines.append((oob, 0, 0, 'o'))
        self.codes = canonical(self.lines)
    def encode(self, w, v):
        for (p, r, low, k), c in zip(self.lines, self.codes):
            if (v is None and k == 'o') or (v is not None and k == 'n' and low <= v < low + (1 << r)):
                w.bits(c, p); 
                if k == 'n': w.bits(v - low, r)
                return
        for (p, r, low, k), c in zip(self.lines, self.codes):
            if v is not None and k == 'u' and v >= low: w.bits(c, p); w.bits(v - low, 32); return
            if v is not None and k == 'l' and v < low + 1: w.bits(c, p); w.bits(low - v, 32); return
        raise ValueError((v,))

class W:
    def __init__(s): s.out = bytearray(); s.acc = 0; s.n = 0
    def bits(s, v, n):
        for i in range(n-1, -1, -1):
            s.acc = (s.acc << 1) | ((v >> i) & 1); s.n += 1
            if s.n == 8: s.out.append(s.acc); s.acc = 0; s.n = 0
    def align(s):
        if s.n: s.bits(0, 8 - s.n)
    def bytes(s): s.align(); return bytes(s.out)

def seg(num, kind, data, refs=()):
    return struct.pack(">IB", num, kind) + bytes([len(refs) << 5]) + bytes(refs) + bytes([1]) + struct.pack(">I", len(data)) + data

font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 28)
text = "huffman coded jbig2 text regions"
chars = sorted(set(text.replace(" ", "")))
glyphs = {}
for ch in chars:
    im = Image.new("1", (40, 40), 0); d = ImageDraw.Draw(im); d.text((2, 2), ch, font=font, fill=1)
    bb = im.getbbox(); glyphs[ch] = im.crop(bb)
syms = list(chars)
# Symbol dictionary: classes by height, widths ascending.
order = sorted(syms, key=lambda c: (glyphs[c].height, glyphs[c].width))
w = W(); dh, dw, bms, ex = Huff(4), Huff(2), Huff(1), Huff(1)
prev_h = 0; i = 0
while i < len(order):
    h = glyphs[order[i]].height
    cls = [c for c in order if glyphs[c].height == h]
    dh.encode(w, h - prev_h); prev_h = h
    prev_w = 0
    for c in cls: dw.encode(w, glyphs[c].width - prev_w); prev_w = glyphs[c].width
    dw.encode(w, None)
    bms.encode(w, 0); w.align()
    tot = sum(glyphs[c].width for c in cls)
    for y in range(h):
        x0 = 0
        for c in cls:
            g = glyphs[c]
            for x in range(g.width): w.bits(g.getpixel((x, y)) and 1, 1)
        w.align()
    i += len(cls)
ex.encode(w, 0); ex.encode(w, len(order))
sd = struct.pack(">H", 1) + struct.pack(">II", len(order), len(order)) + w.bytes()
# Text region: two lines of text.
W_, H_ = 640, 120
inst = []  # (x, top, symbol index)
lines = [(10, text[:16]), (60, text[16:])]
for top, s in lines:
    x = 10
    for ch in s:
        if ch == " ": x += 10; continue
        inst.append((x, top + 30 - glyphs[ch].height, order.index(ch))); x += glyphs[ch].width + 2
w = W(); fs, ds, dt = Huff(6), Huff(8), Huff(11)
L = max(1, (len(order) - 1).bit_length())
# Symbol ID code table: run code L has length 1, nothing else.
for v in range(35): w.bits(1 if v == L else 0, 4)
for _ in order: w.bits(0, 1)
w.align()
dt.encode(w, 1)  # STRIPT = -1
strip_t = -1; first_s = 0
# One strip per distinct top value (T increasing).
from itertools import groupby
inst.sort(key=lambda t: (t[1], t[0]))
for top, group in groupby(inst, key=lambda t: t[1]):
    group = list(group)
    dt.encode(w, top - strip_t); strip_t = top
    fs.encode(w, group[0][0] - first_s); first_s = group[0][0]
    cur = group[0][0]
    for k, (x, _, sid) in enumerate(group):
        if k > 0: ds.encode(w, x - cur); cur = x
        w.bits(sid, L)
        cur += glyphs[order[sid]].width - 1
    ds.encode(w, None)
flags = 1 | (1 << 4)   # SBHUFF, REFCORNER = TOPLEFT
tr = struct.pack(">IIIIB", W_, H_, 0, 0, 0) + struct.pack(">HH", flags, 0) + struct.pack(">I", len(inst)) + w.bytes()
page = struct.pack(">IIIIBH", W_, H_, 0, 0, 0, 0)
open(sys.argv[1], "wb").write(seg(0, 48, page) + seg(1, 0, sd) + seg(2, 6, tr, (1,)) + seg(3, 49, b""))
