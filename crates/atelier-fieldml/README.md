# atelier-fieldml

Two models that read form fields' labels. Both are plain logistic
regression over hashed words: no dependencies, deterministic, fast on any
CPU.

## Label analysis (`analysis`, current)

For every field of a form, `AnalysisModel::analyze` says:

- **kind**: what detail the label asks for (43 kinds: `FirstName`,
  `BirthDate`, `AddressLine1`, `Email`, ..., `Other`), whoever it belongs to;
- **group**: which fields describe the same entity (one person, one pet,
  one vehicle, one employer);
- **role**: whose block it is (`applicant`, `applicant-alt`, `guardian`,
  `spouse`, `child`, `emergency`, `employer`, `reference`, `provider`, ...).

It does not decide fills: `Analysis::fills` (the applicant's own block,
a detail the profile holds) and the rules in `atelier-form` do. The
vocabulary and annotation rules are in [LABELS.md](LABELS.md).

```rust
use atelier_fieldml::analysis::{AnalysisModel, Document, Input};

let model = AnalysisModel::bundled();
let doc = Document {
    fields: vec![Input { label: "Dog's Name".into(), section: "Pet".into(), page: 1, rect: None }],
    pages: Vec::new(), // or each page's text lines, boxes and rules
};
let said = model.analyze(&doc); // kind "Other": an animal's name is not a person's
```

What it reads, besides the label and its section:

- **layout**, when `Document::pages` is given: each field's rectangle,
  the text lines around it with font size and weight, drawn boxes, shaded
  bars and rules. From these it builds reading order, headings (larger,
  bold, capitalised or shaded text), the text left of, above and below each
  field, enclosing boxes, rows and columns, and ruled separators between
  blocks;
- **entity words**: a small lexicon of people, organisations, animals,
  things and places. A field owned by an animal, thing or place never gets a
  person's kind, and an organisation's "Name" is `Organization`;
- **cues** for alternate details ("former", "maiden", "if different") and
  for minors versus parents.

Four heads (kind, role, same-entity pair scorer, block role) are each
trained with three seeds and averaged, then stored as 8-bit weights with a
scale per class. Groups come from average-linkage clustering of pair
scores. The applicant is the heaviest applicant cluster plus any other
applicant cluster that does not repeat one of its identity details.

**Bundled model:** `data/analysis.bin`, 991,553 bytes (295 KB gzipped),
trained on all 30 reviewed forms (1,341 fields). It takes about 0.6 ms per
field to analyze.

### Results

Development is leave one form out over 26 forms. "Reviewed" uses the
annotated label text; "extracted" uses what `tools/fieldml_labels.py` read
from the PDF. "Detail-group F1" counts only pairs of fields that both ask
for a profile detail. "Foreign" fills land in somebody else's block.

| | kind | role | group F1 | detail-group F1 | fills right | foreign | wrong kind |
| --- | --- | --- | --- | --- | --- | --- | --- |
| development, reviewed | 86.1% | 68.2% | 69.1% | 75.3% | 158/205 | 22 | 16 |
| development, extracted | 81.3% | 62.9% | 65.7% | 74.0% | 142/205 | 28 | 24 |
| v5 holdout, reviewed | 87.3% | 78.5% | 52.8% | 88.2% | 20/25 | 8 | 0 |
| v5 holdout, extracted | 82.9% | 61.8% | 60.1% | 72.0% | 19/25 | 11 | 1 |
| v5 holdout without layout, reviewed | 85.5% | 78.9% | 40.2% | 69.9% | 21/25 | 13 | 1 |

The v5 holdout is four forms from new publishers, frozen in
`data/v5/freeze.json` and run once with the three-seed ensemble; averaging
the seeds' weights afterwards changed the development totals by at most one
field per column (`data/v4/runs/dev-merged`). Known weak spots: youth
registrations, where the parent's block is taken as the subject (7 of the 8
foreign fills); table rows of owned things (pets, vehicles) merged into the
applicant's group; combined labels such as "Company Name & Address".

### Reproduce

