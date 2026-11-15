#!/usr/bin/env python3
"""Draft and check kind/group/role annotations for the form-label model (v4).

The v4 model analyzes labels; it does not decide fills. Each field gets:

- kind:  the detail its label asks for, whoever it belongs to;
- group: the entity instance the field describes ("applicant", "guardian-1");
- role:  whose block that is (applicant, guardian, emergency, ...).

See crates/atelier-fieldml/LABELS.md for the vocabulary and rules.

    python3 tools/fieldml_v4_labels.py draft   # writes data/v4/labels-draft.tsv
    python3 tools/fieldml_v4_labels.py apply   # draft + review.txt -> labels.tsv
    python3 tools/fieldml_v4_labels.py check   # validates data/v4/labels.tsv
    python3 tools/fieldml_v4_labels.py corpus  # labels + extracted text -> corpus TSVs

The draft is a starting point for review only. `labels.tsv` is the reviewed
file; `draft` never writes it.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "crates/atelier-fieldml/data"
SOURCES = [
    ROOT / "v3/development-fields-v2.json",
    ROOT / "v3/fresh-fields.json",
    ROOT / "v3/round3-fields.json",
]
OUT = ROOT / "v4"
HEADER = ["form", "page", "field", "kind", "group", "role", "label", "section"]

KINDS = {
    # names
    "FullName", "FirstName", "MiddleName", "MiddleInitial", "LastName",
    "NameLastFirst", "FirstNameMiddleInitial", "Honorific",
    # birth
    "BirthDate", "BirthMonth", "BirthDay", "BirthYear", "Age",
    # address
    "AddressWhole", "AddressLine1", "AddressLine2", "WorkAddress",
    "StreetNumber", "StreetDirection", "StreetName", "StreetSuffix", "City", "State", "Zip",
    "CityState", "CityStateZip", "CityStateZipCountry", "County", "Country",
    # contact
    "Phone", "WorkPhone", "Fax", "PhoneArea", "PhoneLocal", "PhonePrefix", "PhoneLine", "Email",
    # work
    "Organization", "JobTitle",
    # structure
    "Relationship", "Signature", "Date", "Other",
}
ROLES = {
    "applicant", "applicant-alt", "guardian", "emergency", "spouse", "child",
    "employer", "reference", "provider", "other", "office",
}

# Old fill classes carry over as kinds unchanged.
CLASS_KIND = {k: k for k in KINDS}

ROLE_WORDS = [
    ("office", r"office use|staff use|for office|official use"),
    ("applicant-alt", r"if (not same|different)|previous|former|prior address|mailing address \(|other names|maiden|alias|legal name \(if"),
    ("emergency", r"emergency|in case of|secondary contact|primary contact"),
    ("guardian", r"parent|guardian|mother|father|responsible party|policy ?holder|insured|guarantor"),
    ("spouse", r"spouse|partner|husband|wife"),
    ("employer", r"employer|supervisor|employment|company|business"),
    ("reference", r"reference"),
    ("provider", r"physician|doctor|dentist|veterinar|clinic|hospital|insur|school|landlord|pharmacy"),
    ("child", r"child|children|dependent|second camper|sibling"),
]

KIND_WORDS = [
    ("Signature", r"signature|signed"),
    ("Relationship", r"relationship|relation to"),
    ("Email", r"e-?mail"),
    ("BirthDate", r"birth ?date|date of birth|\bdob\b|birthday"),
    ("Age", r"^age\b"),
    ("Date", r"\bdate\b"),
    ("PhoneArea", r"area code"),
    ("PhoneLocal", r"without area code"),
    ("Phone", r"phone|tel\b|telephone|cell|mobile|fax"),
    ("CityStateZip", r"city.*state.*zip"),
    ("City", r"\bcity\b|\btown\b"),
    ("State", r"\bstate\b"),
    ("Zip", r"\bzip|postal"),
    ("County", r"\bcounty\b"),
    ("Country", r"\bcountry\b"),
    ("AddressLine2", r"apt|unit|suite"),
    ("AddressWhole", r"address"),
    ("AddressLine1", r"street"),
    ("LastName", r"\blast\b|surname|family name"),
    ("FirstName", r"\bfirst\b|given name"),
    ("MiddleInitial", r"\bm\.?i\.?\b|middle initial"),
    ("MiddleName", r"\bmiddle\b"),
    ("FullName", r"\bname\b"),
    ("JobTitle", r"title|position|occupation"),
    ("Organization", r"employer|company|business name|insurance (provider|company)|school"),
]


def load_fields():
    fields = []
    for path in SOURCES:
        fields.extend(json.loads(path.read_text()))
    return fields


def guess_kind(f):
    if f["klass"] != "Other":
        return CLASS_KIND[f["klass"]]
    text = f["label"].lower()
    for kind, pat in KIND_WORDS:
        if re.search(pat, text):
            return kind
    return "Other"


def guess_role(f):
    text = f"{f['section']} {f['label']}".lower()
    if f["klass"] != "Other":
        return "applicant"
    for role, pat in ROLE_WORDS:
        if re.search(pat, text):
            return role
    return "applicant"


def guess_instance(f):
    text = f"{f['section']} {f['label']}".lower()
    m = re.search(r"#\s*(\d)|\b(\d)\b|(primary|secondary)", text)
    if not m:
        return None
    word = next(g for g in m.groups() if g)
    return {"primary": "1", "secondary": "2"}.get(word, word)


def draft():
    OUT.mkdir(exist_ok=True)
    rows = []
    for f in load_fields():
        role = guess_role(f)
        group = role
        inst = guess_instance(f)
        if inst and role not in ("applicant", "office"):
            group = f"{role}-{inst}"
        rows.append([f["source"], str(f["page"]), f["field"], guess_kind(f), group, role,
                     f["label"], f["section"]])
    path = OUT / "labels-draft.tsv"
    with path.open("w") as out:
        out.write("# " + "\t".join(HEADER) + "\n")
        for r in rows:
            out.write("\t".join(clean(c) for c in r) + "\n")
    print(f"wrote {len(rows)} rows to {path}")


def clean(s):
    return s.replace("\t", " ").replace("\n", " ")


def read_labels(path):
    rows = []
    for line in path.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        cols = line.split("\t")
        rows.append(dict(zip(HEADER, cols + [""] * (len(HEADER) - len(cols)))))
    return rows


def apply():
    rows = read_labels(OUT / "labels-draft.tsv")
    by_key = {(r["form"], r["field"]): r for r in rows}
    errors = 0
    for n, line in enumerate((OUT / "review.txt").read_text().splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        parts = line.split("|")
        if len(parts) != 5:
            print(f"review.txt:{n}: expected 5 columns"); errors += 1; continue
        form, field, *vals = parts
        row = by_key.get((form, field))
        if row is None:
            print(f"review.txt:{n}: unknown field {form}|{field}"); errors += 1; continue
        for col, val in zip(("kind", "group", "role"), vals):
            if val != ".":
                row[col] = val
    if errors:
        sys.exit(1)
    with (OUT / "labels.tsv").open("w") as out:
        out.write("# " + "\t".join(HEADER) + "\n")
        for r in rows:
            out.write("\t".join(r[c] for c in HEADER) + "\n")
    check()


def check(path=OUT / "labels.tsv"):
    rows = read_labels(path)
    expected = {(f["source"], f["field"]) for f in load_fields()}
    seen, errors = set(), []
    group_roles = {}
    for r in rows:
        key = (r["form"], r["field"])
        if key in seen:
            errors.append(f"duplicate {key}")
        seen.add(key)
        if key not in expected:
            errors.append(f"unknown field {key}")
        if r["kind"] not in KINDS:
            errors.append(f"{key}: unknown kind {r['kind']!r}")
        if r["role"] not in ROLES:
            errors.append(f"{key}: unknown role {r['role']!r}")
        if not re.fullmatch(r"[a-z][a-z0-9-]*", r["group"]):
            errors.append(f"{key}: bad group {r['group']!r}")
        prev = group_roles.setdefault((r["form"], r["group"]), r["role"])
        if prev != r["role"]:
            errors.append(f"{key}: group {r['group']} has roles {prev} and {r['role']}")
    for key in sorted(expected - seen):
        errors.append(f"missing {key}")
    for e in errors:
        print(e)
    forms = {r["form"] for r in rows}
    print(f"{len(rows)} rows, {len(forms)} forms, {len(errors)} errors")
    return not errors


EXTRACTED = [
    ROOT / "v3/development-extracted-v2.tsv",
    ROOT / "v3/fresh-extracted.tsv",
    ROOT / "v3/round3-extracted.tsv",
]
CORPUS_HEADER = HEADER[:6] + ["label", "section", "ext_label", "ext_section"]


def corpus():
    """Join the reviewed annotations with the extracted-label track.

    Writes v4/corpus.tsv (the 22 training forms) and v4/holdout-corpus.tsv.
    Rows keep form reading order; `label`/`section` are the reviewed text and
    `ext_label`/`ext_section` what tools/fieldml_labels.py read from the page.
    """
    for labels, extracted, out in [
        (OUT / "labels.tsv", EXTRACTED, OUT / "corpus.tsv"),
        (OUT / "holdout-labels.tsv", [OUT / "holdout-extracted.tsv"], OUT / "holdout-corpus.tsv"),
    ]:
        ext = {}
        for path in extracted:
            for line in path.read_text().splitlines():
                if not line or line.startswith("#"):
                    continue
                c = line.split("\t") + [""]
                ext[(c[1], c[2])] = (c[4], c[5])
        rows = read_labels(labels)
        missing = [(r["form"], r["field"]) for r in rows if (r["form"], r["field"]) not in ext]
        if missing:
            sys.exit(f"{out.name}: no extracted label for {missing[:3]}")
        with out.open("w") as f:
            f.write("# " + "\t".join(CORPUS_HEADER) + "\n")
            for r in rows:
                e = ext[(r["form"], r["field"])]
                f.write("\t".join([*(r[c] for c in HEADER), clean(e[0]), clean(e[1])]) + "\n")
        print(f"{out.name}: {len(rows)} rows")


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    if cmd == "draft":
        draft()
    elif cmd == "apply":
        apply()
    elif cmd == "corpus":
        corpus()
    elif cmd == "check":
        sys.exit(0 if check() else 1)
    else:
        sys.exit(f"unknown command {cmd}")
