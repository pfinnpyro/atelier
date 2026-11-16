//! Factorized Personal model: profile component and ownership are learned
//! independently. Calibration is fitted only on disjoint validation forms.
use crate::{Class, Model, synth, tokens};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Head {
    pub(crate) buckets: usize,
    pub(crate) classes: usize,
    weights: Vec<f32>,
}
pub(crate) fn hash(s: &str, n: usize) -> usize {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    }) as usize
        % n
}
fn parts(label: &str) -> (&str, &str) {
    label.split_once(" || ").unwrap_or((label, ""))
}
pub(crate) fn normalized(label: &str) -> Vec<String> {
    let raw = tokens(
        &label
            .replace("D.O.B.", "DOB")
            .replace("d.o.b.", "dob")
            .replace("M.I.", "MI"),
    );
    let mut out = Vec::new();
    let mut at = 0;
    while at < raw.len() {
        let word = raw[at].as_str();
        let next = raw.get(at + 1).map(String::as_str);
        if word == "h" && next == Some("p") {
            out.push("phone".into());
            at += 2;
            continue;
        }
        if (word == "postal" || word == "zip") && next == Some("code") {
            out.push("zip".into());
            at += 2;
            continue;
        }
        if (word == "given" || word == "family") && next == Some("name") {
            out.push(if word == "given" { "first" } else { "last" }.into());
            out.push("name".into());
            at += 2;
            continue;
        }
        match word {
            "surname" => out.extend(["last".into(), "name".into()]),
            "forename" => out.extend(["first".into(), "name".into()]),
            "suburb" | "locality" | "municipality" | "town" => out.push("city".into()),
            "province" | "territory" => out.push("state".into()),
            "postcode" | "zipcode" => out.push("zip".into()),
            "telephone" | "tel" | "mobile" | "mob" | "cell" => out.push("phone".into()),
            "present"
                if raw
                    .iter()
                    .any(|w| ["address", "street", "residence"].contains(&w.as_str())) =>
            {
                out.push("current".into())
            }
            "home" if raw.len() == 1 => out.push("phone".into()),
            _ => out.push(raw[at].clone()),
        }
        at += 1;
    }
    out
}
pub(crate) fn component_words(label: &str) -> Vec<String> {
    let raw = normalized(label);
    let mut words: Vec<_> = raw
        .iter()
        .filter(|word| {
            ![
                "applicant",
                "employee",
                "patient",
                "spouse",
                "parent",
                "guardian",
                "emergency",
                "employer",
                "company",
                "reference",
                "your",
                "work",
            ]
            .contains(&word.as_str())
        })
        .cloned()
        .collect();
    if words.is_empty() {
        return raw;
    }
    // Qualified metadata such as PostalAddressCity carries both a group prefix
    // and the actual leaf label. Keep explicit address line numbers intact.
    if words.len() > 2
        && ["postal", "mailing", "home", "residential", "current"].contains(&words[0].as_str())
        && words[1] == "address"
        && words[2] != "line"
        && !words[2].chars().all(|c| c.is_ascii_digit())
    {
        words = words[2..].to_vec();
    }
    words
}
fn component_label(label: &str) -> String {
    component_words(label).join(" ")
}
pub(crate) fn feature_words(
    out: &mut Vec<usize>,
    prefix: &str,
    words: &[String],
    buckets: usize,
    characters: bool,
) {
    for word in words {
        out.push(hash(&format!("{prefix}:{word}"), buckets));
        if characters && word.len() >= 4 && word.is_ascii() {
            let word = format!("^{word}$");
            for n in 3..=5 {
                for chars in word.as_bytes().windows(n) {
                    out.push(hash(
                        &format!("{prefix}:char:{}", std::str::from_utf8(chars).unwrap()),
                        buckets,
                    ));
                }
            }
        }
    }
    for n in 2..=3 {
        for pair in words.windows(n) {
            out.push(hash(&format!("{prefix}:{n}:{}", pair.join(" ")), buckets));
        }
    }
}
fn features(form: &[(String, Class)], at: usize, buckets: usize, ownership: bool) -> Vec<usize> {
    let (label, section) = parts(&form[at].0);
    let own = if ownership {
        normalized(label)
    } else {
        component_words(label)
    };
    let ctx = normalized(section);
    let mut out = vec![hash("bias", buckets)];
    feature_words(&mut out, "own", &own, buckets, true);
    if ownership {
        // Ownership words must transfer between inline labels and headings:
        // "employer phone" and a "Phone" in "Employer" describe the same owner.
        feature_words(&mut out, "meaning", &own, buckets, true);
        feature_words(&mut out, "meaning", &ctx, buckets, true);
        feature_words(&mut out, "section", &ctx, buckets, true);
        out.push(hash(&format!("section:entire:{}", ctx.join(" ")), buckets));
        for c in &ctx {
            for w in &own {
                out.push(hash(&format!("section-label:{c}|{w}"), buckets));
            }
        }
    } else {
        if let Some(first) = own.first() {
            out.push(hash(&format!("first:{first}"), buckets));
        }
        if let Some(last) = own.last() {
            out.push(hash(&format!("last:{last}"), buckets));
        }
        let mut previous_address = false;
        let mut next_postal = false;
        let mut nearby_locality = false;
        for off in 1..=3 {
            if let Some(index) = at.checked_sub(off) {
                let (other, ctx) = parts(&form[index].0);
                if ctx == section {
                    previous_address |= normalized(other)
                        .iter()
                        .any(|w| w == "address" || w == "street");
                }
            }
            if let Some((other, _)) = form.get(at + off) {
                let (other, ctx) = parts(other);
                if ctx == section {
                    let words = normalized(other);
                    if !words.iter().any(|w| w == "birth") {
                        nearby_locality |= words.iter().any(|w| {
                            ["city", "town", "state", "zip", "postcode", "postal"]
                                .contains(&w.as_str())
                        });
                        next_postal |= words
                            .iter()
                            .any(|w| ["zip", "postcode"].contains(&w.as_str()));
                    }
                }
            }
        }
        out.push(hash(
            &format!("layout:previous-address:{previous_address}:next-postal:{next_postal}"),
            buckets,
        ));
        if own.iter().any(|w| w == "address" || w == "street") {
            out.push(hash(
                &format!("address:locality-fields:{nearby_locality}"),
                buckets,
            ));
        }
        for off in 1..=3 {
            for (index, side) in [
                (at.checked_sub(off), 'p'),
                (at.checked_add(off).filter(|i| *i < form.len()), 'n'),
            ] {
                if let Some(index) = index {
                    let (other, other_section) = parts(&form[index].0);
                    if section == other_section {
                        let near = component_words(other);
                        for w in &near {
                            out.push(hash(&format!("{side}{off}:{w}"), buckets));
                        }
                        if off == 1 {
                            for a in &own {
                                for b in &near {
                                    out.push(hash(&format!("adjacent:{a}|{b}"), buckets));
                                }
                            }
                        }
                    }
                }
            }
        }
        let run = (0..at)
            .rev()
            .take_while(|i| parts(&form[*i].0) == (label, section))
            .count()
            .min(4);
        out.push(hash(&format!("run:{run}"), buckets));
        for w in &own {
            out.push(hash(&format!("run:{run}:{w}"), buckets));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}
impl Head {
    /// The element-wise mean of heads of one shape. For a linear model this
    /// averages the members' logits, a geometric-mean ensemble at the cost
    /// of one head.
    pub(crate) fn mean(heads: &[&Head]) -> Self {
        let first = heads[0];
        assert!(
            heads
                .iter()
                .all(|h| h.buckets == first.buckets && h.classes == first.classes)
        );
        let n = heads.len() as f32;
        let weights = (0..first.weights.len())
            .map(|i| heads.iter().map(|h| h.weights[i]).sum::<f32>() / n)
            .collect();
        Self {
            buckets: first.buckets,
            classes: first.classes,
            weights,
        }
    }
    /// Rounds every weight to one of 255 steps of its class's largest, so the
    /// head is exactly what `write` stores and `read` gives back.
    pub(crate) fn quantize(&mut self) {
        let buckets = self.buckets;
        for row in self.weights.chunks_mut(buckets) {
            let scale = Self::scale(row);
            if scale > 0.0 {
                row.iter_mut()
                    .for_each(|w| *w = (*w / scale).round().clamp(-127.0, 127.0) * scale);
            }
        }
    }
    fn scale(row: &[f32]) -> f32 {
        row.iter().fold(0f32, |m, w| m.max(w.abs())) / 127.0
    }
    /// Buckets and classes as little-endian u32, then per class its scale
    /// (f32) and one signed byte per bucket: weight = byte * scale. Exact
    /// for a quantized head.
    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.buckets as u32).to_le_bytes());
        out.extend_from_slice(&(self.classes as u32).to_le_bytes());
        for row in self.weights.chunks(self.buckets) {
            let scale = Self::scale(row);
            out.extend_from_slice(&scale.to_le_bytes());
            for w in row {
                let q = if scale > 0.0 {
                    (w / scale).round().clamp(-127.0, 127.0) as i8
                } else {
                    0
                };
                out.push(q as u8);
            }
        }
    }
    pub(crate) fn read(bytes: &[u8], at: &mut usize) -> Option<Self> {
        let word = |at: &mut usize| {
            let v = u32::from_le_bytes(bytes.get(*at..*at + 4)?.try_into().ok()?);
            *at += 4;
            Some(v)
        };
        let buckets = word(at)? as usize;
        let classes = word(at)? as usize;
        if !(1..=1 << 20).contains(&buckets) || !(1..=256).contains(&classes) {
            return None;
        }
        let mut weights = Vec::with_capacity(buckets * classes);
        for _ in 0..classes {
            let scale = f32::from_bits(word(at)?);
            if !scale.is_finite() || scale < 0.0 {
                return None;
            }
            let row = bytes.get(*at..*at + buckets)?;
            *at += buckets;
            weights.extend(row.iter().map(|&b| f32::from(b as i8) * scale));
        }
        Some(Self {
            buckets,
            classes,
            weights,
        })
    }
    pub(crate) fn probabilities(&self, f: &[usize]) -> Vec<f32> {
        let mut values: Vec<f32> = (0..self.classes)
            .map(|c| f.iter().map(|i| self.weights[c * self.buckets + i]).sum())
            .collect();
        let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for value in &mut values {
            *value = (*value - max).exp();
        }
        let total: f32 = values.iter().sum();
        for value in &mut values {
            *value /= total;
        }
        values
    }
    pub(crate) fn fit(
        classes: usize,
        buckets: usize,
        epochs: usize,
        seed: u64,
        examples: Vec<(Vec<usize>, usize, f32)>,
    ) -> Self {
        Self::fit_with(classes, buckets, epochs, seed, examples, 0.997, false)
    }
    pub(crate) fn fit_decay(
        classes: usize,
        buckets: usize,
        epochs: usize,
        seed: u64,
        examples: Vec<(Vec<usize>, usize, f32)>,
        decay: f32,
    ) -> Self {
        Self::fit_with(classes, buckets, epochs, seed, examples, decay, true)
    }
    /// `fit` with the per-epoch weight decay given (smaller is stronger
    /// L2) and, if `average`, the weights averaged over the second half of
    /// the epochs: far less dependent on the shuffling seed.
    fn fit_with(
        classes: usize,
        buckets: usize,
        epochs: usize,
        seed: u64,
        mut examples: Vec<(Vec<usize>, usize, f32)>,
        decay: f32,
        average: bool,
    ) -> Self {
        let mut sum = vec![0f32; if average { classes * buckets } else { 0 }];
        let mut summed = 0;
        let mut model = Self {
            classes,
            buckets,
            weights: vec![0.; classes * buckets],
        };
        let mut rng = synth::Rng::new(seed);
        for epoch in 0..epochs {
            let rate = 0.045 / (1. + epoch as f32).sqrt();
            for i in (1..examples.len()).rev() {
                examples.swap(i, rng.below(i + 1));
            }
            for (f, target, weight) in &examples {
                let p = model.probabilities(f);
                for (class, p) in p.iter().enumerate() {
                    let step = rate * weight * (p - f32::from(u8::from(class == *target)));
                    for i in f {
                        model.weights[class * buckets + i] -= step;
                    }
                }
            }
            for w in &mut model.weights {
                *w *= decay;
            }
            if average && epoch >= epochs / 2 {
                for (s, w) in sum.iter_mut().zip(&model.weights) {
                    *s += w;
                }
                summed += 1;
            }
        }
        if average && summed > 0 {
            for (w, s) in model.weights.iter_mut().zip(&sum) {
                *w = s / summed as f32;
            }
        }
        model
    }
}

