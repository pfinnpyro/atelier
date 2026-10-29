"""Builds the JPEG 2000 test fixtures in crates/atelier-filters/tests/data/jpx.

Each case is a small codestream or JP2 file from OpenJPEG's encoder
(opj_compress) plus what OpenJPEG's decoder makes of it, as 8-bit PNM (PGM
or PPM; alpha in a separate PGM). A few files are assembled here because
the encoder cannot write them: palettes, sYCC, and packed packet headers
(PPM and PPT markers), which are rewritten from streams with SOP and EPH
markers.

Usage: python3 tools/jpx_samples.py   (needs opj_compress, opj_decompress,
numpy and Pillow)
"""
import os, shutil, struct, subprocess, tempfile
import numpy as np
from PIL import Image, ImageDraw

OUT = os.path.join(os.path.dirname(__file__), "..", "crates", "atelier-filters", "tests", "data", "jpx")
TMP = tempfile.mkdtemp()
W, H = 67, 45


def source():
    rng = np.random.default_rng(7)
    y, x = np.mgrid[0:H, 0:W]
    img = np.stack([128 + 120 * np.sin(x / 7.0), 255 * y / H, (x * y) % 256], axis=2)
    im = Image.fromarray(img.astype(np.uint8))
    d = ImageDraw.Draw(im)
    for _ in range(8):
        x0, y0 = int(rng.integers(W)), int(rng.integers(H))
        d.ellipse([x0, y0, x0 + int(rng.integers(4, 25)), y0 + int(rng.integers(4, 25))],
                  fill=tuple(int(v) for v in rng.integers(0, 256, 3)))
    a = np.array(im)
    noise = rng.random((H, W)) < 0.05
    a[noise] = rng.integers(0, 256, (int(noise.sum()), 3))
    return a


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def pnm(path, a):
    a = np.asarray(a, dtype=np.uint8)
    magic = b"P6" if a.ndim == 3 else b"P5"
    with open(path, "wb") as f:
        f.write(magic + b"\n%d %d\n255\n" % (a.shape[1], a.shape[0]) + a.tobytes())


def reference(stream, name):
    """OpenJPEG's decode of `stream`, saved as the expected output."""
    src = os.path.join(TMP, name + os.path.splitext(stream)[1])
    out = os.path.join(TMP, name + ".png")
    run("opj_decompress", "-i", stream, "-o", out)
    a = np.array(Image.open(out))
    if a.ndim == 3 and a.shape[2] == 4:
        pnm(os.path.join(OUT, name + ".alpha.pgm"), a[:, :, 3])
        a = a[:, :, :3]
    if a.dtype != np.uint8:
        a = np.minimum((a.astype(np.int64) + 128) >> 8, 255)
    pnm(os.path.join(OUT, name + ".pnm"), a)


def encode(name, src, *args, ext="j2k"):
    path = os.path.join(OUT, f"{name}.{ext}")
    run("opj_compress", "-i", src, "-o", path, *args)
    reference(path, name)
    return path


def box(kind, body):
    return struct.pack(">I", 8 + len(body)) + kind + body


def jp2(codestream, ncomp, colr, extra=b""):
    ihdr = box(b"ihdr", struct.pack(">IIHBBBB", H, W, ncomp, 7, 7, 0, 0))
    return (box(b"jP  ", b"\r\n\x87\n") + box(b"ftyp", b"jp2 \0\0\0\0jp2 ")
            + box(b"jp2h", ihdr + colr + extra) + box(b"jp2c", codestream))


def markers(d):
    """(marker, start, end) for the main header and each tile-part."""
    out, pos = [], 2
    while pos + 4 <= len(d):
        m = d[pos + 1]
        length = struct.unpack(">H", d[pos + 2:pos + 4])[0]
        out.append((m, pos, pos + 2 + length))
        if m == 0x90:
            psot = struct.unpack(">I", d[pos + 6:pos + 10])[0]
            # skip to SOD, then the tile-part's data
            p = pos + 2 + length
            while d[p:p + 2] != b"\xff\x93":
                p += 2 + struct.unpack(">H", d[p + 2:p + 4])[0]
            out.append((0x93, p, pos + psot))
            pos += psot
        elif m == 0xD9:
            break
        else:
            pos += 2 + length
    return out


def split_packets(data):
    """Packet headers (up to and including EPH) and bodies of tile data
    written with SOP and EPH markers."""
    heads, bodies = [], []
    starts = []
    i = 0
    while True:
        i = data.find(b"\xff\x91", i)
        if i < 0:
            break
        starts.append(i)
        i += 6
    starts.append(len(data))
    for a, b in zip(starts, starts[1:]):
        packet = data[a + 6:b]
        e = packet.find(b"\xff\x92")
        heads.append(packet[:e + 2])
        bodies.append(packet[e + 2:])
    return heads, bodies


def packed_headers(path, name, where):
    """Moves packet headers into PPT (tile-part headers) or PPM (main
    header) markers."""
    d = open(path, "rb").read()
    ms = markers(d)
    first_sot = next(s for m, s, e in ms if m == 0x90)
    main = d[:first_sot]
    parts = []
    for k, (m, s, e) in enumerate(ms):
        if m == 0x90:
            sod = ms[k + 1]
            header = d[s:sod[1]]
            heads, bodies = split_packets(d[sod[1] + 2:sod[2]])
            parts.append((header, b"".join(heads), b"".join(bodies)))
    out = bytearray(main)
    if where == "ppm":
        ppm = b"".join(struct.pack(">I", len(h)) + h for _, h, _ in parts)
        out[len(main):] = b""
        out += b"\xff\x60" + struct.pack(">H", 3 + len(ppm)) + b"\x00" + ppm
    for header, heads, bodies in parts:
        extra = b""
        if where == "ppt":
            extra = b"\xff\x61" + struct.pack(">H", 3 + len(heads)) + b"\x00" + heads
        tp = header + extra + b"\xff\x93" + bodies
        # Fix Psot.
        tp = tp[:6] + struct.pack(">I", len(tp)) + tp[10:]
        out += tp
    out += b"\xff\xd9"
    dst = os.path.join(OUT, name + ".j2k")
    open(dst, "wb").write(bytes(out))
    return dst


