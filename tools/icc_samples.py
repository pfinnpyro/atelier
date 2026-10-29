"""Builds the ICC test profiles in crates/atelier-color/tests/data and what
Little CMS (through Pillow's ImageCms) makes of a grid of device values in
each, converted to sRGB.

Each profile exercises one pipeline: RGB matrix/TRC (gamma and parametric
curves), a gray table, lut8, lut16 and v4 lutAtoB (with and without a
CLUT, with every curve set and the matrix). Output per profile:
NAME.icc and NAME.ref, the reference being the 8-bit inputs followed by
the 8-bit sRGB results, row by row.

Usage: python3 tools/icc_samples.py   (needs Pillow with ImageCms)
"""
import io, itertools, math, os, struct
from PIL import Image, ImageCms

OUT = os.path.join(os.path.dirname(__file__), "..", "crates", "atelier-color", "tests", "data")
D50 = (0.9642, 1.0, 0.8249)


def s15(v):
    return struct.pack(">i", int(round(v * 65536)))


def xyz(v):
    return b"XYZ \0\0\0\0" + b"".join(s15(c) for c in v)


def curv_gamma(g):
    return b"curv\0\0\0\0" + struct.pack(">IH", 1, int(round(g * 256)))


def curv_table(values):
    return b"curv\0\0\0\0" + struct.pack(">I", len(values)) + b"".join(
        struct.pack(">H", max(0, min(65535, int(round(v * 65535))))) for v in values)


def para(kind, *p):
    return b"para\0\0\0\0" + struct.pack(">HH", kind, 0) + b"".join(s15(v) for v in p)


def pad4(b):
    return b + b"\0" * (-len(b) % 4)


def desc(text, v4):
    if v4:
        t = text.encode("utf-16-be")
        return b"mluc\0\0\0\0" + struct.pack(">II", 1, 12) + b"enUS" + struct.pack(">II", len(t), 28) + t
    t = text.encode() + b"\0"
    return b"desc\0\0\0\0" + struct.pack(">I", len(t)) + t + b"\0" * 12 + b"\0" * 67


def profile(cls, space, pcs, tags, v4=False):
    tags = [(b"desc", desc("atelier test", v4)), (b"wtpt", xyz(D50))] + tags
    n = len(tags)
    offset = 128 + 4 + 12 * n
    table, body = b"", b""
    for sig, data in tags:
        data = pad4(data)
        table += sig + struct.pack(">II", offset + len(body), len(data))
        body += data
    size = offset + len(body)
    header = struct.pack(">I", size) + b"lcms" + (b"\x04\x30\0\0" if v4 else b"\x02\x10\0\0")
    header += cls + space + pcs + b"\0" * 12 + b"acsp" + b"APPL" + b"\0" * 4
    header += b"\0" * 8 + b"\0" * 8 + struct.pack(">I", 0) + b"".join(s15(c) for c in D50)
    header += b"\0" * 4 + b"\0" * 16 + b"\0" * 28
    assert len(header) == 128
    return header + struct.pack(">I", n) + table + body


# A device model for the LUT profiles: CMYK or RGB to Lab (D50).
def srgb_to_lab(r, g, b):
    lin = [c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4 for c in (r, g, b)]
    m = [[0.4360747, 0.3850649, 0.1430804], [0.2225045, 0.7168786, 0.0606169], [0.0139322, 0.0971045, 0.7141733]]
    X, Y, Z = (sum(m[i][j] * lin[j] for j in range(3)) for i in range(3))
    f = lambda t: t ** (1 / 3) if t > (6 / 29) ** 3 else t / (3 * (6 / 29) ** 2) + 4 / 29
    fx, fy, fz = f(X / D50[0]), f(Y / D50[1]), f(Z / D50[2])
    return 116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)


def cmyk_model(c, m, y, k):
    # Inks that are not quite primaries, with some dot gain.
    c, m, y, k = (v ** 0.8 for v in (c, m, y, k))
    r = (1 - c) * (1 - 0.1 * m) * (1 - k)
    g = (1 - m) * (1 - 0.15 * c) * (1 - k)
    b = (1 - y) * (1 - 0.1 * c - 0.05 * m) * (1 - k)
    return srgb_to_lab(max(r, 0), max(g, 0), max(b, 0))


