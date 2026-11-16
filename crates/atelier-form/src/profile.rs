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
///
/// A name and an address are kept in their parts, so each field of a form
/// can be given the part it asks for and a field that asks for several
/// gets them put together (see [`Profile::format`]). `Name` is the whole
/// name as one piece, for an organisation or a name that does not split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standard {
    Name,
    /// Mr, Ms, Dr: the title that goes before the name.
    Honorific,
    FirstName,
    MiddleName,
    LastName,
    Poc,
    /// A mailing address written out whole, when it differs from the
    /// address in parts.
    MailingAddress,
    /// The first line of the address.
    StreetAddress,
    AddressLine2,
    Email,
    City,
    State,
    Province,
    County,
    Zip,
    Country,
    Phone,
    /// Kept as typed; written the way a field asks ([`Want::Date`]).
    BirthDate,
    /// The business or organisation the person fills forms in for.
    Company,
    /// The name the business trades under.
    Dba,
    Division,
    JobTitle,
    WorkEmail,
    WorkPhone,
    Website,
}

impl Standard {
    /// Every detail, in the order a profile is written out and edited:
    /// the person, how to reach them, the address, the business.
    pub const ALL: [Standard; 25] = [
        Self::Name,
        Self::Honorific,
        Self::FirstName,
        Self::MiddleName,
        Self::LastName,
        Self::BirthDate,
        Self::Email,
        Self::Phone,
        Self::Poc,
        Self::StreetAddress,
        Self::AddressLine2,
        Self::City,
        Self::State,
        Self::Province,
        Self::County,
        Self::Zip,
        Self::Country,
        Self::MailingAddress,
        Self::Company,
        Self::Dba,
        Self::Division,
        Self::JobTitle,
        Self::WorkEmail,
        Self::WorkPhone,
        Self::Website,
    ];

