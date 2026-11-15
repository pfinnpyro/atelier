#!/usr/bin/env python3
"""v5 holdout: four families from four publishers no earlier round used.

Picked, downloaded, hash-pinned and annotated on 2026-11-14 **before any
model was run on them**, after the layout-aware model was developed on the
26 earlier forms (the v4 holdout included, which is spent). Annotations
follow crates/atelier-fieldml/LABELS.md (kind + group + role) and come from
reading the rendered pages, never from predictions.

    python3 tools/fieldml_fetch.py            # restores the PDFs
    cd tools && /tmp/atelier-fieldml-venv/bin/python fieldml_v5_holdout.py

Writes data/v5/holdout-{sources.json,fields.json,labels.tsv}, the model's
input holdout-corpus.tsv (reviewed and extracted label text) and
holdout-corpus-layout.tsv (page layout, as tools/fieldml_v4_layout.py).

Coverage: every native text field once per name, except widgets that are
not places to write (listed in SKIP with the reason). Extraction uses
fieldml_labels unchanged and never reads the annotations.
"""
import hashlib
import json
from pathlib import Path

import pymupdf as fitz

from fieldml_labels import clean, extract
from fieldml_v4_labels import KINDS, ROLES
from fieldml_v4_layout import num, page_records

DATA = Path(__file__).resolve().parents[1] / 'crates/atelier-fieldml/data/v5'

SKIP = {
    ('employment-bridgeport', 'Personal Information'): 'text widget drawn over the red section bar',
    ('employment-bridgeport', 'Additional Information'): 'text widget drawn over the red section bar',
    ('employment-bridgeport', 'Education'): 'text widget drawn over the red section bar',
    ('employment-bridgeport', 'Skills  Qualifications'): 'text widget drawn over the red section bar',
    ('employment-bridgeport', 'Employment History'): 'text widget drawn over the red section bar',
    ('employment-bridgeport', 'US Military Service'): 'text widget drawn over the red section bar',
    ('employment-bridgeport', 'Applicant Statement'): 'text widget drawn over the red section bar',
    ('parking-charleston', 'Make_2'): 'a 17 pt text widget standing in for the visitor-permit checkbox',
}