def rgb_model(r, g, b):
    return srgb_to_lab(r ** 1.1, (0.9 * g + 0.1 * r), b ** 0.9)


def lab_legacy16(lab):
    L, a, b = lab
    return [max(0, min(65535, int(round(L / 100 * 65280)))),
            max(0, min(65535, int(round((a + 128) / 255 * 65280)))),
            max(0, min(65535, int(round((b + 128) / 255 * 65280))))]


def lab_8(lab):
    L, a, b = lab
    return [max(0, min(255, int(round(L / 100 * 255)))),
            max(0, min(255, int(round(a + 128)))),
            max(0, min(255, int(round(b + 128))))]


def grid_points(inputs, g):
    return itertools.product(*[[i / (g - 1) for i in range(g)]] * inputs)


def mft2(inputs, g, model):
    n = m = 256
    out = struct.pack(">BBBB", inputs, 3, g, 0)
    out += b"".join(s15(v) for v in (1, 0, 0, 0, 1, 0, 0, 0, 1))
    out += struct.pack(">HH", n, m)
    # Input curves: a mild S-curve; output curves: identity.
    curve_in = [0.5 - 0.5 * math.cos(math.pi * i / (n - 1)) for i in range(n)]
    for _ in range(inputs):
        out += b"".join(struct.pack(">H", int(round(v * 65535))) for v in curve_in)
    for p in grid_points(inputs, g):
        # The grid is indexed after the input curves.
        x = [math.acos(1 - 2 * v) / math.pi for v in p]
        out += b"".join(struct.pack(">H", v) for v in lab_legacy16(model(*x)))
    for _ in range(3):
        out += b"".join(struct.pack(">H", int(round(i / (m - 1) * 65535))) for i in range(m))
    return b"mft2\0\0\0\0" + out


def mft1(inputs, g, model):
    out = struct.pack(">BBBB", inputs, 3, g, 0)
    out += b"".join(s15(v) for v in (1, 0, 0, 0, 1, 0, 0, 0, 1))
    out += bytes(range(256)) * inputs
    for p in grid_points(inputs, g):
        out += bytes(lab_8(model(*p)))
    out += bytes(range(256)) * 3
    return b"mft1\0\0\0\0" + out


def mab(inputs, outputs, clut_grid, precision, model_v4, a=None, m=None, matrix=None, b=None):
    """lutAtoBType with the elements given (curves as raw tag bytes)."""
    header_len = 32
    parts = []
    pos = header_len

    def place(data):
        nonlocal pos
        at = pos
        data = pad4(data)
        parts.append(data)
        pos += len(data)
        return at

    off_b = place(b"".join(pad4(c) for c in b)) if b else 0
    off_matrix = place(b"".join(s15(v) for v in matrix)) if matrix else 0
    off_m = place(b"".join(pad4(c) for c in m)) if m else 0
    off_clut = 0
    if clut_grid:
        body = bytes([clut_grid] * inputs + [0] * (16 - inputs)) + bytes([precision, 0, 0, 0])
        for p in grid_points(inputs, clut_grid):
            vals = model_v4(*p)
            if precision == 1:
                body += bytes(max(0, min(255, int(round(v * 255)))) for v in vals)
            else:
                body += b"".join(struct.pack(">H", max(0, min(65535, int(round(v * 65535))))) for v in vals)
        off_clut = place(body)
    off_a = place(b"".join(pad4(c) for c in a)) if a else 0
    head = b"mAB \0\0\0\0" + struct.pack(">BBH", inputs, outputs, 0)
    head += struct.pack(">IIIII", off_b, off_matrix, off_m, off_clut, off_a)
    assert len(head) == header_len
    return head + b"".join(parts)


