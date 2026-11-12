//! Profiles: a named set of details about a person or an organisation
//! (name, addresses, phone and so on, plus anything else the person adds)
//! and the matching that works out which of a form's fields each one
//! belongs in.
//!
//! Matching goes by what a field is called: its tooltip and its name.
//! The comparison is forgiving of word order, filler words, abbreviations
//! and small misspellings, and unforgiving of extra words that change the
//! meaning: "Applicant name" takes the profile's name, "Company name"
//! does not.
//!
//! A profile's values are overwritten in memory when it is dropped.

use crate::{Field, FieldKind, Form, Value, flags};

/// The details every profile has a place for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standard {
    Name,
    Poc,
    MailingAddress,
    StreetAddress,
    Email,
    City,
    State,
    Zip,
    Phone,
}

impl Standard {
    pub const ALL: [Standard; 9] = [
        Self::Name,
        Self::Poc,
        Self::MailingAddress,
        Self::StreetAddress,
        Self::City,
        Self::State,
        Self::Zip,
        Self::Email,
        Self::Phone,
    ];

    /// What to call it on screen.
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Poc => "Point of contact",
            Self::MailingAddress => "Mailing address",
            Self::StreetAddress => "Street address",
            Self::Email => "Email",
            Self::City => "City",
            Self::State => "State",
            Self::Zip => "ZIP code",
            Self::Phone => "Phone",
        }
    }

    /// Its name in a saved profile.
    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Poc => "poc",
            Self::MailingAddress => "mailing-address",
            Self::StreetAddress => "street-address",
            Self::Email => "email",
            Self::City => "city",
            Self::State => "state",
            Self::Zip => "zip",
            Self::Phone => "phone",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// Whether the value may run over several lines.
    pub fn multiline(self) -> bool {
        matches!(self, Self::MailingAddress)
    }

    /// What forms call it, and how sure each wording is. A wording two
    /// details share goes to the surer one when the profile has both.
    fn phrases(self) -> &'static [(&'static str, f32)] {
        match self {
            Self::Name => &[("name", 1.0), ("name of applicant", 1.0)],
            Self::Poc => &[
                ("poc", 1.0),
                ("point of contact", 1.0),
                ("contact", 1.0),
                ("contact name", 1.0),
                ("contact person", 1.0),
                ("name of contact", 1.0),
                ("attention", 1.0),
                ("attn", 1.0),
            ],
            Self::MailingAddress => &[
                ("mailing address", 1.0),
                ("mail address", 1.0),
                ("postal address", 1.0),
                ("correspondence address", 1.0),
                ("address", 0.9),
            ],
            Self::StreetAddress => &[
                ("street address", 1.0),
                ("street", 1.0),
                ("address", 1.0),
                ("physical address", 1.0),
                ("residential address", 1.0),
                ("mailing address", 0.7),
            ],
            Self::Email => &[
                ("email", 1.0),
                ("email address", 1.0),
                ("electronic mail", 1.0),
            ],
            Self::City => &[
                ("city", 1.0),
                ("town", 1.0),
                ("city town", 1.0),
                ("city or town", 1.0),
                ("municipality", 1.0),
            ],
            Self::State => &[
                ("state", 1.0),
                ("province", 1.0),
                ("state province", 1.0),
                ("state or province", 1.0),
            ],
            Self::Zip => &[
                ("zip", 1.0),
                ("zip code", 1.0),
                ("zipcode", 1.0),
                ("postal code", 1.0),
                ("postcode", 1.0),
                ("post code", 1.0),
                ("zip postal code", 1.0),
                ("zip or postal code", 1.0),
            ],
            Self::Phone => &[
                ("phone", 1.0),
                ("phone number", 1.0),
                ("phone no", 1.0),
                ("telephone", 1.0),
                ("telephone number", 1.0),
                ("telephone no", 1.0),
                ("tel", 1.0),
                ("tel no", 1.0),
                ("mobile", 1.0),
                ("mobile phone", 1.0),
                ("mobile number", 1.0),
                ("cell", 1.0),
                ("cell phone", 1.0),
                ("cell number", 1.0),
                ("contact phone", 1.0),
                ("contact number", 1.0),
            ],
        }
    }
}

