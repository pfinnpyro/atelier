//! Label analysis (v4). For each field of a form the model says what detail
//! its label asks for (kind), which fields describe the same entity instance
//! (group), and whose block that is (role). It does not decide fills: the
//! rules in `atelier-form` do that from this structure. See `LABELS.md`.
//!
//! The model reads the visible text and the page layout: where each field
//! sits, the text beside, above and below it, the headings over it (found
//! by typography: larger type, bold, shaded bars), the boxes drawn around
//! it, table rows and columns, and the rules between blocks. Three learned
//! parts, all multinomial logistic regressions over hashed features:
//!
//! - a kind head and a role head, one prediction per field;
//! - a pair scorer that says how likely two fields of a form describe the
//!   same entity. Groups are the clusters of average-linkage merging while
//!   the mean pair score stays above one half.
//!
//! The role head reads the predicted kinds and the pair scorer reads the
//! predicted kinds and roles, trained out of fold so it learns how far to
//! trust them. A small word list says which words name people,
//! organizations, animals and owned things, so "Dog Name" can be told
//! from a person's name.
use crate::personal::{Head, component_words, feature_words, hash, normalized};
use crate::{Class, synth};
use std::collections::{HashMap, HashSet};

/// What a label asks for, whoever it belongs to. `Other` is first.
pub const KINDS: [&str; 43] = [
    "Other",
    "FullName",
    "FirstName",
    "MiddleName",
    "MiddleInitial",
    "LastName",
    "NameLastFirst",
    "FirstNameMiddleInitial",
    "Honorific",
    "BirthDate",
    "BirthMonth",
    "BirthDay",
    "BirthYear",
    "Age",
    "AddressWhole",
    "AddressLine1",
    "AddressLine2",
    "WorkAddress",
    "StreetNumber",
    "StreetDirection",
    "StreetName",
    "StreetSuffix",
    "City",
    "State",
    "Zip",
    "CityState",
    "CityStateZip",
    "CityStateZipCountry",
    "County",
    "Country",
    "Phone",
    "WorkPhone",
    "Fax",
    "PhoneArea",
    "PhoneLocal",
    "PhonePrefix",
    "PhoneLine",
    "Email",
    "Organization",
    "JobTitle",
    "Relationship",
    "Signature",
    "Date",
];

/// Whose block a field is in. `applicant` is first.
pub const ROLES: [&str; 11] = [
    "applicant",
    "applicant-alt",
    "guardian",
    "emergency",
    "spouse",
    "child",
    "employer",
    "reference",
    "provider",
    "other",
    "office",
];

/// The kinds the existing profile rules know how to fill: the names of the
/// Personal [`Class`]es.
pub fn fillable(kind: &str) -> bool {
    Class::ALL
        .iter()
        .any(|c| *c != Class::Other && format!("{c:?}") == kind)
}

/// A rectangle in PDF points, from the page's top left.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    fn cy(&self) -> f32 {
        (self.y0 + self.y1) / 2.
    }
    fn area(&self) -> f32 {
        self.width().max(0.) * self.height().max(0.)
    }
    fn contains(&self, o: &Rect, slack: f32) -> bool {
        o.x0 >= self.x0 - slack
            && o.y0 >= self.y0 - slack
            && o.x1 <= self.x1 + slack
            && o.y1 <= self.y1 + slack
    }
    fn x_overlap(&self, o: &Rect) -> f32 {
        (self.x1.min(o.x1) - self.x0.max(o.x0)).max(0.)
    }
    fn y_overlap(&self, o: &Rect) -> f32 {
        (self.y1.min(o.y1) - self.y0.max(o.y0)).max(0.)
    }
}

/// One line of text on a page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Text {
    pub rect: Rect,
    /// The largest font size on the line.
    pub size: f32,
    /// Most of its characters are in a bold face.
    pub bold: bool,
    pub text: String,
}

/// A drawn rectangle: a box, a table cell, or a shaded bar.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Shape {
    pub rect: Rect,
    /// Filled with something other than white.
    pub filled: bool,
    pub stroked: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub number: u32,
    pub width: f32,
    pub height: f32,
    pub texts: Vec<Text>,
    pub shapes: Vec<Shape>,
    /// Lines and hairline bars.
    pub rules: Vec<Rect>,
}

/// What the model sees of a field: its label, the heading the label
/// extractor gave it, and where it sits (when the layout is known).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Input {
    pub label: String,
    pub section: String,
    pub page: u32,
    pub rect: Option<Rect>,
}

/// A form to analyze: its fields and, if known, its pages' layout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub fields: Vec<Input>,
    pub pages: Vec<Page>,
}

/// One field's analysis. `group` numbers are only meaningful within a form.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub kind: &'static str,
    pub kind_confidence: f32,
    pub role: &'static str,
    pub role_confidence: f32,
    pub group: usize,
}

impl Analysis {
    /// Whether the existing rules would fill this field from the profile:
    /// the applicant's own block, asking for a detail the profile holds.
    pub fn fills(&self) -> bool {
        self.role == "applicant" && fillable(self.kind)
    }
}

/// A reviewed field from `data/v4/*corpus.tsv`.
#[derive(Debug, Clone)]
pub struct Field {
    pub id: String,
    pub kind: usize,
    pub role: usize,
    pub group: String,
    pub gold: Input,
    pub extracted: Input,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub id: String,
    pub fields: Vec<Field>,
    pub pages: Vec<Page>,
}

