//! Says which detail of a profile each field of a form asks for, from
//! what the fields are called and what stands around them.
//!
//! The model is a multinomial logistic regression over hashed features of
//! a field's own words and its neighbours' words. It can train on forms
//! made up by [`synth`], which knows the right answer for every field it
//! writes, or explicitly labeled forms through [`Model::train_on`]. Everything is deterministic: the same budget and seed give the
//! same model, bit for bit.
//!
//! Models come in classes with fixed training budgets ([`Budget`]). Only
//! the Personal class is built so far: a person's name, date of birth,
//! address and how to reach them, and which *form* of each a field wants
//! (a whole address or its first line, a whole name or a part).
//!
//! [`analysis`] is the newer, form-level model: for every field it says
//! what detail the label asks for, which fields describe the same person or
//! thing, and whose block it is, reading the page's layout and typography
//! when it is given. `analysis::AnalysisModel::bundled()` is trained and
//! ready (about 1 MB); the rules in `atelier-form` decide what to fill.

pub mod analysis;
pub mod corpus;
pub mod personal;
pub mod synth;

/// What a field asks for. `Other` is everything a personal profile does
/// not hold, which is left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Other,
    FullName,
    /// Several parts of the name in one field, surname first.
    NameLastFirst,
    Honorific,
    FirstName,
    MiddleName,
    MiddleInitial,
    LastName,
    BirthDate,
    BirthMonth,
    BirthDay,
    BirthYear,
    /// The whole address in one field.
    AddressWhole,
    AddressLine1,
    AddressLine2,
    /// A third or later line of a column of address lines.
    AddressOverflow,
    City,
    State,
    Zip,
    CityStateZip,
    County,
    Country,
    Email,
    Phone,
    FirstNameMiddleInitial,
    CityStateZipCountry,
}

impl Class {
    pub const ALL: [Class; 26] = [
        Class::Other,
        Class::FullName,
        Class::NameLastFirst,
        Class::Honorific,
        Class::FirstName,
        Class::MiddleName,
        Class::MiddleInitial,
        Class::LastName,
        Class::BirthDate,
        Class::BirthMonth,
        Class::BirthDay,
        Class::BirthYear,
        Class::AddressWhole,
        Class::AddressLine1,
        Class::AddressLine2,
        Class::AddressOverflow,
        Class::City,
        Class::State,
        Class::Zip,
        Class::CityStateZip,
        Class::County,
        Class::Country,
        Class::Email,
        Class::Phone,
        Class::FirstNameMiddleInitial,
        Class::CityStateZipCountry,
    ];

    fn index(self) -> usize {
        Class::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// How much a class of model may cost to train and to ship. The numbers
/// are fixed per class so models of a class are comparable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub name: &'static str,
    /// Feature buckets; the model is `buckets` x classes 32-bit weights.
    pub buckets: usize,
    /// Forms in the training budget, generated or explicitly labeled.
    pub forms: usize,
    /// Passes over them.
    pub epochs: usize,
}

impl Budget {
    /// Names, dates of birth, addresses, email and phone: about 0.8 MB.
    pub const PERSONAL: Budget = Budget {
        name: "personal",
        buckets: 1 << 13,
        forms: 4000,
        epochs: 6,
    };
    /// Reserved for the Business class (company, DBA, division, work
    /// contact details). Its classes and generator are not written yet.
    pub const BUSINESS: Budget = Budget {
        name: "business",
        buckets: 1 << 15,
        forms: 20_000,
        epochs: 8,
    };
    /// Reserved for the Enterprise class.
    pub const ENTERPRISE: Budget = Budget {
        name: "enterprise",
        buckets: 1 << 17,
        forms: 80_000,
        epochs: 10,
    };
}

/// The words of a label, lower case, with `camelCase` and `snake_case`
/// split apart and letters parted from digits.
pub fn tokens(label: &str) -> Vec<String> {
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
        } else {
            spaced.push(' ');
        }
        before = Some(c);
    }
    spaced.split_whitespace().map(str::to_string).collect()
}

