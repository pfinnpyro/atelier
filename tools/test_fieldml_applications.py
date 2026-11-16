"""Regression checks for source isolation, annotations, and label association."""
import csv
import hashlib
import json
import unittest
from collections import Counter

# These checks need pymupdf (tools/fieldml-requirements.txt) and the
# publishers' PDFs, which tools/fieldml_fetch.py downloads; the release
# workflow has neither, so they skip there.
try:
    import pymupdf as fitz
    from fieldml_applications import APP, DATA, extract
    # Every source, not just some: publishers take forms down, and a
    # partial set fails these checks for a reason that is not theirs.
    READY = all(
        (APP / (s['id'] + '.pdf')).is_file()
        for s in json.loads((DATA / 'applications.json').read_text(encoding='utf-8'))
    )
except ImportError:
    READY = False


@unittest.skipUnless(READY, 'needs pymupdf and every fetched source PDF')
class ApplicationCorpusTests(unittest.TestCase):
    def test_sources_and_families_are_pinned_and_disjoint(self):
        sources=json.loads((DATA/'applications.json').read_text(encoding='utf-8'))
        self.assertEqual(len(sources),11)
        self.assertEqual(len({s['id'] for s in sources}),11)
        self.assertEqual(len({s['family'] for s in sources}),11)
        self.assertEqual(len({s['sha256'] for s in sources}),11)
        self.assertEqual(Counter(s['split'] for s in sources),{'train':4,'validation':3,'test':4})
        for source in sources:
            self.assertRegex(source['id'],r'^[a-z0-9-]+$')
            self.assertEqual(hashlib.sha256((APP/(source['id']+'.pdf')).read_bytes()).hexdigest(),source['sha256'])

    def test_gold_and_extracted_slots_match_without_duplicate_locations(self):
        def rows(path):
            return [row for row in csv.reader(path.read_text(encoding='utf-8').splitlines(),delimiter='\t') if row and not row[0].startswith('#')]
        gold=rows(DATA/'applications-gold.tsv');actual=rows(DATA/'applications-extracted.tsv')
        self.assertEqual([r[:4] for r in gold],[r[:4] for r in actual])
        self.assertEqual(len(gold),434)
        slots=json.loads((DATA/'applications-fields.json').read_text(encoding='utf-8'))
        identities=[(f['source'],f['field']) for f in slots]
        positions=[(f['source'],f['page'],tuple(round(x,1) for x in f['rect'])) for f in slots]
        self.assertEqual(len(set(identities)),len(slots))
        self.assertEqual(len(set(positions)),len(slots))

    def test_geometry_extraction_does_not_read_gold_labels_or_classes(self):
        doc=fitz.open();page=doc.new_page()
        page.insert_text((20,40),'EMERGENCY CONTACT')
        page.insert_text((20,90),'Name:')
        field={'rect':[60,78,200,94],'native_name':'Text1','tooltip':'Text1',
               'label':'Applicant email GOLD MUST NEVER LEAK','klass':'Email','section':'Applicant'}
        label,section,evidence=extract(page,field)
        self.assertEqual(label,'Name:')
        self.assertEqual(section,'EMERGENCY CONTACT')
        self.assertTrue(evidence)
        field.update(label='Secret birthdate',klass='BirthDate',section='Patient')
        self.assertEqual(extract(page,field),(label,section,evidence))
        doc.close()


if __name__=='__main__': unittest.main()