impl Form {
    /// The reviewed text (`extracted == false`) or what the label extractor
    /// read from the page, with the page layout either way.
    pub fn document(&self, extracted: bool) -> Document {
        Document {
            fields: self
                .fields
                .iter()
                .map(|f| {
                    if extracted {
                        f.extracted.clone()
                    } else {
                        f.gold.clone()
                    }
                })
                .collect(),
            pages: self.pages.clone(),
        }
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Columns: form page field kind group role label section ext_label
/// ext_section. Rejects unknown kinds and roles, duplicate fields, a form
/// split into separate runs, and a group with more than one role.
pub fn parse(tsv: &str) -> Result<Vec<Form>, String> {
    let mut forms: Vec<Form> = Vec::new();
    let mut seen = HashSet::new();
    let mut roles = HashMap::new();
    for (line, row) in tsv.lines().enumerate() {
        if row.is_empty() || row.starts_with('#') {
            continue;
        }
        let fail = |why: &str| format!("corpus row {}: {why}", line + 1);
        let c: Vec<_> = row.split('\t').collect();
        if c.len() != 10 {
            return Err(fail("expected 10 columns"));
        }
        let kind = KINDS
            .iter()
            .position(|k| *k == c[3])
            .ok_or_else(|| fail("unknown kind"))?;
        let role = ROLES
            .iter()
            .position(|r| *r == c[5])
            .ok_or_else(|| fail("unknown role"))?;
        if !seen.insert((c[0].to_string(), c[2].to_string())) {
            return Err(fail("duplicate field"));
        }
        if *roles
            .entry((c[0].to_string(), c[4].to_string()))
            .or_insert(role)
            != role
        {
            return Err(fail("group with two roles"));
        }
        if forms.last().is_none_or(|f| f.id != c[0]) {
            if forms.iter().any(|f| f.id == c[0]) {
                return Err(fail("form rows are not contiguous"));
            }
            forms.push(Form {
                id: c[0].to_string(),
                fields: Vec::new(),
                pages: Vec::new(),
            });
        }
        let page = c[1].parse().map_err(|_| fail("bad page"))?;
        let input = |label: &str, section: &str| Input {
            label: if label.trim().is_empty() { c[2] } else { label }.to_string(),
            section: section.to_string(),
            page,
            rect: None,
        };
        forms.last_mut().unwrap().fields.push(Field {
            id: c[2].to_string(),
            kind,
            role,
            group: c[4].to_string(),
            gold: input(c[6], c[7]),
            extracted: input(c[8], c[9]),
        });
    }
    Ok(forms)
}

/// Attach `data/v4/layout.tsv` (written by `tools/fieldml_v4_layout.py`) to
/// parsed forms. Forms the file does not mention keep no layout; a form it
/// does mention must have every field placed.
pub fn attach_layout(tsv: &str, forms: &mut [Form]) -> Result<(), String> {
    let index: HashMap<_, _> = forms
        .iter()
        .enumerate()
        .map(|(n, f)| (f.id.clone(), n))
        .collect();
    let mut placed: HashMap<(usize, String), (u32, Rect)> = HashMap::new();
    let mut laid = HashSet::new();
    for (line, row) in tsv.lines().enumerate() {
        if row.is_empty() || row.starts_with('#') {
            continue;
        }
        let fail = |why: &str| format!("layout row {}: {why}", line + 1);
        let c: Vec<_> = row.split('\t').collect();
        if c.len() < 3 {
            return Err(fail("short row"));
        }
        let Some(&form) = index.get(c[1]) else {
            continue;
        };
        laid.insert(form);
        let num = |i: usize| -> Result<f32, String> {
            c.get(i)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| fail("bad number"))
        };
        let rect = |at: usize| -> Result<Rect, String> {
            Ok(Rect {
                x0: num(at)?,
                y0: num(at + 1)?,
                x1: num(at + 2)?,
                y1: num(at + 3)?,
            })
        };
        let page_no = |s: &str| -> Result<u32, String> { s.parse().map_err(|_| fail("bad page")) };
        let find = |pages: &[Page], n: u32| -> Result<usize, String> {
            pages
                .iter()
                .position(|p| p.number == n)
                .ok_or_else(|| fail("record before its page"))
        };
        let pages = &mut forms[form].pages;
        match c[0] {
            "P" => pages.push(Page {
                number: page_no(c[2])?,
                width: num(3)?,
                height: num(4)?,
                ..Page::default()
            }),
            "F" => {
                placed.insert((form, squash(c[2])), (page_no(c[3])?, rect(4)?));
            }
            "T" => {
                let p = find(pages, page_no(c[2])?)?;
                pages[p].texts.push(Text {
                    rect: rect(3)?,
                    size: num(7)?,
                    bold: c.get(8) == Some(&"1"),
                    text: c.get(9).unwrap_or(&"").to_string(),
                });
            }
            "S" => {
                let p = find(pages, page_no(c[2])?)?;
                pages[p].shapes.push(Shape {
                    rect: rect(3)?,
                    filled: c.get(7) == Some(&"1"),
                    stroked: c.get(8) == Some(&"1"),
                });
            }
            "L" => {
                let p = find(pages, page_no(c[2])?)?;
                pages[p].rules.push(rect(3)?);
            }
            _ => return Err(fail("unknown record")),
        }
    }
    for form in laid {
        let f = &mut forms[form];
        for field in &mut f.fields {
            let (page, rect) = *placed
                .get(&(form, squash(&field.id)))
                .ok_or_else(|| format!("layout: {} has no rectangle for {}", f.id, field.id))?;
            for input in [&mut field.gold, &mut field.extracted] {
                input.page = page;
                input.rect = Some(rect);
            }
        }
    }
    Ok(())
}

const MAGIC: &[u8; 8] = b"ATLANA02";
const BUCKETS: usize = 1 << 14;
const PAIR_BUCKETS: usize = 1 << 13;
const GROUP_BUCKETS: usize = 1 << 13;

/// Words that name a kind of entity. A label's owner word says whose
/// detail it asks for: "Dog Name" is an animal's, "Club Name" an
/// organization's, "Mother's Name" a person's.
const ENTITIES: [(&str, &[&str]); 5] = [
    (
        "person",
        &[
            "applicant",
            "parent",
            "guardian",
            "mother",
            "father",
            "mom",
            "dad",
            "spouse",
            "partner",
            "husband",
            "wife",
            "child",
            "children",
            "son",
            "daughter",
            "student",
            "participant",
            "member",
            "owner",
            "contact",
            "employee",
            "patient",
            "player",
            "camper",
            "sibling",
            "siblings",
            "brother",
            "sister",
            "person",
            "adult",
            "minor",
            "youth",
            "tenant",
            "resident",
            "cardholder",
            "signer",
            "witness",
            "preparer",
            "translator",
            "reference",
            "supervisor",
            "physician",
            "doctor",
            "dentist",
            "pediatrician",
            "nurse",
            "insured",
            "policyholder",
            "subscriber",
            "relative",
            "grandparent",
            "volunteer",
            "individual",
            "proposer",
            "seconder",
            "candidate",
            "beneficiary",
            "holder",
            "landlord",
            "veterinarian",
            "vet",
            "occupant",
            "occupants",
        ],
    ),
    (
        "organization",
        &[
            "company",
            "employer",
            "business",
            "school",
            "club",
            "clinic",
            "hospital",
            "insurance",
            "insurer",
            "carrier",
            "bank",
            "church",
            "organization",
            "organisation",
            "agency",
            "college",
            "university",
            "firm",
            "practice",
            "department",
            "institution",
            "pharmacy",
            "league",
            "association",
            "union",
            "district",
            "corporation",
            "facility",
            "center",
            "centre",
            "program",
            "veterinary",
            "institute",
        ],
    ),
    (
        "animal",
        &[
            "dog", "dogs", "cat", "cats", "pet", "pets", "animal", "animals", "puppy", "kitten",
            "horse", "breed", "canine", "feline", "rabies", "spayed", "neutered",
        ],
    ),
    (
        "thing",
        &[
            "vehicle",
            "car",
            "auto",
            "automobile",
            "boat",
            "plate",
            "property",
            "building",
            "product",
            "item",
            "device",
            "equipment",
            "project",
            "event",
            "team",
            "course",
            "account",
            "policy",
            "document",
            "passport",
            "license",
            "licence",
            "permit",
        ],
    ),
    (
        "place",
        &[
            "street",
            "road",
            "avenue",
            "city",
            "town",
            "county",
            "country",
            "birthplace",
            "location",
            "residence",
        ],
    ),
];

/// Words that mark the subject's other, conditional values: a former
/// address, a maiden or preferred name, a mailing address "if different".
fn alternate(words: &[String]) -> bool {
    const ONE: [&str; 11] = [
        "former",
        "formerly",
        "maiden",
        "previous",
        "prior",
        "alias",
        "aliases",
        "nickname",
        "preferred",
        "aka",
        "alternate",
    ];
    const TWO: [(&str, &str); 7] = [
        ("if", "different"),
        ("if", "not"),
        ("not", "same"),
        ("other", "names"),
        ("other", "last"),
        ("also", "known"),
        ("if", "other"),
    ];
    words.iter().any(|w| ONE.contains(&w.as_str()))
        || words
            .windows(2)
            .any(|p| TWO.contains(&(p[0].as_str(), p[1].as_str())))
}

const MINORS: [&str; 12] = [
    "child",
    "children",
    "student",
    "participant",
    "camper",
    "player",
    "minor",
    "youth",
    "son",
    "daughter",
    "kid",
    "infant",
];
const PARENTS: [&str; 7] = [
    "parent",
    "parents",
    "guardian",
    "guardians",
    "mother",
    "father",
    "stepparent",
];

fn any_of(words: &[String], list: &[&str]) -> bool {
    words.iter().any(|w| list.contains(&w.as_str()))
}

/// Whether any label or heading on the form names a minor: then the minor
/// is usually the subject and the parents are guardians.
fn names_minor(ctx: &Ctx) -> bool {
    ctx.fields
        .iter()
        .any(|f| any_of(&normalized(&f.label), &MINORS) || any_of(&normalized(&f.section), &MINORS))
        || ctx.headings.iter().any(|h| any_of(&h.words, &MINORS))
}

/// Words that carry no detail of their own in a label.
const FILLER: [&str; 14] = [
    "of", "the", "your", "s", "if", "any", "name", "names", "or", "and", "a", "in", "to", "for",
];

/// A label that is nothing but an entity word ("Physician:", "Company",
/// "Proposer") asks for that entity's name.
fn bare(words: &[String]) -> Option<&'static str> {
    let content: Vec<_> = words
        .iter()
        .filter(|w| !FILLER.contains(&w.as_str()))
        .collect();
    let class = |w: &String| {
        ENTITIES
            .iter()
            .find(|(_, l)| l.contains(&w.as_str()))
            .map(|(c, _)| *c)
    };
    let first = class(content.first()?)?;
    content
        .iter()
        .all(|w| class(w) == Some(first))
        .then_some(first)
}

/// Whose detail a field is, when its text says so: a dog's, a clinic's,
/// a street's. `None` means a person or unknown.
fn owner(ctx: &Ctx, at: usize) -> Option<&'static str> {
    let own = normalized(&ctx.fields[at].label);
    if let Some(o) = name_owner(&own) {
        return (o != "person").then_some(o);
    }
    let label = entities(&own);
    if label.contains(&"person") {
        return None;
    }
    for class in ["animal", "thing"] {
        if label.contains(&class) {
            return Some(class);
        }
    }
    // "Name" or "Age" under "Dog Information #2".
    let heading = entities(ctx.heading_words(ctx.places[at].heading));
    let section = entities(&normalized(&ctx.fields[at].section));
    for e in [heading, section] {
        if e.contains(&"person") {
            return None;
        }
        for class in ["animal", "thing"] {
            if e.contains(&class) {
                return Some(class);
            }
        }
    }
    None
}

/// Kinds only a person has.
fn personal_kind(kind: &str) -> bool {
    matches!(
        kind,
        "FullName"
            | "FirstName"
            | "MiddleName"
            | "MiddleInitial"
            | "LastName"
            | "NameLastFirst"
            | "FirstNameMiddleInitial"
            | "Honorific"
            | "BirthDate"
            | "BirthMonth"
            | "BirthDay"
            | "BirthYear"
            | "Age"
    )
}

/// The kind, from the kind head's distribution and what the text says
/// owns the field: an animal, a thing or a place has no person's name or
/// birth date, and an organization's name is `Organization`.
fn decide_kind(ctx: &Ctx, at: usize, p: &[f32]) -> (usize, f32) {
    let (k, v) = best(p);
    let Some(o) = owner(ctx, at) else {
        return (k, v);
    };
    if !personal_kind(KINDS[k]) {
        return (k, v);
    }
    if o == "organization" && KINDS[k].contains("Name") {
        return (KINDS.iter().position(|x| *x == "Organization").unwrap(), v);
    }
    if o == "organization" {
        return (k, v);
    }
    let allowed: Vec<f32> = p
        .iter()
        .enumerate()
        .map(|(i, x)| if personal_kind(KINDS[i]) { 0. } else { *x })
        .collect();
    let (k, x) = best(&allowed);
    (k, x / allowed.iter().sum::<f32>().max(1e-9))
}

fn entities(words: &[String]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (class, list) in ENTITIES {
        if words.iter().any(|w| list.contains(&w.as_str())) {
            out.push(class);
        }
    }
    out
}

/// The class of the word that owns "name" in a label: "Dog Name",
/// "Name of Club", "Mother's Name".
fn name_owner(words: &[String]) -> Option<&'static str> {
    let at = words.iter().position(|w| w == "name" || w == "names")?;
    let class = |w: &String| {
        ENTITIES
            .iter()
            .find(|(_, l)| l.contains(&w.as_str()))
            .map(|(c, _)| *c)
    };
    let before = words[..at].iter().rev().find_map(class);
    let after = words
        .get(at + 1)
        .filter(|w| *w == "of")
        .and_then(|_| words[at + 2..].iter().find_map(class));
    before.or(after)
}

/// The instance number named in a text: "Parent 2", "Emergency Contact
/// #1", "Secondary Insurance", "Row_3".
fn instance_in(text: &str) -> Option<u8> {
    let words = crate::tokens(text);
    for (at, w) in words.iter().enumerate() {
        match w.as_str() {
            "primary" => return Some(1),
            "secondary" => return Some(2),
            _ => {}
        }
        // A lone digit, not part of a date or a phone number.
        if w.len() == 1
            && w.as_bytes()[0].is_ascii_digit()
            && w != "0"
            && words
                .get(at + 1)
                .is_none_or(|n| !n.chars().all(|c| c.is_ascii_digit()))
        {
            return Some(w.as_bytes()[0] - b'0');
        }
    }
    None
}