```sh
python3 -m venv /tmp/atelier-fieldml-venv
/tmp/atelier-fieldml-venv/bin/pip install -r tools/fieldml-requirements.txt
python3 tools/fieldml_fetch.py --check          # the publishers' PDFs are not in Git
python3 tools/fieldml_v4_labels.py corpus       # v4/corpus.tsv, holdout-corpus.tsv
/tmp/atelier-fieldml-venv/bin/python tools/fieldml_v4_layout.py   # v4/layout.tsv
/tmp/atelier-fieldml-venv/bin/python tools/fieldml_v5_holdout.py  # v5/holdout-*
cargo run --release -p atelier-fieldml --example analysis_v4 -- dev crates/atelier-fieldml/data OUT
cargo run --release -p atelier-fieldml --example analysis_v4 -- ship crates/atelier-fieldml/data crates/atelier-fieldml/data
cargo test -p atelier-fieldml
```

`examples/layout_debug.rs` prints what the layout reader made of each field.

## History

The sections below describe the earlier per-field Personal classifier
(`Model`, `personal`), kept with its frozen weights and reports. Its later
rounds are in [APPLICATIONS.md](APPLICATIONS.md).

### Personal field classifier: first training corpus

This model learns which of 24 personal-profile classes a field requests from
its label and neighboring labels. It does not train on page pixels or learn
profile values. PDF fixtures exercise the surrounding fill workflow.

All filled examples use invented identities, `example.org` email addresses,
fictional 202-555-01xx numbers, and invented addresses. No user documents,
submitted forms, or personal records are used.

#### Public online examples

Vendored **blank** forms from the IRS:

| Form | Revision | Split | Text fields | Source |
| --- | --- | --- | ---: | --- |
| W-9 | March 2024 | train | 15 | https://www.irs.gov/pub/irs-pdf/fw9.pdf |
| W-7 | December 2024 | validation | 48 | https://www.irs.gov/pub/irs-pdf/fw7.pdf |
| W-4 | 2026 | test | 43 | https://www.irs.gov/pub/irs-pdf/fw4.pdf |

`data/sources.json` records URL, SHA-256, revision and labeling provenance.
These are U.S. federal government blank forms, retained as source fixtures;
filled derivatives are marked as fictional examples. They are not completed tax
forms. Checkboxes, signatures, tax IDs, tax amounts, and business/third-party
information are outside this personal text-field corpus.

The PDFs have opaque `/T` names and no `/TU` tooltips. `data/online.tsv` contains
human-transcribed visible labels with section context, associated with the real
field IDs. This is a supervised label benchmark, not an automatic PDF label
extractor. Both visible-label and raw-metadata results are reported. Raw metadata
recognized **zero** supported personal fields across these forms on this run.
A page-text/widget label extraction step is needed before using these PDFs with
the classifier in an actual automatic filling pipeline.

