#!/usr/bin/env python3
"""Writes small vector and image test PDFs (no text) for comparing Atelier's
renderer against a reference renderer. Usage: make_samples.py OUT_DIR"""

import os
import sys
import zlib


def pdf(objects, page_count_hint=None):
    """objects: list of bytes bodies, object 1 must be the catalog."""
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f\r\n" % (len(objects) + 1)
    for off in offsets:
        out += b"%010d 00000 n\r\n" % off
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    return bytes(out)


def stream(dict_body, data, compress=True):
    if compress:
        data = zlib.compress(data)
        dict_body += b" /Filter /FlateDecode"
    return b"<< " + dict_body + b" /Length %d >>\nstream\n" % len(data) + data + b"\nendstream"


def single_page(content, resources=b"<< >>", media=b"[0 0 400 400]", extra_objects=(), rotate=0):
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox " + media + b" /Rotate %d" % rotate
        + b" /Resources " + resources + b" /Contents 4 0 R >>",
        stream(b"", content),
    ]
    objs.extend(extra_objects)
    return pdf(objs)


SAMPLES = {}

SAMPLES["fills"] = single_page(b"""
1 0 0 rg 20 20 160 160 re f
0 0 1 rg 100 100 m 300 120 l 180 300 l h f
0 0.6 0 rg
220 20 m 380 20 l 380 180 l 220 180 l h 260 60 m 340 60 l 340 140 l 260 140 l h f*
0.5 g
220 220 m 380 220 l 380 380 l 220 380 l h 260 260 m 260 340 l 340 340 l 340 260 l h f
1 0.5 0 rg 100 300 m 100 355 145 380 170 380 c 195 380 240 355 240 300 c 240 245 195 220 170 220 c 145 220 100 245 100 300 c f
""")

SAMPLES["strokes"] = single_page(b"""
0 G 12 w 0 J 0 j 40 360 m 160 360 l S
1 J 40 320 m 160 320 l S
2 J 40 280 m 160 280 l S
0 J 0 j 10 M 220 240 m 280 360 l 340 240 l S
1 j 220 140 m 280 220 l 340 140 l S
2 j 220 40 m 280 120 l 340 40 l S
1 0 0 RG 4 w [12 6] 0 d 40 220 m 160 220 l S
[12 6 2 6] 3 d 40 190 m 160 160 l S
[] 0 d 0 0 1 RG 0 w 40 120 m 160 140 l S
0.5 w 40 100 m 160 120 l S
1 J [0 10] 0 d 6 w 40 60 m 160 60 l S
""")

SAMPLES["transforms"] = single_page(b"""
q 0.866 0.5 -0.5 0.866 200 200 cm 0 0.4 0.8 rg -80 -40 160 80 re f
0 0 0 RG 3 w -80 -40 160 80 re S Q
q 1 0 0 0.3 50 30 cm 0 0 0 RG 10 w 0 0 m 300 0 l S Q
q 2 0 0 2 0 0 cm 1 0 0 RG 1 w 20 150 m 40 180 l 60 150 l h S Q
""")

SAMPLES["clipping"] = single_page(b"""
q 200 200 m 200 310 290 380 200 380 c 110 380 90 310 200 200 c W n
0 0 1 rg 0 0 400 400 re f Q
q 20 20 150 150 re W n 90 90 m 300 90 l 300 300 l 90 300 l h W n
1 0 0 rg 0 0 400 400 re f Q
q 220 20 m 380 20 l 380 180 l 220 180 l h 260 60 m 340 60 l 340 140 l 260 140 l h W* n
0 0.7 0 rg 0 0 400 400 re f Q
""")

