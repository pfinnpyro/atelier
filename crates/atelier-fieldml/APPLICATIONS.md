# Application benchmark and contextual classifier

> **Latest result (round 3, 2026-11-13): the round-2 candidate fails a fresh
> holdout.** See "Round 3" at the end. Automatic filling stays disabled.

This iteration adds non-bank application templates, hard negative examples,
section ownership, two composite outputs, independent page-label association,
and controlled training experiments. Every profile value is fictional. The
source files are publicly offered **blank** templates, not public records,
submitted applications, patient records, or user documents.

## Sources and evaluation boundaries

`data/applications.json` pins URLs, SHA-256, source families, profile targets,
coverage and provenance. Eleven new families contain 434 reviewed text slots.
The selected fields include complete relevant groups; this is not a claim to
annotate every field, checkbox, page or conditional workflow in each PDF.

| Split | Source | Application | Expected negative examples |
| --- | --- | --- | --- |
| Train | Bachman's | Employment | employers, supervisors, references, education, signatures |
| Train | Affordable Vet Care | Pet adoption | veterinarian, clinic, landlord, references, pet details |
| Train | Freedom House Illinois | Volunteer | former address, maiden name, aliases, emergency contacts, children/schools |
| Train | Rotary Club of Great Neck | Membership | spouse, business contact, children, past membership |
| Validation | Taylor Real Estate | Rental | landlord, previous residence, employer, emergency contact, office fields |
| Validation | Turning Points | Volunteer | birthplace, work phone, primary/secondary emergency contact |
| Validation | YMCA Coastal Bend | Camp | second child, parents/guardians, emergency contacts, physician/hospital |
| Test | School District 91 | Scholarship | parent/guardian, parents' employers, school, proposed institution |
| Test | Ripley Dental | Patient registration | responsible party, policyholder, employer, insurer, work contact |
| Test | Pearce Community Center | Camp | parents/guardians, emergency contacts, signatures |
| Test | USCIS I-9 | Employment eligibility | other names, employer, preparer/translator, document identifiers |

Camp profiles refer to the **first camper**, patient profiles to the patient,
and other profiles to the individual applicant. A parent completing a camp
form is not automatically the profile subject. Conditional third-party fields
stay blank when the profile alone cannot establish the required person.

The original IRS W-9 retains its fixed training weight. W-7 and W-4 are now
**legacy diagnostics**: their previously inspected mistakes cannot serve as
fresh validation or test evidence. No new validation/test family is replayed.
New family assignments are fixed in the manifest before model evaluation.

Original documents are in `data/sources/applications/`. Publishers retain their
rights; public availability does not imply an open redistribution license.
These downloads are local evaluation fixtures, and the manifest records their
source. Some unavailable search results were not added to the corpus.

## Labels, context and extraction

TSV now accepts an optional sixth column: `section`. Existing five-column
corpora still work. `contextual_label(label, section)` encodes the field's own
heading; the model learns section and section/label features. A section's
features are excluded from neighboring fields, so a nearby emergency-contact
heading does not silently become the applicant field's owner.

The model has 26 classes. Added classes are `FirstNameMiddleInitial` and
`CityStateZipCountry`. The fixture formatter supports both. Model format v2
writes `ATLFML02`; v1 24-class weights still load, with the two untrained rows
suppressed to preserve original predictions. The frozen first-run model is
retained at `data/reference/personal-v1.bin`.

`tools/fieldml_applications.py` builds two independent input tracks:

- **Gold**: reviewed printed labels and headings, used to measure semantic
  classification with known readable inputs.
- **Extracted**: readable native tooltips, otherwise nearby printed words from
  PDF geometry, plus heuristic heading/role boundaries. The extractor never
  reads gold labels, expected classes or profile values.

Both tracks use the same field identities/classes and geometry reading order.
Native widgets supply their own rectangles. Printable forms require manually
located blank slots. This implements label association at known locations;
**automatic discovery of writable areas and OCR are not implemented**. The
extracted track is therefore not a complete end-to-end PDF automation score.
`applications-fields.json` records rectangles, origin, extracted text and word
geometry evidence. Original source hashes are checked before regeneration.

## Controlled training

Each fit uses 4,000 forms, six epochs and 8,192 buckets. There are three
predeclared training seeds (1, 7, 19); no best seed is selected using test data.
Each fit retains 2,800 ordinary synthetic forms and **200 W-9 replays**.

