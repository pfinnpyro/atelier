#!/usr/bin/env python3
"""Compiles Adobe's predefined CJK CMaps into
crates/atelier-render/src/font/data/cmaps.bin.

Source: Adobe's cmap-resources (BSD-3-Clause, notice in
crates/atelier-render/src/font/data/ADOBE-CMAPS-LICENSE.txt), read from
poppler-data's copy (/usr/share/poppler/cMap). Only the cMap directory
is used; poppler-data's other tables are under the GPL.

UTF-8 CMaps are left out (PDF does not use them), and so are the big
UTF-16 horizontal CMaps: the reader derives those from the UTF-32 ones.

The UCS-2 CMaps and UniJIS2004 are stored as the UTF-32 CMap of their
collection plus the codes that differ; they then also map the codes the
UTF-32 CMap adds, which is harmless. JIS X 0212/0213 CMaps, which PDF does
not predefine, are left out.

The reader also inverts each collection's UTF-32 CMap into a CID to
Unicode table, for text extraction and for showing CJK fonts that are not
embedded.

Format, before deflate (all integers LEB128 unless noted; names are a
length then bytes):
  "ACM1"
  orderings: count, then per ordering its name and its UTF-32 CMap's name
  cmaps: count, then per CMap: name, parent name (empty if none),
    ordering index, wmode (byte), codespace count, per range: byte
    length then low and high bytes, then range count and per range:
    zigzag(low - previous high), high - low, zigzag(cid - previous end)

Usage: gen_cmaps.py [CMAP_DIR]   (output is committed)
"""

import os
import re
import sys
import zlib

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
OUT = os.path.join(ROOT, "crates", "atelier-render", "src", "font", "data")
SRC = sys.argv[1] if len(sys.argv) > 1 else "/usr/share/poppler/cMap"
COLLECTIONS = ["Adobe-Japan1", "Adobe-GB1", "Adobe-CNS1", "Adobe-Korea1", "Adobe-KR"]
UTF32 = {
    "Japan1": "UniJIS-UTF32-H",
    "GB1": "UniGB-UTF32-H",
    "CNS1": "UniCNS-UTF32-H",
    "Korea1": "UniKS-UTF32-H",
    "KR": "UniAKR-UTF32-H",
}


def leb(n):
    assert n >= 0
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def zz(n):
    return leb(n * 2 if n >= 0 else -n * 2 - 1)


def name(s):
    b = s.encode()
    return leb(len(b)) + b


HEX = re.compile(rb"<([0-9A-Fa-f]*)>")


def parse(path):
    text = open(path, "rb").read()
    text = re.sub(rb"%[^\n\r]*", b"", text)
    cm = {"parent": "", "wmode": 0, "codespace": [], "ranges": [], "ordering": None}
    m = re.search(rb"/(\S+)\s+usecmap", text)
    if m:
        cm["parent"] = m.group(1).decode()
    m = re.search(rb"/WMode\s+(\d)\s+def", text)
    if m:
        cm["wmode"] = int(m.group(1))
    m = re.search(rb"/Ordering\s+\((\w+)\)", text)
    if m:
        cm["ordering"] = m.group(1).decode()
    for block in re.findall(rb"begincodespacerange(.*?)endcodespacerange", text, re.S):
        h = HEX.findall(block)
        for lo, hi in zip(h[0::2], h[1::2]):
            cm["codespace"].append((bytes.fromhex(lo.decode()), bytes.fromhex(hi.decode())))
    for kind, block in re.findall(rb"begincid(range|char)(.*?)endcid\1", text, re.S):
        toks = re.findall(rb"<[0-9A-Fa-f]*>|\d+", block)
        step = 3 if kind == b"range" else 2
        for i in range(0, len(toks) - step + 1, step):
            lo = int(toks[i][1:-1], 16)
            hi = int(toks[i + 1][1:-1], 16) if step == 3 else lo
            cid = int(toks[i + step - 1])
            r = cm["ranges"]
            if r and r[-1][1] + 1 == lo and r[-1][2] + r[-1][1] - r[-1][0] + 1 == cid:
                r[-1] = (r[-1][0], hi, r[-1][2])
            else:
                r.append((lo, hi, cid))
    return cm