    /// The heading it comes under where a profile is edited.
    pub fn group(self) -> &'static str {
        match self {
            Self::Name
            | Self::Honorific
            | Self::FirstName
            | Self::MiddleName
            | Self::LastName
            | Self::BirthDate => "Person",
            Self::Email | Self::Phone | Self::Poc => "Contact",
            Self::StreetAddress
            | Self::AddressLine2
            | Self::City
            | Self::State
            | Self::Province
            | Self::County
            | Self::Zip
            | Self::Country
            | Self::MailingAddress => "Address",
            Self::Company
            | Self::Dba
            | Self::Division
            | Self::JobTitle
            | Self::WorkEmail
            | Self::WorkPhone
            | Self::Website => "Business",
        }
    }

    /// What to call it on screen.
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Full name",
            Self::Honorific => "Title (Mr, Ms, Dr)",
            Self::FirstName => "First name",
            Self::MiddleName => "Middle name or initial",
            Self::LastName => "Last name",
            Self::Poc => "Point of contact",
            Self::MailingAddress => "Mailing address, if different",
            Self::StreetAddress => "Address line 1",
            Self::AddressLine2 => "Address line 2",
            Self::Email => "Email",
            Self::City => "City",
            Self::State => "State",
            Self::Province => "Province",
            Self::County => "County",
            Self::Zip => "ZIP code",
            Self::Country => "Country",
            Self::Phone => "Phone",
            Self::BirthDate => "Date of birth",
            Self::Company => "Business or company name",
            Self::Dba => "Doing business as (DBA)",
            Self::Division => "Division or department",
            Self::JobTitle => "Job title",
            Self::WorkEmail => "Work email",
            Self::WorkPhone => "Work phone",
            Self::Website => "Website",
        }
    }

    /// Its name in a saved profile.
    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Honorific => "honorific",
            Self::FirstName => "first-name",
            Self::MiddleName => "middle-name",
            Self::LastName => "last-name",
            Self::Poc => "poc",
            Self::MailingAddress => "mailing-address",
            Self::StreetAddress => "street-address",
            Self::AddressLine2 => "address-line-2",
            Self::Email => "email",
            Self::City => "city",
            Self::State => "state",
            Self::Province => "province",
            Self::County => "county",
            Self::Zip => "zip",
            Self::Country => "country",
            Self::Phone => "phone",
            Self::BirthDate => "birth-date",
            Self::Company => "company",
            Self::Dba => "dba",
            Self::Division => "division",
            Self::JobTitle => "job-title",
            Self::WorkEmail => "work-email",
            Self::WorkPhone => "work-phone",
            Self::Website => "website",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// Whether the value may run over several lines.
    pub fn multiline(self) -> bool {
        matches!(self, Self::MailingAddress)
    }

    /// What forms call it, and how sure each wording is. Addresses are
    /// not matched by wording alone; see [`address_line`].
    fn phrases(self) -> &'static [(&'static str, f32)] {
        match self {
            Self::Name => &[("name", 1.0), ("name of applicant", 1.0)],
            // A bare "Title" is told from a job title by the fields
            // around it; see [`titles`].
            Self::Honorific => &[
                ("prefix", 1.0),
                ("name prefix", 1.0),
                ("salutation", 1.0),
                ("honorific", 1.0),
                ("courtesy title", 1.0),
                ("mr mrs ms", 1.0),
                ("mr ms", 1.0),
                ("mr mrs ms dr", 1.0),
            ],
            Self::FirstName => &[
                ("first name", 1.0),
                ("given name", 1.0),
                ("forename", 1.0),
                ("first", 1.0),
                ("fname", 1.0),
            ],
            Self::MiddleName => &[("middle name", 1.0), ("middle", 1.0)],
            Self::LastName => &[
                ("last name", 1.0),
                ("surname", 1.0),
                ("family name", 1.0),
                ("last", 1.0),
                ("lname", 1.0),
            ],
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
            Self::MailingAddress | Self::StreetAddress => &[],
            Self::AddressLine2 => &[
                ("apt", 1.0),
                ("apartment", 1.0),
                ("suite", 1.0),
                ("unit", 1.0),
                ("apt suite", 1.0),
                ("apt unit", 1.0),
                ("suite unit", 1.0),
                ("apt suite unit", 1.0),
                ("apartment suite unit", 1.0),
                ("apt number", 1.0),
                ("apt no", 1.0),
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
                ("state province", 1.0),
                ("state or province", 1.0),
                ("state region", 1.0),
            ],
            Self::Province => &[("province", 1.0), ("province state", 1.0), ("region", 0.9)],
            Self::County => &[("county", 1.0), ("parish", 0.9)],
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
            Self::Country => &[("country", 1.0), ("nation", 1.0), ("country region", 1.0)],
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
            Self::BirthDate => &[
                ("date of birth", 1.0),
                ("birth date", 1.0),
                ("birthdate", 1.0),
                ("birthday", 1.0),
                ("dob", 1.0),
                ("d o b", 1.0),
            ],
            Self::Company => &[
                ("company", 1.0),
                ("company name", 1.0),
                ("name of company", 1.0),
                ("business", 1.0),
                ("business name", 1.0),
                ("name of business", 1.0),
                ("organization", 1.0),
                ("organization name", 1.0),
                ("organisation", 1.0),
                ("organisation name", 1.0),
                ("employer", 1.0),
                ("employer name", 1.0),
                ("name of employer", 1.0),
                ("firm", 1.0),
                ("firm name", 1.0),
            ],
            Self::Dba => &[
                ("dba", 1.0),
                ("d b a", 1.0),
                ("dba name", 1.0),
                ("doing business as", 1.0),
                ("trade name", 1.0),
                ("trading as", 1.0),
                ("fictitious business name", 1.0),
            ],
            Self::Division => &[
                ("division", 1.0),
                ("department", 1.0),
                ("dept", 1.0),
                ("division department", 1.0),
                ("business unit", 1.0),
                ("branch", 0.9),
            ],
            Self::JobTitle => &[
                ("job title", 1.0),
                ("position", 1.0),
                ("position title", 1.0),
                ("title position", 1.0),
                ("occupation", 0.9),
            ],
            Self::WorkEmail => &[
                ("work email", 1.0),
                ("work email address", 1.0),
                ("business email", 1.0),
                ("business email address", 1.0),
                ("company email", 1.0),
                ("office email", 1.0),
            ],
            Self::WorkPhone => &[
                ("work phone", 1.0),
                ("work phone number", 1.0),
                ("work telephone", 1.0),
                ("work number", 1.0),
                ("business phone", 1.0),
                ("business phone number", 1.0),
                ("business telephone", 1.0),
                ("company phone", 1.0),
                ("office phone", 1.0),
                ("office number", 1.0),
                ("office telephone", 1.0),
            ],
            Self::Website => &[
                ("website", 1.0),
                ("web site", 1.0),
                ("web address", 1.0),
                ("url", 1.0),
                ("homepage", 1.0),
                ("company website", 1.0),
                ("business website", 1.0),
            ],
        }
    }
}

/// A part of a date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePart {
    Month,
    Day,
    Year,
    /// The year's last two digits.
    ShortYear,
}

/// The year, month and day a date says, however it was typed:
/// `1815-12-10`, `12/10/1815`, `10 Dec 1815`, `December 10, 1815`. Two
/// numbers that could each be the month are read month first, as forms
/// in the United States write them; a day over 12 settles it. A year of
/// two digits is taken as 1931 to 2030.
pub fn parse_date(text: &str) -> Option<(u32, u32, u32)> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let tokens: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect();
    let [a, b, c] = tokens.as_slice() else {
        return None;
    };
    let named = |t: &str| {
        MONTHS
            .iter()
            .position(|m| t.starts_with(m))
            .map(|i| i as u32 + 1)
    };
    let number = |t: &str| t.parse::<u32>().ok();
    let year = |t: &str| {
        let n = number(t)?;
        Some(match t.len() {
            1 | 2 if n <= 30 => 2000 + n,
            1 | 2 => 1900 + n,
            _ => n,
        })
    };
    let (y, m, d) = if let Some(m) = named(a) {
        (year(c)?, m, number(b)?)
    } else if let Some(m) = named(b) {
        // "10 Dec 1815", or "1815 Dec 10".
        if a.len() == 4 {
            (year(a)?, m, number(c)?)
        } else {
            (year(c)?, m, number(a)?)
        }
    } else if a.len() == 4 {
        (year(a)?, number(b)?, number(c)?)
    } else if number(a)? > 12 {
        (year(c)?, number(b)?, number(a)?)
    } else {
        (year(c)?, number(a)?, number(b)?)
    };
    ((1..=12).contains(&m) && (1..=31).contains(&d) && (1..=9999).contains(&y)).then_some((y, m, d))
}