/// How far to either side a field's neighbours are read.
const REACH: usize = 3;

fn bucket(feature: &str, buckets: usize) -> u32 {
    // FNV-1a.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in feature.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % buckets as u64) as u32
}

/// The features of field `at` among a form's fields, as bucket numbers.
fn features(form: &[Vec<String>], at: usize, buckets: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(64);
    let mut add = |f: String| out.push(bucket(&f, buckets));
    let all_own = &form[at];
    let own: Vec<_> = all_own.iter().filter(|w| !w.starts_with('@')).collect();
    for section in all_own.iter().filter_map(|w| w.strip_prefix('@')) {
        add(format!("section:{section}"));
        for word in &own {
            add(format!("section+label:{section}|{word}"));
        }
    }
    add("bias".into());
    add(format!("len:{}", own.len().min(6)));
    for w in &own {
        add(format!("w:{w}"));
    }
    for pair in own.windows(2) {
        add(format!("w2:{} {}", pair[0], pair[1]));
    }
    // How many fields just before say the same thing but for a number:
    // the place in a column of "Address, Address, Address".
    let plain = |t: &[String]| -> Vec<String> {
        t.iter()
            .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
            .cloned()
            .collect()
    };
    let mine = plain(all_own);
    let run = (0..at)
        .rev()
        .take_while(|i| plain(&form[*i]) == mine)
        .count();
    add(format!("run:{}", run.min(4)));
    for w in &mine {
        add(format!("w+run:{w}:{}", run.min(4)));
    }
    for off in 1..=REACH {
        let sides = [
            (at.checked_sub(off).map(|i| &form[i]), 'p'),
            (form.get(at + off), 'n'),
        ];
        for (near, side) in sides {
            let Some(near) = near else {
                if off == 1 {
                    add(format!("{side}:edge"));
                }
                continue;
            };
            for w in near.iter().filter(|w| !w.starts_with('@')) {
                add(format!("{side}{off}:{w}"));
                add(format!("near:{w}"));
                // What this field says together with what stands right
                // beside it: "address" by "city" is not "address" alone.
                if off <= 2 {
                    for mine in &own {
                        add(format!("x:{mine}|{w}"));
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A trained model.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    buckets: usize,
    /// One row of `buckets` weights per class.
    weights: Vec<f32>,
}

const MAGIC: &[u8; 8] = b"ATLFML02";

/// Encodes a field and its own section without leaking that section to
/// neighboring fields. Use the visible heading, never an expected class.
pub fn contextual_label(label: &str, section: &str) -> String {
    if section.is_empty() {
        label.to_string()
    } else {
        format!("{label} || {section}")
    }
}

fn field_tokens(label: &str) -> Vec<String> {
    let (label, section) = label.split_once(" || ").unwrap_or((label, ""));
    let mut out = tokens(label);
    out.extend(tokens(section).into_iter().map(|w| format!("@{w}")));
    out
}

impl Model {
    fn scores(&self, features: &[u32]) -> Vec<f32> {
        let mut scores = vec![0.0f32; Class::ALL.len()];
        for (c, score) in scores.iter_mut().enumerate() {
            let row = &self.weights[c * self.buckets..(c + 1) * self.buckets];
            *score = features.iter().map(|f| row[*f as usize]).sum();
        }
        // Softmax.
        let top = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut total = 0.0;
        for s in &mut scores {
            *s = (*s - top).exp();
            total += *s;
        }
        for s in &mut scores {
            *s /= total;
        }
        scores
    }

    /// What each field of a form asks for and how sure the model is,
    /// from 0 to 1. `labels` are the fields' names (or tooltips) in the
    /// order the form has them.
    pub fn classify(&self, labels: &[&str]) -> Vec<(Class, f32)> {
        let form: Vec<Vec<String>> = labels.iter().map(|l| field_tokens(l)).collect();
        (0..form.len())
            .map(|at| {
                let scores = self.scores(&features(&form, at, self.buckets));
                let (best, sure) = scores
                    .iter()
                    .copied()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap_or((0, 0.0));
                (Class::ALL[best], sure)
            })
            .collect()
    }

    /// Applies an independently validated confidence operating point. `None`
    /// means leave the field for review. Scores are not calibrated probabilities.
    pub fn classify_for_fill(
        &self,
        labels: &[&str],
        minimum_confidence: f32,
    ) -> Vec<Option<(Class, f32)>> {
        assert!(minimum_confidence.is_finite() && (0.0..=1.0).contains(&minimum_confidence));
        self.classify(labels)
            .into_iter()
            .map(|(class, confidence)| {
                (class != Class::Other && confidence >= minimum_confidence)
                    .then_some((class, confidence))
            })
            .collect()
    }

    /// Trains a model within a budget on forms made up from `seed`.
    pub fn train(budget: Budget, seed: u64) -> Model {
        let mut rng = synth::Rng::new(seed);
        let forms: Vec<_> = (0..budget.forms).map(|_| synth::form(&mut rng)).collect();
        Self::fit(budget, &forms, &mut rng, 1.0)
    }

    /// Train on explicitly labeled forms, including external examples and
    /// reviewed corrections. The caller controls sampling; `budget.forms`
    /// must match the number of forms. Validation/test forms must stay out.
    pub fn train_on(budget: Budget, seed: u64, forms: &[Vec<(String, Class)>]) -> Model {
        assert_eq!(budget.forms, forms.len(), "training form budget mismatch");
        Self::fit(budget, forms, &mut synth::Rng::new(seed), 1.0)
    }

    /// Explicitly controls the loss of negative (`Other`) examples, without
    /// changing per-source sampling. Useful for comparing recall tradeoffs.
    pub fn train_on_weighted(
        budget: Budget,
        seed: u64,
        forms: &[Vec<(String, Class)>],
        other_weight: f32,
    ) -> Model {
        assert_eq!(budget.forms, forms.len());
        assert!(other_weight.is_finite() && other_weight > 0.0);
        Self::fit(budget, forms, &mut synth::Rng::new(seed), other_weight)
    }

    fn fit(
        budget: Budget,
        forms: &[Vec<(String, Class)>],
        rng: &mut synth::Rng,
        other_weight: f32,
    ) -> Model {
        assert!(budget.buckets > 0 && budget.epochs > 0);
        assert!(!forms.is_empty() && forms.iter().all(|f| !f.is_empty()));
        let mut examples: Vec<(Vec<u32>, usize)> = Vec::new();
        for form in forms {
            let words: Vec<Vec<String>> = form.iter().map(|(l, _)| field_tokens(l)).collect();
            for (at, (_, class)) in form.iter().enumerate() {
                examples.push((features(&words, at, budget.buckets), class.index()));
            }
        }
        let mut model = Model {
            buckets: budget.buckets,
            weights: vec![0.0; budget.buckets * Class::ALL.len()],
        };
        for epoch in 0..budget.epochs {
            let rate = 0.2 / (1.0 + epoch as f32);
            // A fresh order each pass.
            for i in (1..examples.len()).rev() {
                examples.swap(i, rng.below(i + 1));
            }
            for (features, class) in &examples {
                let scores = model.scores(features);
                for (c, p) in scores.iter().enumerate() {
                    let loss_weight = if *class == Class::Other.index() {
                        other_weight
                    } else {
                        1.0
                    };
                    let step = rate * loss_weight * (p - f32::from(u8::from(c == *class)));
                    let row = &mut model.weights[c * budget.buckets..(c + 1) * budget.buckets];
                    for f in features {
                        row[*f as usize] -= step;
                    }
                }
            }
        }
        model
    }

    /// The share of fields of `forms` made-up forms the model gets right,
    /// and the mistakes as (label, wanted, said), for forms from `seed`.
    pub fn evaluate(&self, forms: usize, seed: u64) -> (f32, Vec<(String, Class, Class)>) {
        let mut rng = synth::Rng::new(seed);
        let (mut right, mut all, mut wrong) = (0usize, 0usize, Vec::new());
        for _ in 0..forms {
            let form = synth::form(&mut rng);
            let labels: Vec<&str> = form.iter().map(|(l, _)| l.as_str()).collect();
            for ((label, want), (said, _)) in form.iter().zip(self.classify(&labels)) {
                all += 1;
                if *want == said {
                    right += 1;
                } else {
                    wrong.push((label.clone(), *want, said));
                }
            }
        }
        (right as f32 / all.max(1) as f32, wrong)
    }

    /// The model as bytes to keep in a file.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.weights.len() * 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.buckets as u32).to_le_bytes());
        out.extend_from_slice(&(Class::ALL.len() as u32).to_le_bytes());
        for w in &self.weights {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out
    }

    /// Reads a model back. `None` for bytes that are not one, or one
    /// for another set of classes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Model> {
        let (head, body) = bytes.split_at_checked(16)?;
        let number = |at: usize| Some(u32::from_le_bytes(head[at..at + 4].try_into().ok()?));
        let (buckets, classes) = (number(8)? as usize, number(12)? as usize);
        let legacy = &head[..8] == b"ATLFML01" && classes == 24;
        let fits = ((&head[..8] == MAGIC && classes == Class::ALL.len()) || legacy)
            && buckets > 0
            && buckets.checked_mul(classes)?.checked_mul(4)? == body.len();
        fits.then(|| {
            let mut weights: Vec<f32> = body
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            // Legacy models never learned the added classes. Suppress these
            // rows so importing one preserves its original predictions.
            weights.resize(buckets * Class::ALL.len(), -100.0);
            Model { buckets, weights }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Class as C;

    fn model() -> &'static Model {
        static MODEL: std::sync::OnceLock<Model> = std::sync::OnceLock::new();
        MODEL.get_or_init(|| Model::train(Budget::PERSONAL, 1))
    }

    fn said(labels: &[&str]) -> Vec<Class> {
        model()
            .classify(labels)
            .into_iter()
            .map(|(c, _)| c)
            .collect()
    }

    #[test]
    fn forms_it_has_not_seen_are_read_right() {
        // Forms from another seed: the same kinds of form, other choices.
        let (share, wrong) = model().evaluate(500, 99);
        let sample: Vec<_> = wrong.iter().take(12).collect();
        assert!(share >= 0.985, "{share}: {sample:?}");
    }

    #[test]
    fn forms_written_by_hand_are_read_right() {
        assert_eq!(
            said(&["Name", "Address", "Phone", "Signature", "Date"]),
            [C::FullName, C::AddressWhole, C::Phone, C::Other, C::Other]
        );
        assert_eq!(
            said(&[
                "Title",
                "First Name",
                "MI",
                "Last Name",
                "Date of Birth (MM/DD/YYYY)",
                "Street Address",
                "Apt/Suite",
                "City",
                "State",
                "Zip Code",
                "Email Address",
                "Phone Number",
            ]),
            [
                C::Honorific,
                C::FirstName,
                C::MiddleInitial,
                C::LastName,
                C::BirthDate,
                C::AddressLine1,
                C::AddressLine2,
                C::City,
                C::State,
                C::Zip,
                C::Email,
                C::Phone,
            ]
        );
        assert_eq!(
            said(&[
                "Address",
                "Address",
                "Address",
                "Employer",
                "Title",
                "Occupation"
            ]),
            [
                C::AddressLine1,
                C::AddressLine2,
                C::AddressOverflow,
                C::Other,
                C::Other,
                C::Other,
            ]
        );
    }

    #[test]
    fn a_model_comes_back_from_its_bytes_and_trains_the_same_twice() {
        let small = Budget {
            name: "test",
            buckets: 256,
            forms: 20,
            epochs: 1,
        };
        let a = Model::train(small, 7);
        assert_eq!(a, Model::train(small, 7));
        let bytes = a.to_bytes();
        assert_eq!(Model::from_bytes(&bytes), Some(a));
        assert_eq!(Model::from_bytes(&bytes[..bytes.len() - 1]), None);
        assert_eq!(Model::from_bytes(b"nonsense"), None);
    }

    #[test]
    fn explicit_labels_train_a_model_without_the_generator() {
        let budget = Budget {
            name: "reviewed",
            buckets: 256,
            forms: 1,
            epochs: 8,
        };
        let forms = vec![vec![("alpha".into(), C::Email), ("beta".into(), C::Other)]];
        let model = Model::train_on(budget, 7, &forms);
        assert_eq!(model, Model::train_on(budget, 7, &forms));
        assert_eq!(model, Model::train_on_weighted(budget, 7, &forms, 1.0));
        assert_eq!(
            model
                .classify(&["alpha", "beta"])
                .iter()
                .map(|(c, _)| *c)
                .collect::<Vec<_>>(),
            [C::Email, C::Other]
        );
    }

    #[test]
    fn old_models_remain_loadable_and_do_not_predict_untrained_composites() {
        let bytes = include_bytes!("../data/reference/personal-v1.bin");
        let model = Model::from_bytes(bytes).expect("legacy 24-class model");
        assert_eq!(
            model
                .classify(&["Name", "Address", "Phone", "Signature", "Date"])
                .iter()
                .map(|(c, _)| *c)
                .collect::<Vec<_>>(),
            [C::FullName, C::AddressWhole, C::Phone, C::Other, C::Other]
        );
        let saved = model.to_bytes();
        assert!(saved.starts_with(b"ATLFML02"));
        assert_eq!(Model::from_bytes(&saved), Some(model));
    }

    #[test]
    fn section_ownership_is_distinct_and_does_not_bleed_to_neighbors() {
        let pairs = vec![
            (contextual_label("Name", "Applicant"), C::FullName),
            (contextual_label("Name", "Emergency contact"), C::Other),
            (contextual_label("Email", "Applicant"), C::Email),
            (contextual_label("Email", "Emergency contact"), C::Other),
        ];
        let budget = Budget {
            name: "sections",
            buckets: 8192,
            forms: 40,
            epochs: 8,
        };
        let forms = vec![pairs; 40];
        let model = Model::train_on(budget, 1, &forms);
        let labels = [
            contextual_label("Name", "Applicant"),
            contextual_label("Name", "Emergency contact"),
            contextual_label("Email", "Applicant"),
            contextual_label("Email", "Emergency contact"),
        ];
        let refs: Vec<_> = labels.iter().map(String::as_str).collect();
        assert_eq!(
            model
                .classify(&refs)
                .iter()
                .map(|(c, _)| *c)
                .collect::<Vec<_>>(),
            [C::FullName, C::Other, C::Email, C::Other]
        );
        let fills = model.classify_for_fill(&refs, 0.5);
        assert!(
            fills[0].is_some() && fills[1].is_none() && fills[2].is_some() && fills[3].is_none()
        );
        let words: Vec<_> = labels.iter().map(|l| field_tokens(l)).collect();
        let mut changed = words.clone();
        changed[1] = field_tokens(&contextual_label("Name", "Landlord"));
        assert_eq!(features(&words, 0, 8192), features(&changed, 0, 8192));
    }

    #[test]
    fn labels_split_into_words() {
        assert_eq!(
            tokens("firstName_2 (MM/DD)"),
            ["first", "name", "2", "mm", "dd"]
        );
    }
}