| Variant | 800 augmentation slots | 200 remaining slots | Section inputs | Other loss weight |
| --- | --- | --- | --- | --- |
| control | ordinary synthetic | ordinary synthetic | no | 1 |
| hard-without-context | application hard negatives | ordinary synthetic | no | 1 |
| hard-negatives | application hard negatives | ordinary synthetic | yes | 1 |
| public-without-context | application hard negatives | 50 replays per new training source | no | 1 |
| contextual | application hard negatives | 50 replays per new training source | yes | 1 |
| contextual-recall | application hard negatives | 50 replays per new training source | yes | 0.25 |

This isolates the section contribution using identical examples in
`public-without-context` versus `contextual`, and the negative loss weight using
`contextual` versus `contextual-recall`. New sources cannot silently dilute
existing sampling weights. Replays are weighting, not independent examples.
The frozen legacy model is evaluated alongside 18 new fits.

The separate application generator creates 800 training, 300 validation and
300 test forms. It contrasts the same labels under applicant, landlord,
reference, emergency contact, spouse, parent, guarantor, sponsor, clinician,
policyholder, previous address, historical name and other headings. It also
adds inline ownership qualifiers and unsupported IDs, dates and pet fields.
Synthetic splits share a grammar; different seeds do not establish unseen
layout generalization.

## Reproduce

From the repository root:

```sh
python3 -m venv /tmp/atelier-fieldml-venv
/tmp/atelier-fieldml-venv/bin/pip install -r tools/fieldml-requirements.txt
python3 tools/fieldml_fetch.py   # the publishers' PDFs are not in Git
/tmp/atelier-fieldml-venv/bin/python tools/fieldml_applications.py
cargo run --offline --release -p atelier-fieldml --example improve -- \
  crates/atelier-fieldml/data crates/atelier-fieldml/data/runs/applications-v2
/tmp/atelier-fieldml-venv/bin/python tools/fieldml_applications.py --fixtures \
  crates/atelier-fieldml/data/runs/applications-v2
cargo test --offline -p atelier-fieldml
cargo clippy --offline -p atelier-fieldml --all-targets -- -D warnings
/tmp/atelier-fieldml-venv/bin/python -m unittest discover -s tools \
  -p test_fieldml_applications.py
```

Rust training stays dependency-free. Python's pinned `pypdf` and `pymupdf`
dependencies are tooling only. Regeneration and training work offline after
checkout and dependency installation.

The run directory contains weights for every variant/seed; `personal.bin` is
the **seed-1 contextual review candidate**, not a selected production winner.
`metrics.tsv` includes accuracy, macro recall, supported-field recall, false
fills, accepted fills and thresholds. `risk-coverage.tsv` records the fixed-grid
validation sweep, and `policies.tsv` records operating points selected separately
for each input path using its validation families only. Synthetic validation
thresholds are diagnostic and do not configure the public candidate.

The selection rule maximizes validation coverage subject to zero observed
incorrect fills. `1.01` means abstain from all fields, not a probability.
A few apparently correct validation fills do not establish safe filling on
other templates. `classify_for_fill` filters `Other`/low-confidence predictions;
it expects an externally validated operating point and does not calibrate
scores. `policy.json` explicitly records whether automatic filling is enabled.
This iteration does not install a model or enable automatic filling in the app.

`predictions.tsv` and `corrections.tsv` expose classification errors.
`expectations.jsonl` and `pdfs/` contain 27 fictional example sets: all eleven
public families plus sixteen generated forms. Each set has blank, expected,
predicted and accepted PDFs, plus a JSON sidecar. The expected PDF is the
corrected reference. Printable slots receive annotation-derived widgets only
in these derivatives, which are excluded from extracted-input evaluation.
Native widgets retain their original positions. Saved field values and
original page content streams are checked on read-back.

For reviewed corrections, pass an optional final `REVIEWED_TRAINING_TSV` argument
to `improve`. Supply complete training forms, preserving context and ordering;
the diagnostic queue is not an ingestion file. Up to twenty correction forms
receive ten replays each, replacing augmentation slots only. W-9 and the four
new public source weights stay fixed. Non-training rows and known held-out IDs
(including an ID relabeled as `train`) are rejected. Renaming an evaluation
family does not make it independent training data; source provenance must be
reviewed when adding examples.

## Recorded results

See `data/application-run-v2.txt` and `data/application-metrics-v2.tsv` for all
fits and per-family results. These representative seed-1 classification
metrics are **before abstention**, after normalizing field order from geometry:

