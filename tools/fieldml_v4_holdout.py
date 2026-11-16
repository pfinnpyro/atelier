#!/usr/bin/env python3
"""v4 holdout: four families from four publishers no earlier round used.

Picked, downloaded, hash-pinned and annotated on 2026-11-14 **before any
model was run on them**. Annotations follow crates/atelier-fieldml/LABELS.md
(kind + group + role) and come from reading the forms, never from
predictions. Once anything trains on or tunes against these families they are
spent as a test.

    python3 tools/fieldml_fetch.py            # restores the PDFs
    cd tools && /tmp/atelier-fieldml-venv/bin/python fieldml_v4_holdout.py

Writes data/v4/holdout-{sources.json,fields.json,labels.tsv} and, for the
older pipeline, holdout-gold.tsv / holdout-extracted.tsv in the v3 layout
(class = kind for the applicant's own fields, Other for everything else).

Coverage: every native text field once per name, or every printable writing
slot with a printed label; checkboxes, circle-one choices and blanks inside
sentences are excluded. Extraction uses fieldml_labels unchanged and never
reads the annotations.
"""
import hashlib
import json
from pathlib import Path

import pymupdf as fitz

from fieldml_labels import clean, extract
from fieldml_v4_labels import KINDS, ROLES

DATA = Path(__file__).resolve().parents[1] / 'crates/atelier-fieldml/data/v4'

# Classes of the older fill classifier (src/lib.rs `Class`).
OLD_CLASSES = {
    'FullName', 'NameLastFirst', 'Honorific', 'FirstName', 'MiddleName', 'MiddleInitial',
    'LastName', 'BirthDate', 'BirthMonth', 'BirthDay', 'BirthYear', 'AddressWhole',
    'AddressLine1', 'AddressLine2', 'City', 'State', 'Zip', 'CityStateZip', 'County',
    'Country', 'Email', 'Phone', 'FirstNameMiddleInitial', 'CityStateZipCountry',
}