/// A date with its parts in `order`: month/day/year style with slashes,
/// or with hyphens when the year leads.
fn write_date((y, m, d): (u32, u32, u32), order: &[DatePart]) -> String {
    let between = if order.first() == Some(&DatePart::Year) {
        "-"
    } else {
        "/"
    };
    order
        .iter()
        .map(|p| match p {
            DatePart::Month => format!("{m:02}"),
            DatePart::Day => format!("{d:02}"),
            DatePart::Year => format!("{y:04}"),
            DatePart::ShortYear => format!("{:02}", y % 100),
        })
        .collect::<Vec<_>>()
        .join(between)
}

/// A part of a person's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamePart {
    First,
    Middle,
    MiddleInitial,
    Last,
}

/// What a field of a form asks a profile for. One detail as it is kept,
/// or details put together the way the field wants them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// One detail. A name comes whole, made from its parts if need be;
    /// a state falls back to the province and the other way about.
    Detail(Standard),
    /// The first letter of the middle name.
    MiddleInitial,
    /// Several parts of the name in one field, in the field's order:
    /// "Last name, First name, MI".
    NameParts(Vec<NamePart>),
    /// City, state and ZIP code on one line.
    CityStateZip,
    /// The date of birth with its parts in the order the field shows,
    /// such as "(MM/DD/YYYY)".
    Date(Vec<DatePart>),
    /// One part of the date of birth, for a form that asks for the
    /// month, the day and the year apart.
    BirthPart(DatePart),
}

impl Want {
    /// What the plan says the value comes from.
    fn source(&self) -> &'static str {
        match self {
            Want::Detail(Standard::Name) | Want::NameParts(_) => "Name",
            Want::Detail(s) => s.label(),
            Want::MiddleInitial => "Middle initial",
            Want::CityStateZip => "City, State and ZIP code",
            Want::Date(_) | Want::BirthPart(_) => Standard::BirthDate.label(),
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

/// The pieces that are not empty, with `between` between them.
fn join(parts: &[String], between: &str) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(between)
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

    /// A detail trimmed, as text of its own.
    fn part(&self, which: Standard) -> String {
        self.standard(which).trim().to_string()
    }

    /// The details a field asks for, put together the way it asks:
    /// the formatting every fill goes through. Empty when the profile
    /// does not have what is wanted.
    pub fn format(&self, want: &Want) -> String {
        use Standard as S;
        let either = |a: S, b: S| {
            let first = self.part(a);
            if first.is_empty() {
                self.part(b)
            } else {
                first
            }
        };
        let initial = || {
            self.part(S::MiddleName)
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect::<String>())
                .unwrap_or_default()
        };
        match want {
            Want::Detail(S::Name) => {
                let whole = self.part(S::Name);
                if !whole.is_empty() {
                    return whole;
                }
                // A lone middle initial is written with its full stop.
                let mut middle = self.part(S::MiddleName);
                if middle.chars().count() == 1 {
                    middle.push('.');
                }
                join(
                    &[self.part(S::FirstName), middle, self.part(S::LastName)],
                    " ",
                )
            }
            // A form that asks only for one email or phone gets the one
            // the profile has.
            Want::Detail(S::Email) => either(S::Email, S::WorkEmail),
            Want::Detail(S::WorkEmail) => either(S::WorkEmail, S::Email),
            Want::Detail(S::Phone) => either(S::Phone, S::WorkPhone),
            Want::Detail(S::WorkPhone) => either(S::WorkPhone, S::Phone),
            // A business with no trading name trades under its own.
            Want::Detail(S::Dba) => either(S::Dba, S::Company),
            // A date that cannot be read is written as it was typed.
            Want::Date(order) => match parse_date(&self.part(S::BirthDate)) {
                Some(date) if !order.is_empty() => write_date(date, order),
                _ => self.part(S::BirthDate),
            },
            Want::BirthPart(part) => parse_date(&self.part(S::BirthDate))
                .map(|date| write_date(date, &[*part]))
                .unwrap_or_default(),
            Want::Detail(S::State) => either(S::State, S::Province),
            Want::Detail(S::Province) => either(S::Province, S::State),
            Want::Detail(s) => self.part(*s),
            Want::MiddleInitial => initial(),
            Want::NameParts(order) => {
                let text = |p: &NamePart| match p {
                    NamePart::First => self.part(S::FirstName),
                    NamePart::Middle => self.part(S::MiddleName),
                    NamePart::MiddleInitial => initial(),
                    NamePart::Last => self.part(S::LastName),
                };
                // Every part named must be there, or the field is left.
                let parts: Vec<String> = order.iter().map(text).collect();
                let needed = |p: &NamePart| matches!(p, NamePart::First | NamePart::Last);
                if order
                    .iter()
                    .zip(&parts)
                    .any(|(p, t)| needed(p) && t.is_empty())
                {
                    return String::new();
                }
                match order.first() {
                    // Surname first is set off with a comma.
                    Some(NamePart::Last) if parts.len() > 1 => {
                        format!("{}, {}", parts[0], join(&parts[1..], " "))
                    }
                    _ => join(&parts, " "),
                }
            }
            Want::CityStateZip => {
                let (city, state, zip) = (
                    self.part(S::City),
                    either(S::State, S::Province),
                    self.part(S::Zip),
                );
                if city.is_empty() || state.is_empty() || zip.is_empty() {
                    return String::new();
                }
                format!("{city}, {state} {zip}")
            }
        }
    }