def build():
    sources = json.loads((DATA / 'holdout-sources.json').read_text())
    for s in sources:
        path = DATA / 'sources' / (s['id'] + '.pdf')
        if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest() != s['sha256']:
            raise SystemExit(f"{s['id']}: missing or changed; run tools/fieldml_fetch.py")
    docs = {s['id']: fitz.open(DATA / 'sources' / (s['id'] + '.pdf')) for s in sources}
    fields = []

    def native(id, rules):
        """Every native text field once per name. `rules` maps each name to
        (kind, group, role, printed label, section); none may be missing."""
        seen = set()
        for pi, p in enumerate(docs[id]):
            for w in p.widgets() or []:
                if w.field_type != fitz.PDF_WIDGET_TYPE_TEXT or w.field_name in seen:
                    continue
                seen.add(w.field_name)
                if (id, w.field_name) in SKIP:
                    continue
                if w.field_name not in rules:
                    raise SystemExit(f'{id}: no annotation for {w.field_name!r}')
                kind, group, role, label, section = rules[w.field_name]
                assert kind in KINDS and role in ROLES, (id, w.field_name, kind, role)
                fields.append(dict(source=id, page=pi + 1, field=w.field_name, label=label,
                                   kind=kind, group=group, role=role, section=section,
                                   rect=[round(v, 2) for v in w.rect], native_name=w.field_name,
                                   tooltip=w.field_label or '', origin='native text widget'))
        if extra := set(rules) - seen:
            raise SystemExit(f'{id}: annotated names not in the form: {sorted(extra)}')

    # --- Bridgeport, WV employment application (fillable, 4 pages). Five
    # employment-history blocks; three schools; military service is the
    # applicant's own; five references. Date ranges are Other, as in the
    # development forms; single dates are Date.
    A = ('applicant', 'applicant')
    pi, ai = 'Personal Information', 'Additional Information'
    rules = {
        'Last': ('LastName', *A, 'Name: (Last)', pi),
        'First': ('FirstName', *A, '(First)', pi),
        'Middle': ('MiddleName', *A, '(Middle)', pi),
        'Soc Sec No': ('Other', *A, 'Soc. Sec. No.', pi),
        'Other Names Used if applicable': (
            'FullName', 'applicant-other-names', 'applicant-alt', 'Other Names Used (if applicable):', pi),
        'Street': ('AddressLine1', *A, 'Address: Street', pi),
        'City': ('City', *A, 'City', pi),
        'State': ('State', *A, 'State', pi),
        'Zip': ('Zip', *A, 'Zip', pi),
        'Primary': ('Phone', *A, 'Telephone: Primary', pi),
        'Secondary': ('Phone', *A, 'Secondary', pi),
        'Other': ('Phone', *A, 'Other', pi),
        'Best time to call': ('Other', *A, 'Best time to call:', pi),
        'Email': ('Email', *A, 'Email:', pi),
        'Position desired': ('Other', *A, 'Position desired:', ai),
        'Date available to work': ('Date', *A, 'Date available to work:', ai),
        'Desired Salary': ('Other', *A, 'Desired Salary:', ai),
        'Text1': ('Other', *A, 'If Yes, please explain:', ai),
        'Summarize specific skills and qualifications acquired from employment or other experiences '
        'that are related to the position for which you are applying': (
            'Other', *A, 'Summarize specific skills and qualifications ...', 'Skills & Qualifications'),
        'Date': ('Date', *A, 'Date:', 'Applicant Statement'),
    }
    ed = 'Education'
    rules.update({
        'High School': ('Organization', 'school-1', 'provider', 'High School:', ed),
        'Location': ('AddressWhole', 'school-1', 'provider', 'Location:', ed),
        'Type of degreediplomacertificate': ('Other', 'school-1', 'provider', 'Type of degree/diploma/certificate:', ed),
    })
    for n, suf in [(2, ''), (3, '_2')]:
        g = f'school-{n}'
        rules.update({
            f'School{suf}': ('Organization', g, 'provider', 'School:', ed),
            f'From{suf}': ('Other', g, 'provider', 'From:', ed),
            f'To{suf}': ('Other', g, 'provider', 'To:', ed),
            f'Location_{n}': ('AddressWhole', g, 'provider', 'Location:', ed),
            f'Type of degreediplomacertificate_{n}': ('Other', g, 'provider', 'Type of degree/diploma/certificate:', ed),
        })
    eh = 'Employment History'
    summaries = {2: 'Summarize job responsibilitiesRow1_2', 3: 'Summarize job responsibilities',
                 4: 'Summarize job responsibilitiesRow1_3', 5: 'Summarize job responsibilitiesRow1_4'}
    for n in range(1, 6):
        g, suf = f'employer-{n}', '' if n == 1 else f'_{n}'
        rules.update({
            f'Company Name  Address{suf}': ('Organization', g, 'employer', 'Company Name & Address:', eh),
            f'From_{n + 2}': ('Other', g, 'employer', 'Dates Employed: From:', eh),
            f'To_{n + 2}': ('Other', g, 'employer', 'To:', eh),
            f'Starting Title  Salary{suf}': ('JobTitle', g, 'employer', 'Starting Title & Salary:', eh),
            f'Final Title  Salary{suf}': ('JobTitle', g, 'employer', 'Final Title & Salary:', eh),
            f'May we contact for reference  YES  NO{suf}': ('Other', g, 'employer', 'May we contact for reference?', eh),
            f'Company Phone{suf}': ('Phone', g, 'employer', 'Company Phone #:', eh),
            f'Reason for leaving this employer{suf}': ('Other', g, 'employer', 'Reason for leaving this employer:', eh),
        })
        if n in summaries:
            rules[summaries[n]] = ('Other', g, 'employer', 'Summarize job responsibilities:', eh)
    ms = 'U.S. Military Service'
    rules.update({
        'Branch of Service': ('Other', 'military', 'applicant', 'Branch of Service:', ms),
        'Rank  Type of Service': ('Other', 'military', 'applicant', 'Rank & Type of Service:', ms),
        'Dates From To': ('Other', 'military', 'applicant', 'Dates', ms),
        'undefined_4': ('Other', 'military', 'applicant', 'From:', ms),
        'undefined_5': ('Other', 'military', 'applicant', 'To:', ms),
        'Training  experience received': ('Other', 'military', 'applicant', 'Training & experience received:', ms),
    })
    rf = 'References'
    for n in range(1, 6):
        g = f'reference-{n}'
        rules.update({
            f'NameRow{n}': ('FullName', g, 'reference', 'Name', rf),
            f'CompanyTitleRow{n}': ('Organization', g, 'reference', 'Company/Title', rf),
            f'PhoneRow{n}': ('Phone', g, 'reference', 'Phone', rf),
            f'EmailRow{n}': ('Email', g, 'reference', 'Email', rf),
            f'Years KnownRow{n}': ('Other', g, 'reference', 'Years Known', rf),
        })
    native('employment-bridgeport', rules)

    # --- Bartlett Park District youth sport league registration (fillable).
    # Sports rule: the first participant is the applicant, further rows are
    # children; the primary contact, father and mother are guardians; the
    # coaching volunteer and the card holder are other people.
    G = ('guardian-primary', 'guardian')
    rules = {
        'Head of Household': ('FullName', *G, 'Primary Contact', ''),
        'Address': ('AddressLine1', *G, 'Address', ''),
        'City': ('City', *G, 'City', ''),
        'Zip': ('Zip', *G, 'Zip', ''),
        'Email Address': ('Email', *G, 'E-mail Address', ''),
        'Primary Contact Phone': ('PhoneArea', *G, 'Primary Contact Phone (area code)', ''),
        'undefined': ('PhoneLocal', *G, 'Primary Contact Phone', ''),
        'undefined_2': ('PhoneArea', *G, 'Cell Phone (area code)', ''),
        'undefined_3': ('PhoneLocal', *G, 'Cell Phone', ''),
        'cannot be honored': ('Other', *G, 'I understand that carpool ... cannot be honored. (Initial required.)', ''),
        'Fathers Name': ('FullName', 'guardian-father', 'guardian', "Father's Name:", ''),
        'Mothers Name': ('FullName', 'guardian-mother', 'guardian', "Mother's Name:", ''),
        'Name of the person interested in coaching or coordinating': (
            'FullName', 'volunteer', 'other', 'Name of the person interested in coaching or coordinating*:', ''),
        'Volunteer Email mandatory': ('Email', 'volunteer', 'other', '*Volunteer E-mail (mandatory):', ''),
        'Card Holders Name': ('FullName', 'cardholder', 'other', "Card Holder's Name", ''),
        'Expiration': ('Other', 'cardholder', 'other', 'Expiration', ''),
        'CID': ('Other', 'cardholder', 'other', 'CID#', ''),
        'Amount Charged': ('Other', 'cardholder', 'other', 'Amount Charged $', ''),
        'inclusion into the programs A two week notice is recommended 1': (
            'Other', *A, 'Please list any medications ... (line 1)', 'Special Modifications/A.D.A'),
        'inclusion into the programs A two week notice is recommended 2': (
            'Other', *A, 'Please list any medications ... (line 2)', 'Special Modifications/A.D.A'),
        'warning of risk assumption of risk': ('FullName', *A, "Participant's Name", 'Registration/Brochure Waiver & Release'),
        'Date': ('Date', *A, 'Date', 'Registration/Brochure Waiver & Release'),
    }
    for n in range(1, 5):
        g, role = ('applicant', 'applicant') if n == 1 else (f'child-{n}', 'child')
        school = 'School Name' if n == 1 else f'School Name {n}'
        rules.update({
            f'ParticipantRow{n}': ('FullName', g, role, 'Participant', ''),
            f'GenderRow{n}': ('Other', g, role, 'Gender', ''),
            f'Birth DateRow{n}': ('BirthDate', g, role, 'Birth Date', ''),
            f'GradeRow{n}': ('Other', g, role, 'Grade', ''),
            school: ('Organization', g, role, 'Attends (School Name)', ''),
            f'Uniform SizeRow{n}': ('Other', g, role, 'Uniform Size*', ''),
            f'Program NameRow{n}': ('Other', g, role, 'Program Name', ''),
            f'ID NumberRow{n}': ('Other', g, role, 'I.D. Number', ''),
        })
    native('youth-bartlett', rules)

    # --- City of Charleston, WV resident parking permit (fillable). The
    # resident is the applicant; each vehicle is a thing they own (kind
    # Other). The second vehicle's Make box is named "undefined".
    rules = {
        'Name': ('FullName', *A, 'Name', ''),
        'Home Address': ('AddressLine1', *A, 'Home Address', ''),
        'City': ('City', *A, 'City', ''),
        'State': ('State', *A, 'State', ''),
        'Zip Code': ('Zip', *A, 'Zip Code', ''),
        'Home Phone': ('Phone', *A, 'Home Phone #', ''),
        'Work': ('WorkPhone', *A, 'Work #', ''),
        'Cell': ('Phone', *A, 'Cell #', ''),
        'Date': ('Date', *A, 'Date', ''),
    }
    for n, (vin, lic, make, model, color) in [
            (1, ('Vehicle 1 VIN', 'License', 'Make', 'Model', 'Color')),
            (2, ('Vehicle 2 VIN', 'License_2', 'undefined', 'Model_2', 'Color_2'))]:
        g = f'vehicle-{n}'
        rules.update({
            vin: ('Other', g, 'applicant', f'Vehicle #{n} VIN', ''),
            lic: ('Other', g, 'applicant', 'License #', ''),
            make: ('Other', g, 'applicant', 'Make', ''),
            model: ('Other', g, 'applicant', 'Model', ''),
            color: ('Other', g, 'applicant', 'Color', ''),
        })
    native('parking-charleston', rules)

    # --- Columbia County, OR Sheriff Animal Control adoption application
    # (fillable). The adopter is the applicant. The animal applied for and
    # each current pet are things the applicant asks about or owns (kind
    # Other: a pet's name is not a person's name). Two answer lines carry
    # the name of the question above the previous one; they are labelled by
    # position.
    rules = {
        'Name of the animals that you are interested in': (
            'Other', 'animal-wanted', 'applicant', 'Name of the animal(s) that you are interested in:', ''),
        'Name': ('FullName', *A, 'Name:', ''),
        'Phone': ('Phone', *A, 'Phone:', ''),
        'Email Address': ('Email', *A, 'Email Address:', ''),
        'Drivers License Number': ('Other', *A, "Driver's License Number:", ''),
        'License Expiration Date': ('Date', *A, 'License Expiration Date:', ''),
        'If yes please explain': ('Other', *A, 'If yes, please explain:', ''),
        'Address': ('AddressWhole', *A, 'Address:', ''),
        'Other': ('Other', *A, 'Do you currently live in: Other', ''),
        'Rent If renting have you discussed with your landlord': (
            'Other', *A, 'If renting; have you discussed with your landlord:', ''),
        'How long have you lived at your current residence': (
            'Other', *A, 'How long have you lived at your current residence:', ''),
        'If less than 6 monthsprevious address': (
            'AddressWhole', 'applicant-previous', 'applicant-alt', 'If less than 6 months-previous address:', ''),
        'How many people are in your home  ages': ('Other', *A, 'How many people are in your home & ages?', ''),
        'Children': ('Other', *A, 'Children?', ''),
        'Please provide information on where the dog will be when you are not home': (
            'Other', *A, 'If no fence how do you intend on confining the dog to your property:', ''),
        'Please list any current pets': (
            'Other', *A, 'Please provide information on where the dog will be when you are not home:', ''),
        'Date': ('Date', *A, 'Date:', ''),
    }
    pets = 'Please list any current pets:'
    for n in range(1, 6):
        g = f'pet-{n}'
        rules.update({
            f'Name  SpeciesRow{n}': ('Other', g, 'applicant', 'Name & Species', pets),
            f'Male or FemaleRow{n}': ('Other', g, 'applicant', 'Male or Female', pets),
            f'SpayedNeuteredRow{n}': ('Other', g, 'applicant', 'Spayed/Neutered', pets),
            f'BreedRow{n}': ('Other', g, 'applicant', 'Breed', pets),
            f'Current on VaccinesRow{n}': ('Other', g, 'applicant', 'Current on Vaccines', pets),
            f'Years ownedRow{n}': ('Other', g, 'applicant', 'Years owned', pets),
        })
    native('adoption-columbia', rules)

    order = {s['id']: i for i, s in enumerate(sources)}
    fields.sort(key=lambda f: (order[f['source']], f['page'], round(f['rect'][1] / 5), f['rect'][0]))
    labels = ['# form\tpage\tfield\tkind\tgroup\trole\tlabel\tsection']
    corpus = ['# form\tpage\tfield\tkind\tgroup\trole\tlabel\tsection\text_label\text_section']
    for f in fields:
        label, context, evidence = extract(docs[f['source']][f['page'] - 1], f)
        f.update(extracted_label=label, extracted_section=context, label_evidence=evidence)
        row = [f['source'], str(f['page']), f['field'], f['kind'], f['group'], f['role'],
               clean(f['label']), clean(f['section'])]
        labels.append('\t'.join(row))
        corpus.append('\t'.join(row + [clean(label or f['field']), clean(context)]))
    layout = ['# P form page width height | F form field page x0 y0 x1 y1 | '
              'T form page x0 y0 x1 y1 size bold text | S form page x0 y0 x1 y1 filled stroked | '
              'L form page x0 y0 x1 y1']
    for s in sources:
        mine = [f for f in fields if f['source'] == s['id']]
        s['annotated_fields'] = len(mine)
        for f in mine:
            layout.append('\t'.join(['F', s['id'], ' '.join(f['field'].split()), str(f['page']), *num(*f['rect'])]))
        for n in sorted({f['page'] for f in mine}):
            widgets = [fitz.Rect(f['rect']) for f in mine if f['page'] == n]
            layout.extend('\t'.join(r) for r in page_records(s['id'], n, docs[s['id']][n - 1], widgets))
    (DATA / 'holdout-sources.json').write_text(json.dumps(sources, indent=2) + '\n')
    (DATA / 'holdout-fields.json').write_text(json.dumps(fields, indent=2, ensure_ascii=False) + '\n')
    (DATA / 'holdout-labels.tsv').write_text('\n'.join(labels) + '\n')
    (DATA / 'holdout-corpus.tsv').write_text('\n'.join(corpus) + '\n')
    (DATA / 'holdout-corpus-layout.tsv').write_text('\n'.join(layout) + '\n')
    print('v5 holdout fields:', len(fields), {s['id']: s['annotated_fields'] for s in sources})


if __name__ == '__main__':
    build()