def build():
    sources = json.loads((DATA / 'holdout-sources.json').read_text(encoding='utf-8'))
    for s in sources:
        path = DATA / 'sources' / (s['id'] + '.pdf')
        if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest() != s['sha256']:
            raise SystemExit(f"{s['id']}: missing or changed; run tools/fieldml_fetch.py")
    docs = {s['id']: fitz.open(DATA / 'sources' / (s['id'] + '.pdf')) for s in sources}
    fields = []

    def slot(id, page, label, kind, group, role, section, rect,
             origin='reviewed printable blank slot', native='', tooltip=''):
        assert kind in KINDS and role in ROLES, (id, label, kind, role)
        fields.append(dict(source=id, page=page, field=native or f'p{page}-{len(fields):03}',
                           label=label, kind=kind, group=group, role=role, section=section,
                           rect=[round(v, 2) for v in rect], native_name=native,
                           tooltip=tooltip, origin=origin))

    def native(id, rules):
        """Every native text field once per name. `rules` maps each name to
        (kind, group, role, printed label, section); none may be missing."""
        seen = set()
        for pi, p in enumerate(docs[id]):
            for w in p.widgets() or []:
                if w.field_type != fitz.PDF_WIDGET_TYPE_TEXT or w.field_name in seen:
                    continue
                seen.add(w.field_name)
                if w.field_name not in rules:
                    raise SystemExit(f'{id}: no annotation for {w.field_name!r}')
                kind, group, role, label, section = rules[w.field_name]
                slot(id, pi + 1, label, kind, group, role, section, w.rect,
                     origin='native text widget', native=w.field_name,
                     tooltip=w.field_label or '')
        if extra := set(rules) - seen:
            raise SystemExit(f'{id}: annotated names not in the form: {sorted(extra)}')

    # --- Saint Paul dog license (fillable). The owner is the applicant. A dog's
    # details describe the applicant's dog (kind Other, never a person's
    # name or age); each dog's rabies clinic is a provider. The unlabeled line
    # under "Clinic Address" continues that address.
    owner = 'Pet Owner Information'
    rules = {
        'First Name': ('FirstName', 'applicant', 'applicant', 'First Name', owner),
        'Last Name': ('LastName', 'applicant', 'applicant', 'Last Name', owner),
        'Address': ('AddressLine1', 'applicant', 'applicant', 'Address', owner),
        'Apt': ('AddressLine2', 'applicant', 'applicant', 'Apt #', owner),
        'Zip Code': ('Zip', 'applicant', 'applicant', 'Zip Code', owner),
        'Primary Phone': ('Phone', 'applicant', 'applicant', 'Primary Phone', owner),
        'Alternate Phone': ('Phone', 'applicant', 'applicant', 'Alternate Phone', owner),
        'Email': ('Email', 'applicant', 'applicant', 'E-mail', owner),
        'Owners date of birth monthdateyear': (
            'BirthDate', 'applicant', 'applicant', "Owner's date of birth (month/date/year)", owner),
    }
    for n, suf in [(1, ''), (2, '_2'), (3, '_3')]:
        dog, vet = f'dog-{n}', f'vet-{n}'
        sec, rab = f'Dog Information #{n}', 'Rabies Vaccination Information'
        rules.update({
            f'Dog Name{suf}': ('Other', dog, 'applicant', 'Dog Name', sec),
            f'Dog Age{suf}': ('Other', dog, 'applicant', 'Dog Age', sec),
            f'Sex{suf}': ('Other', dog, 'applicant', 'Sex', sec),
            f'Breed{suf}': ('Other', dog, 'applicant', 'Breed', sec),
            f'Colors{suf}': ('Other', dog, 'applicant', 'Color(s)', sec),
            f'Microchip Brand and Number required for lifetime license{suf}': (
                'Other', dog, 'applicant', 'Microchip Brand and Number (required for lifetime license)', sec),
            f'Clinic Name{suf}': ('Organization', vet, 'provider', 'Clinic Name', rab),
            f'Clinic Address{suf}': ('AddressLine1', vet, 'provider', 'Clinic Address', rab),
            f'1{suf}': ('AddressLine2', vet, 'provider', 'Clinic Address (continued)', rab),
            f'2{suf}': ('Phone', vet, 'provider', 'Clinic Phone #', rab),
            f'Vaccination Expiration Date monthdateyear{suf}': (
                'Date', dog, 'applicant', 'Vaccination Expiration Date (month/date/year)', rab),
        })
    fees = 'Dog License Fees'
    for name, label in [('Dogs', '# Dogs ($80/Year)'), ('undefined', 'Fee ($80/Year)'),
                        ('X', '# Dogs ($20/Year)'), ('1_4', 'Fee ($20/Year)'),
                        ('X_2', '# Dogs ($140/Lifetime)'), ('2_4', 'Fee ($140/Lifetime)'),
                        ('Dogs_2', '# Dogs ($10/Year)'), ('X10', 'Fee ($10/Year)'),
                        ('Dogs_3', '# Dogs ($70/Lifetime)'), ('x70', 'Fee ($70/Lifetime)'),
                        ('Intact PD Only', '# Dogs Intact (PD Only)'), ('undefined_2', 'Fee Intact (PD Only)'),
                        ('Altered PD or DD', '# Dogs Altered (PD or DD)'), ('undefined_3', 'Fee Altered (PD or DD)')]:
        rules[name] = ('Other', 'applicant', 'applicant', label, fees)
    native('dog-stpaul', rules)

    # --- Montana DPHHS child care emergency contact (fillable). The child is
    # the applicant; two parents/guardians with home and work details; three
    # non-parent contacts authorized to pick up; the child's medical provider.
    child, pg = "Child", 'Parent or Guardian Contact Information'
    rules = {
        'Childs Name First Last': ('FullName', 'applicant', 'applicant', "Child's Name (First, Last)", child),
        'Date of Birth': ('BirthDate', 'applicant', 'applicant', 'Date of Birth', child),
    }
    for n, suf in [(1, ''), (2, '_2')]:
        g = f'guardian-{n}'
        rules.update({
            f'Name First Last{suf}': ('FullName', g, 'guardian', 'Name (First, Last)', pg),
            f'Relationship{suf}': ('Relationship', g, 'guardian', 'Relationship', pg),
            f'Home Address Street City Zip{suf}': ('AddressWhole', g, 'guardian', 'Home Address (Street, City, Zip)', pg),
            f'Primary Phone{suf}': ('Phone', g, 'guardian', 'Primary Phone', pg),
            f'Email Address{suf}': ('Email', g, 'guardian', 'Email Address', pg),
            f'Work Address Street City Zip{suf}': ('WorkAddress', g, 'guardian', 'Work Address (Street, City, Zip)', pg),
            f'Work Phone{suf}': ('WorkPhone', g, 'guardian', 'Work Phone', pg),
        })
    em = 'Required Emergency Contact Information – person other than parent or guardian that is authorized to pick up child'
    for n, (name, phone, rel) in enumerate([('Name First Last_3', 'Phone_0', 'Relationship_3'),
                                            ('Name First Last_4', 'Phone_1', 'Relationship_4'),
                                            ('Name First Last_5', 'Phone_2', 'Relationship_5')], 1):
        g = f'emergency-{n}'
        rules.update({name: ('FullName', g, 'emergency', 'Name (First, Last)', em),
                      phone: ('Phone', g, 'emergency', 'Phone', em),
                      rel: ('Relationship', g, 'emergency', 'Relationship', em)})
    med = 'Required Medical Information'
    rules.update({
        'Medical Provider': ('FullName', 'physician', 'provider', 'Primary Medical Care Provider', med),
        'Phone': ('Phone', 'physician', 'provider', 'Phone', med),
        'Health Concerns Please explainRow1': ('Other', 'applicant', 'applicant', 'Health Concerns (Please explain)', med),
        'AllergiesRow1': ('Other', 'applicant', 'applicant', 'Allergies', med),
        'Date': ('Date', 'guardian-signer', 'guardian', 'Date', 'Parent or Guardian Authorization'),
    })
    native('emergency-mt', rules)

    # --- Des Plaines Park District Health Club membership (fillable). The household's
    # named adult is the applicant. Participant rows may include the
    # applicant, but their owner is not stated: role other. The card holder
    # is stated separately from the member: role other.
    hh = 'Household'
    rules = {
        'Date': ('Date', 'applicant', 'applicant', 'Date', ''),
        'Household Last Name': ('LastName', 'applicant', 'applicant', 'Household Last Name', hh),
        'First Name': ('FirstName', 'applicant', 'applicant', 'First Name', hh),
        'Address': ('AddressLine1', 'applicant', 'applicant', 'Address', hh),
        'City': ('City', 'applicant', 'applicant', 'City', hh),
        'Zip': ('Zip', 'applicant', 'applicant', 'Zip', hh),
        'Primary Phone': ('Phone', 'applicant', 'applicant', 'Primary Phone', hh),
        'Secondary Phone': ('Phone', 'applicant', 'applicant', 'Secondary Phone', hh),
        'Email': ('Email', 'applicant', 'applicant', 'Email', hh),
        'Emergency Contact Name': ('FullName', 'emergency-1', 'emergency', 'Emergency Contact Name', hh),
        'Emergency Phone': ('Phone', 'emergency-1', 'emergency', 'Emergency Phone', hh),
        'Membership Number': ('Other', 'applicant', 'applicant', 'Membership Number', 'SilverSneakers / Prime / Renew Active'),
        'Text12': ('Other', 'applicant', 'applicant', 'Total: $', 'Participants'),
        'Members Name': ('FullName', 'applicant', 'applicant', "Member's Name (Please print)", 'Waiver and Release'),
        'If over 18': ('Signature', 'applicant', 'applicant', 'Signature (If over 18)', 'Waiver and Release'),
        'Credit Card Number': ('Other', 'cardholder', 'other', 'Credit Card Number', 'Payment'),
        'undefined': ('Other', 'cardholder', 'other', 'Credit Card Number (part 2)', 'Payment'),
        'undefined_2': ('Other', 'cardholder', 'other', 'Credit Card Number (part 3)', 'Payment'),
        'undefined_3': ('Other', 'cardholder', 'other', 'Credit Card Number (part 4)', 'Payment'),
        'Exp Date': ('Date', 'cardholder', 'other', 'Exp. Date', 'Payment'),
        'CVV': ('Other', 'cardholder', 'other', 'CVV', 'Payment'),
        'Card Holder Name': ('FullName', 'cardholder', 'other', 'Card Holder Name', 'Payment'),
        'Members Name Print': ('FullName', 'applicant', 'applicant', "Member's Name (Print)", 'Tivity Health Waiver'),
        'Date_2': ('Date', 'applicant', 'applicant', 'Date', 'Tivity Health Waiver'),
        'Emergency Contact Name_2': ('FullName', 'emergency-2', 'emergency', 'Emergency Contact Name', 'Tivity Health Waiver'),
        'Contact Phone Number': ('Phone', 'emergency-2', 'emergency', 'Contact Phone Number', 'Tivity Health Waiver'),
    }
    for n in range(1, 6):
        g = f'participant-{n}'
        rules.update({
            f'Participant First NameRow{n}': ('FirstName', g, 'other', 'Participant First Name', 'Participants'),
            f'MFRow{n}': ('Other', g, 'other', 'M/F', 'Participants'),
            f'Birth DateRow{n}': ('BirthDate', g, 'other', 'Birth Date', 'Participants'),
            f'FeeRow{n}': ('Other', g, 'other', 'Fee', 'Participants'),
        })
    native('fitness-dpparks', rules)

    # --- Philadelphia EH-40 student registration (printable). The student is
    # the applicant. Grid cells carry their label at the top; slots are the
    # writing area below it. Page 2 lines carry their label to the left or,
    # for emergency contacts, underneath.
    k = 'school-philasd'
    s1 = 'STUDENT INFORMATION - SECTION 1'
    for label, kind, x0, x1 in [('Last Name', 'LastName', 37, 170), ('First Name', 'FirstName', 170, 280),
                                ('M.I.', 'MiddleInitial', 280, 315)]:
        slot(k, 1, label, kind, 'applicant', 'applicant', s1, [x0 + 1, 114, x1 - 1, 135])
    for label, kind, x0, x1 in [('Date of Birth MONTH', 'BirthMonth', 315, 354),
                                ('Date of Birth DAY', 'BirthDay', 354, 403),
                                ('Date of Birth YEAR', 'BirthYear', 403, 448)]:
        slot(k, 1, label, kind, 'applicant', 'applicant', s1, [x0 + 1, 122, x1 - 1, 135])
    slot(k, 1, 'STUDENT ID NUMBER', 'Other', 'applicant', 'applicant', s1, [449, 114, 562, 135])
    for label, kind, x0, x1 in [('House No.', 'StreetNumber', 37, 109), ('Dir', 'StreetDirection', 109, 170),
                                ('Street Name', 'StreetName', 170, 272), ('St., Ave., Etc.', 'StreetSuffix', 354, 403),
                                ('Apt#', 'AddressLine2', 403, 448), ('Zip Code', 'Zip', 448, 487),
                                ('Phone Number', 'Phone', 487, 563)]:
        slot(k, 1, label, kind, 'applicant', 'applicant', s1, [x0 + 1, 149, x1 - 1, 169])
    for label, kind, y, x0, x1 in [('Country of Birth:', 'Other', 186, 377, 432),
                                   ('Home Primary Language', 'Other', 196, 404, 521),
                                   ('Date child first enrolled into a U.S. School', 'Date', 209, 451, 541)]:
        slot(k, 1, label, kind, 'applicant', 'applicant', s1, [x0, y - 11, x1, y - 1])
    s2 = 'STUDENT ENROLLMENT HISTORY - SECTION 2'
    for label, kind, y, x0, x1 in [('Public School', 'Organization', 260, 358, 483),
                                   ('Other City:', 'City', 273, 183, 260),
                                   ('Non Public School', 'Organization', 274, 371, 482)]:
        slot(k, 1, label, kind, 'school-previous', 'provider', s2, [x0, y - 11, x1, y - 1])
    for label, kind, x0, x1 in [('Date Last Attended', 'Date', 37, 110), ('Grade Last Attended', 'Other', 110, 179),
                                ('Name of School', 'Organization', 179, 238), ('Address', 'AddressLine1', 238, 388),
                                ('City', 'City', 388, 513), ('State', 'State', 513, 562)]:
        slot(k, 1, label, kind, 'school-previous', 'provider', s2, [x0 + 1, 294, x1 - 1, 307])
    for label, y, x0, x1 in [('Special Education Services: If yes, which state:', 393, 490, 563),
                             ('If yes, what', 419, 465, 563),
                             ('If yes, which state:', 446, 491, 560)]:
        slot(k, 1, label, 'Other', 'applicant', 'applicant', s2, [x0, y - 11, x1, y - 1])
    s3 = 'LANGUAGE SURVEY - SECTION 3'
    for i, y in enumerate([518, 531, 545, 559, 572, 586], 1):
        slot(k, 1, f'Other Language (question {i})', 'Other', 'applicant', 'applicant', s3, [489, y - 11, 539, y - 1])

    s4 = 'HOUSEHOLD INFORMATION - SECTION 4'
    for n, (x0, x1, lx) in enumerate([(68, 298, 41), (327, 564, 300)], 1):
        g = f'guardian-{n}'
        slot(k, 2, 'Parent / Guardian Name:', 'FullName', g, 'guardian', s4, [lx + 1, 129, x1 - 1, 141])
        slot(k, 2, 'Mother / Father / Stepparent / Guardian / Other', 'Relationship', g, 'guardian', s4,
             [x1 - 54, 144, x1, 154])
        slot(k, 2, 'Address:', 'AddressLine1', g, 'guardian', s4, [x0 + 4, 176, x1, 186])
        slot(k, 2, 'Address: (second line)', 'CityStateZip', g, 'guardian', s4, [x0 + 4, 191, x1, 201])
        slot(k, 2, 'Phone: (Home)', 'Phone', g, 'guardian', s4, [x0 + 22, 218, x1, 236])
        slot(k, 2, 'Phone: (Cell)', 'Phone', g, 'guardian', s4, [x0 + 22, 238, x1, 256])
        slot(k, 2, 'Phone: (Work)', 'WorkPhone', g, 'guardian', s4, [x0 + 22, 258, x1, 276])
        slot(k, 2, 'Email:', 'Email', g, 'guardian', s4, [x0 + 4, 285, x1, 295])
        slot(k, 2, 'Preferred Language for School Related Communications:', 'Other', g, 'guardian', s4,
             [lx + 190, 300, x1 - 1, 314])
    s5 = 'SIBLING INFORMATION - SECTION 5'
    for n, (y0, y1) in enumerate([(455, 468), (468, 480), (480, 493), (493, 506), (506, 524)], 1):
        for label, kind, x0, x1 in [('Name', 'FullName', 40, 221), ('D.O.B.', 'BirthDate', 221, 282),
                                    ('Current School', 'Organization', 282, 406), ('Grade', 'Other', 406, 452),
                                    ('Student ID# if available', 'Other', 452, 565)]:
            slot(k, 2, label, kind, f'child-{n}', 'child', s5, [x0 + 1, y0 + 1, x1 - 1, y1 - 1])
    s6 = 'EMERGENCY CONTACT INFORMATION - SECTION 6'
    for n, (y1, y2) in enumerate([(584, 604), (645, 665)], 1):
        g = f'emergency-{n}'
        slot(k, 2, 'Name', 'FullName', g, 'emergency', s6, [50, y1 - 12, 272, y1 - 1])
        slot(k, 2, 'Relationship', 'Relationship', g, 'emergency', s6, [313, y1 - 12, 451, y1 - 1])
        slot(k, 2, 'Phone (1)', 'Phone', g, 'emergency', s6, [50, y2 - 12, 272, y2 - 1])
        slot(k, 2, 'Phone (2)', 'Phone', g, 'emergency', s6, [313, y2 - 12, 555, y2 - 1])
    for n, y in enumerate([728, 776], 1):
        g = f'guardian-signer-{n}'
        slot(k, 2, 'Parent / Guardian Signature', 'Signature', g, 'guardian', '', [41, y - 14, 265, y - 1])
        slot(k, 2, 'Date', 'Date', g, 'guardian', '', [302, y - 14, 519, y - 1])

    order = {s['id']: i for i, s in enumerate(sources)}
    fields.sort(key=lambda f: (order[f['source']], f['page'], round(f['rect'][1] / 5), f['rect'][0]))
    header = '# split\tform\tfield\tclass\tlabel\tsection'
    gold, actual = [header], [header]
    labels = ['# form\tpage\tfield\tkind\tgroup\trole\tlabel\tsection']
    for f in fields:
        label, context, evidence = extract(docs[f['source']][f['page'] - 1], f)
        f.update(extracted_label=label, extracted_section=context, label_evidence=evidence)
        own = f['role'] == 'applicant' and f['group'] == 'applicant'
        f['klass'] = f['kind'] if own and f['kind'] in OLD_CLASSES else 'Other'
        prefix = ['test', f['source'], f['field'], f['klass']]
        gold.append('\t'.join(prefix + [clean(f['label']) or f['field'], clean(f['section'])]))
        actual.append('\t'.join(prefix + [label or f['field'], context]))
        labels.append('\t'.join([f['source'], str(f['page']), f['field'], f['kind'], f['group'],
                                 f['role'], clean(f['label']), clean(f['section'])]))
    for s in sources:
        s['annotated_fields'] = sum(f['source'] == s['id'] for f in fields)
    (DATA / 'holdout-sources.json').write_text(json.dumps(sources, indent=2) + '\n', encoding='utf-8')
    (DATA / 'holdout-fields.json').write_text(json.dumps(fields, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    (DATA / 'holdout-labels.tsv').write_text('\n'.join(labels) + '\n', encoding='utf-8')
    (DATA / 'holdout-gold.tsv').write_text('\n'.join(gold) + '\n', encoding='utf-8')
    (DATA / 'holdout-extracted.tsv').write_text('\n'.join(actual) + '\n', encoding='utf-8')
    print('v4 holdout fields:', len(fields), 'families:', len(sources),
          {s['id']: s['annotated_fields'] for s in sources})


if __name__ == '__main__':
    build()