def main():
    os.makedirs(OUT, exist_ok=True)
    a = source()
    rgb = os.path.join(TMP, "src.ppm")
    gray = os.path.join(TMP, "src.pgm")
    Image.fromarray(a).save(rgb)
    Image.fromarray(a[:, :, 1]).save(gray)

    small = ["-b", "16,16", "-n", "4"]
    encode("gray", gray)
    encode("rgb-layers", rgb, "-r", "40,10,1", *small)
    encode("rgb-97", rgb, "-I", "-r", "8", *small)
    for p in ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"]:
        encode("order-" + p.lower(), rgb, "-p", p, "-r", "20,5,1", "-b", "8,8",
               "-c", "[16,16],[16,16],[32,32]", "-t", "40,32")
    encode("poc", rgb, "-POC", "T0=0,0,2,4,3,LRCP/T0=0,0,3,6,3,RPCL", "-r", "20,5,1",
           "-t", "40,32", *small)
    encode("tile-offsets", rgb, "-t", "30,20", "-T", "3,5", "-d", "7,11", *small)
    encode("modes", rgb, "-M", "63", "-r", "20,5,1", *small)
    encode("modes-97", rgb, "-M", "63", "-I", "-r", "10,3", *small)
    encode("roi", rgb, "-ROI", "c=0,U=5", "-r", "10", *small)
    encode("subsampled", rgb, "-s", "2,2")
    sop = encode("sop-eph", rgb, "-SOP", "-EPH", "-r", "20,5,1", "-t", "40,32", *small)
    for where in ["ppm", "ppt"]:
        packed_headers(sop, "packed-" + where, where)
        # The same pixels as the stream they came from.
        shutil.copy(os.path.join(OUT, "sop-eph.pnm"), os.path.join(OUT, f"packed-{where}.pnm"))

    # 12-bit and signed samples from raw input.
    raw12 = os.path.join(TMP, "g12.raw")
    (a[:, :, 1].astype(">u2") * 16 + (a[:, :, 0] >> 4)).astype(">u2").tofile(raw12)
    path = os.path.join(OUT, "gray12.j2k")
    run("opj_compress", "-i", raw12, "-o", path, "-F", f"{W},{H},1,12,u@1x1")
    out = os.path.join(TMP, "g12.out.raw")
    run("opj_decompress", "-i", path, "-o", out)
    v = np.fromfile(out, dtype="<u2").astype(np.int64).reshape(H, W)
    pnm(os.path.join(OUT, "gray12.pnm"), np.minimum((v + 8) >> 4, 255))
    raws = os.path.join(TMP, "s8.raw")
    (a[:, :, 1].astype(np.int64) - 128).astype(np.int8).tofile(raws)
    path = os.path.join(OUT, "signed.j2k")
    run("opj_compress", "-i", raws, "-o", path, "-F", f"{W},{H},1,8,s@1x1")
    out = os.path.join(TMP, "s8.out.raw")
    run("opj_decompress", "-i", path, "-o", out)
    pnm(os.path.join(OUT, "signed.pnm"), np.fromfile(out, dtype=np.int8).astype(np.int64).reshape(H, W) + 128)

    # JP2: alpha through cdef, a palette, and sYCC.
    rgba = np.dstack([a, ((np.mgrid[0:H, 0:W][1] * 255) // W).astype(np.uint8)])
    png = os.path.join(TMP, "rgba.png")
    Image.fromarray(rgba).save(png)
    encode("rgba", png, *small, ext="jp2")

    idx = os.path.join(TMP, "idx.j2k")
    run("opj_compress", "-i", gray, "-o", idx)
    pal = np.random.default_rng(3).integers(0, 256, (256, 3))
    pclr = box(b"pclr", struct.pack(">HB", 256, 3) + bytes([7, 7, 7]) + bytes(pal.astype(np.uint8).flatten()))
    cmap = box(b"cmap", b"".join(struct.pack(">HBB", 0, 1, k) for k in range(3)))
    colr = box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 16))
    open(os.path.join(OUT, "palette.jp2"), "wb").write(jp2(open(idx, "rb").read(), 1, colr, pclr + cmap))
    pnm(os.path.join(OUT, "palette.pnm"), pal[a[:, :, 1]])

    # sYCC 4:2:0: Y full size, chroma at half size, colour converted by us.
    yuv = os.path.join(TMP, "yuv.raw")
    yy = a[:, :, 1]
    cb = a[::2, ::2, 2]
    cr = a[::2, ::2, 0]
    with open(yuv, "wb") as f:
        f.write(yy.tobytes() + cb.tobytes() + cr.tobytes())
    ycs = os.path.join(TMP, "yuv.j2k")
    run("opj_compress", "-i", yuv, "-o", ycs, "-F", f"{W},{H},3,8,u@1x1:2x2:2x2")
    colr = box(b"colr", bytes([1, 0, 0]) + struct.pack(">I", 18))
    path = os.path.join(OUT, "sycc.jp2")
    open(path, "wb").write(jp2(open(ycs, "rb").read(), 3, colr))
    reference(path, "sycc")


if __name__ == "__main__":
    main()