/// The label without numbers, for spotting a block that repeats.
fn stem(input: &Input) -> String {
    component_words(&input.label)
        .into_iter()
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A heading found on the page by its typography.
#[derive(Debug, Clone)]
struct Heading {
    page: u32,
    rect: Rect,
    /// 3: larger type than the page's body text; 2: on a shaded bar;
    /// 1: bold or capitals, not ending in a colon.
    level: u8,
    words: Vec<String>,
    text: String,
}

/// Where a field sits and what is drawn and written around it.
#[derive(Debug, Clone, Default)]
struct Place {
    heading: Option<usize>,
    /// The nearest heading in larger type (a part of the form).
    major: Option<usize>,
    /// The smallest drawn box around the field (page, shape index).
    shape: Option<(u32, usize)>,
    row: usize,
    row_len: usize,
    row_index: usize,
    col_len: usize,
    left: Vec<String>,
    above: Vec<String>,
    below: Vec<String>,
    instance: Option<u8>,
    /// Width in units of the page's body text size.
    width: f32,
    tall: bool,
    body: f32,
}

/// A document in reading order with its layout worked out.
struct Ctx<'a> {
    /// `order[k]` is the document index of the `k`th field in reading order.
    order: Vec<usize>,
    fields: Vec<&'a Input>,
    places: Vec<Place>,
    headings: Vec<Heading>,
    pages: &'a [Page],
    laid: bool,
}

fn body_size(page: &Page) -> f32 {
    let mut sizes: Vec<_> = page.texts.iter().map(|t| (t.size, t.text.len())).collect();
    sizes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: usize = sizes.iter().map(|s| s.1).sum();
    let mut run = 0;
    for (size, n) in sizes {
        run += n;
        if run * 2 >= total {
            return size.max(4.);
        }
    }
    9.
}

/// Reading order: by page, then rows (fields whose middles fall within the
/// first field's height), then left to right. Fields without a rectangle
/// keep their order. Also returns each field's row.
fn reading_order(fields: &[Input]) -> (Vec<usize>, Vec<usize>) {
    let n = fields.len();
    if n == 0 || fields.iter().any(|f| f.rect.is_none()) {
        return ((0..n).collect(), (0..n).collect());
    }
    let r = |i: usize| fields[i].rect.unwrap();
    let mut by_y: Vec<_> = (0..n).collect();
    by_y.sort_by(|a, b| {
        fields[*a]
            .page
            .cmp(&fields[*b].page)
            .then(r(*a).cy().total_cmp(&r(*b).cy()))
    });
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for i in by_y {
        let joins = rows.last().is_some_and(|row| {
            let first = row[0];
            fields[first].page == fields[i].page
                && r(i).cy() <= r(first).y1
                && r(i).cy() >= r(first).y0
        });
        if joins {
            rows.last_mut().unwrap().push(i);
        } else {
            rows.push(vec![i]);
        }
    }
    let mut order = Vec::with_capacity(n);
    let mut row_of = vec![0; n];
    for (k, row) in rows.iter_mut().enumerate() {
        row.sort_by(|a, b| r(*a).x0.total_cmp(&r(*b).x0));
        for &i in row.iter() {
            row_of[i] = k;
            order.push(i);
        }
    }
    (order, row_of)
}

/// Whether a text line reads as some field's own label: just left of it
/// on its row, or just under it (printed forms label under the line).
fn label_of(t: &Text, f: &Rect) -> bool {
    let left = t.rect.y_overlap(f) >= 0.4 * t.rect.height().min(f.height())
        && t.rect.x1 <= f.x0 + 4.
        && f.x0 - t.rect.x1 < 60.;
    let below = t.rect.y0 >= f.y1 - 3.
        && t.rect.y0 - f.y1 < t.size * 1.2
        && t.rect.x_overlap(f) > 0.5 * t.rect.width();
    left || below
}

impl<'a> Ctx<'a> {
    fn new(doc: &'a Document) -> Self {
        let (order, row_of) = reading_order(&doc.fields);
        let fields: Vec<_> = order.iter().map(|i| &doc.fields[*i]).collect();
        let laid = !doc.pages.is_empty() && fields.iter().all(|f| f.rect.is_some());
        let mut ctx = Ctx {
            order,
            places: vec![Place::default(); fields.len()],
            fields,
            headings: Vec::new(),
            pages: &doc.pages,
            laid,
        };
        if !laid {
            return ctx;
        }
        // Headings, by typography.
        for page in &doc.pages {
            let body = body_size(page);
            let rects: Vec<_> = ctx
                .fields
                .iter()
                .filter(|f| f.page == page.number)
                .map(|f| f.rect.unwrap())
                .collect();
            for t in &page.texts {
                let words = normalized(&t.text);
                let letters: Vec<_> = t.text.chars().filter(|c| c.is_alphabetic()).collect();
                if words.is_empty() || words.len() > 12 || letters.len() < 3 {
                    continue;
                }
                // Not a line of blanks to write on, nor a sentence.
                let blanks = t.text.contains("__") || t.text.contains("....");
                let sentence = t.text.trim_end().ends_with('.') && words.len() > 3;
                if blanks || sentence || words.len() > 9 {
                    continue;
                }
                let colon = t.text.trim_end().ends_with(':');
                let caps = letters.len() >= 6
                    && letters.iter().all(|c| c.is_uppercase())
                    && words.len() >= 2;
                let shaded = page.shapes.iter().any(|s| {
                    s.filled
                        && s.rect.contains(&t.rect, 2.)
                        && s.rect.height() < t.rect.height() * 3. + 8.
                });
                let level = if t.size >= body * 1.15 {
                    3
                } else if shaded {
                    2
                } else if (t.bold || caps) && !colon && !rects.iter().any(|f| label_of(t, f)) {
                    1
                } else {
                    continue;
                };
                ctx.headings.push(Heading {
                    page: page.number,
                    rect: t.rect,
                    level,
                    words,
                    text: t.text.clone(),
                });
            }
        }
        ctx.headings.sort_by(|a, b| {
            a.page
                .cmp(&b.page)
                .then(a.rect.y1.total_cmp(&b.rect.y1))
                .then(a.rect.x0.total_cmp(&b.rect.x0))
        });
        let mut rows: HashMap<usize, Vec<usize>> = HashMap::new();
        for (k, &i) in ctx.order.iter().enumerate() {
            rows.entry(row_of[i]).or_default().push(k);
        }
        for k in 0..ctx.fields.len() {
            let f = ctx.fields[k];
            let r = f.rect.unwrap();
            let Some(page) = doc.pages.iter().find(|p| p.number == f.page) else {
                continue;
            };
            let body = body_size(page);
            let row = &rows[&row_of[ctx.order[k]]];
            let mut place = Place {
                row: row_of[ctx.order[k]],
                row_len: row.len(),
                row_index: row.iter().position(|x| *x == k).unwrap(),
                col_len: ctx
                    .fields
                    .iter()
                    .filter(|g| g.page == f.page && (g.rect.unwrap().x0 - r.x0).abs() < 4.)
                    .count(),
                body,
                width: r.width() / body,
                tall: r.height() > body * 2.4,
                ..Place::default()
            };
            // The nearest heading above, in this column; else the last one
            // on an earlier page.
            let above = |h: &Heading| {
                h.page == f.page
                    && h.rect.y1 <= r.y0 + 2.
                    && h.rect.x0 <= r.x1 + 10.
                    && h.rect.x1 >= r.x0 - 320.
            };
            let nearest = |min_level: u8| {
                ctx.headings
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| h.level >= min_level && above(h))
                    .min_by(|a, b| (r.y0 - a.1.rect.y1).total_cmp(&(r.y0 - b.1.rect.y1)))
                    .map(|(n, _)| n)
                    .or_else(|| {
                        ctx.headings
                            .iter()
                            .rposition(|h| h.level >= min_level && h.page < f.page)
                    })
            };
            place.heading = nearest(1);
            place.major = nearest(3);
            place.shape = page
                .shapes
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.rect.contains(&r, 2.)
                        && s.rect.area() >= r.area() * 1.8
                        && s.rect.area() <= page.width * page.height * 0.6
                })
                .min_by(|a, b| a.1.rect.area().total_cmp(&b.1.rect.area()))
                .map(|(n, _)| (page.number, n));
            let mut left: Option<&Text> = None;
            let mut over: Option<&Text> = None;
            let mut under: Option<&Text> = None;
            for t in &page.texts {
                if t.rect.y_overlap(&r) >= 0.4 * t.rect.height().min(r.height())
                    && t.rect.x1 <= r.x0 + 4.
                    && r.x0 - t.rect.x1 < 250.
                    && left.is_none_or(|l| t.rect.x1 > l.rect.x1)
                {
                    left = Some(t);
                }
                if t.rect.y1 <= r.y0 + 3.
                    && r.y0 - t.rect.y1 < 2.2 * t.size.max(6.)
                    && t.rect.x_overlap(&r) > 0.
                    && over.is_none_or(|o| t.rect.y1 > o.rect.y1)
                {
                    over = Some(t);
                }
                if t.rect.y0 >= r.y1 - 3.
                    && t.rect.y0 - r.y1 < 1.6 * t.size.max(6.)
                    && t.rect.x_overlap(&r) > 0.
                    && under.is_none_or(|u| t.rect.y0 < u.rect.y0)
                {
                    under = Some(t);
                }
            }
            place.left = left.map(|t| normalized(&t.text)).unwrap_or_default();
            place.above = over.map(|t| normalized(&t.text)).unwrap_or_default();
            place.below = under.map(|t| normalized(&t.text)).unwrap_or_default();
            let heading_text = place.heading.map(|h| ctx.headings[h].text.as_str());
            place.instance = [
                Some(f.label.as_str()),
                Some(f.section.as_str()),
                heading_text,
                over.map(|t| t.text.as_str()),
            ]
            .into_iter()
            .flatten()
            .find_map(instance_in);
            ctx.places[k] = place;
        }
        ctx
    }

    fn len(&self) -> usize {
        self.fields.len()
    }

    /// Fields in the same block: the same extracted section and, when the
    /// layout is known, the same heading.
    fn same_block(&self, a: usize, b: usize) -> bool {
        self.fields[a].section == self.fields[b].section
            && self.places[a].heading == self.places[b].heading
    }

    fn instance(&self, at: usize) -> Option<u8> {
        if self.laid {
            self.places[at].instance
        } else {
            [&self.fields[at].label, &self.fields[at].section]
                .into_iter()
                .find_map(|t| instance_in(t))
        }
    }

    fn heading_words(&self, h: Option<usize>) -> &[String] {
        h.map_or(&[], |h| &self.headings[h].words)
    }

    /// A rule drawn between two fields across both of their columns, or a
    /// box that holds one of them and not the other.
    fn ruled(&self, a: usize, b: usize) -> bool {
        let (fa, fb) = (self.fields[a], self.fields[b]);
        if !self.laid || fa.page != fb.page {
            return false;
        }
        let (ra, rb) = (fa.rect.unwrap(), fb.rect.unwrap());
        let Some(page) = self.pages.iter().find(|p| p.number == fa.page) else {
            return false;
        };
        let (top, bottom) = if ra.y1 <= rb.y0 {
            (ra, rb)
        } else if rb.y1 <= ra.y0 {
            (rb, ra)
        } else {
            return false;
        };
        page.rules.iter().any(|l| {
            l.width() > l.height() * 4.
                && l.cy() > top.y1 + 1.
                && l.cy() < bottom.y0 - 1.
                && l.x_overlap(&ra) > 0.
                && l.x_overlap(&rb) > 0.
        }) || page.shapes.iter().any(|s| {
            s.rect.contains(&ra, 2.) != s.rect.contains(&rb, 2.)
                && s.rect.area() <= page.width * page.height * 0.6
                && s.rect.area() >= ra.area().max(rb.area()) * 1.8
        })
    }
}

