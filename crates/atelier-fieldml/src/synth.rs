//! Makes up forms to train on: the labels of a form's fields in order,
//! each with what it asks for. A form is a shuffle of sections (a name,
//! a date of birth, an address, ways to reach the person) laid out in
//! one of the ways real forms lay them out, among fields about other
//! things and other people, with the labels dressed the way form
//! authors dress them.

use crate::Class;
use crate::Class as C;

/// A small, fast, repeatable source of choices (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below `n` (which must not be zero).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() >> 11) as usize % n
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.below(from.len())]
    }
}

type Fields = Vec<(String, Class)>;

fn one(label: &str, class: Class) -> (String, Class) {
    (label.to_string(), class)
}

fn name(r: &mut Rng) -> Fields {
    const WHOLE: &[&str] = &[
        "Name",
        "Full name",
        "Applicant name",
        "Your name",
        "Name of applicant",
        "Legal name",
        "Print name",
        "Printed name",
        "Customer name",
        "First and last name",
        "Patient name",
        "Member name",
    ];
    const TOGETHER: &[&str] = &[
        "Name (Last, First, MI)",
        "Last name, First name",
        "Last, First, Middle initial",
        "Name: Last, First",
        "Last name, first name, MI",
        "Name (Last, First)",
    ];
    const TITLE: &[&str] = &[
        "Title",
        "Prefix",
        "Salutation",
        "Mr/Mrs/Ms",
        "Title (Mr, Ms, Dr)",
    ];
    const FIRST: &[&str] = &[
        "First name",
        "Given name",
        "First",
        "Forename",
        "FName",
        "Name (first)",
    ];
    const LAST: &[&str] = &[
        "Last name",
        "Surname",
        "Family name",
        "Last",
        "LName",
        "Name (last)",
    ];
    const INITIAL: &[&str] = &["MI", "M.I.", "Middle initial", "Initial", "Middle init."];
    const MIDDLE: &[&str] = &["Middle name", "Middle", "Middle name(s)"];
    match r.below(10) {
        0..=3 => vec![one(r.pick(WHOLE), C::FullName)],
        4 => vec![one(r.pick(TOGETHER), C::NameLastFirst)],
        _ => {
            let mut out = Vec::new();
            if r.chance(30) {
                out.push(one(r.pick(TITLE), C::Honorific));
            }
            let first = one(r.pick(FIRST), C::FirstName);
            let last = one(r.pick(LAST), C::LastName);
            let middle = match r.below(4) {
                0 | 1 => None,
                2 => Some(one(r.pick(INITIAL), C::MiddleInitial)),
                _ => Some(one(r.pick(MIDDLE), C::MiddleName)),
            };
            if r.chance(20) {
                // Surname first.
                out.push(last);
                out.push(first);
                out.extend(middle);
            } else {
                out.push(first);
                out.extend(middle);
                out.push(last);
            }
            out
        }
    }
}

fn birth(r: &mut Rng) -> Fields {
    const WHOLE: &[&str] = &[
        "Date of birth",
        "DOB",
        "Birth date",
        "Birthdate",
        "Birthday",
        "D.O.B.",
    ];
    const PATTERN: &[&str] = &[
        "",
        "",
        " (MM/DD/YYYY)",
        " mm/dd/yyyy",
        " (DD/MM/YYYY)",
        " YYYY-MM-DD",
        " (mm/dd/yy)",
    ];
    if r.chance(80) {
        return vec![one(
            &format!("{}{}", r.pick(WHOLE), r.pick(PATTERN)),
            C::BirthDate,
        )];
    }
    let style = r.below(3);
    let part = |word: &str| match style {
        0 => format!("Birth {word}"),
        1 => format!("{word} of birth"),
        _ => format!("DOB {word}"),
    };
    vec![
        (part("month"), C::BirthMonth),
        (part("day"), C::BirthDay),
        (part("year"), C::BirthYear),
    ]
}