/// Two learned heads. The ownership head receives the actual field and its
/// section, never a source ID, expected class, value or template lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonalModel {
    component: Head,
    ownership: Head,
    temperature: f32,
}
impl PersonalModel {
    pub fn train(
        forms: &[Vec<(String, Class)>],
        teacher: &Model,
        epochs: usize,
        seed: u64,
    ) -> Self {
        assert!(epochs > 0 && !forms.is_empty());
        let mut components = Vec::new();
        let mut owners = Vec::new();
        for form in forms {
            let labels: Vec<_> = form
                .iter()
                .map(|(l, _)| component_label(parts(l).0))
                .collect();
            let refs: Vec<_> = labels.iter().map(String::as_str).collect();
            let weak = teacher.classify(&refs);
            for (at, (_, class)) in form.iter().enumerate() {
                let personal = *class != Class::Other;
                let component = if personal {
                    *class
                } else if labels[at] == "title" {
                    Class::Honorific
                } else {
                    weak[at].0
                };
                let target = Class::ALL.iter().position(|c| *c == component).unwrap();
                components.push((
                    features(form, at, 8192, false),
                    target,
                    if personal || component == Class::Other {
                        1.
                    } else {
                        0.35
                    },
                ));
                owners.push((features(form, at, 8192, true), usize::from(personal), 1.));
            }
        }
        Self {
            component: Head::fit(26, 8192, epochs, seed, components),
            ownership: Head::fit(2, 8192, epochs, seed, owners),
            temperature: 1.,
        }
    }
    fn distribution(&self, form: &[(String, Class)], at: usize) -> Vec<f32> {
        let owner = self
            .ownership
            .probabilities(&features(form, at, self.ownership.buckets, true))[1];
        let mut p =
            self.component
                .probabilities(&features(form, at, self.component.buckets, false));
        for value in &mut p {
            *value *= owner;
        }
        p[0] += 1. - owner;
        p
    }
    fn calibrated(&self, mut p: Vec<f32>) -> Vec<f32> {
        for value in &mut p {
            *value = value.max(1e-12).powf(1. / self.temperature);
        }
        let total: f32 = p.iter().sum();
        for value in &mut p {
            *value /= total;
        }
        p
    }
    pub fn classify(&self, labels: &[&str]) -> Vec<(Class, f32)> {
        let form: Vec<_> = labels
            .iter()
            .map(|l| (l.to_string(), Class::Other))
            .collect();
        (0..form.len())
            .map(|at| {
                let p = self.calibrated(self.distribution(&form, at));
                let (at, p) = p
                    .into_iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap();
                (Class::ALL[at], p)
            })
            .collect()
    }
    /// Fit a single temperature by validation negative log likelihood. This
    /// only softens confidence (temperature >= 1), never raises it or changes the predicted class. Returns the temperature.
    pub fn calibrate(&mut self, validation: &[Vec<(String, Class)>]) -> f32 {
        let mut examples = Vec::new();
        for form in validation {
            for (at, (_, class)) in form.iter().enumerate() {
                examples.push((
                    self.distribution(form, at),
                    Class::ALL.iter().position(|c| c == class).unwrap(),
                ));
            }
        }
        assert!(!examples.is_empty());
        let mut best = (f64::INFINITY, 1.);
        for temperature in [1., 1.25, 1.5, 2., 3., 4.] {
            self.temperature = temperature;
            let loss: f64 = examples
                .iter()
                .map(|(p, target)| -f64::from(self.calibrated(p.clone())[*target].max(1e-12)).ln())
                .sum();
            if loss < best.0 {
                best = (loss, temperature);
            }
        }
        self.temperature = best.1;
        self.temperature
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = b"ATLPER04".to_vec();
        out.extend_from_slice(&self.temperature.to_le_bytes());
        for head in [&self.component, &self.ownership] {
            out.extend_from_slice(&(head.buckets as u32).to_le_bytes());
            out.extend_from_slice(&(head.classes as u32).to_le_bytes());
            for w in &head.weights {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        out
    }
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.get(..8)? != b"ATLPER04" {
            return None;
        }
        let mut at = 8;
        let read = |at: &mut usize| {
            let value: u32 = u32::from_le_bytes(bytes.get(*at..*at + 4)?.try_into().ok()?);
            *at += 4;
            Some(value)
        };
        let temperature = f32::from_bits(read(&mut at)?);
        if !temperature.is_finite() || temperature <= 0. {
            return None;
        }
        let mut heads = Vec::new();
        for classes in [26, 2] {
            let buckets = read(&mut at)? as usize;
            if !(1..=1 << 20).contains(&buckets) || read(&mut at)? as usize != classes {
                return None;
            }
            let mut weights = Vec::new();
            for _ in 0..buckets.checked_mul(classes)? {
                let w = f32::from_bits(read(&mut at)?);
                if !w.is_finite() {
                    return None;
                }
                weights.push(w);
            }
            heads.push(Head {
                buckets,
                classes,
                weights,
            });
        }
        if at != bytes.len() {
            return None;
        }
        let ownership = heads.pop()?;
        let component = heads.pop()?;
        Some(Self {
            component,
            ownership,
            temperature,
        })
    }
}