/// What the layout reader made of a document, one line per field in
/// reading order: label | heading (level) | major | left | above | below.
pub fn describe_layout(doc: &Document) -> String {
    let ctx = Ctx::new(doc);
    let h = |x: Option<usize>| {
        x.map_or("-".to_string(), |h| {
            format!("{} ({})", ctx.headings[h].text, ctx.headings[h].level)
        })
    };
    let mut out = String::new();
    for (k, f) in ctx.fields.iter().enumerate() {
        let p = &ctx.places[k];
        out.push_str(&format!(
            "{:<40} | {:<45} | {:<30} | L {} | A {} | B {} | row {}/{} shape {:?}\n",
            f.label.chars().take(40).collect::<String>(),
            h(p.heading).chars().take(45).collect::<String>(),
            h(p.major).chars().take(30).collect::<String>(),
            p.left.join(" "),
            p.above.join(" "),
            p.below.join(" "),
            p.row_index,
            p.row_len,
            p.shape.map(|s| s.1)
        ));
    }
    out
}

fn bucket(x: f32, edges: &[f32]) -> usize {
    edges.iter().take_while(|e| x >= **e).count()
}

fn kind_features(ctx: &Ctx, at: usize) -> Vec<usize> {
    let b = BUCKETS;
    let f = ctx.fields[at];
    let place = &ctx.places[at];
    let own = normalized(&f.label);
    let comp = component_words(&f.label);
    let sec = normalized(&f.section);
    let mut out = vec![hash("bias", b)];
    feature_words(&mut out, "own", &own, b, true);
    feature_words(&mut out, "comp", &comp, b, false);
    for w in &sec {
        out.push(hash(&format!("sec:{w}"), b));
    }
    if let Some(first) = comp.first() {
        out.push(hash(&format!("first:{first}"), b));
    }
    if let Some(last) = comp.last() {
        out.push(hash(&format!("last:{last}"), b));
        for e in entities(&own) {
            out.push(hash(&format!("ent-last:{e}|{last}"), b));
        }
    }
    out.push(hash(&format!("len:{}", own.len().min(6)), b));
    // Whose detail: the owner word of a name, and owner words around it.
    if let Some(owner) = name_owner(&own) {
        out.push(hash(&format!("name-of:{owner}"), b));
    }
    if let Some(class) = bare(&own) {
        out.push(hash(&format!("bare:{class}"), b));
    }
    // "Name (First, Last)" is a whole name; "Last, First" is surname first.
    let first = own.iter().position(|w| w == "first" || w == "given");
    let last = own
        .iter()
        .position(|w| w == "last" || w == "surname" || w == "family");
    if let (Some(a), Some(z)) = (first, last) {
        out.push(hash(
            &format!(
                "name-order:{}",
                if a < z { "first-last" } else { "last-first" }
            ),
            b,
        ));
    }
    for e in entities(&own) {
        out.push(hash(&format!("ent:{e}"), b));
    }
    let heading = ctx.heading_words(place.heading);
    for e in entities(&sec).into_iter().chain(entities(heading)) {
        out.push(hash(&format!("heading-ent:{e}"), b));
        if let Some(last) = comp.last() {
            out.push(hash(&format!("heading-ent:{e}|{last}"), b));
        }
    }
    let has = |i: usize, set: &[&str]| {
        normalized(&ctx.fields[i].label)
            .iter()
            .any(|w| set.contains(&w.as_str()))
    };
    let mut previous_address = false;
    let mut nearby_locality = false;
    let mut next_postal = false;
    for off in 1..=3 {
        if let Some(i) = at.checked_sub(off).filter(|i| ctx.same_block(*i, at)) {
            previous_address |= has(i, &["address", "street"]);
        }
        if let Some(i) = Some(at + off).filter(|i| *i < ctx.len() && ctx.same_block(*i, at))
            && !has(i, &["birth"])
        {
            nearby_locality |= has(i, &["city", "state", "zip", "postal"]);
            next_postal |= has(i, &["zip"]);
        }
    }
    out.push(hash(&format!("layout:{previous_address}:{next_postal}"), b));
    if own.iter().any(|w| w == "address" || w == "street") {
        out.push(hash(&format!("address:locality:{nearby_locality}"), b));
    }
    for off in 1..=3 {
        for (i, side) in [
            (at.checked_sub(off), 'p'),
            (Some(at + off).filter(|i| *i < ctx.len()), 'n'),
        ] {
            let Some(i) = i.filter(|i| ctx.same_block(*i, at)) else {
                continue;
            };
            let near = component_words(&ctx.fields[i].label);
            for w in &near {
                out.push(hash(&format!("{side}{off}:{w}"), b));
            }
            if off == 1 {
                for x in &comp {
                    for y in &near {
                        out.push(hash(&format!("adj:{side}:{x}|{y}"), b));
                    }
                }
            }
        }
    }
    let run = (0..at)
        .rev()
        .take_while(|i| ctx.fields[*i].label == f.label && ctx.same_block(*i, at))
        .count()
        .min(4);
    out.push(hash(&format!("run:{run}"), b));
    for w in &comp {
        out.push(hash(&format!("run:{run}:{w}"), b));
    }
    if ctx.laid {
        // Size says a lot: a state or an initial is a short box, a whole
        // address a long or tall one.
        let width = bucket(place.width, &[2.5, 4., 7., 12., 20., 35., 55.]);
        out.push(hash(&format!("width:{width}"), b));
        out.push(hash(&format!("tall:{}", place.tall), b));
        if let Some(last) = comp.last() {
            out.push(hash(&format!("width:{width}:{last}"), b));
        }
        out.push(hash(
            &format!("row:{}:{}", place.row_len.min(6), place.row_index.min(5)),
            b,
        ));
        // City, state or zip boxes on this row or the next few, under the
        // same heading: then an address box is only its first line.
        let locality = ctx.places.iter().zip(&ctx.fields).any(|(p, g)| {
            g.page == f.page
                && p.heading == place.heading
                && p.row >= place.row
                && p.row <= place.row + 3
                && !std::ptr::eq(*g, f)
                && normalized(&g.label)
                    .iter()
                    .any(|w| ["city", "state", "zip", "postal"].contains(&w.as_str()))
        });
        out.push(hash(&format!("locality-near:{locality}"), b));
        if let Some(last) = comp.last() {
            out.push(hash(&format!("locality-near:{locality}:{last}"), b));
        }
        for (prefix, words) in [
            ("left", &place.left),
            ("above", &place.above),
            ("below", &place.below),
        ] {
            for w in words {
                out.push(hash(&format!("{prefix}:{w}"), b));
            }
            if let Some(owner) = name_owner(words) {
                out.push(hash(&format!("name-of:{owner}"), b));
            }
        }
        for w in heading {
            out.push(hash(&format!("heading:{w}"), b));
        }
    } else {
        out.push(hash("no-layout", b));
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn role_features(ctx: &Ctx, kinds: &[usize], at: usize) -> Vec<usize> {
    let b = BUCKETS;
    let f = ctx.fields[at];
    let place = &ctx.places[at];
    let own = normalized(&f.label);
    let sec = normalized(&f.section);
    let mut out = vec![hash("bias", b)];
    feature_words(&mut out, "own", &own, b, true);
    // Owner words transfer between inline labels and headings: "Employer
    // Phone" and a "Phone" under "Employer" have the same owner.
    feature_words(&mut out, "meaning", &own, b, true);
    feature_words(&mut out, "meaning", &sec, b, true);
    feature_words(&mut out, "section", &sec, b, false);
    out.push(hash(&format!("section:entire:{}", sec.join(" ")), b));
    for c in &sec {
        for w in &own {
            out.push(hash(&format!("section-label:{c}|{w}"), b));
        }
    }
    for e in entities(&own) {
        out.push(hash(&format!("ent:{e}"), b));
    }
    for e in entities(&sec) {
        out.push(hash(&format!("section-ent:{e}"), b));
    }
    if ctx.laid {
        // Plain words and word types only: a heading's exact wording is
        // particular to its form, and memorizing it does not carry over.
        let heading = ctx.heading_words(place.heading);
        let major = ctx.heading_words(place.major);
        for (prefix, words) in [("heading", heading), ("major", major)] {
            for w in words {
                out.push(hash(&format!("{prefix}:{w}"), b));
            }
            for e in entities(words) {
                out.push(hash(&format!("{prefix}-ent:{e}"), b));
            }
        }
        if let Some(h) = place.heading {
            out.push(hash(&format!("heading-level:{}", ctx.headings[h].level), b));
        }
        for (prefix, words) in [("left", &place.left), ("above", &place.above)] {
            for e in entities(words) {
                out.push(hash(&format!("{prefix}-ent:{e}"), b));
            }
        }
        // Which part of the form, by its headings.
        let mut seen = Vec::new();
        for p in &ctx.places[..=at] {
            if !seen.contains(&p.heading) {
                seen.push(p.heading);
            }
        }
        out.push(hash(&format!("heading-order:{}", seen.len().min(6)), b));
        out.push(hash(&format!("page:{}", f.page.min(4)), b));
    }
    // Conditional "other" values, and parents on a form about a minor.
    let heading = ctx.heading_words(place.heading);
    out.push(hash(
        &format!(
            "alternate:{}:{}",
            alternate(&own),
            alternate(&sec) || alternate(heading)
        ),
        b,
    ));
    let minor = names_minor(ctx);
    let parent = any_of(&own, &PARENTS) || any_of(&sec, &PARENTS) || any_of(heading, &PARENTS);
    let young = any_of(&own, &MINORS) || any_of(&sec, &MINORS) || any_of(heading, &MINORS);
    out.push(hash(
        &format!("minor-form:{minor}:parent:{parent}:young:{young}"),
        b,
    ));
    // Where on the form: the first block is usually the subject's.
    let mut sections = Vec::new();
    for input in &ctx.fields[..=at] {
        if !sections.contains(&&input.section) {
            sections.push(&input.section);
        }
    }
    out.push(hash(&format!("order:{}", sections.len().min(6)), b));
    out.push(hash(&format!("pos:{}", at * 5 / ctx.len().max(1)), b));
    // A label seen before on this form starts another person's copy.
    let s = stem(f);
    let repeat = ctx.fields[..at]
        .iter()
        .filter(|g| stem(g) == s)
        .count()
        .min(3);
    out.push(hash(&format!("repeat:{repeat}"), b));
    for w in &own {
        out.push(hash(&format!("repeat:{repeat}:{w}"), b));
    }
    if let Some(n) = ctx.instance(at) {
        out.push(hash(&format!("instance:{}", n.min(3)), b));
    }
    // The other labels of the same block: a "Relationship" field nearby
    // says the block is somebody else's.
    for i in (0..ctx.len()).filter(|i| i.abs_diff(at) <= 8 && *i != at && ctx.same_block(*i, at)) {
        for w in component_words(&ctx.fields[i].label) {
            out.push(hash(&format!("mate:{w}"), b));
        }
        out.push(hash(&format!("mate-kind:{}", KINDS[kinds[i]]), b));
    }
    // Structure from the kinds: a block that asks again for a detail the
    // form already asked for is usually somebody else's.
    let detail = |k: usize| !["Other", "Date", "Signature"].contains(&KINDS[k]);
    let kind = kinds[at];
    out.push(hash(&format!("kind:{}", KINDS[kind]), b));
    if detail(kind) {
        let before: Vec<_> = (0..at).filter(|i| kinds[*i] == kind).collect();
        let elsewhere = before.iter().any(|i| !ctx.same_block(*i, at));
        let n = before.len().min(2);
        for f in [
            format!("kseen:{n}"),
            format!("kseen:{n}:{}", KINDS[kind]),
            format!("kseen-elsewhere:{elsewhere}"),
        ] {
            out.push(hash(&f, b));
        }
    }
    let is_name = |k: usize| KINDS[k].contains("Name") && KINDS[k] != "StreetName";
    if let Some(first) = (0..ctx.len()).find(|i| is_name(kinds[*i])) {
        let block = (first.min(at)..=first.max(at)).all(|i| ctx.same_block(i, at));
        out.push(hash(&format!("first-name-block:{block}"), b));
        out.push(hash(&format!("after-first-name:{}", at >= first), b));
    }
    // Owner words in the labels just before and after, for forms without
    // headings.
    for off in 1..=3 {
        if let Some(i) = at.checked_sub(off) {
            for w in normalized(&ctx.fields[i].label) {
                out.push(hash(&format!("before:{w}"), b));
            }
        }
        if let Some(g) = ctx.fields.get(at + off) {
            for w in normalized(&g.label) {
                out.push(hash(&format!("after:{w}"), b));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn distance(d: usize) -> &'static str {
    match d {
        1 => "1",
        2 => "2",
        3 => "3",
        4..=6 => "4-6",
        7..=12 => "7-12",
        13..=25 => "13-25",
        _ => "far",
    }
}

fn same(a: Option<usize>, b: Option<usize>) -> &'static str {
    match (a, b) {
        (Some(a), Some(b)) if a == b => "same",
        (None, None) => "none",
        (Some(_), Some(_)) => "differ",
        _ => "one",
    }
}

fn pair_features(ctx: &Ctx, kinds: &[usize], roles: &[usize], i: usize, j: usize) -> Vec<usize> {
    let b = PAIR_BUCKETS;
    let (ki, kj) = (KINDS[kinds[i]], KINDS[kinds[j]]);
    let (ri, rj) = (ROLES[roles[i]], ROLES[roles[j]]);
    let same_role = ri == rj;
    let (fi, fj) = (ctx.fields[i], ctx.fields[j]);
    let section = if fi.section.is_empty() && fj.section.is_empty() {
        "none"
    } else if fi.section == fj.section {
        "same"
    } else {
        "differ"
    };
    let changes = (i..j).filter(|k| !ctx.same_block(*k, k + 1)).count().min(3);
    // Another copy of a detail between the two (a second "First Name")
    // usually means another instance has started.
    let mut seen = HashSet::new();
    let mut repeated = false;
    for k in i..=j {
        if kinds[k] != 0
            && !["Date", "Signature"].contains(&KINDS[kinds[k]])
            && !seen.insert(kinds[k])
        {
            repeated = true;
        }
    }
    let stem_repeat = ((i + 1)..=j).any(|k| stem(ctx.fields[k]) == stem(fi));
    let inst = match (ctx.instance(i), ctx.instance(j)) {
        (Some(a), Some(b)) if a == b => "same",
        (Some(_), Some(_)) => "differ",
        (None, None) => "none",
        _ => "one",
    };
    let d = distance(j - i);
    let mut feats = vec![
        format!("dist:{d}"),
        format!("section:{section}"),
        format!("changes:{changes}"),
        format!("roles:{ri}|{rj}"),
        format!("same-role:{same_role}"),
        format!("kinds:{ki}|{kj}"),
        format!("same-kind:{}", ki == kj),
        format!("repeated:{repeated}"),
        format!("stem-repeat:{stem_repeat}"),
        format!("instance:{inst}"),
        format!("role-dist:{same_role}:{d}"),
        format!("role-section:{same_role}:{section}"),
        format!("role-changes:{same_role}:{changes}"),
        format!("role-repeated:{same_role}:{repeated}"),
        format!("role-instance:{same_role}:{inst}"),
        format!("role-pair-section:{ri}|{rj}:{section}"),
        format!("role-pair-repeated:{ri}|{rj}:{repeated}"),
        format!("section-repeated:{section}:{repeated}"),
    ];
    if ctx.laid {
        let (pi, pj) = (&ctx.places[i], &ctx.places[j]);
        let (ra, rb) = (fi.rect.unwrap(), fj.rect.unwrap());
        let same_page = fi.page == fj.page;
        let heading = same(pi.heading, pj.heading);
        let major = same(pi.major, pj.major);
        let headings_between = match (pi.heading, pj.heading) {
            (Some(a), Some(b)) => a.abs_diff(b).min(3),
            _ => 0,
        };
        let shape = same(
            pi.shape.map(|s| s.1 + 100_000 * s.0 as usize),
            pj.shape.map(|s| s.1 + 100_000 * s.0 as usize),
        );
        let same_row = pi.row == pj.row;
        let gap = if same_page {
            bucket(
                (ra.cy() - rb.cy()).abs() / pi.body,
                &[0.5, 1.5, 3., 6., 12., 25.],
            )
        } else {
            9
        };
        let column = same_page && (ra.x0 - rb.x0).abs() < 4.;
        let grid = pi.row_len >= 3 && pj.row_len >= 3 && pi.col_len >= 3 && pj.col_len >= 3;
        let ruled = ctx.ruled(i, j);
        let page = if same_page {
            "same".to_string()
        } else {
            fi.page.abs_diff(fj.page).min(2).to_string()
        };
        feats.extend([
            format!("page:{page}"),
            format!("heading:{heading}"),
            format!("major:{major}"),
            format!("headings-between:{headings_between}"),
            format!("shape:{shape}"),
            format!("row:{same_row}"),
            format!("gap:{gap}"),
            format!("column:{column}"),
            format!("grid:{grid}:{same_row}:{column}"),
            format!("ruled:{ruled}"),
            format!("role-heading:{same_role}:{heading}"),
            format!("role-shape:{same_role}:{shape}"),
            format!("role-row:{same_role}:{same_row}"),
            format!("role-gap:{same_role}:{gap}"),
            format!("role-ruled:{same_role}:{ruled}"),
            format!("heading-repeated:{heading}:{repeated}"),
            format!("heading-instance:{heading}:{inst}"),
            format!("row-kinds:{same_row}:{}", ki == kj),
            format!("gap-ruled:{gap}:{ruled}"),
            format!("shape-heading:{shape}:{heading}"),
            format!("major-role:{major}:{ri}|{rj}"),
        ]);
    } else {
        feats.push("no-layout".into());
    }
    let mut out = vec![hash("bias", b)];
    out.extend(feats.iter().map(|f| hash(f, b)));
    out.sort_unstable();
    out.dedup();
    out
}

/// Features of each group of a form for the block-level role head, keyed
/// by group id. `assign[k]` is field `k`'s group; `kinds` its kind.
fn group_features(ctx: &Ctx, assign: &[usize], kinds: &[usize]) -> Vec<(usize, Vec<usize>)> {
    let b = GROUP_BUCKETS;
    let mut ids: Vec<usize> = Vec::new();
    for g in assign {
        if !ids.contains(g) {
            ids.push(*g);
        }
    }
    let detail = |k: usize| !["Other", "Date", "Signature"].contains(&KINDS[k]);
    let mut asked: HashSet<usize> = HashSet::new();
    let mut named_before = false;
    let minor = names_minor(ctx);
    let mut out = Vec::new();
    for (rank, &g) in ids.iter().enumerate() {
        let members: Vec<_> = (0..ctx.len()).filter(|k| assign[*k] == g).collect();
        let mut f = vec![hash("bias", b)];
        let mut add = |s: String| f.push(hash(&s, b));
        let mut words = HashSet::new();
        let mut heads = HashSet::new();
        let mut sections = HashSet::new();
        let mut group_kinds = HashSet::new();
        for &k in &members {
            words.extend(normalized(&ctx.fields[k].label));
            sections.extend(normalized(&ctx.fields[k].section));
            heads.extend(ctx.heading_words(ctx.places[k].heading).iter().cloned());
            heads.extend(ctx.heading_words(ctx.places[k].major).iter().cloned());
            group_kinds.insert(kinds[k]);
            if let Some(n) = ctx.instance(k) {
                add(format!("instance:{}", n.min(3)));
            }
        }
        for w in &words {
            add(format!("w:{w}"));
        }
        for w in &sections {
            add(format!("s:{w}"));
        }
        for w in &heads {
            add(format!("h:{w}"));
        }
        let list = |set: &HashSet<String>| set.iter().cloned().collect::<Vec<_>>();
        for e in entities(&list(&words)) {
            add(format!("label-ent:{e}"));
        }
        for e in entities(&list(&sections))
            .into_iter()
            .chain(entities(&list(&heads)))
        {
            add(format!("heading-ent:{e}"));
        }
        for k in &group_kinds {
            add(format!("k:{}", KINDS[*k]));
        }
        let (words_v, heads_v) = (
            list(&words),
            list(&sections)
                .into_iter()
                .chain(list(&heads))
                .collect::<Vec<_>>(),
        );
        add(format!(
            "alternate:{}:{}",
            alternate(&words_v),
            alternate(&heads_v)
        ));
        let parent = any_of(&words_v, &PARENTS) || any_of(&heads_v, &PARENTS);
        let young = any_of(&words_v, &MINORS) || any_of(&heads_v, &MINORS);
        add(format!("minor-form:{minor}:parent:{parent}:young:{young}"));
        let details: HashSet<_> = group_kinds.iter().copied().filter(|k| detail(*k)).collect();
        let overlap = details.intersection(&asked).count();
        add(format!("repeat:{}", overlap.min(3)));
        add(format!("rank:{}", rank.min(5)));
        let named = group_kinds
            .iter()
            .any(|k| KINDS[*k].contains("Name") && KINDS[*k] != "StreetName");
        add(format!("first-named:{}", named && !named_before));
        add(format!("named:{named}"));
        add(format!(
            "size:{}",
            bucket(members.len() as f32, &[2., 3., 5., 8., 13.])
        ));
        add(format!("page:{}", ctx.fields[members[0]].page.min(4)));
        add(format!("pos:{}", members[0] * 5 / ctx.len().max(1)));
        add(format!("repeat-named:{}:{named}", overlap.min(2)));
        named_before |= named;
        asked.extend(details);
        f.sort_unstable();
        f.dedup();
        out.push((g, f));
    }
    out
}

fn best(p: &[f32]) -> (usize, f32) {
    p.iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
}

/// Average-linkage clustering of `n` fields over pair scores `p[i][j]`
/// (`i < j`), merging while the best mean score is above `cut`.
#[allow(clippy::needless_range_loop)] // rows `a`, `b` and column `c` together
fn cluster(n: usize, p: &[Vec<f32>], cut: f32) -> Vec<usize> {
    // Score sums between clusters, kept up to date as they merge: O(n^2)
    // per merge, so a few hundred fields stay quick.
    let mut size = vec![1usize; n];
    let mut sum: Vec<Vec<f32>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| if i == j { 0. } else { p[i.min(j)][i.max(j)] })
                .collect()
        })
        .collect();
    let mut alive = vec![true; n];
    let mut root: Vec<usize> = (0..n).collect();
    loop {
        let mut top = (cut, usize::MAX, usize::MAX);
        for a in (0..n).filter(|a| alive[*a]) {
            for b in (a + 1..n).filter(|b| alive[*b]) {
                let mean = sum[a][b] / (size[a] * size[b]) as f32;
                if mean > top.0 {
                    top = (mean, a, b);
                }
            }
        }
        let (_, a, b) = top;
        if a == usize::MAX {
            break;
        }
        alive[b] = false;
        size[a] += size[b];
        for r in root.iter_mut().filter(|r| **r == b) {
            *r = a;
        }
        for c in 0..n {
            let s = sum[b][c];
            sum[a][c] += s;
            sum[c][a] = sum[a][c];
        }
    }
    let mut ids = HashMap::new();
    root.iter()
        .map(|r| {
            let next = ids.len();
            *ids.entry(*r).or_insert(next)
        })
        .collect()
}

/// Training settings. Fixed numbers, so runs are comparable.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub epochs: usize,
    pub pair_epochs: usize,
    /// Folds for the out-of-fold predictions the pair scorer learns from.
    pub folds: usize,
    /// Generated forms whose personal fields add kind examples (0 = none).
    pub synthetic: usize,
    pub seed: u64,
    /// Per-epoch weight decay of the kind and role heads.
    pub kind_decay: f32,
    pub role_decay: f32,
    /// Weight of the block-level role head (0 = fields' votes only).
    pub block_weight: f32,
    /// See [`AnalysisModel::join`].
    pub join: f32,
    /// See [`AnalysisModel::cut`].
    pub cut: f32,
    /// Ensemble members, each trained with its own seed.
    pub members: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            epochs: 20,
            pair_epochs: 12,
            folds: 5,
            synthetic: 500,
            seed: 1,
            kind_decay: 0.997,
            role_decay: 0.997,
            block_weight: 0.5,
            join: 0.0,
            cut: 0.5,
            members: 3,
        }
    }
}