An individual taxpayer is assumed for W-9. Unsupported composites (W-4's first
name plus middle initial, W-7's city/state/country/postcode) are labeled `Other`
so the model should leave them blank. Historical names, foreign addresses, and
agent/delegate details are also `Other`; they cannot safely use the one current
personal profile. The first address field includes the apartment when there is
no separate apartment field. The policies are explicit in
`tools/fieldml_corpus.py`; they are independent of model predictions.

#### Reproduce

From the repository root, with Python 3.9+ and Rust installed:

```sh
python3 -m venv /tmp/atelier-fieldml-venv
/tmp/atelier-fieldml-venv/bin/pip install -r tools/fieldml-requirements.txt
python3 tools/fieldml_fetch.py   # the publishers' PDFs are not in Git
# Rebuild reviewed label files from the vendored blank PDFs:
/tmp/atelier-fieldml-venv/bin/python tools/fieldml_corpus.py annotate
cargo run --offline --release -p atelier-fieldml --example first_run -- \
  crates/atelier-fieldml/data crates/atelier-fieldml/data/runs/first
/tmp/atelier-fieldml-venv/bin/python tools/fieldml_corpus.py pdfs
cargo test --offline -p atelier-fieldml
```

The native Rust crate remains dependency-free. Python's pinned `pypdf` dependency
is used only for inspecting and filling public fixtures and reading PDFs back.
Training and evaluation work offline after checkout. To refresh a source, fetch
its manifest URL into `data/sources/`, then review both the revised PDF and the
annotation mappings before regenerating labels; counts and absent tooltips are
checked, and new hashes are recorded.

The run directory contains:

- `baseline.bin` and `personal.bin`: baseline and mixed-corpus model weights;
  786,448 bytes each, loadable with `Model::from_bytes`.
- `synthetic.tsv`: 4,000 training / 500 validation / 500 test forms, generated
  with fixed seeds 1 / 98 / 99. All 24 classes are represented.
- `report.txt`, `predictions.tsv`, `corrections.tsv`: metrics, field predictions,
  confidences, and a review queue of predicted vs. expected classes.
- `expectations.jsonl`: fictional profiles and expected values/actions for each
  materialized form, alongside predicted classes/values and correction flags.
- `pdfs/`: 48 generated forms (32 train, 8 validation, 8 test), alternating one-
  and two-column layouts, plus all three public forms. Each has `-blank.pdf`,
  `-predicted.pdf`, `-expected.pdf` and a JSON sidecar. The expected version is
  the corrected reference; `Other` fields stay blank. All saved field values
  are read back and checked; original public page text must remain intact.

Generated runs are ignored by Git; the source PDFs, reviewed annotations,
generator, training runner and first-run summary are retained.

#### Results and corrections

See `data/first-run.txt` for the recorded first run. The mixed model trains for
six epochs using 8,192 feature buckets and 4,000 forms: 3,800 synthetic forms
plus 200 replays of the W-9 training form. Replays are weighting, not 200 new
independent examples. W-7/W-4 never enter training.

Synthetic held-out accuracy is 99.33% (baseline 99.46%). Annotated W-7 accuracy
is 72.92% (baseline 66.67%), with 6/6 supported personal fields recognized but
13/42 negative fields incorrectly filled. Annotated W-4 is 97.67%, including
3/3 supported fields, with 1/40 negatives incorrectly filled. These are small,
negative-heavy benchmarks; macro recall, positive counts and false fills are
reported to expose this. Confidence is not calibrated, and these results do
not establish safe automatic filling. The candidate is saved for inspection;
this work does not install it into the application.

Synthetic splits share the same generator grammar. Different seeds do not
prove generalization to unseen template families. Public families are split
at source level; filled variants must retain their source split.

To train with reviewed corrections, create a TSV of **complete training forms**
using the same columns as `online.tsv`:

```text
# split<TAB>form<TAB>field<TAB>class<TAB>label
train<TAB>reviewed-training-001<TAB>f0<TAB>Email<TAB>Contact email
```

Use literal tabs, exact class variant names from `Class::ALL`, unique field IDs,
and preserve field order and context. Pass this file as the optional fourth
argument to `first_run` (after the data and output directories); the 200 replay
slots are shared across the public training forms and reviewed training forms.
Write the new run to a different output directory and pass `--out` to the Python
PDF command. Keep source and corrected labels together for review.

`corrections.tsv` is a diagnostic queue, **not** an ingestible training file.
Validation/test mistakes must not be fed back while continuing to claim those
forms as held out. Reviewing an evaluation error is useful; training on it
requires retiring that family from evaluation and finding a fresh holdout.
Known held-out IDs and non-training correction rows are rejected by the runner.

A second run was also performed using `data/training-corrections.tsv`: nine
complete synthetic **training** forms (82 fields), selected from first-run
training errors and checked against generator ground truth. No validation or
test labels were replayed. To reproduce that pass:

```sh
cargo run --offline --release -p atelier-fieldml --example first_run -- \
  crates/atelier-fieldml/data crates/atelier-fieldml/data/runs/corrected \
  crates/atelier-fieldml/data/training-corrections.tsv
/tmp/atelier-fieldml-venv/bin/python tools/fieldml_corpus.py pdfs \
  --out crates/atelier-fieldml/data/runs/corrected
```

`data/correction-run.txt` records the result: synthetic test accuracy rose to
99.38%, but annotated W-7 validation fell to 68.75%, with 15 false fills.
W-4 remained 97.67%. This correction pass demonstrates replay; it does not
justify replacing the first candidate. Both runs and their artifacts are kept.
The synthetic corpus contains no exact form duplicates across splits, though
all splits still share the generator grammar.

Verification for this change: six Rust tests passed, Clippy passed with warnings
denied, all 153 first-run PDFs were parsed by Atelier, and representative
synthetic/public filled pages were rendered and visually checked. The Python
materializer independently verifies saved text-field values and preservation of
public page text for every run. A correction file impersonating held-out W-7 as
a training form was rejected by the runner.