    /// The address as the lines of an envelope. `place` adds the line of
    /// city, state and ZIP code and the country, for a form with no
    /// fields of their own for them; `second` leaves the second line out
    /// when the form has a field for that too.
    pub fn address_lines(&self, place: bool, second: bool) -> Vec<String> {
        use Standard as S;
        // Without its first line there is no address to write.
        if self.part(S::StreetAddress).is_empty() {
            return Vec::new();
        }
        let mut lines = vec![self.part(S::StreetAddress)];
        if second {
            lines.push(self.part(S::AddressLine2));
        }
        if place {
            let state = self.format(&Want::Detail(S::State));
            let tail = join(&[state, self.part(S::Zip)], " ");
            lines.push(join(&[self.part(S::City), tail], ", "));
            lines.push(self.part(S::Country));
        }
        lines.retain(|l| !l.is_empty());
        lines
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
    // The pattern a date is wanted in says nothing of which date.
    "mm",
    "dd",
    "yy",
    "yyyy",
    "mmddyyyy",
    "ddmmyyyy",
    "yyyymmdd",
    "format",
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
    /// The order a date is wanted in, when the tooltip or the name shows
    /// a pattern such as "MM/DD/YYYY".
    date_order: Vec<DatePart>,
}

/// The order of a date pattern among a label's words: "mm dd yyyy",
/// "yyyy-mm-dd", "ddmmyyyy".
fn date_order(label: &str) -> Vec<DatePart> {
    let mut order = Vec::new();
    for w in words(label) {
        let mut rest = w.as_str();
        let mut found = Vec::new();
        while !rest.is_empty() {
            let (part, len) = if rest.starts_with("yyyy") {
                (DatePart::Year, 4)
            } else if rest.starts_with("yy") {
                (DatePart::ShortYear, 2)
            } else if rest.starts_with("mm") {
                (DatePart::Month, 2)
            } else if rest.starts_with("dd") {
                (DatePart::Day, 2)
            } else {
                found.clear();
                break;
            };
            found.push(part);
            rest = &rest[len..];
        }
        order.extend(found);
    }
    // A pattern names each part once and has all three.
    let complete = order.len() == 3
        && order.contains(&DatePart::Month)
        && order.contains(&DatePart::Day)
        && (order.contains(&DatePart::Year) || order.contains(&DatePart::ShortYear));
    if complete { order } else { Vec::new() }
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
            date_order: field
                .tooltip
                .as_deref()
                .map(date_order)
                .filter(|o| !o.is_empty())
                .unwrap_or_else(|| date_order(last)),
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

    /// The labels to read the field by: its tooltip, and its own name
    /// when that is to be believed.
    fn readings(&self) -> impl Iterator<Item = &[String]> {
        let named = (self.by_name && self.by_own).then_some(self.own.as_slice());
        std::iter::once(self.tooltip.as_slice())
            .chain(named)
            .filter(|l| !l.is_empty())
    }

    /// Which line of an address the field is, when it is one: "Address",
    /// "Street address", "Mailing address line 2", "Address 3". The line's
    /// number (when the label gives one) and whether it says mailing.
    fn address_line(&self) -> Option<(Option<u32>, bool)> {
        self.readings().find_map(|label| {
            let mut number = None;
            let mut rest: Vec<&str> = Vec::new();
            for w in label {
                match w.as_str() {
                    // "Street number" is no line of an address.
                    "line" => {}
                    w if w.chars().all(|c| c.is_ascii_digit()) => number = w.parse().ok(),
                    w => rest.push(w),
                }
            }
            // "Addr" and "adress" are addresses too.
            let address = |w: &str| word_match(w, "address") > 0.0;
            let mailing = match rest.as_slice() {
                [w] if *w == "street" || address(w) => false,
                ["street" | "physical" | "residential" | "permanent", w] if address(w) => false,
                ["mailing" | "mail" | "postal" | "correspondence", w] if address(w) => true,
                _ => return None,
            };
            Some((number, mailing))
        })
    }

    /// The parts of a name the field takes together, in its order, when
    /// it names more than one: "Last name, First name, MI".
    fn name_parts(&self) -> Option<Vec<NamePart>> {
        self.readings().find_map(|label| {
            let mut order: Vec<NamePart> = Vec::new();
            fn add(order: &mut Vec<NamePart>, p: NamePart) {
                if !order.contains(&p) {
                    order.push(p);
                }
            }
            for w in label {
                match w.as_str() {
                    "first" | "given" | "forename" => add(&mut order, NamePart::First),
                    "last" | "surname" | "family" => add(&mut order, NamePart::Last),
                    "middle" => add(&mut order, NamePart::Middle),
                    "mi" => add(&mut order, NamePart::MiddleInitial),
                    // "Middle initial" is the initial, not the name.
                    "initial" | "init" => {
                        order.retain(|p| *p != NamePart::Middle);
                        add(&mut order, NamePart::MiddleInitial);
                    }
                    "name" | "names" => {}
                    _ => return None,
                }
            }
            (order.len() > 1).then_some(order)
        })
    }

    /// What of a profile the field asks for by its wording, and how
    /// surely. Address lines are worked out apart, from the fields
    /// around them.
    fn want(&self, profile: &Profile) -> Option<(f32, Want)> {
        if let Some(order) = self.name_parts() {
            return Some((1.0, Want::NameParts(order)));
        }
        if self.city_state_zip() {
            return Some((1.0, Want::CityStateZip));
        }
        const INITIAL: &[(&str, f32)] = &[
            ("middle initial", 1.0),
            ("middle init", 1.0),
            ("mi", 1.0),
            ("m i", 1.0),
            ("initial", 0.9),
        ];
        const BIRTH: [(DatePart, &[(&str, f32)]); 3] = [
            (
                DatePart::Month,
                &[
                    ("birth month", 1.0),
                    ("month of birth", 1.0),
                    ("dob month", 1.0),
                ],
            ),
            (
                DatePart::Day,
                &[("birth day", 1.0), ("day of birth", 1.0), ("dob day", 1.0)],
            ),
            (
                DatePart::Year,
                &[
                    ("birth year", 1.0),
                    ("year of birth", 1.0),
                    ("dob year", 1.0),
                ],
            ),
        ];
        let mut best = Some((self.score(INITIAL), Want::MiddleInitial));
        let mut offer = |score: f32, want: Want| {
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, want));
            }
        };
        for (part, phrases) in BIRTH {
            offer(self.score(phrases), Want::BirthPart(part));
        }
        for s in Standard::ALL {
            let want = match s {
                Standard::BirthDate => Want::Date(self.date_order.clone()),
                s => Want::Detail(s),
            };
            offer(self.score(s.phrases()), want);
        }
        best.filter(|(score, want)| *score >= GOOD_ENOUGH && !profile.format(want).is_empty())
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