fn address(r: &mut Rng) -> Fields {
    const KIND: &[&str] = &[
        "",
        "",
        "",
        "Home ",
        "Mailing ",
        "Street ",
        "Residential ",
        "Current ",
        "Permanent ",
        "Postal ",
    ];
    const SECOND: &[&str] = &[
        "Address line 2",
        "Address 2",
        "Apt",
        "Apt/Suite",
        "Suite",
        "Unit",
        "Apartment, suite, unit",
        "Apt. number",
    ];
    const CITY: &[&str] = &["City", "Town", "City/Town", "City or town"];
    const STATE: &[&str] = &[
        "State",
        "Province",
        "State/Province",
        "ST",
        "State or province",
    ];
    const ZIP: &[&str] = &[
        "ZIP",
        "Zip code",
        "Postal code",
        "Postcode",
        "ZIP/Postal code",
        "Zipcode",
    ];
    const TOGETHER: &[&str] = &[
        "City, State, ZIP",
        "City/State/Zip",
        "CityStateZip",
        "City, State and ZIP code",
        "City, state, zip code",
    ];
    const COUNTY: &[&str] = &["County", "Parish"];
    const COUNTRY: &[&str] = &["Country", "Nation", "Country/Region"];
    let kind = r.pick(KIND);
    match r.below(10) {
        // One box for all of it.
        0 | 1 => vec![one(&format!("{kind}Address"), C::AddressWhole)],
        // A column of lines and nothing else.
        2 | 3 => {
            let lines = 2 + r.below(3);
            let style = r.below(4);
            (1..=lines)
                .map(|n| {
                    let label = match style {
                        0 => format!("{kind}Address {n}"),
                        1 => format!("{kind}Address line {n}"),
                        2 => format!("{kind}Address"),
                        _ if n == 1 => format!("{kind}Address"),
                        _ => format!("{kind}Address {n}"),
                    };
                    let class = match n {
                        1 => C::AddressLine1,
                        2 => C::AddressLine2,
                        _ => C::AddressOverflow,
                    };
                    (label, class)
                })
                .collect()
        }
        // The street, then where it is in fields of their own.
        _ => {
            let first = match r.below(5) {
                0 => format!("{kind}Address line 1"),
                1 => format!("{kind}Address 1"),
                2 => "Street".to_string(),
                _ => format!("{kind}Address"),
            };
            let mut out = vec![(first, C::AddressLine1)];
            if r.chance(50) {
                out.push(one(r.pick(SECOND), C::AddressLine2));
            }
            if r.chance(20) {
                out.push(one(r.pick(TOGETHER), C::CityStateZip));
            } else {
                out.push(one(r.pick(CITY), C::City));
                if r.chance(15) {
                    out.push(one(r.pick(COUNTY), C::County));
                }
                out.push(one(r.pick(STATE), C::State));
                out.push(one(r.pick(ZIP), C::Zip));
            }
            if r.chance(30) {
                out.push(one(r.pick(COUNTRY), C::Country));
            }
            out
        }
    }
}

fn email(r: &mut Rng) -> Fields {
    const EMAIL: &[&str] = &[
        "Email",
        "E-mail",
        "Email address",
        "E-mail address",
        "Electronic mail",
    ];
    vec![one(r.pick(EMAIL), C::Email)]
}

fn phone(r: &mut Rng) -> Fields {
    const PHONE: &[&str] = &[
        "Phone",
        "Phone number",
        "Telephone",
        "Tel",
        "Tel. no.",
        "Mobile",
        "Cell phone",
        "Daytime phone",
        "Contact number",
        "Phone no.",
        "Home phone",
        "Telephone number",
    ];
    vec![one(r.pick(PHONE), C::Phone)]
}

/// Fields a personal profile has nothing for.
fn other(r: &mut Rng) -> Fields {
    const ALONE: &[&str] = &[
        "Signature",
        "Date",
        "Today's date",
        "Date signed",
        "Comments",
        "Notes",
        "Amount",
        "Account number",
        "SSN",
        "Social security number",
        "Reference number",
        "Policy number",
        "Relationship",
        "Gender",
        "Marital status",
        "Driver's license number",
        "Check number",
        "Description",
        "Quantity",
        "Total",
        "Fax",
        "Start date",
        "Expiration date",
        "Nationality",
        "Place of birth",
        "Website",
        "Bank name",
        "Name of bank",
        "Witness name",
        "Street number",
        "Text1",
        "Statement",
        "Routing number",
        "Member ID",
        "Reason for visit",
    ];
    // Groups about work or about somebody else: their names, addresses
    // and phones are not the person's own.
    const WORK: &[&str] = &[
        "Employer",
        "Employer name",
        "Company name",
        "Business name",
        "Title",
        "Job title",
        "Occupation",
        "Department",
        "Work phone",
        "Work email",
        "Employer address",
        "Employer city",
        "Employer phone",
        "Supervisor name",
        "Position",
    ];
    const SPOUSE: &[&str] = &[
        "Spouse name",
        "Spouse first name",
        "Spouse last name",
        "Spouse date of birth",
        "Spouse phone",
        "Spouse email",
    ];
    const EMERGENCY: &[&str] = &[
        "Emergency contact",
        "Emergency contact name",
        "Emergency contact phone",
        "Emergency phone",
        "Emergency contact address",
        "Emergency contact relationship",
    ];
    let group = |r: &mut Rng, from: &[&str], most: usize| -> Fields {
        let mut out: Fields = Vec::new();
        for _ in 0..1 + r.below(most) {
            let label = r.pick(from);
            if out.iter().all(|(l, _)| l != label) {
                out.push(one(label, C::Other));
            }
        }
        out
    };
    match r.below(10) {
        0..=5 => vec![one(r.pick(ALONE), C::Other)],
        6 | 7 => group(r, WORK, 4),
        8 => group(r, SPOUSE, 3),
        _ => group(r, EMERGENCY, 3),
    }
}