/// What a profile entry is: one of the standard details, or something
/// the person added, under the label they gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    Standard(Standard),
    /// The label names the fields it fills. Commas separate other names
    /// the same thing goes by: "SSN, Social security number".
    Custom(String),
}

/// One detail of a profile.
#[derive(Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    pub value: String,
}

impl Entry {
    pub fn label(&self) -> &str {
        match &self.kind {
            EntryKind::Standard(s) => s.label(),
            EntryKind::Custom(label) => label,
        }
    }
}

/// A named set of details to fill forms with.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Profile {
    pub name: String,
    pub entries: Vec<Entry>,
}

/// Values stay out of debug output, which ends up in logs.
impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("name", &self.name)
            .field("entries", &self.entries.len())
            .finish()
    }
}

/// Overwrites a string that held something private, leaving it empty.
pub fn wipe_string(text: &mut String) {
    let mut bytes = std::mem::take(text).into_bytes();
    // The spare room past the end may hold what was typed and deleted.
    bytes.resize(bytes.capacity(), 0);
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

impl Drop for Profile {
    fn drop(&mut self) {
        for e in &mut self.entries {
            wipe_string(&mut e.value);
        }
    }
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '=' => out.push_str("\\e"),
            c => out.push(c),
        }
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('e') => out.push('='),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

impl Profile {
    /// The value of a standard detail; empty when the profile has none.
    pub fn standard(&self, which: Standard) -> &str {
        self.entries
            .iter()
            .find(|e| e.kind == EntryKind::Standard(which))
            .map_or("", |e| e.value.as_str())
    }

    /// The profile as the text it is saved as: `key = value` lines. The
    /// caller owns the text and should wipe it once it is written out.
    pub fn to_text(&self) -> String {
        let mut text = String::with_capacity(
            64 + self.name.len()
                + self
                    .entries
                    .iter()
                    .map(|e| 2 * (e.label().len() + e.value.len()) + 24)
                    .sum::<usize>(),
        );
        text.push_str("profile = ");
        escape(&self.name, &mut text);
        text.push('\n');
        for e in &self.entries {
            match &e.kind {
                EntryKind::Standard(s) => text.push_str(s.key()),
                EntryKind::Custom(label) => {
                    text.push_str("custom ");
                    escape(label, &mut text);
                }
            }
            text.push_str(" = ");
            escape(&e.value, &mut text);
            text.push('\n');
        }
        text
    }

    /// Reads a saved profile. Lines it does not understand are skipped.
    pub fn parse(text: &str) -> Self {
        let mut profile = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = || unescape(value.strip_prefix(' ').unwrap_or(value));
            if key == "profile" {
                profile.name = value();
            } else if let Some(label) = key.strip_prefix("custom ") {
                profile.entries.push(Entry {
                    kind: EntryKind::Custom(unescape(label.trim())),
                    value: value(),
                });
            } else if let Some(s) = Standard::from_key(key) {
                profile.entries.push(Entry {
                    kind: EntryKind::Standard(s),
                    value: value(),
                });
            }
        }
        profile
    }
}

/// Words that say nothing about which detail a field wants.
const FILLER: &[&str] = &[
    "a",
    "an",
    "the",
    "of",
    "or",
    "and",
    "your",
    "my",
    "our",
    "please",
    "enter",
    "type",
    "print",
    "printed",
    "full",
    "complete",
    "legal",
    "current",
    "primary",
    "main",
    "home",
    "daytime",
    "applicant",
    "applicants",
    "line",
    "1",
    "field",
    "text",
    "required",
    "optional",
    "here",
];

