#!/usr/bin/env python3
"""Page layout for the label-analysis model (v4): where fields sit, the text
around them with its typography, and the drawn boxes, bars and rules.

The Rust model reads only this file, never a PDF, so the same input can be
produced at run time by atelier's own parser.

    /tmp/atelier-fieldml-venv/bin/python tools/fieldml_v4_layout.py

writes crates/atelier-fieldml/data/v4/layout.tsv with one record per line,
tab separated, coordinates in PDF points from the page's top left:

    P form page width height
    F form field page x0 y0 x1 y1
    T form page x0 y0 x1 y1 size bold text      (one text line)
    S form page x0 y0 x1 y1 filled stroked      (a drawn rectangle)
    L form page x0 y0 x1 y1                     (a rule: a line or a hairline bar)
"""

import json
from pathlib import Path

import fitz

DATA = Path(__file__).resolve().parent.parent / "crates/atelier-fieldml/data"
FIELDS = [
    (DATA / "v3/development-fields-v2.json", DATA / "sources/applications"),
    (DATA / "v3/fresh-fields.json", DATA / "v3/sources"),
    (DATA / "v3/round3-fields.json", DATA / "v3/sources"),
    (DATA / "v4/holdout-fields.json", DATA / "v4/sources"),
]
OUT = DATA / "v4/layout.tsv"


def rect(v):
    return fitz.Rect(json.loads(v) if isinstance(v, str) else v)


def clean(s):
    s = "".join(c if c.isprintable() else " " for c in s)
    return " ".join(s.split())


def num(*xs):
    return [f"{x:.2f}" for x in xs]


def white(c):
    return c is None or all(v > 0.92 for v in c)


def page_records(form, number, page, widgets):
    out = [["P", form, str(number), *num(page.rect.width, page.rect.height)]]
    for block in page.get_text("dict")["blocks"]:
        for line in block.get("lines", []):
            spans = [s for s in line["spans"] if s["text"].strip()]
            if not spans:
                continue
            text = clean("".join(s["text"] for s in line["spans"]))
            if not text:
                continue
            box = fitz.Rect(line["bbox"])
            # Text typed into a field is a value, not layout.
            if any(w.contains(box) for w in widgets):
                continue
            chars = sum(len(s["text"].strip()) for s in spans)
            bold = sum(
                len(s["text"].strip())
                for s in spans
                if s["flags"] & 16 or "bold" in s["font"].lower() or "black" in s["font"].lower()
            )
            size = max(s["size"] for s in spans)
            out.append(["T", form, str(number), *num(*box), f"{size:.1f}", str(int(bold * 2 > chars)), text])
    for d in page.get_drawings():
        for item in d["items"]:
            if item[0] == "re":
                r = fitz.Rect(item[1])
            elif item[0] == "l":
                r = fitz.Rect(item[1], item[2])
                r.normalize()
                if r.width < 8 and r.height < 8:
                    continue
                out.append(["L", form, str(number), *num(*r)])
                continue
            else:
                continue
            r.normalize()
            if r.width < 3 and r.height < 3:
                continue
            if r.width <= 1.6 or r.height <= 1.6:
                if max(r.width, r.height) >= 8:
                    out.append(["L", form, str(number), *num(*r)])
                continue
            if r.width > page.rect.width * 0.97 and r.height > page.rect.height * 0.97:
                continue
            filled = "f" in d["type"] and not white(d.get("fill"))
            stroked = "s" in d["type"]
            if filled or stroked:
                out.append(["S", form, str(number), *num(*r), str(int(filled)), str(int(stroked))])
    return out


def main():
    records = []
    forms = 0
    for fields_path, folder in FIELDS:
        by_form = {}
        for f in json.loads(fields_path.read_text()):
            by_form.setdefault(f["source"], []).append(f)
        for form, fields in by_form.items():
            doc = fitz.open(folder / f"{form}.pdf")
            forms += 1
            pages = sorted({int(f["page"]) for f in fields})
            widgets = {n: [rect(f["rect"]) for f in fields if int(f["page"]) == n] for n in pages}
            for f in fields:
                records.append(["F", form, clean(f["field"]), str(f["page"]), *num(*rect(f["rect"]))])
            for n in pages:
                records.extend(page_records(form, n, doc[n - 1], widgets[n]))
    with OUT.open("w") as out:
        out.write("# P form page width height | F form field page x0 y0 x1 y1 | "
                  "T form page x0 y0 x1 y1 size bold text | S form page x0 y0 x1 y1 filled stroked | "
                  "L form page x0 y0 x1 y1\n")
        for r in records:
            out.write("\t".join(r) + "\n")
    kinds = {}
    for r in records:
        kinds[r[0]] = kinds.get(r[0], 0) + 1
    print(f"{OUT.name}: {forms} forms, {kinds}")


if __name__ == "__main__":
    main()
