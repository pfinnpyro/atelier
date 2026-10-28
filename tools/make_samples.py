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


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "samples"
    os.makedirs(out_dir, exist_ok=True)
    for name, data in SAMPLES.items():
        with open(os.path.join(out_dir, name + ".pdf"), "wb") as f:
            f.write(data)
    print("wrote %d files to %s" % (len(SAMPLES), out_dir))


if __name__ == "__main__":
    main()