/// The fields of a form that go together: those under the same parent
/// in their names. A flat form is one group.
fn group_of(field: &Field) -> String {
    let parts: Vec<&str> = field.name.split('.').map(strip_index).collect();
    parts[..parts.len().saturating_sub(1)]
        .join(".")
        .to_lowercase()
}

/// What each address-line field gets. The lines of the address go down
/// the fields in order, one each; when there are more lines than fields
/// the last field takes what is left.
///
/// A form with fields of its own for city, state or ZIP code beside the
/// address gets only the street lines here. One without (a lone "Address"
/// box, or a column of "Address 1, 2, 3") gets the whole address.
fn address_fills(
    fields: &[(usize, &Field, Labels)],
    profile: &Profile,
) -> std::collections::HashMap<usize, (&'static str, String)> {
    use std::collections::{BTreeMap, HashMap};
    let asks = |labels: &Labels, which: &[Standard]| {
        labels.city_state_zip()
            || which
                .iter()
                .any(|s| labels.score(s.phrases()) >= GOOD_ENOUGH)
    };
    // Each group's address fields, mailing ones apart from the others.
    let mut groups: BTreeMap<(String, bool), Vec<(u32, usize)>> = BTreeMap::new();
    for (index, field, labels) in fields {
        if let Some((number, mailing)) = labels.address_line() {
            groups
                .entry((group_of(field), mailing))
                .or_default()
                .push((number.unwrap_or(0), *index));
        }
    }
    let mut out = HashMap::new();
    for ((group, mailing), mut members) in groups {
        let beside = |which: &[Standard]| {
            fields
                .iter()
                .any(|(_, f, labels)| group_of(f) == group && asks(labels, which))
        };
        let place = !beside(&[
            Standard::City,
            Standard::Zip,
            Standard::State,
            Standard::Province,
        ]);
        let second = !beside(&[Standard::AddressLine2]);
        let written: Vec<String> = profile
            .part(Standard::MailingAddress)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        let parts = profile.address_lines(place, second);
        // A mailing address written out whole goes to the mailing
        // fields, and to any address when it is all the profile has.
        let whole = (mailing && !written.is_empty()) || parts.is_empty();
        let (from, lines) = if whole {
            (Standard::MailingAddress.label(), written)
        } else {
            ("Address", parts)
        };
        // Each field's place among the lines: the number the form gives
        // it, else the next place free in the order the fields come.
        members.sort();
        let mut places: Vec<(usize, usize)> = Vec::new();
        for (number, index) in members {
            let place = match number {
                0 => (0..)
                    .find(|p| places.iter().all(|(q, _)| q != p))
                    .unwrap_or(0),
                n => n as usize - 1,
            };
            if places.iter().all(|(q, _)| *q != place) {
                places.push((place, index));
            }
        }
        places.sort();
        // A second line with no first beside it belongs to something
        // this cannot see; it is left alone.
        if places.first().is_none_or(|(p, _)| *p != 0) {
            continue;
        }
        let last = places.len() - 1;
        for (i, (place, index)) in places.into_iter().enumerate() {
            // The last field takes whatever lines are left.
            let text = if i == last {
                lines.get(place..).map(|rest| rest.join("\n"))
            } else {
                lines.get(place).cloned()
            };
            if let Some(text) = text.filter(|t| !t.is_empty()) {
                out.insert(index, (from, text));
            }
        }
    }
    out
}