/// How roles and groups are settled after the pair clustering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decode {
    /// Each cluster takes the mean of its fields' role distributions.
    Vote,
    /// Vote, then the applicant is the heaviest applicant cluster plus any
    /// others that ask for none of the details it already has, as one group.
    #[default]
    Subject,
}

/// The four heads: one per training seed while training, then their
/// average, quantized, is the model.
#[derive(Debug, Clone, PartialEq)]
struct Parts {
    kind: Head,
    role: Head,
    pair: Head,
    /// Block-level roles: one example per group.
    block: Head,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisModel {
    /// Members trained with different seeds, their weights averaged: the
    /// result depends far less on any one shuffle, at the size of one.
    parts: Parts,
    pub decode: Decode,
    /// How much the block head counts against the fields' own role votes.
    pub block_weight: f32,
    /// The least mean pair score for another applicant cluster to join the
    /// subject's group.
    pub join: f32,
    /// Clusters merge while their mean pair score is above this.
    pub cut: f32,
}

/// Both text tracks of each form, with their truths in document order.
fn tracks<'a>(forms: &[&'a Form]) -> Vec<(Document, Vec<&'a Field>)> {
    let mut out = Vec::new();
    for form in forms {
        for extracted in [false, true] {
            out.push((form.document(extracted), form.fields.iter().collect()));
        }
    }
    out
}

