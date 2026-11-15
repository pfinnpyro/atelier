#!/usr/bin/env python3
"""Public blank forms + fictional fill expectations. Never reads user documents.
Requires pypdf==6.1.1. See crates/atelier-fieldml/README.md.
"""
import argparse
import csv
import hashlib
import json
import re
from collections import defaultdict
from pathlib import Path
from pypdf import PdfReader, PdfWriter
from pypdf.generic import DecodedStreamObject, DictionaryObject, NameObject

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'crates/atelier-fieldml/data'
SOURCES = {
    'w9': ('train', 'https://www.irs.gov/pub/irs-pdf/fw9.pdf', 'March 2024'),
    'w7': ('validation', 'https://www.irs.gov/pub/irs-pdf/fw7.pdf', 'December 2024'),
    'w4': ('test', 'https://www.irs.gov/pub/irs-pdf/fw4.pdf', '2026'),
}
# Reviewed against printed labels and widget positions. Context is included
# explicitly; it is NOT inferred from opaque PDF field names. Taxpayer is
# assumed to be an individual. Unsupported composites must remain blank.
W9 = [
 ('FullName', 'Name of entity/individual'),
 ('Other', 'Business name/disregarded entity name'),
 ('Other', 'LLC tax classification'), ('Other', 'Other tax classification'),
 ('Other', 'Exempt payee code'), ('Other', 'FATCA reporting exemption code'),
 ('AddressLine1', 'Address (number, street, and apt. or suite no.)'),
 ('CityStateZip', 'City, state, and ZIP code'),
 ('Other', 'Requester name and address'), ('Other', 'List account numbers'),
 *[('Other', 'Social security number')]*3,
 *[('Other', 'Employer identification number')]*2,
]
W7 = [
 ('Other', 'Dependent relationship to U.S. citizen/resident alien'),
 ('Other', 'Spouse or sponsor SSN/ITIN'), ('Other', 'Other reason for application'),
 ('Other', 'Tax treaty additional information'), ('Other', 'Tax treaty country'),
 ('Other', 'Tax treaty article number'),
 ('FirstName', 'First name'), ('MiddleName', 'Middle name'), ('LastName', 'Last name'),
 ('Other', 'Name at birth if different: first name'),
 ('Other', 'Name at birth if different: middle name'),
 ('Other', 'Name at birth if different: last name'),
 ('AddressLine1', 'Applicant mailing street address, apartment number, or rural route number'),
 ('Other', 'Applicant mailing city or town, state or province, country and ZIP code'),
 ('Other', 'Foreign non-U.S. street address'),
 ('Other', 'Foreign non-U.S. city, state, country and postal code'),
 ('BirthDate', 'Date of birth (month / day / year)'),
 ('Other', 'Country of birth'), ('Other', 'City and state or province of birth'),
 ('Other', 'Countries of citizenship'), ('Other', 'Foreign tax ID number'),
 ('Other', 'U.S. visa type, number and expiration date'),
 ('Other', 'Other identification document'), ('Other', 'Identification document issued by'),
 ('Other', 'Identification document number'), ('Other', 'Identification document expiration date'),
 ('Other', 'Date of entry into the United States'),
 *[('Other', 'Previously issued ITIN')]*3,
 *[('Other', 'Previously issued IRSN')]*3,
 ('Other', 'Previously issued name: first name'),
 ('Other', 'Previously issued name: middle name'),
 ('Other', 'Previously issued name: last name'),
 ('Other', 'Name of college/university or company'),
 ('Other', 'College or company city and state'), ('Other', 'Length of stay'),
 ('Phone', 'Applicant phone number'), ('Other', 'Name of delegate'),
 ('Other', 'Acceptance agent phone'), ('Other', 'Acceptance agent fax'),
 ('Other', 'Acceptance agent name and title'), ('Other', 'Acceptance agent company name'),
 ('Other', 'Acceptance agent EIN'), ('Other', 'Acceptance agent PTIN'),
 ('Other', 'Acceptance agent office code'),
]
W4 = [
 ('Other', 'First name and middle initial'), # no such composite in 24 classes
 ('LastName', 'Last name'), ('AddressLine1', 'Address'),
 ('CityStateZip', 'City or town, state, and ZIP code'),
 ('Other', 'Social security number'),
 ('Other', 'Qualifying children credit'), ('Other', 'Other dependents credit'),
 ('Other', 'Total dependent and other credits'), ('Other', 'Other income'),
 ('Other', 'Deductions'), ('Other', 'Extra withholding'),
 ('Other', 'Employer name and address'), ('Other', 'First date of employment'),
 ('Other', 'Employer identification number'),
]