/// Which title each field labelled just "Title" asks for, by the company
/// it keeps. Beside the parts of a name it is Mr or Ms; beside an
/// employer, a department, an occupation or anything else about work it
/// is the job. Nearer fields count for more, and with nothing to go by
/// it is the job, the commoner of the two on forms.
fn titles(fields: &[(usize, &Field, Labels)]) -> std::collections::HashMap<usize, Standard> {
    const REACH: usize = 3;
    let good = |labels: &Labels, which: &[Standard]| {
        which
            .iter()
            .any(|s| labels.score(s.phrases()) >= GOOD_ENOUGH)
    };
    let of_a_name = |labels: &Labels| {
        labels.name_parts().is_some()
            || good(
                labels,
                &[
                    Standard::Name,
                    Standard::FirstName,
                    Standard::MiddleName,
                    Standard::LastName,
                ],
            )
    };
    let of_work = |labels: &Labels| {
        const WORDS: &[&str] = &[
            "job",
            "occupation",
            "employer",
            "employment",
            "employed",
            "work",
        ];
        labels
            .readings()
            .flatten()
            .any(|w| WORDS.iter().any(|known| word_match(w, known) > 0.0))
            || good(
                labels,
                &[
                    Standard::Company,
                    Standard::Dba,
                    Standard::Division,
                    Standard::JobTitle,
                    Standard::WorkEmail,
                    Standard::WorkPhone,
                ],
            )
    };
    let mut out = std::collections::HashMap::new();
    for (at, (index, field, labels)) in fields.iter().enumerate() {
        if !labels.readings().any(|l| l.len() == 1 && l[0] == "title") {
            continue;
        }
        let group = group_of(field);
        let (mut name, mut work) = (0.0f32, 0.0f32);
        let from = at.saturating_sub(REACH);
        for (near, (_, other, other_labels)) in
            fields.iter().enumerate().skip(from).take(2 * REACH + 1)
        {
            if near == at || group_of(other) != group {
                continue;
            }
            let weight = 1.0 / near.abs_diff(at) as f32;
            if of_a_name(other_labels) {
                name += weight;
            } else if of_work(other_labels) {
                work += weight;
            }
        }
        let which = if name > work {
            Standard::Honorific
        } else {
            Standard::JobTitle
        };
        out.insert(*index, which);
    }
    out
}