fn heads(forms: &[&Form], settings: Settings) -> (Head, Head) {
    let mut kinds = Vec::new();
    let mut roles = Vec::new();
    for (doc, fields) in tracks(forms) {
        let ctx = Ctx::new(&doc);
        let truth: Vec<_> = ctx.order.iter().map(|i| fields[*i]).collect();
        let gold_kinds: Vec<_> = truth.iter().map(|f| f.kind).collect();
        for (at, f) in truth.iter().enumerate() {
            kinds.push((kind_features(&ctx, at), f.kind, 1.));
            roles.push((role_features(&ctx, &gold_kinds, at), f.role, 1.));
        }
    }
    // Generated forms know the right kind of their personal fields. Their
    // `Other` fields are not used: there, a parent's phone is `Other`.
    let mut rng = synth::Rng::new(settings.seed ^ 0x5eed);
    for n in 0..settings.synthetic {
        let form = if n % 2 == 0 {
            synth::form(&mut rng)
        } else {
            synth::application(&mut rng)
        };
        let doc = Document {
            fields: form
                .iter()
                .map(|(l, _)| {
                    let (label, section) = l.split_once(" || ").unwrap_or((l, ""));
                    Input {
                        label: label.into(),
                        section: section.into(),
                        ..Input::default()
                    }
                })
                .collect(),
            pages: Vec::new(),
        };
        let ctx = Ctx::new(&doc);
        for (at, (_, class)) in form.iter().enumerate() {
            let name = format!("{class:?}");
            if let Some(kind) = KINDS.iter().position(|k| *k == name).filter(|k| *k != 0) {
                kinds.push((kind_features(&ctx, at), kind, 0.3));
            }
        }
    }
    (
        Head::fit_decay(
            KINDS.len(),
            BUCKETS,
            settings.epochs,
            settings.seed,
            kinds,
            settings.kind_decay,
        ),
        Head::fit_decay(
            ROLES.len(),
            BUCKETS,
            settings.epochs,
            settings.seed,
            roles,
            settings.role_decay,
        ),
    )
}

fn predict(head: &Head, f: &[usize]) -> (usize, f32) {
    best(&head.probabilities(f))
}