def clean(value):
    return ' '.join(str(value).split())


def annotate():
    lines = ['# split\tform\tfield\tclass\tlabel']
    manifest = []
    for source, (split, url, revision) in SOURCES.items():
        path = DATA / 'sources' / f'{source}.pdf'
        reader = PdfReader(path)
        fields = [(k, v) for k, v in reader.get_fields().items() if v.get('/FT') == '/Tx']
        primary = [v for k, v in fields if '.Page1[0].' in k]
        labels = {'w9': W9, 'w7': W7, 'w4': W4}[source]
        assert len(primary) == len(labels), (source, len(primary), len(labels))
        i = 0
        for key, value in fields:
            if '.Page1[0].' in key:
                klass, label = labels[i]
                i += 1
            else:
                klass, label = 'Other', f'Withholding worksheet amount {key.rsplit(".", 1)[-1]}'
            # These sources are intentionally metadata-poor benchmarks.
            assert not value.get('/TU'), (source, key, 'review annotation: source has changed')
            lines.append('\t'.join([split, 'public-' + source, key, klass, clean(label)]))
        manifest.append(dict(id='public-'+source, split=split, url=url, revision=revision,
            sha256=hashlib.sha256(path.read_bytes()).hexdigest(), bytes=path.stat().st_size,
            text_fields=len(fields), label_origin='human transcription with section context',
            scope='individual applicant; text fields only; unsupported details left blank',
            provenance='Public blank U.S. federal government form; no submitted or personal records.'))
    (DATA / 'online.tsv').write_text('\n'.join(lines) + '\n')
    (DATA / 'sources.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('Annotated', sum(s['text_fields'] for s in manifest), 'public blank text fields')


def load_forms(path):
    forms = defaultdict(list)
    for row in csv.reader(path.read_text().splitlines(), delimiter='\t'):
        if not row or row[0].startswith('#'): continue
        split, form, field, klass, label = row
        forms[(split, form)].append(dict(id=field, label=label, klass=klass))
    return forms


def profile(n):
    # Invented identities only. Reserved example.org and fictional 555 numbers.
    first = ['Avery', 'Morgan', 'Jordan', 'Taylor'][n % 4]
    last = ['Example', 'Sample', 'Demo', 'Testperson'][(n // 4) % 4]
    return dict(first=first, middle='Quinn', last=last, honorific='Mx',
        street=f'{100+n} Example Lane', unit=f'Unit {1+n%9}', city='Exampleville',
        state='NY', zip='10001', county='Example County', country='United States',
        email=f'person{n}@example.org', phone=f'202-555-{100+n%100:04}',
        birth_year='1990', birth_month='07', birth_day='23')


def expected(klass, label, fields, i, p):
    city = f'{p["city"]}, {p["state"]} {p["zip"]}'
    address = [p['street'], p['unit'], city, p['country']]
    name = f'{p["first"]} {p["middle"]} {p["last"]}'
    simple = dict(Other='', FullName=name, Honorific=p['honorific'], FirstName=p['first'],
        MiddleName=p['middle'], MiddleInitial=p['middle'][0], LastName=p['last'],
        FirstNameMiddleInitial=f'{p["first"]} {p["middle"][0]}', CityStateZipCountry=f'{city}, {p["country"]}', BirthMonth=p['birth_month'], BirthDay=p['birth_day'], BirthYear=p['birth_year'],
        AddressWhole=', '.join(address), City=p['city'], State=p['state'], Zip=p['zip'],
        CityStateZip=city, County=p['county'], Country=p['country'], Email=p['email'], Phone=p['phone'])
    normalized = re.sub('[^a-z]', '', label.lower())
    if klass == 'NameLastFirst':
        value = f'{p["last"]}, {p["first"]}'
        if 'middleinitial' in normalized or 'mi' in normalized: value += ', ' + p['middle'][0]
        elif 'middle' in normalized: value += ', ' + p['middle']
        return value
    if klass == 'BirthDate':
        y, m, d = p['birth_year'], p['birth_month'], p['birth_day']
        if 'yyyymmdd' in normalized: return f'{y}-{m}-{d}'
        if 'ddmmyyyy' in normalized: return f'{d}/{m}/{y}'
        if 'mmddyy' in normalized and 'mmddyyyy' not in normalized: return f'{m}/{d}/{y[-2:]}'
        return f'{m}/{d}/{y}'
    if klass in ['AddressLine1', 'AddressLine2', 'AddressOverflow']:
        # Determine the contiguous address block, preserving generator semantics.
        start = i
        while start and fields[start-1]['klass'] in ['AddressLine1', 'AddressLine2', 'AddressOverflow']:
            start -= 1
        end = i+1
        while end < len(fields) and fields[end]['klass'] in ['AddressLine1', 'AddressLine2', 'AddressOverflow']:
            end += 1
        separate_locality = end < len(fields) and fields[end]['klass'] in ['City','CityStateZip','CityStateZipCountry','County']
        offset = i-start
        if separate_locality:
            return ', '.join(address[:2]) if end-start == 1 else (address[offset] if offset < len(address) else '')
        if klass == 'AddressLine1' and end-start == 1:
            # A manually annotated street field may have a compound locality Other next.
            return ', '.join(address[:2])
        if i == end-1: return ', '.join(address[offset:])
        return address[offset] if offset < len(address) else ''
    return simple[klass]


# A tiny PDF writer keeps generated fixtures independent of a layout toolkit.
def literal(s):
    return '(' + s.replace('\\','\\\\').replace('(','\\(').replace(')','\\)') + ')'


def fixture_pdf(fields, values, title, columns):
    assert len(fields) <= 24, 'generator needs pagination for this form'
    objects = [b'', b'', b'', b'', b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>']
    content = f'BT /F1 12 Tf 30 760 Td {literal(title)} Tj ET\n'
    content += 'BT /F1 9 Tf 30 742 Td (FICTIONAL TRAINING EXAMPLE - NOT FOR SUBMISSION) Tj ET\n'
    refs = []
    rows = (len(fields) + columns - 1) // columns
    for i, (field, value) in enumerate(zip(fields, values)):
        col, row = divmod(i, rows)
        x, y = 30 + col * 280, 690 - row * (640 / max(rows, 1))
        width = 260 if columns == 2 else 550
        # Human-readable full labels remain in /TU even if display needs wrapping.
        words = field['label']; display = [words[j:j+int(width/5)] for j in range(0,len(words),int(width/5))]
        for j, text in enumerate(display):
            content += f'BT /F1 8 Tf {x} {y+18-j*9} Td {literal(text)} Tj ET\n'
        content += f'0.6 G {x} {y-16} {width} 23 re S\n'
        refs.append(f'{len(objects)+1} 0 R')
        # Appearance streams are embedded so filled values show without viewer regeneration.
        appearance = f'BT /F1 10 Tf 3 7 Td {literal(value)} Tj ET'.encode()
        ap_ref = len(objects)+2
        objects.append((f'<< /Type /Annot /Subtype /Widget /FT /Tx /T {literal(field["id"])} '
            f'/TU {literal(field["label"])} /Rect [{x} {y-16} {x+width} {y+7}] '
            f'/P 3 0 R /F 4 /V {literal(value)} /DA (/F1 10 Tf 0 g) /AP << /N {ap_ref} 0 R >> >>').encode())
        objects.append((f'<< /Type /XObject /Subtype /Form /BBox [0 0 {width} 23] '
            f'/Resources << /Font << /F1 5 0 R >> >> /Length {len(appearance)} >>\nstream\n').encode()+appearance+b'\nendstream')
    objects[0] = f'<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{" ".join(refs)}] /DR << /Font << /F1 5 0 R >> >> /DA (/F1 10 Tf 0 g) >> >>'.encode()
    objects[1] = b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>'
    objects[2] = f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Annots [{" ".join(refs)}] /Contents 4 0 R >>'.encode()
    stream = content.encode()
    objects[3] = f'<< /Length {len(stream)} >>\nstream\n'.encode()+stream+b'\nendstream'
    out = bytearray(b'%PDF-1.7\n')
    offsets = [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(out)); out += f'{i} 0 obj\n'.encode()+obj+b'\nendobj\n'
    xref = len(out)
    out += f'xref\n0 {len(objects)+1}\n0000000000 65535 f \n'.encode()
    for offset in offsets[1:]: out += f'{offset:010} 00000 n \n'.encode()
    out += f'trailer\n<< /Size {len(objects)+1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode()
    return out


def verify_pdf(path, values):
    fields = PdfReader(path).get_fields()
    actual = {k: str(v.get('/V', '')) for k, v in fields.items() if v.get('/FT') == '/Tx'}
    assert actual == values, (path, actual, values)


def materialize(out):
    forms = load_forms(out / 'synthetic.tsv')
    forms.update(load_forms(DATA / 'online.tsv'))
    dest = out / 'pdfs'; dest.mkdir(exist_ok=True)
    predictions = {}
    for row in csv.DictReader((out / 'predictions.tsv').open(), delimiter='\t'):
        predictions[(row['form'], row['field'])] = row['predicted']
    counts = defaultdict(int)
    examples = []
    for (split, form), fields in forms.items():
        public = form.startswith('public-')
        if not public and counts[split] >= {'train':32, 'validation':8, 'test':8}[split]: continue
        p = profile(len(examples))
        values = [expected(f['klass'], f['label'], fields, i, p) for i, f in enumerate(fields)]
        predicted = [expected(predictions[(form,f['id'])], f['label'], fields, i, p) for i,f in enumerate(fields)]
        # Classifier errors may be structurally impossible address positions;
        # expected() returns empty for unsupported overflow rather than guessing.
        blank = dest / f'{form}-blank.pdf'; filled = dest / f'{form}-expected.pdf'
        proposed = dest / f'{form}-predicted.pdf'
        if public:
            source = DATA / 'sources' / (form.removeprefix('public-') + '.pdf')
            blank.write_bytes(source.read_bytes())
            for path, vals in [(filled, values), (proposed, predicted)]:
                writer = PdfWriter(clone_from=source)
                mapping = dict(zip([f['id'] for f in fields], vals))
                writer.update_page_form_field_values(list(writer.pages), mapping, auto_regenerate=False)
                # Watermark every page so sample fills cannot be mistaken for submitted data.
                for page in writer.pages:
                    stream = DecodedStreamObject()
                    stream.set_data(b'q 0.8 0 0 rg BT /AtelierExampleFont 10 Tf 36 775 Td (FICTIONAL TRAINING EXAMPLE - NOT FOR SUBMISSION) Tj ET Q')
                    font = writer._add_object(DictionaryObject({NameObject('/Type'):NameObject('/Font'),NameObject('/Subtype'):NameObject('/Type1'),NameObject('/BaseFont'):NameObject('/Helvetica')}))
                    resources = page['/Resources'].get_object()
                    resources['/Font'][NameObject('/AtelierExampleFont')] = font
                    contents = page.get_contents()
                    original = contents.get_data() if contents is not None else b''
                    stream.set_data(b'q\n'+original+b'\nQ\n'+stream.get_data())
                    page.replace_contents(stream)
                with path.open('wb') as handle: writer.write(handle)
                reopened = PdfReader(path)
                for before, after in zip(PdfReader(source).pages, reopened.pages):
                    assert after.extract_text().startswith(before.extract_text()), 'original form content lost'
        else:
            columns = 1 if counts[split] % 2 == 0 else 2
            for path, vals in [(blank, ['']*len(fields)), (filled, values), (proposed,predicted)]:
                path.write_bytes(fixture_pdf(fields, vals, form, columns))
            counts[split] += 1
        for path, vals in [(blank,['']*len(fields)), (filled,values), (proposed,predicted)]:
            verify_pdf(path, dict(zip([f['id'] for f in fields], vals)))
        records = []
        for f, value, prediction in zip(fields, values, predicted):
            records.append(dict(**f, expected_value=value, action='leave_blank' if f['klass']=='Other' else 'fill',
                predicted_class=predictions[(form,f['id'])], predicted_value=prediction,
                correction_needed=predictions[(form,f['id'])] != f['klass']))
        example = dict(form=form, split=split, fictional_profile=p, fields=records)
        (dest / f'{form}.json').write_text(json.dumps(example,indent=2)+'\n')
        examples.append(example)
    (out / 'expectations.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in examples))
    print(f'Wrote and verified {len(examples)} blank/expected/predicted PDF triples plus JSON expectations')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['annotate', 'pdfs'])
    parser.add_argument('--out', type=Path, default=DATA / 'runs/first')
    args = parser.parse_args()
    if args.command == 'annotate': annotate()
    else: materialize(args.out)

if __name__ == '__main__': main()
