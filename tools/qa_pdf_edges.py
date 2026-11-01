#!/usr/bin/env python3
"""Dependency-free CLI smoke corpus. Run after cargo build -p atelier-cli.

Usage: python3 tools/qa_pdf_edges.py /tmp/atelier-qa
Writes disposable PDF/PNG fixtures and a JSON result log to the supplied folder.
"""
import json
from pathlib import Path
import re
import subprocess
import sys


def fixture():
    content = b"0 0.4 0.8 rg 20 20 120 120 re f BT /F1 24 Tf 30 300 Td (Atelier QA - Page One) Tj ET"
    second = b"BT /F1 20 Tf 0 100 Td (Rotated page two) Tj ET"

    def stream(data):
        return b"<< /Length %d >>\nstream\n" % len(data) + data + b"\nendstream"

    objects = [
        b"<< /Type /Catalog /Pages 2 0 R /OpenAction << /S /JavaScript /JS (app.alert(1)) >> >>",
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F1 6 0 R >> >> /Contents 4 0 R >>",
        stream(content),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [-100 -100 300 300] /Rotate 90 /Resources << /Font << /F1 6 0 R >> >> /Contents 7 0 R >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        stream(second),
    ]
    data = bytearray(b"%PDF-1.7\n")
    offsets = []
    for number, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(b"%d 0 obj\n" % number + obj + b"\nendobj\n")
    xref = len(data)
    data.extend(b"xref\n0 8\n0000000000 65535 f\r\n")
    for offset in offsets:
        data.extend(b"%010d 00000 n\r\n" % offset)
    data.extend(b"trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % xref)
    return bytes(data)


def main():
    root = Path(__file__).resolve().parent.parent
    destination = Path(sys.argv[1]).resolve()
    destination.mkdir(parents=True, exist_ok=True)
    base = fixture()
    # Preserve byte offsets in the wrong-length case to isolate stream repair.
    wrong_length = re.sub(rb"/Length (\d+)", lambda m: b"/Length " + b"1".rjust(len(m[1]), b"0"), base)
    cases = {
        "valid": base,
        "bad-startxref": re.sub(rb"startxref\n\d+", b"startxref\n1", base),
        "no-xref": base[:base.index(b"xref")],
        "wrong-stream-length": wrong_length,
        "truncated-eof": base[:-15],
        "empty": b"",
        "not-pdf": b"This is not a PDF",
    }
    results = []
    for name, data in cases.items():
        path = destination / (name + ".pdf")
        path.write_bytes(data)
        output = destination / (name + ".png")
        for args in (["info", str(path)], ["render", str(path), "all", "--dpi", "72", "-o", str(output)]):
            expected = 1 if name in ("empty", "not-pdf") else 0
            try:
                run = subprocess.run([str(root / "target/debug/atelier"), *args], capture_output=True, text=True, timeout=15)
                passed = run.returncode == expected
                results.append(dict(case=name, command=args[0], exit=run.returncode, passed=passed, output=run.stdout + run.stderr))
            except subprocess.TimeoutExpired:
                results.append(dict(case=name, command=args[0], passed=False, timeout=True))
    # Repair must preserve rendering, not merely return success.
    for name in list(cases)[1:5]:
        for page in (1, 2):
            actual = destination / f"{name}.png-{page}.png"
            reference = destination / f"valid.png-{page}.png"
            results.append(dict(case=name, command=f"render-equivalence-page-{page}", passed=actual.exists() and reference.exists() and actual.read_bytes() == reference.read_bytes()))
    (destination / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    failures = [result for result in results if not result["passed"]]
    print(f"{len(results) - len(failures)}/{len(results)} checks passed; details: {destination / 'results.json'}")
    return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