/// Words in the name of a group of fields that say the group is about
/// somebody other than the person filling in the form.
const SOMEONE_ELSE: &[&str] = &[
    "spouse",
    "partner",
    "employer",
    "company",
    "business",
    "organization",
    "organisation",
    "bank",
    "emergency",
    "dependent",
    "child",
    "children",
    "parent",
    "mother",
    "father",
    "guardian",
    "coapplicant",
    "cosigner",
    "joint",
    "witness",
    "notary",
    "preparer",
    "physician",
    "doctor",
    "landlord",
    "reference",
    "agent",
    "attorney",
    "representative",
    "beneficiary",
    "supervisor",
    "school",
    "insurer",
    "previous",
    "former",
    "other",
];

/// The words of a label, lower case: punctuation gone, `camelCase` and
/// `snake_case` split apart, "e-mail" joined up.
fn words(label: &str) -> Vec<String> {
    let mut spaced = String::with_capacity(label.len() + 8);
    let mut before: Option<char> = None;
    for c in label.chars() {
        if let Some(b) = before {
            let split = (b.is_lowercase() && c.is_uppercase())
                || (b.is_alphabetic() && c.is_ascii_digit())
                || (b.is_ascii_digit() && c.is_alphabetic());
            if split {
                spaced.push(' ');
            }
        }
        if c.is_alphanumeric() {
            spaced.extend(c.to_lowercase());
        } else if c != '\'' && c != '’' {
            spaced.push(' ');
        }
        before = Some(c);
    }
    let mut out: Vec<String> = Vec::new();
    for w in spaced.split_whitespace() {
        if w == "mail" && out.last().is_some_and(|l| l == "e") {
            out.pop();
            out.push("email".into());
        } else {
            out.push(w.to_string());
        }
    }
    out
}

/// A label's words without the filler.
fn meaning(label: &str) -> Vec<String> {
    let mut w = words(label);
    w.retain(|w| !FILLER.contains(&w.as_str()));
    w
}

/// How many single-character edits turn one word into another, up to
/// `limit`; anything further apart is `limit + 1`.
fn distance(a: &str, b: &str, limit: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > limit {
        return limit + 1;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let above = row[j];
            let swap = diagonal + usize::from(a[i - 1] != b[j - 1]);
            row[j] = swap.min(above + 1).min(row[j - 1] + 1);
            diagonal = above;
        }
    }
    row[b.len()].min(limit + 1)
}

/// How well a word on a form stands for a word we know: 1 for the word
/// itself, less for a misspelling or an abbreviation, 0 for another word.
fn word_match(on_form: &str, known: &str) -> f32 {
    if on_form == known {
        return 1.0;
    }
    let n = known.chars().count();
    let allowed = match n {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    };
    if allowed > 0 && distance(on_form, known, allowed) <= allowed {
        return 0.85;
    }
    // "Tel" for telephone, "addr" for address.
    if on_form.chars().count() >= 3 && n > on_form.len() && known.starts_with(on_form) {
        return 0.8;
    }
    0.0
}

/// How well a label says `phrase`: every word of the phrase must be
/// there, and words left over count against it.
fn phrase_score(label: &[String], phrase: &str) -> f32 {
    let phrase = meaning(phrase);
    if phrase.is_empty() || label.len() < phrase.len() {
        return 0.0;
    }
    let mut used = vec![false; label.len()];
    let mut quality = 0.0;
    for known in &phrase {
        let best = label
            .iter()
            .enumerate()
            .filter(|(i, _)| !used[*i])
            .map(|(i, w)| (i, word_match(w, known)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, q)) if q > 0.0 => {
                used[i] = true;
                quality += q;
            }
            _ => return 0.0,
        }
    }
    (quality / phrase.len() as f32) * (phrase.len() as f32 / label.len() as f32)
}

/// A match has to be at least this good to be offered.
const GOOD_ENOUGH: f32 = 0.6;

/// What a field goes by, for matching.
struct Labels {
    /// The tooltip's words.
    tooltip: Vec<String>,
    /// The words of the last part of its name.
    own: Vec<String>,
    /// The words of its whole name.
    whole: Vec<String>,
    /// Whether the name is to be believed: not when a short tooltip says
    /// what the field is, nor when either puts the field in a group about
    /// somebody else.
    by_name: bool,
    /// The last part of the name can stand alone.
    by_own: bool,
}

