#!/usr/bin/env python3
"""Compares Atelier's glyph outlines with fontTools'. For each glyph it checks
the advance, the outline's signed area and its bounds.

Usage: fontcheck.py FONT [MAX_GLYPHS]   (needs `pip install fonttools`;
build first with `cargo build --release -p atelier-font --example fontcheck`)
       fontcheck.py --expect FONT OUT   writes fontTools' values for the
       atelier-font tests
"""

import os
import subprocess
import sys

from fontTools.pens.areaPen import AreaPen
from fontTools.pens.boundsPen import BoundsPen

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
EXE = os.path.join(ROOT, "target", "release", "examples", "fontcheck")


def reference(path, limit):
    """Yields (label, advance, area, bounds) in em units."""
    if path.lower().endswith((".pfb", ".pfa")):
        from fontTools.t1Lib import T1Font
        f = T1Font(path)
        f.parse()
        cs = f["CharStrings"]
        m = f.font["FontMatrix"][0]
        for name in list(cs.keys())[:limit]:
            g = cs[name]
            ap, bp = AreaPen(cs), BoundsPen(cs)
            g.draw(ap)
            g.draw(bp)
            yield name, g.width * m, ap.value * m * m, bp.bounds and [v * m for v in bp.bounds]
        return
    from fontTools.ttLib import TTFont
    f = TTFont(path, fontNumber=0)
    gs = f.getGlyphSet()
    upem = f["head"].unitsPerEm
    for gid, name in enumerate(f.getGlyphOrder()[:limit]):
        ap, bp = AreaPen(gs), BoundsPen(gs)
        gs[name].draw(ap)
        gs[name].draw(bp)
        adv = f["hmtx"][name][0] / f["head"].unitsPerEm
        yield str(gid), adv, ap.value / upem / upem, bp.bounds and [v / upem for v in bp.bounds]


def write_expected(path, out):
    with open(out, "w") as f:
        f.write("# label advance area x0 y0 x1 y1, from fontTools (tools/fontcheck.py --expect)\n")
        for label, adv, area, bounds in reference(path, 10**9):
            if bounds is None:
                f.write("%s %.6f empty\n" % (label, adv))
            else:
                f.write("%s %.6f %.6f %.5f %.5f %.5f %.5f\n" % ((label, adv, area) + tuple(bounds)))


def main():
    if sys.argv[1] == "--expect":
        write_expected(sys.argv[2], sys.argv[3])
        return
    path = sys.argv[1]
    limit = int(sys.argv[2]) if len(sys.argv) > 2 else 10**9
    out = subprocess.run([EXE, path, str(limit)], capture_output=True, text=True)
    if out.returncode != 0:
        print("atelier failed:", out.stderr.strip())
        sys.exit(1)
    ours = {}
    for line in out.stdout.splitlines():
        parts = line.split()
        ours[parts[0]] = parts[1:]
    bad = total = 0
    for label, adv, area, bounds in reference(path, limit):
        total += 1
        o = ours.get(label)
        problems = []
        if o is None:
            problems.append("missing")
        else:
            if abs(float(o[0]) - adv) > 1e-3:
                problems.append("advance %s vs %.4f" % (o[0], adv))
            if bounds is None:
                if o[1] not in ("empty", "none") and abs(float(o[1])) > 1e-6:
                    problems.append("expected empty, got area %s" % o[1])
            elif o[1] in ("empty", "none"):
                problems.append("expected outline, got %s" % o[1])
            else:
                a = float(o[1])
                if abs(abs(a) - abs(area)) > 1e-4 + 1e-3 * abs(area):
                    problems.append("area %.5f vs %.5f" % (a, area))
                b = [float(v) for v in o[2:6]]
                if max(abs(x - y) for x, y in zip(b, bounds)) > 2e-3:
                    problems.append("bounds %s vs %s" % (b, ["%.4f" % v for v in bounds]))
        if problems:
            bad += 1
            if bad <= 10:
                print("  glyph %s: %s" % (label, "; ".join(problems)))
    print("%s: %d of %d glyphs differ" % (os.path.basename(path), bad, total))


if __name__ == "__main__":
    main()