DERIVED = {
    "UniJIS-UCS2-H": "UniJIS-UTF32-H",
    "UniJIS2004-UTF32-H": "UniJIS-UTF32-H",
    "UniGB-UCS2-H": "UniGB-UTF32-H",
    "UniCNS-UCS2-H": "UniCNS-UTF32-H",
    "UniKS-UCS2-H": "UniKS-UTF32-H",
}


def expand(cm):
    m = {}
    for lo, hi, cid in cm["ranges"]:
        for k in range(hi - lo + 1):
            m[lo + k] = cid + k
    return m


def main():
    cmaps = {}
    for coll in COLLECTIONS:
        d = os.path.join(SRC, coll)
        if not os.path.isdir(d):
            continue
        for f in sorted(os.listdir(d)):
            # Supplement and PostScript-only CMaps are not used by PDF.
            if f.startswith("Adobe-"):
                continue
            # UTF-8 CMaps are not used by PDF, and the UTF-16 ones are the
            # UTF-32 ones with codes re-encoded, so the reader derives them.
            if "-UTF8-" in f or f.endswith("-UTF16-H"):
                continue
            # JIS X 0212 and X 0213 CMaps are not among PDF's predefined ones.
            if f.startswith(("UniHojo", "Hojo", "UniJISX0213", "UniJISX02132004")):
                continue
            cm = parse(os.path.join(d, f))
            if not cm["codespace"] and not cm["parent"]:
                continue
            cmaps[f] = cm
    # Near copies of a UTF-32 CMap are stored as that CMap plus the codes
    # that differ, as if they used it with usecmap.
    for f, base in DERIVED.items():
        if f in cmaps and base in cmaps and not cmaps[f]["parent"]:
            mine, theirs = expand(cmaps[f]), expand(cmaps[base])
            diff = sorted((c, cid) for c, cid in mine.items() if theirs.get(c) != cid)
            ranges = []
            for c, cid in diff:
                if ranges and ranges[-1][1] + 1 == c and ranges[-1][2] + c - ranges[-1][0] == cid:
                    ranges[-1] = (ranges[-1][0], c, ranges[-1][2])
                else:
                    ranges.append((c, c, cid))
            cmaps[f]["ranges"] = ranges
            cmaps[f]["parent"] = base
    orderings = list(UTF32)
    blob = bytearray(b"ACM1")
    blob += leb(len(orderings))
    for o in orderings:
        blob += name(o) + name(UTF32[o])
    blob += leb(len(cmaps))
    for f, cm in sorted(cmaps.items()):
        ordering = cm["ordering"]
        # Inherit the collection from the parent when the CMap omits it.
        p = cm
        while ordering is None and p["parent"] in cmaps:
            p = cmaps[p["parent"]]
            ordering = p["ordering"]
        idx = orderings.index(ordering) + 1 if ordering in orderings else 0
        blob += name(f) + name(cm["parent"]) + leb(idx) + bytes([cm["wmode"]])
        blob += leb(len(cm["codespace"]))
        for lo, hi in cm["codespace"]:
            blob += bytes([len(lo)]) + lo + hi
        blob += leb(len(cm["ranges"]))
        prev_hi = -1
        prev_end = 0
        for lo, hi, cid in cm["ranges"]:
            blob += zz(lo - prev_hi) + leb(hi - lo) + zz(cid - prev_end)
            prev_hi = hi
            prev_end = cid + (hi - lo)
    packed = zlib.compress(bytes(blob), 9)
    os.makedirs(OUT, exist_ok=True)
    open(os.path.join(OUT, "cmaps.bin"), "wb").write(packed)
    print("%d CMaps, %d bytes raw, %d packed" % (len(cmaps), len(blob), len(packed)))


if __name__ == "__main__":
    main()