SAMPLES["colors"] = single_page(b"""
0 0 0 0.2 k 0 0 400 400 re f
1 0 0 0 k 20 300 80 80 re f
0 1 0 0 k 110 300 80 80 re f
0 0 1 0 k 200 300 80 80 re f
0.2 0.4 0.6 0.1 k 290 300 80 80 re f
/CS1 cs 50 60 -40 scn 20 200 80 80 re f
/CS2 cs 1 sc 110 200 80 80 re f
2 sc 200 200 80 80 re f
/GS1 gs 0 0 1 rg 20 20 200 150 re f
1 0 0 rg 120 70 200 150 re f
""", resources=b"""<< /ColorSpace << /CS1 [/Lab << /WhitePoint [0.9505 1 1.089] /Range [-128 127 -128 127] >>]
/CS2 [/Indexed /DeviceRGB 2 <FF000000FF000000FF>] >>
/ExtGState << /GS1 << /ca 0.5 /CA 0.5 >> >> >>""")


def gradient_image(w, h):
    rows = bytearray()
    for y in range(h):
        for x in range(w):
            rows += bytes([x * 255 // max(w - 1, 1), y * 255 // max(h - 1, 1), 128])
    return bytes(rows)


def checker_bits(w, h):
    out = bytearray()
    row_bytes = (w + 7) // 8
    for y in range(h):
        row = bytearray(row_bytes)
        for x in range(w):
            if (x // 2 + y // 2) % 2:
                row[x // 8] |= 0x80 >> (x % 8)
        out += row
    return bytes(out)


SAMPLES["images"] = single_page(
    b"""
q 160 0 0 120 20 260 cm /Im1 Do Q
q 120 0 0 120 220 260 cm /Im2 Do Q
0 0 1 rg q 160 0 0 160 20 40 cm /Im3 Do Q
q 0 120 -120 0 360 60 cm /Im1 Do Q
q 40 0 0 40 240 200 cm BI /W 2 /H 2 /BPC 8 /CS /G ID \x00\xff\xff\x00 EI Q
""",
    resources=b"<< /XObject << /Im1 5 0 R /Im2 6 0 R /Im3 7 0 R >> >>",
    extra_objects=[
        stream(b"/Type /XObject /Subtype /Image /Width 64 /Height 48 /ColorSpace /DeviceRGB /BitsPerComponent 8",
               gradient_image(64, 48)),
        stream(b"/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 8 0 R",
               bytes(range(0, 256, 16))),
        stream(b"/Type /XObject /Subtype /Image /Width 16 /Height 16 /ImageMask true", checker_bits(16, 16)),
        stream(b"/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8",
               bytes([255, 0, 255, 0] * 4)),
    ],
)

def jpeg_fixture(name):
    here = os.path.dirname(os.path.abspath(__file__))
    path = os.path.join(here, "..", "crates", "atelier-filters", "tests", "data", name + ".jpg")
    with open(path, "rb") as f:
        return f.read()


def jpeg_image(name, cs, extra=b""):
    data = jpeg_fixture(name)
    return stream(b"/Type /XObject /Subtype /Image /Width 37 /Height 23 /ColorSpace " + cs
                  + b" /BitsPerComponent 8 /Filter /DCTDecode " + extra, data, compress=False)


# The fixtures are 37 x 23. Adobe CMYK JPEGs store inverted samples, which
# PDFs undo with a Decode array.
SAMPLES["jpeg"] = single_page(
    b"""
q 180 0 0 112 10 280 cm /J1 Do Q
q 180 0 0 112 210 280 cm /J2 Do Q
q 180 0 0 112 10 150 cm /J3 Do Q
q 180 0 0 112 210 150 cm /J4 Do Q
q 180 0 0 112 10 20 cm /J5 Do Q
q 180 0 0 112 210 20 cm /J6 Do Q
""",
    resources=b"<< /XObject << /J1 5 0 R /J2 6 0 R /J3 7 0 R /J4 8 0 R /J5 9 0 R /J6 10 0 R >> >>",
    extra_objects=[
        jpeg_image("rgb420_baseline", b"/DeviceRGB"),
        jpeg_image("rgb420_progressive", b"/DeviceRGB"),
        jpeg_image("rgb422_baseline", b"/DeviceRGB"),
        jpeg_image("rgb420_progressive_restart", b"/DeviceRGB"),
        jpeg_image("gray_progressive", b"/DeviceGray"),
        jpeg_image("cmyk_baseline", b"/DeviceCMYK", b"/Decode [1 0 1 0 1 0 1 0]"),
    ],
)

def font_fixture(name):
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "..", "crates", "atelier-font", "tests", "data", name), "rb") as f:
        return f.read()


def pfb_parts(data):
    """Splits a PFB into the clear and binary parts PDF's FontFile wants."""
    parts, pos = [], 0
    while pos + 6 <= len(data) and data[pos] == 0x80 and data[pos + 1] != 3:
        n = int.from_bytes(data[pos + 2:pos + 6], "little")
        parts.append(data[pos + 6:pos + 6 + n])
        pos += 6 + n
    return parts[0], parts[1]


def font_stream(key_extra, data):
    return stream(b"/Length1 %d " % len(data) + key_extra, data)


def hex_utf16(text):
    return b"<" + text.encode("utf-16-be").hex().upper().encode() + b">"


def to_unicode(pairs):
    """A ToUnicode CMap for 2-byte codes: [(code, text)]."""
    body = b"".join(b"<%04X> %s\n" % (c, hex_utf16(t)) for c, t in pairs)
    return (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n"
            b"/CMapName /Test-UCS def 1 begincodespacerange <0000> <FFFF> endcodespacerange\n"
            + b"%d beginbfchar\n" % len(pairs) + body + b"endbfchar\nendcmap end end\n")


def text_fonts():
    from fontTools.ttLib import TTFont
    import io
    clear, binary = pfb_parts(font_fixture("charter.pfb"))
    lib = font_fixture("liberation.ttf")
    tt = TTFont(io.BytesIO(lib))
    cmap = tt.getBestCmap()
    order = tt.getGlyphOrder()
    cid_text = "Qfi\u00e9\u20ac"  # glyphs present in the subset
    gids = [order.index(cmap[ord(c)]) for c in cid_text]
    cid_hex = b"<" + b"".join(b"%04X" % g for g in gids) + b">"
    uni = font_fixture("unifont_cid.otf")
    content = b"""
BT /F1 18 Tf 20 360 Td (Helvetica: Quick brown fox) Tj ET
BT /F2 18 Tf 20 335 Td (Times-Bold, \\(escaped\\)) Tj ET
BT /F3 14 Tf 20 312 Td (Courier: monospace 0123) Tj ET
BT /F4 22 Tf 20 280 Td (Charter Type 1: Quality!) Tj ET
BT /F5 22 Tf 20 250 Td (\x80 AQ fi gh jk 0123 W&M@%) Tj ET
BT /F6 22 Tf 20 220 Td (QWM 0123 gh, jk; q!) Tj ET
BT /F7 22 Tf 20 190 Td """ + cid_hex + b""" Tj ET
BT /F8 24 Tf 20 150 Td <004200410042> Tj 0 -30 Td <6F23>Tj ET
BT /F9 30 Tf 220 130 Td (abc) Tj ET
BT /F1 12 Tf 20 60 Td [(T) 120 (ype) -500 (kerned) 250 (TJ)] TJ 0 -16 Td 3 Tc 2 Tw (spaced out text) Tj
 0 Tc 0 Tw 0 -16 Td 150 Tz (wide) Tj 50 Tz ( narrow) Tj 100 Tz 5 Ts (rise) Tj ET
"""
    resources = (b"<< /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R /F4 8 0 R /F5 10 0 R /F6 12 0 R"
                 b" /F7 14 0 R /F8 18 0 R /F9 22 0 R >> >>")
    widths = lambda font, n: b"[" + b" ".join(b"%d" % font for _ in range(n)) + b"]"
    objs = [
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",  # 5
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold >>",  # 6
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",  # 7
        b"<< /Type /Font /Subtype /Type1 /BaseFont /CharterBT-Roman /FontDescriptor 9 0 R >>",  # 8
        b"<< /Type /FontDescriptor /FontName /CharterBT-Roman /Flags 34 /FontBBox [-200 -250 1100 950]"
        b" /ItalicAngle 0 /Ascent 900 /Descent -250 /CapHeight 700 /StemV 80 /FontFile 24 0 R >>",  # 9
        b"<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+LiberationSans /Encoding /WinAnsiEncoding"
        b" /FontDescriptor 11 0 R >>",  # 10
        b"<< /Type /FontDescriptor /FontName /ABCDEF+LiberationSans /Flags 32 /FontBBox [-200 -300 1200 1000]"
        b" /ItalicAngle 0 /Ascent 900 /Descent -210 /CapHeight 700 /StemV 80 /FontFile2 25 0 R >>",  # 11
        b"<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Inter /FontDescriptor 13 0 R"
        b" /Encoding << /Differences [32 /space 33 /exclam] >> >>",  # 12
        b"<< /Type /FontDescriptor /FontName /ABCDEF+Inter /Flags 32 /FontBBox [-200 -300 1200 1000]"
        b" /ItalicAngle 0 /Ascent 900 /Descent -210 /CapHeight 700 /StemV 80 /FontFile3 26 0 R >>",  # 13
        b"<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+LiberationSans /Encoding /Identity-H"
        b" /DescendantFonts [15 0 R] /ToUnicode 17 0 R >>",  # 14
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /ABCDEF+LiberationSans /CIDSystemInfo"
        b" << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 16 0 R /DW 600"
        b" /CIDToGIDMap /Identity >>",  # 15
        b"<< /Type /FontDescriptor /FontName /ABCDEF+LiberationSans /Flags 4 /FontBBox [-200 -300 1200 1000]"
        b" /ItalicAngle 0 /Ascent 900 /Descent -210 /CapHeight 700 /StemV 80 /FontFile2 25 0 R >>",  # 16
        stream(b"", to_unicode(list(zip(gids, cid_text)))),  # 17
        b"<< /Type /Font /Subtype /Type0 /BaseFont /Unifont /Encoding /Identity-H"
        b" /DescendantFonts [19 0 R] >>",  # 18
        b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /Unifont /CIDSystemInfo"
        b" << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 20 0 R"
        b" /DW 1000 /W [66 [500 500]] >>",  # 19
        b"<< /Type /FontDescriptor /FontName /Unifont /Flags 4 /FontBBox [0 -125 1000 875]"
        b" /ItalicAngle 0 /Ascent 875 /Descent -125 /CapHeight 700 /StemV 80 /FontFile3 21 0 R >>",  # 20
        stream(b"/Subtype /OpenType", uni),  # 21
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0]"
        b" /CharProcs 23 0 R /Encoding << /Type /Encoding /Differences [97 /square /triangle /diamond] >>"
        b" /FirstChar 97 /LastChar 99 /Widths [1000 1000 1000] /Resources << >> >>",  # 22
        b"<< /square %d 0 R /triangle %d 0 R /diamond %d 0 R >>" % (27, 28, 29),  # 23
        stream(b"/Length1 %d /Length2 %d /Length3 0" % (len(clear), len(binary)), clear + binary),  # 24
        font_stream(b"", lib),  # 25
        stream(b"/Subtype /Type1C", font_fixture("inter.cff")),  # 26
        stream(b"", b"1000 0 d0 0 0.6 0 rg 100 100 800 800 re f"),  # 27
        stream(b"", b"1000 0 0 0 1000 1000 d1 100 100 m 900 100 l 500 900 l f"),  # 28
        stream(b"", b"1000 0 d0 1 0 0 RG 60 w 500 100 m 900 500 l 500 900 l 100 500 l s"),  # 29
    ]
    return single_page(content, resources=resources, extra_objects=objs)


SAMPLES["text_fonts"] = text_fonts()

SAMPLES["text_modes"] = single_page(
    b"""
BT /F1 36 Tf 20 330 Td 0 Tr 0 0 1 rg (Fill) Tj ET
BT /F1 36 Tf 20 280 Td 1 Tr 1 w 1 0 0 RG (Stroke) Tj ET
BT /F1 36 Tf 20 230 Td 2 Tr 0 1 0 rg 0 0 0 RG (Both) Tj ET
BT /F1 36 Tf 20 180 Td 3 Tr (Invisible) Tj ET
q BT /F1 60 Tf 20 90 Td 7 Tr (CLIP) Tj ET 1 0.5 0 rg 0 80 400 30 re f 0 0.5 1 rg 0 110 400 40 re f Q
BT /F1 20 Tf 0 Tr 0 g 20 40 Td 1 0 0.3 1 0 0 Tm 20 40 Td (Skewed text) Tj ET
BT /F1 20 Tf 0.7071 0.7071 -0.7071 0.7071 300 200 Tm (Rotated) Tj ET
""",
    resources=b"<< /Font << /F1 5 0 R >> >>",
    extra_objects=[b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>"],
)

SAMPLES["forms"] = single_page(
    b"""
q 1 0 0 1 50 50 cm /F1 Do Q
q 0.5 0 0 0.5 220 220 cm /F1 Do Q
""",
    resources=b"<< /XObject << /F1 5 0 R >> >>",
    extra_objects=[
        stream(b"/Type /XObject /Subtype /Form /BBox [0 0 120 120] /Matrix [1 0 0 1 10 10]",
               b"1 0 0 rg 0 0 200 200 re f 0 0 1 rg 30 30 60 60 re f"),
    ],
)

SAMPLES["rotated"] = single_page(
    b"0 0 1 rg 20 20 100 50 re f 1 0 0 rg 300 500 80 80 re f",
    media=b"[0 0 400 600]", rotate=90)


def ps_function(domain, rng, code):
    return stream(b"/FunctionType 4 /Domain " + domain + b" /Range " + rng, code, compress=False)


SAMPLES["shadings"] = single_page(
    b"""
q 20 220 170 160 re W n /Ax sh Q
q 210 220 170 160 re W n /Rad sh Q
q 20 20 170 160 re W n /Fun sh Q
q 210 20 170 160 re W n /Stitch sh Q
""",
    resources=b"<< /Shading << /Ax 5 0 R /Rad 6 0 R /Fun 7 0 R /Stitch 8 0 R >> >>",
    extra_objects=[
        # 5: axial, extended both ways, red to blue.
        b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [60 300 150 300] /Extend [true true]"
        b" /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>",
        # 6: radial from a small circle to a big one, not extended.
        b"<< /ShadingType 3 /ColorSpace /DeviceRGB /Coords [270 280 5 300 300 75]"
        b" /Function << /FunctionType 2 /Domain [0 1] /C0 [1 1 0] /C1 [0 0.5 0] /N 1 >> >>",
        # 7: function-based: a sampled 2 x 2 grid stretched over the box.
        b"<< /ShadingType 1 /ColorSpace /DeviceRGB /Domain [0 1 0 1]"
        b" /Matrix [170 0 0 160 20 20] /Function 9 0 R >>",
        # 8: axial with a stitching function, in CMYK.
        b"<< /ShadingType 2 /ColorSpace /DeviceCMYK /Coords [210 20 380 180]"
        b" /Function << /FunctionType 3 /Domain [0 1] /Bounds [0.5] /Encode [0 1 0 1] /Functions ["
        b" << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>"
        b" << /FunctionType 2 /Domain [0 1] /C0 [0 1 0 0] /C1 [0 0 1 0] /N 2 >> ] >> >>",
        stream(
            b"/FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Size [2 2] /BitsPerSample 8",
            bytes([255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]),
        ),
    ],
)


def mesh_bytes(items):
    """items: tuples of (bits, value) packed big-endian."""
    out, acc, n = bytearray(), 0, 0
    for bits, value in items:
        acc = (acc << bits) | value
        n += bits
        while n >= 8:
            n -= 8
            out.append((acc >> n) & 0xFF)
    if n:
        out.append((acc << (8 - n)) & 0xFF)
    return bytes(out)


def coord(v):
    return (16, int(v / 400 * 65535))


def comp(v):
    return (8, int(round(v * 255)))


def type4():
    data = bytearray()
    verts = [
        (0, 20, 220, (1, 0, 0)),
        (0, 180, 220, (0, 1, 0)),
        (0, 100, 380, (0, 0, 1)),
        (1, 20, 380, (1, 1, 0)),
    ]
    for f, x, y, c in verts:
        data += mesh_bytes([(8, f), coord(x), coord(y)] + [comp(v) for v in c])
    return stream(
        b"/ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8"
        b" /BitsPerFlag 8 /Decode [0 400 0 400 0 1 0 1 0 1]",
        bytes(data),
    )


def type5():
    items = []
    for row in range(3):
        for col in range(3):
            x, y = 220 + col * 80, 220 + row * 80
            items += [coord(x), coord(y), comp(col / 2), comp(row / 2), comp(1 - col / 2)]
    return stream(
        b"/ShadingType 5 /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8"
        b" /VerticesPerRow 3 /Decode [0 400 0 400 0 1 0 1 0 1]",
        mesh_bytes(items),
    )


def patch(kind, x0, y0, bend):
    # Data order: p00 p01 p02 p03 p13 p23 p33 p32 p31 p30 p20 p10 (+ inner 4).
    w = 160
    pts = [
        (x0, y0), (x0 - bend, y0 + w / 3), (x0 + bend, y0 + 2 * w / 3), (x0, y0 + w),
        (x0 + w / 3, y0 + w + bend), (x0 + 2 * w / 3, y0 + w - bend), (x0 + w, y0 + w),
        (x0 + w + bend, y0 + 2 * w / 3), (x0 + w - bend, y0 + w / 3), (x0 + w, y0),
        (x0 + 2 * w / 3, y0 - bend), (x0 + w / 3, y0 + bend),
    ]
    if kind == 7:
        pts += [(x0 + w / 3, y0 + w / 3), (x0 + w / 3, y0 + 2 * w / 3),
                (x0 + 2 * w / 3, y0 + 2 * w / 3), (x0 + 2 * w / 3, y0 + w / 3)]
    items = [(8, 0)]
    for x, y in pts:
        items += [coord(x), coord(y)]
    for c in [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 0)]:
        items += [comp(v) for v in c]
    return stream(
        b"/ShadingType %d /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8"
        b" /BitsPerFlag 8 /Decode [0 400 0 400 0 1 0 1 0 1]" % kind,
        mesh_bytes(items),
    )


SAMPLES["meshes"] = single_page(
    b"/T4 sh /T5 sh /T6 sh /T7 sh",
    resources=b"<< /Shading << /T4 5 0 R /T5 6 0 R /T6 7 0 R /T7 8 0 R >> >>",
    extra_objects=[type4(), type5(), patch(6, 20, 20, 20), patch(7, 220, 20, 0)],
)

SAMPLES["patterns"] = single_page(
    b"""
/Pattern cs /Checks scn 20 220 170 160 re f
/P2 cs 0.8 0 0 /Dots scn 210 220 170 160 re f
/Pattern cs /Grad scn 20 20 170 160 re f
/Pattern CS /Grad SCN 18 w 230 40 m 370 160 l S
BT /F1 60 Tf 220 60 Td /Pattern cs /Checks scn (Ab) Tj ET
""",
    resources=b"<< /Pattern << /Checks 5 0 R /Dots 6 0 R /Grad 7 0 R >>"
    b" /ColorSpace << /P2 [/Pattern /DeviceRGB] >>"
    b" /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >> >> >>",
    extra_objects=[
        # 5: coloured 20 x 20 checkerboard cell, rotated a little.
        stream(
            b"/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20"
            b" /Matrix [0.94 0.34 -0.34 0.94 0 0] /Resources << >>",
            b"0 0 1 rg 0 0 10 10 re f 1 0.5 0 rg 10 10 10 10 re f",
        ),
        # 6: uncoloured dots with gaps between cells.
        stream(
            b"/PatternType 1 /PaintType 2 /TilingType 1 /BBox [0 0 10 10] /XStep 16 /YStep 16"
            b" /Resources << >>",
            b"5 0 m 10 0 10 10 5 10 c 0 10 0 0 5 0 c f",
        ),
        # 7: a shading pattern.
        b"<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 400 0]"
        b" /Function << /FunctionType 2 /Domain [0 1] /C0 [0 0.6 0.3] /C1 [0.6 0 0.6] /N 1 >>"
        b" /Extend [true true] >> >>",
    ],
)

SAMPLES["spot"] = single_page(
    b"""
/Orange cs 1 sc 20 220 170 160 re f
0.4 sc 210 220 170 160 re f
/Duo cs 1 0 sc 20 20 110 160 re f
0 1 sc 145 20 110 160 re f
0.5 0.5 sc 270 20 110 160 re f
""",
    resources=b"<< /ColorSpace << /Orange [/Separation /Orange /DeviceCMYK 5 0 R]"
    b" /Duo [/DeviceN [/Cyan /Pantone485] /DeviceCMYK 6 0 R] >> >>",
    extra_objects=[
        ps_function(b"[0 1]", b"[0 1 0 1 0 1 0 1]", b"{ dup 0.5 mul exch 0 exch 0 4 1 roll 3 1 roll }"),
        ps_function(b"[0 1 0 1]", b"[0 1 0 1 0 1 0 1]", b"{ 2 copy 0 exch 4 2 roll pop 0 0 4 2 roll pop }"),
    ],
)


SAMPLES["transparency"] = single_page(
    b"""
1 0.8 0 rg 20 20 360 360 re f
q /Mul gs 0 0.6 1 rg 40 220 150 150 re f Q
q /Scr gs 0.8 0 0.4 rg 210 220 150 150 re f Q
q /Lum gs 0.1 0.3 0.8 rg 40 40 150 150 re f Q
q /Half gs /G Do Q
""",
    resources=b"<< /ExtGState << /Mul << /BM /Multiply >> /Scr << /BM /Screen >>"
    b" /Half << /ca 0.5 /BM /HardLight >>"
    b" /Lum << /SMask << /S /Luminosity /G 6 0 R >> >> >>"
    b" /XObject << /G 5 0 R >> >>",
    extra_objects=[
        # 5: a group of two overlapping discs, composited as one at 50%.
        stream(
            b"/Type /XObject /Subtype /Form /BBox [200 30 380 200] /Group << /S /Transparency >>",
            b"0 0.5 0 rg 270 115 m 270 160 250 190 220 190 c 220 40 l 250 40 270 70 270 115 c f"
            b" 0 0 0.6 rg 220 60 140 120 re f",
        ),
        # 6: a soft mask fading from left to right, with a hole.
        stream(
            b"/Type /XObject /Subtype /Form /BBox [0 0 400 400]"
            b" /Group << /S /Transparency /CS /DeviceGray >>"
            b" /Resources << /Shading << /S << /ShadingType 2 /ColorSpace /DeviceGray"
            b" /Coords [40 0 190 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> >> >> >>",
            b"q 40 40 150 150 re W n /S sh Q 0 g 90 90 50 50 re f",
        ),
    ],
)


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "samples"
    os.makedirs(out_dir, exist_ok=True)
    for name, data in SAMPLES.items():
        with open(os.path.join(out_dir, name + ".pdf"), "wb") as f:
            f.write(data)
    print("wrote %d files to %s" % (len(SAMPLES), out_dir))


if __name__ == "__main__":
    main()