/// A tooltip longer than this is a sentence of instructions rather than
/// the field's name.
const TOOLTIP_WORDS: usize = 4;

fn about_someone_else(words: &[String]) -> bool {
    words.iter().any(|w| SOMEONE_ELSE.contains(&w.as_str()))
}

fn strip_index(part: &str) -> &str {
    // XFA-style names: "Name[0]".
    part.split('[').next().unwrap_or(part)
}

impl Labels {
    fn of(field: &Field) -> Self {
        let parts: Vec<&str> = field
            .name
            .split('.')
            .map(strip_index)
            .filter(|p| !p.trim().is_empty())
            .collect();
        let (last, parents) = parts.split_last().map_or(("", &[][..]), |(l, p)| (*l, p));
        let tooltip = field.tooltip.as_deref().map(meaning).unwrap_or_default();
        let by_name =
            tooltip.is_empty() || (tooltip.len() > TOOLTIP_WORDS && !about_someone_else(&tooltip));
        let parents: Vec<String> = parents.iter().flat_map(|p| words(p)).collect();
        Self {
            tooltip,
            own: meaning(last),
            whole: meaning(&parts.join(" ")),
            by_name,
            by_own: !about_someone_else(&parents),
        }
    }

    /// How well the field's labels say one of these phrases.
    fn score(&self, phrases: &[(&str, f32)]) -> f32 {
        let mut best = 0.0f32;
        for (phrase, sure) in phrases {
            let mut s = phrase_score(&self.tooltip, phrase);
            if self.by_name && self.by_own {
                s = s
                    .max(phrase_score(&self.whole, phrase))
                    .max(phrase_score(&self.own, phrase));
            } else if self.by_name && about_someone_else(&meaning(phrase)) {
                // A group about somebody else takes only what is itself
                // about them: "Spouse name" for Spouse.Name.
                s = s.max(phrase_score(&self.whole, phrase));
            }
            best = best.max(s * sure);
        }
        best
    }

    /// The field wants city, state and ZIP together on one line.
    fn city_state_zip(&self) -> bool {
        let fits = |label: &[String]| {
            let has = |known: &[&str]| label.iter().any(|w| known.contains(&w.as_str()));
            has(&["city", "town"])
                && has(&["state", "province"])
                && has(&["zip", "zipcode", "postcode", "postal"])
                && label.iter().all(|w| {
                    [
                        "city", "town", "state", "province", "zip", "zipcode", "postcode",
                        "postal", "post", "code",
                    ]
                    .contains(&w.as_str())
                })
        };
        fits(&self.tooltip) || (self.by_name && self.by_own && fits(&self.own))
    }
}

/// What a field is called on screen: its tooltip, or failing that its
/// name.
pub fn field_label(field: &Field) -> &str {
    field
        .tooltip
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(&field.name)
}

/// Why a field that matched is not being filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    /// The value is longer than the field allows.
    TooLong,
    /// None of the field's choices is the value.
    NoSuchChoice,
}

/// One field a profile would fill.
#[derive(Clone, PartialEq)]
pub struct Planned {
    /// The field's index in the form.
    pub field: usize,
    /// What the field is called.
    pub label: String,
    /// The profile detail it takes, by its label.
    pub from: String,
    /// The value it would get.
    pub value: Value,
    /// How sure the match is, from the threshold up to 1.
    pub score: f32,
    /// Set when the field cannot take the value.
    pub skipped: Option<Skipped>,
}

impl Drop for Planned {
    fn drop(&mut self) {
        match &mut self.value {
            Value::Text(t) | Value::State(t) => wipe_string(t),
            Value::Choices(c) => c.iter_mut().for_each(wipe_string),
            Value::None => {}
        }
    }
}

