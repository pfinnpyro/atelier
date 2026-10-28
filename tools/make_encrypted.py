#!/usr/bin/env python3
"""Writes the encrypted test files in crates/atelier-syntax/tests/data/encrypted
with pypdf, an independent implementation of PDF encryption.
Usage: make_encrypted.py OUT_DIR (needs reportlab and pypdf)."""

import sys
from reportlab.pdfgen import canvas
from pypdf import PdfReader, PdfWriter
out = sys.argv[1]
c = canvas.Canvas(out + "/plain.pdf", pagesize=(200, 100))
c.setTitle("Secret title")
c.setFillColorRGB(0.1, 0.4, 0.8)
c.rect(10, 10, 80, 80, fill=1, stroke=0)
c.setFillColorRGB(0, 0, 0)
c.setFont("Helvetica", 14)
c.drawString(100, 45, "Hidden")
c.save()
cases = [
    ("rc4-40", "", "owner", "RC4-40"),
    ("rc4-128", "user", "owner", "RC4-128"),
    ("aes-128", "user", "owner", "AES-128"),
    ("aes-256-r5", "user", "owner", "AES-256-R5"),
    ("aes-256", "pässwörd", "owner", "AES-256"),
]
for name, upw, opw, alg in cases:
    w = PdfWriter(clone_from=out + "/plain.pdf")
    w.encrypt(user_password=upw, owner_password=opw, algorithm=alg)
    w.write(f"{out}/{name}.pdf")
