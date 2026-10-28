#!/usr/bin/env python3
"""Renders PDFs with Atelier and with poppler (pdftoppm) and reports how far
apart they are. Usage: refdiff.py [--dpi N] [--keep DIR] file.pdf...

Needs pdftoppm on PATH, Python with Pillow, and a release build of the CLI
(cargo build --release -p atelier-cli). Text is not drawn by Atelier yet, so
pages with text show differences until phase 4."""

import argparse
import glob
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageChops

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "atelier.exe" if os.name == "nt" else "atelier")


def compare(a, b):
    a, b = a.convert("RGB"), b.convert("RGB")
    w, h = min(a.width, b.width), min(a.height, b.height)
    a, b = a.crop((0, 0, w, h)), b.crop((0, 0, w, h))
    diff = ImageChops.difference(a, b).convert("L")
    hist = diff.histogram()
    total = w * h
    mean = sum(i * n for i, n in enumerate(hist)) / total
    big = sum(hist[32:]) / total
    return mean, big, diff


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dpi", type=int, default=100)
    ap.add_argument("--keep", help="write diff images here")
    ap.add_argument("files", nargs="+")
    args = ap.parse_args()

    worst = 0.0
    print("%-28s %5s %10s %12s" % ("file", "page", "mean diff", "pixels >32"))
    for path in args.files:
        with tempfile.TemporaryDirectory() as tmp:
            ours = os.path.join(tmp, "ours")
            subprocess.run([EXE, "render", path, "all", "--dpi", str(args.dpi), "-o", ours],
                           check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["pdftoppm", "-r", str(args.dpi), "-png", path, os.path.join(tmp, "ref")], check=True)
            refs = sorted(glob.glob(os.path.join(tmp, "ref-*.png")))
            for n, ref in enumerate(refs, start=1):
                mine = "%s-%d.png" % (ours, n)
                if not os.path.exists(mine):
                    print("%-28s %5d   missing" % (os.path.basename(path), n))
                    continue
                mean, big, diff = compare(Image.open(mine), Image.open(ref))
                worst = max(worst, big)
                print("%-28s %5d %10.2f %11.2f%%" % (os.path.basename(path), n, mean, big * 100))
                if args.keep:
                    os.makedirs(args.keep, exist_ok=True)
                    stem = "%s-%d" % (os.path.splitext(os.path.basename(path))[0], n)
                    diff.point(lambda v: min(255, v * 4)).save(os.path.join(args.keep, stem + "-diff.png"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