impl Parts {
    fn train(forms: &[Form], settings: Settings) -> Self {
        let all: Vec<_> = forms.iter().collect();
        let (kind, role) = heads(&all, settings);
        // Out-of-fold kinds and roles for the pair scorer.
        let mut pairs = Vec::new();
        for fold in 0..settings.folds {
            let pick = |held: bool| -> Vec<&Form> {
                all.iter()
                    .enumerate()
                    .filter(|(n, _)| (n % settings.folds == fold) == held)
                    .map(|(_, f)| *f)
                    .collect()
            };
            let (train, held) = (pick(false), pick(true));
            if held.is_empty() || train.is_empty() {
                continue;
            }
            let (k, r) = heads(&train, settings);
            for (doc, fields) in tracks(&held) {
                let ctx = Ctx::new(&doc);
                let truth: Vec<_> = ctx.order.iter().map(|i| fields[*i]).collect();
                let kinds: Vec<_> = (0..ctx.len())
                    .map(|at| decide_kind(&ctx, at, &k.probabilities(&kind_features(&ctx, at))).0)
                    .collect();
                let roles: Vec<_> = (0..ctx.len())
                    .map(|at| predict(&r, &role_features(&ctx, &kinds, at)).0)
                    .collect();
                for i in 0..ctx.len() {
                    for j in i + 1..ctx.len() {
                        let same = truth[i].group == truth[j].group;
                        pairs.push((
                            pair_features(&ctx, &kinds, &roles, i, j),
                            usize::from(same),
                            1.,
                        ));
                    }
                }
            }
        }
        let pair = Head::fit_decay(
            2,
            PAIR_BUCKETS,
            settings.pair_epochs,
            settings.seed,
            pairs,
            0.997,
        );
        // Block roles, from the true groups and kinds.
        let mut blocks = Vec::new();
        for (doc, fields) in tracks(&all) {
            let ctx = Ctx::new(&doc);
            let truth: Vec<_> = ctx.order.iter().map(|i| fields[*i]).collect();
            let mut ids: Vec<&str> = Vec::new();
            let assign: Vec<_> = truth
                .iter()
                .map(|f| {
                    ids.iter().position(|g| *g == f.group).unwrap_or_else(|| {
                        ids.push(&f.group);
                        ids.len() - 1
                    })
                })
                .collect();
            let kinds: Vec<_> = truth.iter().map(|f| f.kind).collect();
            for (g, features) in group_features(&ctx, &assign, &kinds) {
                let at = assign.iter().position(|x| *x == g).unwrap();
                let size = assign.iter().filter(|x| **x == g).count() as f32;
                blocks.push((features, truth[at].role, size.sqrt()));
            }
        }
        let block = Head::fit_decay(
            ROLES.len(),
            GROUP_BUCKETS,
            settings.epochs,
            settings.seed,
            blocks,
            settings.role_decay,
        );
        Self {
            kind,
            role,
            pair,
            block,
        }
    }
}

impl AnalysisModel {
    pub fn train(forms: &[Form], settings: Settings) -> Self {
        assert!(!forms.is_empty() && settings.folds >= 2 && settings.members >= 1);
        let members: Vec<_> = (0..settings.members as u64)
            .map(|m| {
                Parts::train(
                    forms,
                    Settings {
                        seed: settings.seed + 1000 * m,
                        ..settings
                    },
                )
            })
            .collect();
        let merge = |head: fn(&Parts) -> &Head| {
            let mut merged = Head::mean(&members.iter().map(head).collect::<Vec<_>>());
            merged.quantize();
            merged
        };
        let parts = Parts {
            kind: merge(|p| &p.kind),
            role: merge(|p| &p.role),
            pair: merge(|p| &p.pair),
            block: merge(|p| &p.block),
        };
        Self {
            parts,
            decode: Decode::default(),
            block_weight: settings.block_weight,
            join: settings.join,
            cut: settings.cut,
        }
    }

    /// `ATLANA02`, the decoding settings, then the four heads (see
    /// `Head::write`): about 1 MB.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.push(match self.decode {
            Decode::Vote => 0,
            Decode::Subject => 1,
        });
        for v in [self.block_weight, self.join, self.cut] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let p = &self.parts;
        for head in [&p.kind, &p.role, &p.pair, &p.block] {
            head.write(&mut out);
        }
        out
    }

    /// The model trained on every reviewed form (30 forms, 1,341 fields),
    /// as `examples/analysis_v4.rs ship` writes it.
    pub fn bundled() -> Self {
        Self::from_bytes(include_bytes!("../data/analysis.bin")).expect("the bundled model reads")
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.get(..8)? != MAGIC {
            return None;
        }
        let decode = match *bytes.get(8)? {
            0 => Decode::Vote,
            1 => Decode::Subject,
            _ => return None,
        };
        let mut at = 9;
        let mut float = || {
            let v = f32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?);
            at += 4;
            v.is_finite().then_some(v)
        };
        let (block_weight, join, cut) = (float()?, float()?, float()?);
        let mut head = |classes: usize| Head::read(bytes, &mut at).filter(|h| h.classes == classes);
        let parts = Parts {
            kind: head(KINDS.len())?,
            role: head(ROLES.len())?,
            pair: head(2)?,
            block: head(ROLES.len())?,
        };
        (at == bytes.len()).then_some(Self {
            parts,
            decode,
            block_weight,
            join,
            cut,
        })
    }

    fn mean(&self, head: impl Fn(&Parts) -> &Head, f: &[usize]) -> Vec<f32> {
        head(&self.parts).probabilities(f)
    }

    /// One analysis per field of `doc`, in the document's own order.
    pub fn analyze(&self, doc: &Document) -> Vec<Analysis> {
        let ctx = Ctx::new(doc);
        let n = ctx.len();
        let kinds: Vec<_> = (0..n)
            .map(|at| decide_kind(&ctx, at, &self.mean(|p| &p.kind, &kind_features(&ctx, at))))
            .collect();
        let k: Vec<_> = kinds.iter().map(|x| x.0).collect();
        let role_p: Vec<_> = (0..n)
            .map(|at| self.mean(|p| &p.role, &role_features(&ctx, &k, at)))
            .collect();
        let r: Vec<_> = role_p.iter().map(|p| best(p).0).collect();
        let mut p = vec![vec![0f32; n]; n];
        for i in 0..n {
            for j in i + 1..n {
                p[i][j] = self.mean(|p| &p.pair, &pair_features(&ctx, &k, &r, i, j))[1];
            }
        }
        let cluster_of = cluster(n, &p, self.cut);
        let mut groups = cluster_of.clone();
        // One role per group: the mean of its fields' role distributions.
        let count = cluster_of.iter().max().map_or(0, |m| m + 1);
        let mut mean = vec![vec![0f32; ROLES.len()]; count];
        for (at, g) in cluster_of.iter().enumerate() {
            for (m, v) in mean[*g].iter_mut().zip(&role_p[at]) {
                *m += v;
            }
        }
        // Mix the fields' votes with the block head's view of the group.
        for (g, features) in group_features(&ctx, &cluster_of, &k) {
            let size = cluster_of.iter().filter(|x| **x == g).count() as f32;
            let block = self.mean(|p| &p.block, &features);
            for (m, v) in mean[g].iter_mut().zip(block) {
                *m = (1. - self.block_weight) * *m / size + self.block_weight * v;
            }
        }
        let share = |m: &[f32], (r, v): (usize, f32)| (r, v / m.iter().sum::<f32>());
        let group_role: Vec<_> = mean.iter().map(|m| share(m, best(m))).collect();
        let mut role_of: Vec<_> = cluster_of.iter().map(|g| group_role[*g]).collect();
        if self.decode == Decode::Subject {
            // The subject is one entity. Take applicant clusters by
            // applicant weight; a cluster joins only when it asks for no
            // detail the subject already has (a second name or phone is
            // somebody else's). The rest take their best other role.
            let mut weight = vec![0f32; count];
            for g in &cluster_of {
                weight[*g] += mean[*g][0];
            }
            let mut order: Vec<_> = (0..count).filter(|g| group_role[*g].0 == 0).collect();
            order.sort_by(|a, b| weight[*b].total_cmp(&weight[*a]));
            // Details a person has once: a second name, birth date or home
            // address is somebody else's. Phones and emails repeat.
            let detail = |k: usize| {
                personal_kind(KINDS[k])
                    || matches!(
                        KINDS[k],
                        "AddressWhole"
                            | "AddressLine1"
                            | "City"
                            | "State"
                            | "Zip"
                            | "CityStateZip"
                            | "CityState"
                    )
            };
            let mut held = HashSet::new();
            let mut subject: Vec<usize> = Vec::new();
            // Fields joined to the subject group, and clusters that keep the
            // applicant role but stay their own group (a dog it owns).
            let mut joined: Vec<usize> = Vec::new();
            let mut own = Vec::new();
            for g in order {
                let members: Vec<_> = (0..n).filter(|at| cluster_of[*at] == g).collect();
                let asks: HashSet<_> = members
                    .iter()
                    .filter(|at| detail(k[**at]))
                    .map(|at| k[*at])
                    .collect();
                if subject.is_empty() {
                    held.extend(asks);
                    subject.push(g);
                    joined.extend(members);
                    continue;
                }
                if !asks.is_disjoint(&held) {
                    continue;
                }
                let mut sum = 0.;
                for &a in &members {
                    for &b in &joined {
                        sum += p[a.min(b)][a.max(b)];
                    }
                }
                let link = sum / (members.len() * joined.len()) as f32;
                if link >= self.join {
                    held.extend(asks);
                    subject.push(g);
                    joined.extend(members);
                } else if asks.is_empty() {
                    own.push(g);
                }
            }
            let first = (0..n).find(|at| subject.contains(&cluster_of[*at]));
            for at in 0..n {
                let g = cluster_of[at];
                if let Some(first) = first.filter(|_| subject.contains(&g)) {
                    groups[at] = groups[first];
                } else if own.contains(&g) {
                } else if group_role[g].0 == 0 {
                    let (r, v) = best(&mean[g][1..]);
                    role_of[at] = share(&mean[g], (r + 1, v));
                }
            }
        }
        let mut out = vec![None; n];
        for at in 0..n {
            out[ctx.order[at]] = Some(Analysis {
                kind: KINDS[k[at]],
                kind_confidence: kinds[at].1,
                role: ROLES[role_of[at].0],
                role_confidence: role_of[at].1,
                group: groups[at],
            });
        }
        out.into_iter().map(Option::unwrap).collect()
    }
}

/// Counts for one form or a total. Group scores are over pairs of fields:
/// a pair is positive when both fields are in the same group.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Score {
    pub fields: usize,
    pub kind_right: usize,
    pub role_right: usize,
    pub pairs_true: usize,
    pub pairs_predicted: usize,
    pub pairs_both: usize,
    /// The same, over pairs of fields that both ask for a detail the
    /// profile holds (names, birth dates, addresses, phones, email).
    pub detail_true: usize,
    pub detail_predicted: usize,
    pub detail_both: usize,
    /// Fields the gold structure says the rules would fill.
    pub fills_gold: usize,
    /// Predicted fills of a field that is not the applicant's own.
    pub fills_foreign: usize,
    /// Predicted fills of an applicant field with the wrong kind.
    pub fills_wrong_kind: usize,
    pub fills_right: usize,
}

fn f1(both: usize, predicted: usize, truth: usize) -> (f32, f32, f32) {
    let p = both as f32 / predicted.max(1) as f32;
    let r = both as f32 / truth.max(1) as f32;
    (p, r, if p + r > 0. { 2. * p * r / (p + r) } else { 0. })
}