/// The label as some form author would have written it.
fn dress(r: &mut Rng, label: &str, class: Class) -> String {
    let mut text = label.to_string();
    // The odd misspelling.
    if r.chance(3) {
        let letters: Vec<usize> = text
            .char_indices()
            .filter(|(_, c)| c.is_ascii_alphabetic())
            .map(|(i, _)| i)
            .collect();
        if letters.len() >= 7 {
            text.remove(letters[1 + r.below(letters.len() - 2)]);
        }
    }
    if class != C::Other && r.chance(8) {
        text = format!("{}{text}", r.pick(&["Applicant ", "Your ", "Applicant's "]));
    }
    match r.below(12) {
        0 => text.to_uppercase(),
        1 => text.to_lowercase(),
        2 => text.replace([' ', '/'], "_"),
        3 => {
            // camelCase, as field names in a file often are.
            let mut out = String::new();
            for (i, word) in crate::tokens(&text).iter().enumerate() {
                let mut chars = word.chars();
                match chars.next() {
                    Some(c) if i > 0 => {
                        out.extend(c.to_uppercase());
                        out.push_str(chars.as_str());
                    }
                    _ => out.push_str(word),
                }
            }
            // Patterns and punctuation that carry meaning are kept as
            // they were.
            if label.contains(['(', ',']) {
                text
            } else {
                out
            }
        }
        4 => format!("{text}:"),
        5 => format!("{text} *"),
        _ => text,
    }
}

/// One made-up form: its fields' labels in order, and what each asks for.
pub fn form(r: &mut Rng) -> Fields {
    let mut sections: Vec<Fields> = vec![name(r)];
    if r.chance(60) {
        sections.push(birth(r));
    }
    if r.chance(85) {
        sections.push(address(r));
    }
    if r.chance(70) {
        sections.push(email(r));
    }
    if r.chance(70) {
        sections.push(phone(r));
    }
    for _ in 0..1 + r.below(4) {
        sections.push(other(r));
    }
    for i in (1..sections.len()).rev() {
        sections.swap(i, r.below(i + 1));
    }
    sections
        .into_iter()
        .flatten()
        .map(|(label, class)| (dress(r, &label, class), class))
        .collect()
}