/// Works out which fields a profile fills and with what.
///
/// Only fields a person could type into are considered: text fields and
/// lists of choices that are not read-only or hidden, and never password
/// fields. `wanted` picks the fields in scope, such as those of one page.
/// Fields that already hold something are left alone unless `replace`.
///
/// What a field gets goes by its wording and by the fields around it. A
/// lone "Name" takes the whole name, "First name" and "Last name" its
/// parts, "Last, First, MI" the parts in that order. An "Address" beside
/// city, state and ZIP fields takes the street; alone, the whole address;
/// and a run of address lines takes the address line by line.
pub fn plan(
    form: &Form,
    profile: &Profile,
    replace: bool,
    mut wanted: impl FnMut(usize) -> bool,
) -> Vec<Planned> {
    let mut fields = Vec::new();
    for (index, field) in form.fields.iter().enumerate() {
        let fillable = matches!(
            field.kind,
            FieldKind::Text | FieldKind::ComboBox | FieldKind::ListBox
        ) && !field.is_read_only()
            && !field.has_flag(flags::PASSWORD)
            && !field.has_flag(flags::FILE_SELECT)
            && field.widgets.iter().any(|w| !w.is_hidden());
        if fillable && wanted(index) {
            fields.push((index, field, Labels::of(field)));
        }
    }
    let mut addresses = address_fills(&fields, profile);
    let titles = titles(&fields);
    let mut out = Vec::new();
    for (index, field, labels) in &fields {
        let (index, field) = (*index, *field);
        let empty = field.value.same_as(&Value::None);
        if !empty && !replace {
            continue;
        }
        // The best of what the person added; it wins a tie with the
        // standard details.
        let mut custom: Option<(f32, &Entry)> = None;
        for entry in &profile.entries {
            let EntryKind::Custom(label) = &entry.kind else {
                continue;
            };
            if entry.value.trim().is_empty() {
                continue;
            }
            let phrases: Vec<(&str, f32)> = label
                .split([',', ';', '|'])
                .map(|p| (p.trim(), 1.0))
                .filter(|(p, _)| !p.is_empty())
                .collect();
            let score = labels.score(&phrases);
            if score >= GOOD_ENOUGH && custom.is_none_or(|(s, _)| score > s) {
                custom = Some((score, entry));
            }
        }
        let standard = match addresses.remove(&index) {
            Some((from, line)) => Some((1.0, from, line)),
            None => titles
                .get(&index)
                .map(|which| (0.9, Want::Detail(*which)))
                .filter(|(_, want)| !profile.format(want).is_empty())
                .or_else(|| labels.want(profile))
                .map(|(score, want)| (score, want.source(), profile.format(&want))),
        };
        let (score, from, text) = match (custom, standard) {
            (Some((c, entry)), Some((s, _, mut text))) if c >= s => {
                wipe_string(&mut text);
                (c, entry.label().to_string(), entry.value.clone())
            }
            (_, Some((s, from, text))) => (s, from.to_string(), text),
            (Some((c, entry)), None) => (c, entry.label().to_string(), entry.value.clone()),
            (None, None) => continue,
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
    for (_, line) in addresses.values_mut() {
        wipe_string(line);
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

    /// The test profile's address on one line.
    const FULL: &str = "12 St James's Square, London, TX 75001";

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
            // An address with no city, state or ZIP field beside it is
            // written out whole.
            ("Address", None, FULL),
            ("Street_Address", None, FULL),
            ("home address line 1", None, FULL),
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
                FULL,
            ),
            // Misspellings and abbreviations.
            ("Adress", None, FULL),
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

    /// What each field of a form with these names is filled with.
    fn fills(p: &Profile, names: &[&str]) -> Vec<Option<String>> {
        let form = form(names.iter().map(|n| field(n, None)).collect());
        let planned = plan(&form, p, false, |_| true);
        (0..names.len())
            .map(|i| {
                planned
                    .iter()
                    .find(|x| x.field == i)
                    .map(|x| x.value.display())
            })
            .collect()
    }

    /// The test profile with its name and address in parts.
    fn in_parts() -> Profile {
        let mut p = profile();
        for (kind, value) in [
            (Standard::FirstName, "Ada"),
            (Standard::MiddleName, "King"),
            (Standard::LastName, "Lovelace"),
            (Standard::AddressLine2, "Flat 3"),
            (Standard::Country, "UK"),
        ] {
            p.entries.push(Entry {
                kind: EntryKind::Standard(kind),
                value: value.into(),
            });
        }
        p
    }

    fn some(values: &[&str]) -> Vec<Option<String>> {
        values.iter().map(|v| Some(v.to_string())).collect()
    }

    #[test]
    fn a_name_goes_whole_or_in_the_parts_the_form_asks_for() {
        let p = in_parts();
        assert_eq!(
            fills(&p, &["First name", "MI", "Last name", "Middle name"]),
            some(&["Ada", "K", "Lovelace", "King"])
        );
        assert_eq!(fills(&p, &["Name"]), some(&["Ada Lovelace"]));
        assert_eq!(
            fills(&p, &["Name (Last, First, MI)", "First and last name"]),
            some(&["Lovelace, Ada K", "Ada Lovelace"])
        );
        // With no whole name kept, it is made from the parts.
        let mut parts = in_parts();
        parts
            .entries
            .retain(|e| e.kind != EntryKind::Standard(Standard::Name));
        assert_eq!(fills(&parts, &["Full name"]), some(&["Ada King Lovelace"]));
        assert_eq!(
            parts.format(&Want::NameParts(vec![NamePart::First, NamePart::Last])),
            "Ada Lovelace"
        );
    }

    #[test]
    fn a_date_of_birth_is_written_the_way_the_field_shows() {
        let mut p = profile();
        p.entries.push(Entry {
            kind: EntryKind::Standard(Standard::BirthDate),
            value: "December 10, 1815".into(),
        });
        assert_eq!(
            fills(
                &p,
                &[
                    "Date of birth",
                    "DOB (MM/DD/YYYY)",
                    "Birth date yyyy-mm-dd",
                    "Date of Birth DD/MM/YY",
                    "Birth month",
                    "Day of birth",
                    "Year of birth",
                    "Date",
                ]
            ),
            vec![
                Some("December 10, 1815".into()),
                Some("12/10/1815".into()),
                Some("1815-12-10".into()),
                Some("10/12/15".into()),
                Some("12".into()),
                Some("10".into()),
                Some("1815".into()),
                None,
            ]
        );
        for (typed, date) in [
            ("1815-12-10", Some((1815, 12, 10))),
            ("12/10/1815", Some((1815, 12, 10))),
            ("25/12/90", Some((1990, 12, 25))),
            ("10 Dec 1815", Some((1815, 12, 10))),
            ("soon", None),
            ("13/13/2000", None),
        ] {
            assert_eq!(parse_date(typed), date, "{typed}");
        }
    }

    #[test]
    fn business_details_fill_the_fields_that_ask_for_the_business() {
        let mut p = profile();
        for (kind, value) in [
            (Standard::Company, "Analytical Engines Ltd"),
            (Standard::Dba, "Engine Works"),
            (Standard::Division, "Research"),
            (Standard::JobTitle, "Mathematician"),
            (Standard::WorkEmail, "ada@engines.example"),
            (Standard::WorkPhone, "555 0199"),
            (Standard::Website, "engines.example"),
        ] {
            p.entries.push(Entry {
                kind: EntryKind::Standard(kind),
                value: value.into(),
            });
        }
        assert_eq!(
            fills(
                &p,
                &[
                    "Company Name",
                    "Business name",
                    "Employer.Name",
                    "DBA",
                    "Doing business as",
                    "Department",
                    "Job title",
                    "Work email",
                    "Business Phone",
                    "Website",
                    "Email",
                    "Name",
                    "Name of Bank",
                    "Employer.Address",
                ]
            ),
            vec![
                Some("Analytical Engines Ltd".into()),
                Some("Analytical Engines Ltd".into()),
                Some("Analytical Engines Ltd".into()),
                Some("Engine Works".into()),
                Some("Engine Works".into()),
                Some("Research".into()),
                Some("Mathematician".into()),
                Some("ada@engines.example".into()),
                Some("555 0199".into()),
                Some("engines.example".into()),
                Some("ada@example.org".into()),
                Some("Ada Lovelace".into()),
                None,
                None,
            ]
        );
        // With only a work email, a plain email field takes it.
        p.entries
            .retain(|e| e.kind != EntryKind::Standard(Standard::Email));
        assert_eq!(fills(&p, &["Email"]), some(&["ada@engines.example"]));
    }

    #[test]
    fn a_title_is_the_one_its_neighbours_say() {
        let mut p = in_parts();
        for (kind, value) in [
            (Standard::Honorific, "Dr"),
            (Standard::JobTitle, "Mathematician"),
            (Standard::Company, "Analytical Engines Ltd"),
        ] {
            p.entries.push(Entry {
                kind: EntryKind::Standard(kind),
                value: value.into(),
            });
        }
        // Beside the name it is Dr; beside the employer, the job.
        assert_eq!(
            fills(
                &p,
                &[
                    "Title",
                    "First name",
                    "Last name",
                    "Phone",
                    "Company",
                    "Title",
                    "Occupation"
                ]
            ),
            vec![
                Some("Dr".into()),
                Some("Ada".into()),
                Some("Lovelace".into()),
                Some("555 0100".into()),
                Some("Analytical Engines Ltd".into()),
                Some("Mathematician".into()),
                Some("Mathematician".into()),
            ]
        );
        // On its own it is the job; a label that says which needs no help.
        assert_eq!(fills(&p, &["Title"]), some(&["Mathematician"]));
        assert_eq!(
            fills(&p, &["Prefix", "Job title"]),
            some(&["Dr", "Mathematician"])
        );
        // A title by the name is not filled with the job for want of a Dr.
        p.entries
            .retain(|e| e.kind != EntryKind::Standard(Standard::Honorific));
        assert_eq!(fills(&p, &["Title", "First name"])[0], None);
    }

    #[test]
    fn an_address_is_split_or_joined_to_suit_the_fields_around_it() {
        let p = in_parts();
        // City, state and ZIP fields beside it: the street lines only,
        // put together when there is one field for both.
        assert_eq!(
            fills(&p, &["Address", "City", "State", "ZIP", "Country"]),
            some(&[
                "12 St James's Square, Flat 3",
                "London",
                "TX",
                "75001",
                "UK"
            ])
        );
        assert_eq!(
            fills(&p, &["Address", "Apt", "City", "State", "ZIP"]),
            some(&["12 St James's Square", "Flat 3", "London", "TX", "75001"])
        );
        assert_eq!(
            fills(&p, &["Address 1", "Address 2", "City", "Zip"])[..2],
            some(&["12 St James's Square", "Flat 3"])[..]
        );
        // A column of address lines and nothing else: line by line, the
        // last taking what is left.
        assert_eq!(
            fills(&p, &["Address", "Address 2", "Address 3", "Address 4"]),
            some(&["12 St James's Square", "Flat 3", "London, TX 75001", "UK"])
        );
        assert_eq!(
            fills(&p, &["Address line 1", "Address line 2"]),
            some(&["12 St James's Square", "Flat 3, London, TX 75001, UK"])
        );
        // A second line on its own is left alone.
        assert_eq!(fills(&p, &["Address line 2"]), vec![None]);
    }

    #[test]
    fn an_address_goes_to_whichever_address_the_profile_has() {
        let mut p = profile();
        p.entries
            .retain(|e| e.kind != EntryKind::Standard(Standard::StreetAddress));
        let form = form(vec![field("Address", None)]);
        let planned = plan(&form, &p, false, |_| true);
        assert_eq!(planned[0].from, "Mailing address, if different");
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