impl Score {
    pub fn of(form: &Form, said: &[Analysis]) -> Self {
        let mut s = Self {
            fields: form.fields.len(),
            ..Self::default()
        };
        for (i, (f, a)) in form.fields.iter().zip(said).enumerate() {
            s.kind_right += usize::from(KINDS[f.kind] == a.kind);
            s.role_right += usize::from(ROLES[f.role] == a.role);
            s.fills_gold += usize::from(f.role == 0 && fillable(KINDS[f.kind]));
            if a.fills() {
                if f.role != 0 {
                    s.fills_foreign += 1;
                } else if KINDS[f.kind] != a.kind {
                    s.fills_wrong_kind += 1;
                } else {
                    s.fills_right += 1;
                }
            }
            for (g, b) in form.fields[i + 1..].iter().zip(&said[i + 1..]) {
                let t = f.group == g.group;
                let p = a.group == b.group;
                s.pairs_true += usize::from(t);
                s.pairs_predicted += usize::from(p);
                s.pairs_both += usize::from(t && p);
                if fillable(KINDS[f.kind]) && fillable(KINDS[g.kind]) {
                    s.detail_true += usize::from(t);
                    s.detail_predicted += usize::from(p);
                    s.detail_both += usize::from(t && p);
                }
            }
        }
        s
    }
    pub fn add(&mut self, o: &Self) {
        self.fields += o.fields;
        self.kind_right += o.kind_right;
        self.role_right += o.role_right;
        self.pairs_true += o.pairs_true;
        self.pairs_predicted += o.pairs_predicted;
        self.pairs_both += o.pairs_both;
        self.detail_true += o.detail_true;
        self.detail_predicted += o.detail_predicted;
        self.detail_both += o.detail_both;
        self.fills_gold += o.fills_gold;
        self.fills_foreign += o.fills_foreign;
        self.fills_wrong_kind += o.fills_wrong_kind;
        self.fills_right += o.fills_right;
    }
    pub fn group_f1(&self) -> (f32, f32, f32) {
        f1(self.pairs_both, self.pairs_predicted, self.pairs_true)
    }
    /// Group F1 over pairs of profile-detail fields only.
    pub fn detail_f1(&self) -> (f32, f32, f32) {
        f1(self.detail_both, self.detail_predicted, self.detail_true)
    }
    pub fn line(&self) -> String {
        let (p, r, f1) = self.group_f1();
        let (_, _, detail) = self.detail_f1();
        let pct = |a: usize, b: usize| 100. * a as f32 / b.max(1) as f32;
        format!(
            "kind {}/{} ({:.1}%); role {}/{} ({:.1}%); group pairs P {:.1}% R {:.1}% F1 {:.1}%; detail F1 {:.1}%; fills right {}/{} gold, foreign {}, wrong kind {}",
            self.kind_right,
            self.fields,
            pct(self.kind_right, self.fields),
            self.role_right,
            self.fields,
            pct(self.role_right, self.fields),
            100. * p,
            100. * r,
            100. * f1,
            100. * detail,
            self.fills_right,
            self.fills_gold,
            self.fills_foreign,
            self.fills_wrong_kind,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_numbers_come_from_the_text() {
        let n: Vec<_> = [
            "Parent 2 Phone",
            "Emergency Contact #1",
            "Secondary Insurance",
            "Date of Birth (MM/DD/YYYY)",
            "Phone 555 1234",
            "Birth Date_Row_3",
        ]
        .iter()
        .map(|t| instance_in(t))
        .collect();
        assert_eq!(n, [Some(2), Some(1), Some(2), None, None, Some(3)]);
    }

    #[test]
    fn the_owner_of_a_name_is_found_either_side() {
        assert_eq!(name_owner(&normalized("Dog Name")), Some("animal"));
        assert_eq!(
            name_owner(&normalized("Name of Club")),
            Some("organization")
        );
        assert_eq!(name_owner(&normalized("Mother's Name")), Some("person"));
        assert_eq!(name_owner(&normalized("Name")), None);
    }

    #[test]
    fn clustering_merges_only_above_one_half() {
        let mut p = vec![vec![0.; 4]; 4];
        p[0][1] = 0.9;
        p[2][3] = 0.8;
        p[0][2] = 0.4;
        p[1][2] = 0.6;
        assert_eq!(cluster(4, &p, 0.5), [0, 0, 1, 1]);
    }

    #[test]
    fn reading_order_goes_by_rows_then_left_to_right() {
        let at = |x: f32, y: f32| Input {
            page: 1,
            rect: Some(Rect {
                x0: x,
                y0: y,
                x1: x + 50.,
                y1: y + 12.,
            }),
            ..Input::default()
        };
        let fields = vec![at(300., 101.), at(20., 100.), at(20., 140.), at(300., 60.)];
        assert_eq!(reading_order(&fields).0, [3, 1, 0, 2]);
    }

    #[test]
    fn headings_are_found_by_typography_and_attached_to_the_fields_below() {
        let text = |x0, y0, size, bold, s: &str| Text {
            rect: Rect {
                x0,
                y0,
                x1: x0 + s.len() as f32 * size * 0.5,
                y1: y0 + size,
            },
            size,
            bold,
            text: s.into(),
        };
        let field = |x0, y0| Input {
            label: "Name".into(),
            page: 1,
            rect: Some(Rect {
                x0,
                y0,
                x1: x0 + 120.,
                y1: y0 + 12.,
            }),
            ..Input::default()
        };
        let doc = Document {
            fields: vec![field(80., 60.), field(80., 160.)],
            pages: vec![Page {
                number: 1,
                width: 612.,
                height: 792.,
                texts: vec![
                    text(20., 40., 14., true, "Applicant Information"),
                    text(20., 62., 9., false, "Name:"),
                    text(20., 140., 9., true, "EMERGENCY CONTACT"),
                    text(20., 162., 9., false, "Name:"),
                    text(
                        20.,
                        300.,
                        9.,
                        false,
                        "Body text that is long enough to set the size of the body.",
                    ),
                ],
                ..Page::default()
            }],
        };
        let ctx = Ctx::new(&doc);
        let heading = |k: usize| ctx.places[k].heading.map(|h| ctx.headings[h].text.clone());
        assert_eq!(heading(0).as_deref(), Some("Applicant Information"));
        assert_eq!(heading(1).as_deref(), Some("EMERGENCY CONTACT"));
        assert_eq!(ctx.places[0].left, ["name"]);
    }

    #[test]
    fn the_corpus_parser_rejects_a_group_with_two_roles() {
        let row =
            |field: &str, role: &str| format!("f\t1\t{field}\tPhone\tg\t{role}\tPhone\t\tPhone\t");
        let good = format!("{}\n{}\n", row("a", "applicant"), row("b", "applicant"));
        assert_eq!(parse(&good).unwrap()[0].fields.len(), 2);
        let bad = format!("{}\n{}\n", row("a", "applicant"), row("b", "guardian"));
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn a_small_corpus_trains_and_reads_its_own_forms() {
        let mut tsv = String::new();
        for n in 0..4 {
            for (field, kind, group, role, label, section) in [
                (
                    "a",
                    "FirstName",
                    "applicant",
                    "applicant",
                    "First Name",
                    "Applicant",
                ),
                (
                    "b",
                    "LastName",
                    "applicant",
                    "applicant",
                    "Last Name",
                    "Applicant",
                ),
                ("c", "Phone", "applicant", "applicant", "Phone", "Applicant"),
                (
                    "d",
                    "FullName",
                    "emergency-1",
                    "emergency",
                    "Name",
                    "Emergency Contact",
                ),
                (
                    "e",
                    "Phone",
                    "emergency-1",
                    "emergency",
                    "Phone",
                    "Emergency Contact",
                ),
                (
                    "f",
                    "Relationship",
                    "emergency-1",
                    "emergency",
                    "Relationship",
                    "Emergency Contact",
                ),
            ] {
                tsv.push_str(&format!(
                    "form{n}\t1\t{field}\t{kind}\t{group}\t{role}\t{label}\t{section}\t{label}\t{section}\n"
                ));
            }
        }
        let forms = parse(&tsv).unwrap();
        let model = AnalysisModel::train(
            &forms,
            Settings {
                folds: 2,
                synthetic: 0,
                ..Settings::default()
            },
        );
        let said = model.analyze(&forms[0].document(false));
        let score = Score::of(&forms[0], &said);
        assert_eq!(score.kind_right, 6, "{said:?}");
        assert_eq!(score.role_right, 6, "{said:?}");
        assert_eq!(score.group_f1().2, 1., "{said:?}");
        assert!(said[2].fills() && !said[4].fills());
        let back = AnalysisModel::from_bytes(&model.to_bytes()).unwrap();
        assert_eq!(back, model);
        assert_eq!(back.analyze(&forms[0].document(false)), said);
        assert!(AnalysisModel::from_bytes(&model.to_bytes()[..100]).is_none());
    }

    #[test]
    fn the_bundled_model_reads_a_form_written_by_hand() {
        let model = AnalysisModel::bundled();
        let field = |label: &str, section: &str| Input {
            label: label.into(),
            section: section.into(),
            page: 1,
            rect: None,
        };
        let doc = Document {
            fields: vec![
                field("First Name", "Applicant Information"),
                field("Last Name", "Applicant Information"),
                field("Date of Birth", "Applicant Information"),
                field("Email Address", "Applicant Information"),
                field("Dog's Name", "Pet Information"),
                field("Breed", "Pet Information"),
                field("Name", "Emergency Contact"),
                field("Phone", "Emergency Contact"),
            ],
            pages: Vec::new(),
        };
        let said = model.analyze(&doc);
        let kinds: Vec<_> = said.iter().map(|a| a.kind).collect();
        assert_eq!(
            &kinds[..4],
            ["FirstName", "LastName", "BirthDate", "Email"],
            "{said:?}"
        );
        assert!(said[..4].iter().all(Analysis::fills), "{said:?}");
        assert!(said[4..].iter().all(|a| !a.fills()), "{said:?}");
        assert_eq!(said[0].group, said[3].group, "{said:?}");
        assert_ne!(said[6].group, said[0].group, "{said:?}");
    }
}
