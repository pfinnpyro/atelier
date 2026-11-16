#!/usr/bin/env python3
"""Round-3 holdout: four families no model or annotation has seen before.

The previous test families (rental-dixon, camp-vbac, volunteer-xavier,
membership-jas) were trained on in round 2, so they no longer test anything.
These four were picked, downloaded and annotated before any model was run on
them; their classes come from reading the forms, never from predictions. The
three calibration families are reused unchanged: calibration only fits a
temperature and cannot touch the learned heads (qualify_v3 checks that).

Profile subject: the applicant, or for the youth sports form the player.
Fields about anyone else (parents, guardians, emergency contacts, employers,
insurers, policy holders, pickup people), conditional "if different" or
"legal name" blocks, and details split into boxes a profile cannot fill (area
code apart from number, PIN and SSN digits) are Other: they are left blank.
A form field that appears under one name in several places is one field and
is listed once, where it first appears.

Extraction (the second track) uses fieldml_labels unchanged and never reads
the classes or gold labels written here.
"""
import csv
import hashlib
import io
import json
from pathlib import Path

import pymupdf as fitz

from fieldml_labels import clean, extract, label_rects

DATA = Path(__file__).resolve().parents[1] / 'crates/atelier-fieldml/data/v3'


def build():
    sources = json.loads((DATA / 'round3-sources.json').read_text(encoding='utf-8'))
    for s in sources:
        path = DATA / 'sources' / (s['id'] + '.pdf')
        if hashlib.sha256(path.read_bytes()).hexdigest() != s['sha256']:
            raise SystemExit(f"{s['id']}: missing or changed; run tools/fieldml_fetch.py")
    docs = {s['id']: fitz.open(DATA / 'sources' / (s['id'] + '.pdf')) for s in sources}
    fields = []

    def slot(id, page, label, klass, section, rect, origin='reviewed printable blank slot',
             native='', tooltip='', label_rect=None):
        fields.append(dict(source=id, page=page, field=native or f'p{page}-{len(fields):03}',
                           label=label, klass=klass, section=section, rect=list(rect),
                           label_rect=list(label_rect) if label_rect else None,
                           native_name=native, tooltip=tooltip, origin=origin))

    def right_of(id, page, anchor, klass, section, occ=0, label=None):
        """A blank to the right of a printed label (no drawn writing line)."""
        p = docs[id][page - 1]
        hits = label_rects(p, anchor)
        if occ >= len(hits):
            raise ValueError((id, page, anchor, occ, len(hits)))
        b = hits[occ]
        slot(id, page, label or anchor, klass, section,
             [b.x1 + 2, b.y0, min(p.rect.width - 18, b.x1 + 160), b.y1], label_rect=b)

    def native(id, rules):
        """Every native text field, once per name; `rules` maps a name to
        (class, printed label, section). Unlisted names are Other."""
        seen = set()
        for pi, p in enumerate(docs[id]):
            for w in p.widgets() or []:
                if w.field_type != fitz.PDF_WIDGET_TYPE_TEXT or w.field_name in seen:
                    continue
                seen.add(w.field_name)
                klass, label, section = rules.get(w.field_name, ('Other', w.field_name, ''))
                slot(id, pi + 1, label, klass, section, w.rect, origin='native text widget',
                     native=w.field_name, tooltip=w.field_label or '')

    # Dearborn library card (fillable). The mailing address is the primary
    # one; the home address is only "if different than mailing". Page 2 is
    # about a minor child and the consenting adults.
    name = 'Library Card for'
    mailing = 'Mailing Address'
    home = 'Home Address (if different than mailing)'
    native('library-dearborn', {
        'Last Name': ('LastName', 'Last', name),
        'First Name': ('FirstName', 'First', name),
        'Middle Initial': ('MiddleInitial', 'Middle Initial', name),
        'Mailing Street': ('AddressLine1', 'Number/Street', mailing),
        'Mailing Apt-Suite': ('AddressLine2', 'Apt./Suite', mailing),
        'Mailing City': ('City', 'City', mailing),
        'Mailing State': ('State', 'State', mailing),
        'Mailing Zip': ('Zip', 'Zip', mailing),
        'Home Street': ('Other', 'Number/Street', home),
        'Home Apt-Suite': ('Other', 'Apt./Suite', home),
        'Home City': ('Other', 'City', home),
        'Home State': ('Other', 'State', home),
        'Home Zip': ('Other', 'Zip', home),
        'Email Address': ('Email', 'Email Address', ''),
        'Preferred Area Code': ('Other', 'Preferred Phone Area Code', ''),
        'Preferred Phone': ('Other', 'Preferred Phone (without area code)', ''),
        'Other Area Code': ('Other', 'Other Phone Area Code', ''),
        'Other Phone': ('Other', 'Other Phone (without area code)', ''),
        'Date of Birth': ('BirthDate', 'Date of Birth mm/dd/yyyy', ''),
        "Driver's License": ('Other', "Driver's License # / State ID", ''),
        **{f'PIN{i}': ('Other', f'PIN digit {i}', '') for i in range(1, 5)},
        **{f'SS#{i}': ('Other', f'Social Security # last 4 digit {i}', '') for i in range(1, 5)},
        'Minor Last Name': ('Other', 'Last', 'Name of minor child'),
        'Minor First Name': ('Other', 'First', 'Name of minor child'),
        'Minor Middle Initial': ('Other', 'Middle Initial', 'Name of minor child'),
        'Minor Date of Birth': ('Other', "Child's date of birth", 'Name of minor child'),
        'Consent 1': ('Other', 'I give additional consent to 1)', 'Consent'),
        'Consent 1 Relation': ('Other', 'Relation to minor', 'Consent'),
        'Consent 2': ('Other', 'I give additional consent to 2)', 'Consent'),
        'Consent 2 Relation': ('Other', 'Relation to minor', 'Consent'),
        'Minor Type Name': ('Other', 'Print/type name', 'Parent declaration'),
        'Minor Today Date': ('Other', 'Date', 'Parent declaration'),
    })

    # LA Parks youth soccer (fillable). The player is the subject; one
    # "Address" box with no city/state/zip boxes takes the whole address.
    player = 'Player'
    native('sports-laparks', {
        'SPORT': ('Other', 'Sport', 'League'),
        'DIVISION': ('Other', 'Division', 'League'),
        'First Name': ('FirstName', 'First Name', player),
        'Las Name': ('LastName', 'Last Name', player),
        'Birth Date': ('BirthDate', 'Birth Date', player),
        'Age': ('Other', 'Age', player),
        'Grade': ('Other', 'Grade', player),
        'Height': ('Other', 'Height', player),
        'Weight': ('Other', 'Weight', player),
        'School': ('Other', 'School', player),
        'Name of sibling': ('Other', 'Name of sibling', player),
        'Address': ('AddressWhole', 'Address', player),
        'Parent 1': ('Other', 'Parent 1', 'Parents'),
        'Parent 2': ('Other', 'Parent 2', 'Parents'),
        'Parent 1 Phone': ('Other', 'Parent 1 Phone#', 'Parents'),
        'Parent 2 Phone': ('Other', 'Parent 2 Phone#', 'Parents'),
        'Parent 1 email': ('Other', 'Parent 1 email', 'Parents'),
        'Parent 2 email': ('Other', 'Parent 2 email', 'Parents'),
        'Emergency Name 1': ('Other', 'Name', 'Emergency contacts'),
        'Eme Ph 1': ('Other', 'Phone', 'Emergency contacts'),
        'Emergency Name 2': ('Other', 'Name', 'Emergency contacts'),
        'Eme Ph 2': ('Other', 'Phone', 'Emergency contacts'),
        'Coach phone': ('Other', 'Volunteer as coach: call me at', 'Volunteer'),
        'Date mm/dd/yyyy': ('Other', 'Date', 'Parent/player consent'),
        'First name, last name': ('Other', 'Name', 'Code of conduct'),
    })

    # KCLS library card (printable). Writing lines are drawn; each slot sits
    # just above its line, with its sublabel under the line.
    k = 'library-kcls'

    def line(label, klass, section, x0, y, x1):
        slot(k, 1, label, klass, section, [x0, y - 14, x1, y - 1])

    for label, klass, x0, x1 in [('Month', 'BirthMonth', 108, 176), ('Day', 'BirthDay', 180, 248),
                                 ('Year', 'BirthYear', 252, 320)]:
        line(label, klass, 'Full Birth Date', x0, 126, x1)
    for y, section, own in [(168, 'Name', True), (208, 'Legal Name (if not same as Name)', False)]:
        for label, klass, x0, x1 in [('Last', 'LastName', 63, 248), ('First', 'FirstName', 252, 428),
                                     ('Middle', 'MiddleName', 432, 558)]:
            line(label, klass if own else 'Other', section, x0, y, x1)
    for base, section, own in [(250, 'Residential Address', True),
                               (315, 'Mailing Address (if not same as Residential)', False)]:
        street = 'Street' if own else 'Street or PO Box'
        for label, klass, x0, dy, x1 in [(street, 'AddressLine1', 63, 0, 432), ('Apt #', 'AddressLine2', 468, 0, 556),
                                         ('City', 'City', 63, 24, 324), ('State', 'State', 360, 24, 422),
                                         ('Zip', 'Zip', 432, 24, 556)]:
            line(label, klass if own else 'Other', section, x0, base + dy, x1)
    for which, y, parts in [('Primary', 368, [(170, 226), (235, 290), (299, 431)]),
                            ('Secondary', 390, [(226, 281), (290, 346), (355, 487)])]:
        for i, (x0, x1) in enumerate(parts):
            line(f'{which} Telephone Number part {i + 1}', 'Other', 'Telephone', x0, y, x1)
    line('Email address', 'Email', 'Email', 63, 427, 556)
    for i, y in enumerate([526, 555, 584, 613]):
        line('First', 'Other', 'Parent(s) or guardian(s)', 63, y, 261)
        line('Last', 'Other', 'Parent(s) or guardian(s)', 288, y, 556)
    line('Persons authorized to pick up items on hold', 'Other', 'Authorized pickup', 63, 708, 439)

    # Jottful new patient intake (printable, no drawn lines): the blank is to
    # the right of each label.
    j = 'patient-jottful'
    for anchor, klass in [('Full Name:', 'FullName'), ('Date of Birth (MM/DD/YYYY):', 'BirthDate'),
                          ('SSN:', 'Other'), ('Gender:', 'Other'), ('Address:', 'AddressLine1'),
                          ('City:', 'City'), ('State:', 'State'), ('Zip Code:', 'Zip'),
                          ('Phone Number:', 'Phone'), ('Email Address:', 'Email')]:
        right_of(j, 1, anchor, klass, 'Patient Information')
    for anchor in ['Emergency Contact Name:', 'Emergency Contact Phone Number:',
                   'Relationship to Emergency Contact:']:
        right_of(j, 1, anchor, 'Other', 'Patient Information')
    for anchor in ['Employer Name:', 'Employer Address:', 'Work Phone:', 'Occupation:']:
        right_of(j, 2, anchor, 'Other', 'Employment Information')
    for anchor in ['Primary Insurance Provider:', 'Policy Holder Name:', 'Policy Number:',
                   'Group Number (if applicable):', 'Insurance Phone Number:',
                   'Billing Address (if different from patient):', 'City:', 'State:', 'Zip Code:',
                   'Secondary Insurance (if applicable):', 'Policy Holder Name (Secondary):',
                   'Policy Number (Secondary):']:
        right_of(j, 2, anchor, 'Other', 'Insurance and Billing Information')
    # "Name:" and "Address:" also end "Employer Name:", "Policy Holder Name:"
    # and "Employer Address:" earlier on the page.
    right_of(j, 2, 'Name:', 'Other', 'Emergency Contact', occ=2)
    right_of(j, 2, 'Address:', 'Other', 'Emergency Contact', occ=1)
    for anchor in ['Phone:', 'Relationship:']:
        right_of(j, 3, anchor, 'Other', 'Emergency Contact')
    for anchor in ['Is this visit related to an accident (Y/N)?',
                   'If so, indicate what type (auto, worker’s comp, etc.):']:
        right_of(j, 3, anchor, 'Other', 'Accident')
    for anchor in ['Signature:', 'Date:']:
        right_of(j, 3, anchor, 'Other', 'Signature')

    order = {s['id']: i for i, s in enumerate(sources)}
    fields.sort(key=lambda f: (order[f['source']], f['page'], round(f['rect'][1] / 5), f['rect'][0]))
    header = '# split\tform\tfield\tclass\tlabel\tsection'
    gold, actual = [header], [header]
    # The reserved calibration families, unchanged from the earlier round.
    for name, out in [('fresh-gold.tsv', gold), ('fresh-extracted.tsv', actual)]:
        out.extend(r for r in (DATA / name).read_text(encoding='utf-8').splitlines() if r.startswith('validation\t'))
    for f in fields:
        label, context, evidence = extract(docs[f['source']][f['page'] - 1], f)
        f.update(extracted_label=label, extracted_section=context, label_evidence=evidence)
        prefix = ['test', f['source'], f['field'], f['klass']]
        gold.append('\t'.join(prefix + [clean(f['label']) or f['field'], clean(f['section'])]))
        actual.append('\t'.join(prefix + [label or f['field'], context]))
    for s in sources:
        s['annotated_fields'] = sum(f['source'] == s['id'] for f in fields)
    (DATA / 'round3-sources.json').write_text(json.dumps(sources, indent=2) + '\n', encoding='utf-8')
    (DATA / 'round3-fields.json').write_text(json.dumps(fields, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    (DATA / 'round3-gold.tsv').write_text('\n'.join(gold) + '\n', encoding='utf-8')
    (DATA / 'round3-extracted.tsv').write_text('\n'.join(actual) + '\n', encoding='utf-8')
    rows = [r for r in csv.reader(io.StringIO('\n'.join(gold[1:])), delimiter='\t') if r[0] == 'test']
    print('Round-3 test slots:', len(fields), 'families:', len(sources),
          'positives:', sum(r[3] != 'Other' for r in rows))


if __name__ == '__main__':
    build()