/// Application sections, including repeated identical labels belonging to
/// different people. This augmentation is separate from the legacy grammar
/// so its effect can be measured with a fixed-budget ablation.
pub fn application(r: &mut Rng) -> Fields {
    let owner = r.pick(&[
        "Applicant information",
        "Primary applicant information",
        "Current address",
        "Present address",
        "Residential address",
        "Mailing address",
        "Application information",
        "Contact details",
        "Employee information",
        "Camper information",
        "Student information",
        "Personal information",
        "Patient information",
        "Participant information",
        "Your details",
        "Residence",
    ]);
    let mut out = match r.below(4) {
        0 => vec![one("Full name", C::FullName)],
        1 => vec![
            one("First name and middle initial", C::FirstNameMiddleInitial),
            one(
                r.pick(&[
                    "Last name",
                    "Last",
                    "Surname",
                    "Family name",
                    "Patient legal name (Last)",
                ]),
                C::LastName,
            ),
        ],
        _ => vec![
            one(
                r.pick(&[
                    "First name",
                    "First",
                    "Given name",
                    "Name (first)",
                    "Patient legal name (First)",
                ]),
                C::FirstName,
            ),
            one(
                r.pick(&["Middle initial", "MI", "M.I.", "Middle Int."]),
                C::MiddleInitial,
            ),
            one(
                r.pick(&[
                    "Last name",
                    "Last",
                    "Surname",
                    "Family name",
                    "Patient legal name (Last)",
                ]),
                C::LastName,
            ),
        ],
    };
    if r.chance(30) {
        out.insert(
            0,
            one(
                r.pick(&["Title", "Prefix", "Salutation", "Mr/Ms/Dr"]),
                C::Honorific,
            ),
        );
    }
    out.push(one(
        r.pick(&["Date of birth", "Birthdate", "DOB (MM/DD/YYYY)"]),
        C::BirthDate,
    ));
    out.push(one("Address", C::AddressLine1));
    if r.chance(50) {
        out.push(one("Apt/Unit", C::AddressLine2));
        out.push(one("City", C::City));
        out.push(one("State", C::State));
        out.push(one("Zip code", C::Zip));
    } else if r.chance(50) {
        out.push(one("City, State, Zip", C::CityStateZip));
    } else {
        out.push(one(
            "City, state, country and postal code",
            C::CityStateZipCountry,
        ));
    }
    out.push(one(
        r.pick(&[
            "Home phone",
            "Cell phone",
            "Telephone number",
            "Home #",
            "Cell #",
            "Cell",
            "Mobile",
            "Home",
            "Contact phone number",
        ]),
        C::Phone,
    ));
    out.push(one(r.pick(&["Email", "Email address", "E-mail"]), C::Email));
    // Being in the applicant's section does not make every detail supported.
    for _ in 0..1 + r.below(3) {
        out.push(one(
            r.pick(&[
                "Signature",
                "Date signed",
                "SSN",
                "Nationality",
                "Preferred full name (if different)",
                "Full given names",
                "First and middle names",
                "Country of birth",
            ]),
            C::Other,
        ));
    }

    let mut sections = vec![
        out.into_iter()
            .map(|(label, class)| {
                (
                    crate::contextual_label(&dress(r, &label, class), owner),
                    class,
                )
            })
            .collect::<Fields>(),
    ];
    let headings = [
        "Emergency contact information",
        "Primary contact",
        "Secondary contact",
        "Previous address",
        "Employer information",
        "References",
        "Spouse information",
        "Dependant information",
        "Dependent information",
        "Dependents",
        "Children and parents",
        "Immediate family members",
        "Family information",
        "Current employment",
        "Previous employment",
        "Work information",
        "Business details",
        "Mailing Address (if different)",
        "Alternate address",
        "Secondary address",
        "Personal representative",
        "GENERAL CONSENT",
        "Parent / Guardian information",
        "Guarantor",
        "Responsible party",
        "Veterinarian information",
        "Physician",
        "Doctor",
        "Hospital",
        "Medical facility",
        "Pharmacy",
        "Driver license",
        "Identification document",
        "Clinic",
        "Proposing sponsor",
        "Authorized pickup",
        "Insurance policyholder",
        "Preparer or translator",
        "Landlord",
        "Secondary participant",
        "Birthplace",
        "Name at birth",
        "Historical information",
        "Foreign address",
        "School information",
        "Office use only",
    ];
    for _ in 0..2 + r.below(4) {
        let heading = r.pick(&headings);
        let labels = match heading {
            "Previous address" | "Foreign address" => {
                vec!["Address", "City", "State", "Zip code", "Country"]
            }
            "Name at birth" | "Historical information" => {
                vec!["First name", "Middle name", "Last name"]
            }
            "Birthplace" => vec!["Country", "City and state"],
            "Employer information" | "School information" | "Clinic" => {
                vec!["Name", "Address", "Phone", "Email"]
            }
            _ => vec!["Name", "Address", "City", "Phone", "Email", "Date of birth"],
        };
        let count = 1 + r.below(labels.len());
        sections.push(
            labels[..count]
                .iter()
                .map(|label| {
                    (
                        crate::contextual_label(&dress(r, label, C::Other), heading),
                        C::Other,
                    )
                })
                .collect(),
        );
    }
    for i in (1..sections.len()).rev() {
        sections.swap(i, r.below(i + 1));
    }
    let mut form: Fields = sections.into_iter().flatten().collect();
    if r.chance(30) {
        for (label, class) in [
            ("Participant name (1)", C::FullName),
            ("Participant name (2)", C::Other),
        ] {
            form.push((
                crate::contextual_label(label, "Participant information"),
                class,
            ));
        }
    }

    // Inline qualifiers are also essential where there is no distinct heading.
    let inline = [
        "Address applying for",
        "Desired property address",
        "Address of proposed rental",
        "Best way to contact you",
        "Preferred contact method",
        "State/Country",
        "Printed name of patient or personal representative",
        "Name of personal representative",
        "Preferred full name (if different)",
        "Full given names",
        "First and middle names",
        "Former address",
        "Maiden name",
        "Other last names used",
        "Name of parent/guardian",
        "Landlord phone",
        "Veterinarian name",
        "Work email",
        "Work phone",
        "Country of birth",
        "City of birth",
        "City/state of birth",
        "State of birth",
        "Birth city",
        "Birth country",
        "Place of birth",
        "Reference phone",
        "Date signed",
        "Driver license number",
        "Insurance policy number",
        "Relationship to applicant",
        "Pet name",
        "Account number",
    ];
    for _ in 0..1 + r.below(5) {
        form.push(one(r.pick(&inline), C::Other));
    }
    form
}