| Input | Split | Model | Accuracy | Personal fields correct | Incorrect fills on negative fields | Macro recall |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| gold | validation | legacy-v1 | 59.65% | 18/19 | 45/95 | 91.93% |
| gold | test | legacy-v1 | 61.64% | 25/30 | 51/116 | 76.11% |
| extracted | validation | legacy-v1 | 58.77% | 17/19 | 45/95 | 81.93% |
| extracted | test | legacy-v1 | 60.96% | 23/30 | 50/116 | 71.04% |
| gold | validation | contextual | 76.32% | 14/19 | 22/95 | 74.35% |
| gold | test | contextual | 73.29% | 15/30 | 24/116 | 48.41% |
| extracted | validation | contextual | 76.32% | 9/19 | 17/95 | 44.88% |
| extracted | test | contextual | 70.55% | 15/30 | 28/116 | 49.43% |

The contextual candidate reduces unwanted fills substantially, but loses
supported-field recall and macro recall. It is **not an overall production
replacement** for the legacy model. The loss-weight ablation did not repair
that tradeoff. All runs, including regressions, are retained.

The confidence experiment also fails to generalize: the contextual model's
0.999 gold-validation cutoff accepts four fields correctly on validation, but
only 7 of 19 accepted test fills are correct. The extracted-input cutoff gets
only 8 of 20 accepted test fills correct. Scores therefore remain unsuitable
as automatic-fill permissions. `policy.json` keeps automatic filling disabled;
`-accepted.pdf` means accepted by the experimental cutoff, not approved output.
Expected PDFs and correction sidecars show the correct action separately.

The next model change should separate **whose information is requested** from
**which profile component is requested**, while preserving the strong existing
component classifier. More independently held-out families are needed before
qualifying a fill policy. The new corpus and ablations make this measurable.

Verification: eight Rust tests, three Python corpus/extraction regression tests,
Clippy with warnings denied, saved-value/content-stream read-back checks for all
108 derivative PDFs, and parsing/rendering checks through the existing Atelier
CLI. Annotation checks include pinned hashes, disjoint source families,
aligned gold/extracted slots, and no duplicate field locations. A relabeled
held-out correction ID is rejected before training.

## Round 3: a fresh holdout for the round-2 candidate

Round 2 trained on attempt 1's four test families, so its passing validation
(on the same three families attempt 1 also passed before failing its test)
said nothing about unseen forms. Four new families from four new publishers
were picked, downloaded, hash-pinned and annotated **before any model was run
on them** (`data/v3/round3-sources.json`, `tools/fieldml_v3_round3.py`):

| Family | Kind | Input | Fields | Applicant's own |
| --- | --- | --- | ---: | ---: |
| library-dearborn | Library card, with a minor-child side | native widgets | 38 | 10 |
| sports-laparks | Youth soccer; the player is the subject | native widgets | 25 | 4 |
| library-kcls | Library card; residential vs mailing, parents | printable lines | 35 | 12 |
| patient-jottful | Patient intake; employer, insurance, emergency | printable | 37 | 8 |

The reserved calibration families are reused unchanged. Recalibrating
reproduced round 2's calibrated weights byte for byte. `data/v3/round3-freeze.json`
pins the model, code, extractor and datasets; the test was then run once:

```sh
python3 tools/fieldml_fetch.py
cd tools && /tmp/atelier-fieldml-venv/bin/python fieldml_v3_round3.py && cd ..
cargo run --offline --release -p atelier-fieldml --example qualify_v3 -- \
  crates/atelier-fieldml/data crates/atelier-fieldml/data/runs/personal-v3-round2/calibrated.bin \
  crates/atelier-fieldml/data/runs/personal-v3-round3 test round3
```

| Track | Accuracy | Own fields found | Wrong fills | Accepted fills correct | ECE |
| --- | ---: | ---: | ---: | ---: | ---: |
| gold labels | 90.37% | 28/34 (82%) | 7/101 (6.9%) | 23/29 (79%) | 9.4% |
| extracted labels | 68.89% | 27/34 (79%) | 35/101 (34.7%) | 23/52 (44%) | 29.3% |

**Neither track qualifies**; the gold track does worse than attempt 1's.
Full results: `data/v3/round3-test-report.txt`. What went wrong:

- **Whose detail it is.** A mailing block marked "if not same as Residential"
  and a minor child's middle initial were filled, some at confidence 1.000.
- **Unfamiliar headings.** A name under "Library Card for", a birth date under
  "Player" and a split Month/Day/Year birth date were missed.
- **Confidence is not evidence.** Wrong answers score 0.98 to 1.000, so no
  cutoff makes automatic filling safe.
- **Label extraction on new layouts.** The extractor carries a heading over
  rows it does not own (KCLS's parents inherit "Residential Address"); most
  of the extracted track's wrong fills start there, not in the model.

These four families are now spent as a test the moment anything trains on
them. The next model change should be judged on yet another fresh set.
