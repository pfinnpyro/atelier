# Label analysis annotations (v4)

The model analyzes labels; it does not decide fills. For each field it says
what the label asks for, which fields belong together, and whose they are.
The rules in `atelier-form` (`Profile::format`, address-line placement) then
decide what, if anything, to fill. The model is scored on these three outputs,
not on fill decisions.

`data/v4/labels.tsv` is the reviewed annotation of all 22 real forms (891
fields). It is built from `labels-draft.tsv` (heuristic, never trusted) plus
the corrections in `review.txt`:

```sh
python3 tools/fieldml_v4_labels.py draft   # regenerate the heuristic draft
python3 tools/fieldml_v4_labels.py apply   # draft + review.txt -> labels.tsv, then check
python3 tools/fieldml_v4_labels.py check   # validate labels.tsv
python3 tools/fieldml_v4_labels.py corpus  # join with extracted text -> corpus.tsv, holdout-corpus.tsv
```

`corpus.tsv` / `holdout-corpus.tsv` add `ext_label ext_section` (what the
label extractor read) and are what `src/analysis.rs` trains and scores on.

Columns: `form page field kind group role label section`. `label` and
`section` are the reviewed gold text, copied for reading only.

## kind: what the label asks for

Whoever it belongs to: "Parent 1 Phone#" is `Phone`, "Insured Date of Birth"
is `BirthDate`. A kind is the detail of the field's own entity. When a label
asks for a different detail of the same type, use `Other`. Examples are the
address of the property being rented, a city of birth, and the state that
issued a licence.

| Kinds | Notes |
| --- | --- |
| `FullName FirstName MiddleName MiddleInitial LastName NameLastFirst FirstNameMiddleInitial Honorific` | an alias or maiden name is a name kind under an `applicant-alt` group |
| `BirthDate BirthMonth BirthDay BirthYear Age` | split dates get their part |
| `AddressWhole AddressLine1 AddressLine2 City State Zip CityState CityStateZip CityStateZipCountry County Country` | `AddressLine1` when city/state/zip follow as separate fields, `AddressWhole` when the field takes all of it; an unlabeled second line under "Address:" is `CityStateZip` (or `AddressLine2` when the form has its own city/zip boxes) |
| `WorkAddress StreetNumber StreetDirection StreetName StreetSuffix` | a person's work address; a street split into house number / direction / name / St.-Ave. cells |
| `Phone WorkPhone Fax PhoneArea PhoneLocal PhonePrefix PhoneLine Email` | `PhoneLocal` = number without area code; `PhonePrefix`/`PhoneLine` = 3- and 4-digit parts |
| `Organization JobTitle` | any organization's name (employer, insurer, school, clinic); a *desired* position is `Other` |
| `Relationship Signature Date Other` | `Date` = any date that is not a birth date |

Things the subject owns (a dog, a vehicle) are described with `Other` kinds:
a dog's name is not a person's name.

## group: one entity instance

All fields describing the same person or organization instance share a group,
wherever they sit on the form. Names follow `<what>[-<n>]`: `applicant`,
`applicant-mailing`, `guardian-1`, `emergency-2`, `employer-current`,
`employer-previous-1`, `insurance-1`, `policyholder-1`, `reference-3`,
`school-2`, `guardian-signer`. A group has exactly one role (checked).

## role: whose block it is

| Role | Meaning |
| --- | --- |
| `applicant` | the form's subject: the applicant, patient, the first camper or player, the card holder. Questions about them (salary wanted, pets, vehicles) too |
| `applicant-alt` | the subject's *other* values, used only conditionally: "if different" mailing/home/billing address, previous address, legal/preferred/former/other names |
| `guardian` | parent, guardian, responsible party; a parent signing for a minor subject |
| `emergency` | emergency contacts |
| `spouse` | spouse or partner |
| `child` | other minors: second camper, siblings, dependents, a minor's card section |
| `employer` | the subject's current or previous work: company, title, work phone, supervisor |
| `reference` | personal or professional references |
| `provider` | organizations serving the subject: school, insurer, physician, hospital, vet, landlord, bank, previous club |
| `other` | any other person: policyholder, preparer/translator, authorized pickup, proposer, family members of unknown relation, unnamed co-signers |
| `office` | completed by the receiving organization (office use, I-9 Section 2) |

Camp and sports forms: the child is the applicant and the parent is a
guardian, even though the parent fills in the form.

## Holdout (v4)

`data/v4/holdout-*` holds four families from publishers no earlier round
used (Saint Paul dog license, Montana DPHHS child care emergency contacts,
Des Plaines Park District health club, Philadelphia EH-40 student
registration; 222 fields). They were annotated in this format **before any
model was run on them** by `tools/fieldml_v4_holdout.py`, which also writes
the extracted-label track and a v3-layout class column. Do not train or tune
on them; once anything does, they are spent.

## Holdout (v5)

`data/v5/holdout-*` holds four families from four further publishers
(Bridgeport WV employment, Bartlett Park District youth sports, Charleston
WV resident parking, Columbia County OR animal adoption; 228 fields),
annotated by `tools/fieldml_v5_holdout.py` before the layout-aware model was
run on them once. Text widgets that are not places to write (drawn over a
section bar, or standing in for a checkbox) are excluded and listed in the
script. Vehicles and pets are owned things: their own groups, role
`applicant`, kind `Other`. Date ranges (From/To) are `Other`, as in the
development forms. The v4 holdout is now part of development.