def reference(name, data, mode, inputs, steps):
    path = os.path.join(OUT, name + ".icc")
    open(path, "wb").write(data)
    values = [round(i * 255 / (steps - 1)) for i in range(steps)]
    pixels = list(itertools.product(*[values] * inputs))
    src = ImageCms.getOpenProfile(path)
    dst = ImageCms.createProfile("sRGB")
    t = ImageCms.buildTransform(src, dst, mode, "RGB",
                                renderingIntent=ImageCms.Intent.RELATIVE_COLORIMETRIC)
    img = Image.new(mode, (len(pixels), 1))
    img.putdata([p[0] if inputs == 1 else tuple(p) for p in pixels])
    out = ImageCms.applyTransform(img, t)
    raw = b"".join(bytes(p) + bytes(out.getpixel((i, 0))) for i, p in enumerate(pixels))
    open(os.path.join(OUT, name + ".ref"), "wb").write(raw)


def main():
    os.makedirs(OUT, exist_ok=True)
    # sRGB from Little CMS itself (parametric curves).
    srgb = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
    reference("srgb", srgb, "RGB", 3, 6)
    # Adobe RGB-like primaries with a 2.2 gamma.
    adobe = [(0.6097, 0.3111, 0.0195), (0.2052, 0.6257, 0.0609), (0.1492, 0.0632, 0.7446)]
    rgb = profile(b"mntr", b"RGB ", b"XYZ ", [
        (b"rXYZ", xyz(adobe[0])), (b"gXYZ", xyz(adobe[1])), (b"bXYZ", xyz(adobe[2])),
        (b"rTRC", curv_gamma(2.2)), (b"gTRC", curv_gamma(2.2)), (b"bTRC", curv_gamma(1.8)),
    ])
    reference("rgb-matrix", rgb, "RGB", 3, 6)
    # Gray with a dot gain table.
    gain = [min(1.0, (i / 255) ** 0.8 * 0.95 + 0.05 * (i / 255)) for i in range(256)]
    reference("gray-table", profile(b"mntr", b"GRAY", b"XYZ ", [(b"kTRC", curv_table(gain))]), "L", 1, 32)
    # lut16 CMYK (perceptual and colorimetric tables).
    t = mft2(4, 9, cmyk_model)
    reference("cmyk-lut16", profile(b"prtr", b"CMYK", b"Lab ", [(b"A2B0", t), (b"A2B1", t)]), "CMYK", 4, 5)
    # lut8 RGB to Lab.
    t = mft1(3, 9, rgb_model)
    reference("rgb-lut8", profile(b"scnr", b"RGB ", b"Lab ", [(b"A2B0", t), (b"A2B1", t)]), "RGB", 3, 6)
    # v4 lutAtoB CMYK to Lab: A curves, 8-bit CLUT, B curves.

    def lab_v4(*p):
        L, a, b = cmyk_model(*p)
        return [L / 100, (a + 128) / 255, (b + 128) / 255]
    t = mab(4, 3, 7, 1, lab_v4, a=[curv_gamma(1.0)] * 4, b=[para(0, 1.0)] * 3)
    reference("cmyk-atob", profile(b"prtr", b"CMYK", b"Lab ", [(b"A2B0", t), (b"A2B1", t)], v4=True), "CMYK", 4, 5)
    # v4 lutAtoB RGB to XYZ without a CLUT: A curves, M curves, matrix, B
    # curves. XYZ values are u1Fixed15, so a 0..1 value is X / (65535/32768).
    k = 32768 / 65535
    matrix = [adobe[0][0] * k, adobe[1][0] * k, adobe[2][0] * k,
              adobe[0][1] * k, adobe[1][1] * k, adobe[2][1] * k,
              adobe[0][2] * k, adobe[1][2] * k, adobe[2][2] * k, 0, 0, 0]
    t = mab(3, 3, 0, 0, None, a=[para(0, 2.2)] * 3,
            m=[para(3, 1.0, 1.0, 0.0, 1.0, 0.0)] * 3, matrix=matrix,
            b=[para(0, 1.0)] * 3)
    reference("rgb-atob-matrix", profile(b"mntr", b"RGB ", b"XYZ ", [(b"A2B0", t), (b"A2B1", t)], v4=True), "RGB", 3, 6)


if __name__ == "__main__":
    main()