/// The value as a field of this kind takes it.
fn fit(field: &Field, value: &str) -> Result<Value, Skipped> {
    let value = value.trim();
    match field.kind {
        FieldKind::ComboBox | FieldKind::ListBox => {
            let same = |a: &str| a.trim().eq_ignore_ascii_case(value);
            let choice = field
                .options
                .iter()
                .find(|o| same(&o.export) || same(&o.display))
                .map(|o| o.export.clone());
            match (choice, field.kind) {
                (Some(c), FieldKind::ListBox) => Ok(Value::Choices(vec![c])),
                (Some(c), _) => Ok(Value::Text(c)),
                (None, FieldKind::ComboBox) if field.has_flag(flags::EDIT) => {
                    Ok(Value::Text(value.to_string()))
                }
                (None, _) => Err(Skipped::NoSuchChoice),
            }
        }
        _ => {
            let text = if field.has_flag(flags::MULTILINE) {
                value.replace("\r\n", "\n")
            } else {
                value
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            if field.max_len.is_some_and(|m| text.chars().count() > m) {
                return Err(Skipped::TooLong);
            }
            Ok(Value::Text(text))
        }
    }
}

/// Works out which fields a profile fills and with what.
///
/// Only fields a person could type into are considered: text fields and
/// lists of choices that are not read-only or hidden, and never password
/// fields. `wanted` picks the fields in scope, such as those of one page.
/// Fields that already hold something are left alone unless `replace`.
pub fn plan(
    form: &Form,
    profile: &Profile,
    replace: bool,
    mut wanted: impl FnMut(usize) -> bool,
) -> Vec<Planned> {
    let mut out = Vec::new();
    for (index, field) in form.fields.iter().enumerate() {
        let fillable = matches!(
            field.kind,
            FieldKind::Text | FieldKind::ComboBox | FieldKind::ListBox
        ) && !field.is_read_only()
            && !field.has_flag(flags::PASSWORD)
            && !field.has_flag(flags::FILE_SELECT)
            && field.widgets.iter().any(|w| !w.is_hidden());
        if !fillable || !wanted(index) {
            continue;
        }
        let empty = field.value.same_as(&Value::None);
        if !empty && !replace {
            continue;
        }
        let labels = Labels::of(field);
        // The best entry; one the person added wins a tie.
        let mut best: Option<(f32, bool, &Entry)> = None;
        for entry in profile
            .entries
            .iter()
            .filter(|e| !e.value.trim().is_empty())
        {
            let (score, custom) = match &entry.kind {
                EntryKind::Standard(s) => (labels.score(s.phrases()), false),
                EntryKind::Custom(label) => {
                    let phrases: Vec<(&str, f32)> = label
                        .split([',', ';', '|'])
                        .map(|p| (p.trim(), 1.0))
                        .filter(|(p, _)| !p.is_empty())
                        .collect();
                    (labels.score(&phrases), true)
                }
            };
            let better = best.is_none_or(|(s, c, _)| score > s || (score == s && custom && !c));
            if score >= GOOD_ENOUGH && better {
                best = Some((score, custom, entry));
            }
        }
        let (score, from, text) = if let Some((score, _, entry)) = best {
            (score, entry.label().to_string(), entry.value.clone())
        } else if labels.city_state_zip() {
            let (city, state, zip) = (
                profile.standard(Standard::City).trim(),
                profile.standard(Standard::State).trim(),
                profile.standard(Standard::Zip).trim(),
            );
            if city.is_empty() || state.is_empty() || zip.is_empty() {
                continue;
            }
            (
                1.0,
                "City, State and ZIP code".to_string(),
                format!("{city}, {state} {zip}"),
            )
        } else {
            continue;
        };
        let mut text = text;
        let fitted = fit(field, &text);
        wipe_string(&mut text);
        let (value, skipped) = match fitted {
            Ok(v) => (v, None),
            Err(why) => (Value::None, Some(why)),
        };
        if skipped.is_none() && field.value.same_as(&value) {
            continue;
        }
        out.push(Planned {
            field: index,
            label: field_label(field).to_string(),
            from,
            value,
            score,
            skipped,
        });
    }
    out
}

/// How well some text answers a search, or `None` when it does not.
/// Every word typed has to be found: at the start of a word, inside one,
/// or misspelt by a letter or two.
pub fn search(query: &str, text: &str) -> Option<f32> {
    let query = words(query);
    if query.is_empty() {
        return Some(1.0);
    }
    let text = words(text);
    let mut total = 0.0;
    for q in &query {
        let best = text
            .iter()
            .map(|w| {
                if w == q {
                    1.0
                } else if w.starts_with(q.as_str()) {
                    0.9
                } else if w.contains(q.as_str()) {
                    0.7
                } else {
                    // A typo in what was typed, against the whole word or
                    // as much of it as has been typed so far.
                    let n = q.chars().count();
                    let allowed = match n {
                        0..=3 => 0,
                        4..=7 => 1,
                        _ => 2,
                    };
                    let start: String = w.chars().take(n).collect();
                    if allowed > 0
                        && (distance(q, w, allowed) <= allowed
                            || distance(q, &start, allowed) <= allowed)
                    {
                        0.5
                    } else {
                        0.0
                    }
                }
            })
            .fold(0.0f32, f32::max);
        if best == 0.0 {
            return None;
        }
        total += best;
    }
    Some(total / query.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChoiceOption, Widget};
    use atelier_syntax::{Dict, ObjRef, Rect};

    fn widget(page: usize) -> Widget {
        Widget {
            obj: ObjRef {
                num: 1,
                generation: 0,
            },
            page: Some(page),
            annot_index: None,
            rect: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 100.0,
                y1: 20.0,
            },
            annot_flags: 0,
            on_state: None,
            rotation: 0,
        }
    }

    fn field(name: &str, tooltip: Option<&str>) -> Field {
        Field {
            obj: ObjRef {
                num: 1,
                generation: 0,
            },
            name: name.into(),
            tooltip: tooltip.map(Into::into),
            kind: FieldKind::Text,
            flags: 0,
            value: Value::None,
            default_value: Value::None,
            options: Vec::new(),
            max_len: None,
            da: None,
            quadding: 0,
            top_index: 0,
            widgets: vec![widget(0)],
        }
    }

    fn form(fields: Vec<Field>) -> Form {
        Form {
            fields,
            need_appearances: false,
            has_xfa: false,
            sig_flags: 0,
            resources: Dict::default(),
            da: None,
            calculation_order: Vec::new(),
        }
    }

    fn profile() -> Profile {
        let standard = |s, v: &str| Entry {
            kind: EntryKind::Standard(s),
            value: v.into(),
        };
        Profile {
            name: "Home".into(),
            entries: vec![
                standard(Standard::Name, "Ada Lovelace"),
                standard(Standard::Poc, "Charles Babbage"),
                standard(Standard::MailingAddress, "PO Box 12\nLondon"),
                standard(Standard::StreetAddress, "12 St James's Square"),
                standard(Standard::City, "London"),
                standard(Standard::State, "TX"),
                standard(Standard::Zip, "75001"),
                standard(Standard::Email, "ada@example.org"),
                standard(Standard::Phone, "555 0100"),
                Entry {
                    kind: EntryKind::Custom("Member number, Membership no".into()),
                    value: "M-4471".into(),
                },
            ],
        }
    }

    /// What a field with this name and tooltip is filled with.
    fn filled(name: &str, tooltip: Option<&str>) -> Option<String> {
        let form = form(vec![field(name, tooltip)]);
        plan(&form, &profile(), false, |_| true)
            .first()
            .map(|p| p.value.display())
    }

    #[test]
    fn labels_split_into_words() {
        assert_eq!(words("mailingAddress_2"), ["mailing", "address", "2"]);
        assert_eq!(words("E-Mail Address:"), ["email", "address"]);
        assert_eq!(words("Applicant's  NAME"), ["applicants", "name"]);
        assert_eq!(meaning("Please enter your full name"), ["name"]);
    }

    #[test]
    fn fields_take_the_detail_their_label_names() {
        for (name, tooltip, want) in [
            ("Name", None, "Ada Lovelace"),
            ("f1", Some("Name of Applicant"), "Ada Lovelace"),
            ("applicantName", None, "Ada Lovelace"),
            ("PRINT NAME", None, "Ada Lovelace"),
            ("POC", None, "Charles Babbage"),
            ("f2", Some("Point of Contact"), "Charles Babbage"),
            ("Contact Name", None, "Charles Babbage"),
            ("Attn", None, "Charles Babbage"),
            ("Mailing Address", None, "PO Box 12, London"),
            ("Address", None, "12 St James's Square"),
            ("Street_Address", None, "12 St James's Square"),
            ("home address line 1", None, "12 St James's Square"),
            ("City", None, "London"),
            ("City/Town", None, "London"),
            ("State", None, "TX"),
            ("State or Province", None, "TX"),
            ("ZIP", None, "75001"),
            ("Zip Code", None, "75001"),
            ("PostalCode", None, "75001"),
            ("E-mail", None, "ada@example.org"),
            ("Email Address", None, "ada@example.org"),
            ("Phone", None, "555 0100"),
            ("Telephone No.", None, "555 0100"),
            ("Daytime phone number", None, "555 0100"),
            ("Tel", None, "555 0100"),
            ("form1[0].Page1[0].Phone[0]", None, "555 0100"),
            ("Applicant.Name", None, "Ada Lovelace"),
            // A tooltip that is a sentence leaves the name to say it.
            (
                "Address",
                Some("Enter the address exactly as it appears on your licence"),
                "12 St James's Square",
            ),
            // Misspellings and abbreviations.
            ("Adress", None, "12 St James's Square"),
            ("Telphone", None, "555 0100"),
            ("Mailing Addr", None, "PO Box 12, London"),
            // All three on one line.
            ("City, State, ZIP", None, "London, TX 75001"),
            ("CityStateZip", None, "London, TX 75001"),
            // The detail the person added, under either of its names.
            ("Member Number", None, "M-4471"),
            ("membership no", None, "M-4471"),
            ("Details.MemberNumber", None, "M-4471"),
        ] {
            assert_eq!(
                filled(name, tooltip).as_deref(),
                Some(want),
                "{name} / {tooltip:?}"
            );
        }
    }

    #[test]
    fn fields_about_something_else_are_left_alone() {
        for (name, tooltip) in [
            ("Company Name", None),
            ("Name of Bank", None),
            ("Spouse name", None),
            ("First name", None),
            ("Name (Last, First, MI)", None),
            ("Spouse.Name", None),
            ("Employer.Address", None),
            ("Emergency Contact.Phone", None),
            ("Address line 2", None),
            ("Street number", None),
            ("Date", None),
            ("Signature", None),
            ("Statement", None),
            ("Text1", None),
            ("Account number", None),
            // The tooltip says what the field is, whatever its name.
            ("Name", Some("Name of your employer")),
            ("Phone", Some("Fax")),
            (
                "Address",
                Some("Enter the address of the employer exactly as it appears"),
            ),
        ] {
            assert_eq!(filled(name, tooltip), None, "{name} / {tooltip:?}");
        }
    }

    #[test]
    fn a_detail_about_somebody_else_fills_their_group() {
        let mut p = profile();
        p.entries.push(Entry {
            kind: EntryKind::Custom("Spouse name".into()),
            value: "William King".into(),
        });
        let form = form(vec![field("Spouse.Name", None), field("Name", None)]);
        let planned = plan(&form, &p, false, |_| true);
        assert_eq!(planned[0].value, Value::Text("William King".into()));
        assert_eq!(planned[1].value, Value::Text("Ada Lovelace".into()));
    }

    #[test]
    fn only_fields_a_person_could_type_into_are_filled() {
        let mut read_only = field("Name", None);
        read_only.flags = flags::READ_ONLY;
        let mut password = field("Name", None);
        password.flags = flags::PASSWORD;
        let mut hidden = field("Name", None);
        hidden.widgets[0].annot_flags = 2;
        let mut check = field("Name", None);
        check.kind = FieldKind::Checkbox;
        let mut taken = field("Name", None);
        taken.value = Value::Text("Grace Hopper".into());
        let mut same = field("City", None);
        same.value = Value::Text("London".into());
        let form = form(vec![read_only, password, hidden, check, taken, same]);
        assert!(plan(&form, &profile(), false, |_| true).is_empty());
        // Replacing overwrites what is there, but not with the same thing.
        let replaced = plan(&form, &profile(), true, |_| true);
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].field, 4);
        assert_eq!(replaced[0].value, Value::Text("Ada Lovelace".into()));
    }

    #[test]
    fn scope_picks_the_fields() {
        let form = form(vec![field("Name", None), field("City", None)]);
        let planned = plan(&form, &profile(), false, |i| i == 1);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].from, "City");
    }

    #[test]
    fn values_fit_the_kind_of_field() {
        let options = |names: &[(&str, &str)]| {
            names
                .iter()
                .map(|(export, display)| ChoiceOption {
                    export: (*export).into(),
                    display: (*display).into(),
                })
                .collect::<Vec<_>>()
        };
        let mut combo = field("State", None);
        combo.kind = FieldKind::ComboBox;
        combo.options = options(&[("ca", "CA"), ("tx", "TX")]);
        let mut list = combo.clone();
        list.kind = FieldKind::ListBox;
        let mut other = combo.clone();
        other.options = options(&[("ca", "California")]);
        let mut typed = other.clone();
        typed.flags = flags::EDIT;
        let mut short = field("ZIP", None);
        short.max_len = Some(3);
        let mut lines = field("Mailing address", None);
        lines.flags = flags::MULTILINE;
        let form = form(vec![combo, list, other, typed, short, lines]);
        let planned = plan(&form, &profile(), false, |_| true);
        assert_eq!(planned[0].value, Value::Text("tx".into()));
        assert_eq!(planned[1].value, Value::Choices(vec!["tx".into()]));
        assert_eq!(planned[2].skipped, Some(Skipped::NoSuchChoice));
        assert_eq!(planned[3].value, Value::Text("TX".into()));
        assert_eq!(planned[4].skipped, Some(Skipped::TooLong));
        assert_eq!(planned[5].value, Value::Text("PO Box 12\nLondon".into()));
    }

    #[test]
    fn an_address_goes_to_whichever_address_the_profile_has() {
        let mut p = profile();
        p.entries
            .retain(|e| e.kind != EntryKind::Standard(Standard::StreetAddress));
        let form = form(vec![field("Address", None)]);
        let planned = plan(&form, &p, false, |_| true);
        assert_eq!(planned[0].from, "Mailing address");
        // An empty detail fills nothing.
        p.entries[2].value = "  ".into();
        assert!(plan(&form, &p, false, |_| true).is_empty());
    }

    #[test]
    fn profiles_survive_being_saved() {
        let mut p = profile();
        p.name = "Work = \\ office".into();
        p.entries.push(Entry {
            kind: EntryKind::Custom("a = b \\ c".into()),
            value: " leading space, = and \\n\nsecond line ".into(),
        });
        let text = p.to_text();
        assert_eq!(Profile::parse(&text), p);
        assert_eq!(text.lines().count(), p.entries.len() + 1);
        assert_eq!(
            Profile::parse("nonsense\nprofile = X\nfax = 1"),
            Profile {
                name: "X".into(),
                entries: Vec::new(),
            }
        );
    }

    #[test]
    fn debug_output_keeps_values_out() {
        let shown = format!("{:?}", profile());
        assert!(shown.contains("Home"));
        assert!(!shown.contains("Ada"));
    }

    #[test]
    fn wiping_empties_a_string() {
        let mut s = String::from("secret");
        wipe_string(&mut s);
        assert!(s.is_empty());
    }

    #[test]
    fn search_is_forgiving() {
        assert!(search("", "anything").is_some());
        assert!(search("hom", "Home address").is_some());
        assert!(search("addr home", "Home address").is_some());
        assert!(search("adress", "Home address").is_some());
        assert!(search("mail", "ada@example.org Email").is_some());
        assert!(search("work", "Home address").is_none());
        assert!(search("home fax", "Home address").is_none());
        assert!(search("home", "Home").unwrap() > search("hom", "Home").unwrap());
    }
}
