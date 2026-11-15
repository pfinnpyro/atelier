//! Fixed-budget ablations. Test families never affect fitting or abstention.
use atelier_fieldml::{Budget, Class, Model, contextual_label, corpus, synth};
use std::{fmt::Write, fs, path::PathBuf};

type Labels = Vec<(String, Class)>;
fn plain(label: &str) -> &str {
    label.split_once(" || ").map_or(label, |p| p.0)
}
fn input(form: &corpus::Form, context: bool) -> Vec<String> {
    form.fields
        .iter()
        .map(|f| {
            if context {
                contextual_label(&f.label, &f.section)
            } else {
                f.label.clone()
            }
        })
        .collect()
}
#[derive(Default, Clone)]
struct Metrics {
    fields: usize,
    correct: usize,
    positives: usize,
    personal: usize,
    negatives: usize,
    false_fills: usize,
    fills: usize,
    safe: usize,
    class_right: [usize; 26],
    class_total: [usize; 26],
}
impl Metrics {
    fn add(&mut self, want: Class, said: Class, confidence: f32, threshold: f32) {
        let at = Class::ALL.iter().position(|c| *c == want).unwrap();
        self.class_total[at] += 1;
        self.class_right[at] += usize::from(want == said);
        self.fields += 1;
        self.correct += usize::from(want == said);
        if want == Class::Other {
            self.negatives += 1;
            self.false_fills += usize::from(said != Class::Other);
        } else {
            self.positives += 1;
            self.personal += usize::from(want == said);
        }
        if said != Class::Other && confidence >= threshold {
            self.fills += 1;
            self.safe += usize::from(said == want);
        }
    }
    fn macro_recall(&self) -> f64 {
        let present: Vec<_> = self
            .class_total
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .collect();
        present
            .iter()
            .map(|(i, n)| self.class_right[*i] as f64 / **n as f64)
            .sum::<f64>()
            / present.len().max(1) as f64
    }
    fn show(&self) -> String {
        format!(
            "accuracy={:.2}% personal={}/{} false_fills={}/{} accepted={}/{} correct; coverage={:.2}% macro_recall={:.2}%",
            100. * self.correct as f64 / self.fields.max(1) as f64,
            self.personal,
            self.positives,
            self.false_fills,
            self.negatives,
            self.safe,
            self.fills,
            100. * self.safe as f64 / self.positives.max(1) as f64,
            100. * self.macro_recall()
        )
    }
}
fn score(model: &Model, forms: &[corpus::Form], context: bool, threshold: f32) -> Metrics {
    let mut m = Metrics::default();
    for form in forms {
        let labels = input(form, context);
        let refs: Vec<_> = labels.iter().map(String::as_str).collect();
        for (field, (said, p)) in form.fields.iter().zip(model.classify(&refs)) {
            m.add(field.class, said, p, threshold);
        }
    }
    m
}
fn threshold(model: &Model, validation: &[corpus::Form], context: bool) -> f32 {
    // Fixed grid. Maximize validation coverage with zero observed incorrect
    // fills. This is an operating point, not probability calibration.
    let mut best = (0, 1.01f32);
    for t in [0., 0.5, 0.7, 0.8, 0.9, 0.95, 0.98, 0.99, 0.995, 0.999, 1.01] {
        let m = score(model, validation, context, t);
        if m.safe == m.fills && m.safe > best.0 {
            best = (m.safe, t);
        }
    }
    best.1
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if !(3..=4).contains(&args.len()) {
        return Err("expected DATA_DIR OUT_DIR [REVIEWED_TRAINING_TSV]".into());
    }
    let data = PathBuf::from(&args[1]);
    let out = PathBuf::from(&args[2]);
    fs::create_dir_all(&out)?;
    let public = corpus::parse(&fs::read_to_string(data.join("applications-gold.tsv"))?)?;
    let extracted = corpus::parse(&fs::read_to_string(
        data.join("applications-extracted.tsv"),
    )?)?;
    assert_eq!(public.len(), extracted.len());
    for (a, b) in public.iter().zip(&extracted) {
        assert_eq!(
            (&a.id, &a.split, a.fields.len()),
            (&b.id, &b.split, b.fields.len())
        );
        for (x, y) in a.fields.iter().zip(&b.fields) {
            assert_eq!((&x.id, x.class), (&y.id, y.class));
        }
    }
    let training_public: Vec<_> = public.iter().filter(|f| f.split == "train").collect();
    assert_eq!(
        training_public.len(),
        4,
        "freeze public per-source sampling when corpus changes"
    );
    let w9 = corpus::parse(&fs::read_to_string(data.join("online.tsv"))?)?
        .into_iter()
        .find(|f| f.id == "public-w9" && f.split == "train")
        .ok_or("missing W9 training form")?
        .labels();
    let mut old_rng = synth::Rng::new(1);
    let old: Vec<Labels> = (0..4000).map(|_| synth::form(&mut old_rng)).collect();
    let mut hard_rng = synth::Rng::new(2026);
    let hard: Vec<Labels> = (0..800)
        .map(|_| synth::application(&mut hard_rng))
        .collect();
    let mut synthetic_tsv = String::from("# split\tform\tfield\tclass\tlabel\tsection\n");
    let mut synthetic = Vec::new();
    for (split, seed, count) in [
        ("train", 2026, 800),
        ("validation", 2027, 300),
        ("test", 2028, 300),
    ] {
        let mut rng = synth::Rng::new(seed);
        for i in 0..count {
            let fields = synth::application(&mut rng);
            for (j, (l, c)) in fields.iter().enumerate() {
                let (label, section) = l.split_once(" || ").unwrap_or((l, ""));
                writeln!(
                    synthetic_tsv,
                    "{split}\tapplication-{split}-{i:04}\tf{j:03}\t{c:?}\t{label}\t{section}"
                )?;
            }
        }
    }
    fs::write(out.join("synthetic.tsv"), &synthetic_tsv)?;
    synthetic.extend(corpus::parse(&synthetic_tsv)?);
    let mut reviewed = Vec::new();
    if let Some(path) = args.get(3) {
        let legacy = corpus::parse(&fs::read_to_string(data.join("online.tsv"))?)?;
        for form in corpus::parse(&fs::read_to_string(path)?)? {
            if form.split != "train"
                || public
                    .iter()
                    .chain(&synthetic)
                    .chain(&legacy)
                    .any(|known| known.id == form.id && known.split != "train")
            {
                return Err("correction replay rejected: held-out/non-training form".into());
            }
            reviewed.push(form.labels());
        }
        if reviewed.len() > 20 {
            return Err("at most 20 complete correction forms; ten replays each".into());
        }
    }
    let mut variants = Vec::new();
    for (name, use_hard, use_public, context) in [
        ("control", false, false, false),
        ("hard-without-context", true, false, false),
        ("hard-negatives", true, false, true),
        ("public-without-context", true, true, false),
        ("contextual", true, true, true),
        ("contextual-recall", true, true, true),
    ] {
        let mut forms = old[..2800].to_vec();
        forms.extend(std::iter::repeat_n(w9.clone(), 200));
        forms.extend(if use_hard {
            hard.clone()
        } else {
            old[2800..3600].to_vec()
        });
        if use_public {
            // Fixed fifty replays PER source. Adding a source must not silently
            // dilute existing sources. Assert above makes changes explicit.
            for source in &training_public {
                forms.extend(std::iter::repeat_n(source.labels(), 50));
            }
        } else {
            forms.extend_from_slice(&old[3600..3800]);
        }
        if use_hard {
            for (i, reviewed_form) in reviewed.iter().enumerate() {
                for j in 0..10 {
                    forms[3000 + i * 10 + j] = reviewed_form.clone();
                }
            }
        }
        if !context {
            for form in &mut forms {
                for (label, _) in form {
                    *label = plain(label).to_string();
                }
            }
        }
        assert_eq!(forms.len(), 4000);
        variants.push((name, context, forms));
    }
    let mut report = String::from(
        "Application benchmark v2\n26 classes, 8192 buckets, 4000 forms, 6 epochs per fit. Seeds: 1, 7, 19.\nCommon: 2800 legacy synthetic + 200 W9 replays. 800 augmentation and 200 public slots vary explicitly.\nPublic training: 50 replays each of employment, adoption-vet, volunteer-freedom, membership-rotary.\nFresh validation: rental-taylor, volunteer-turning, camp-ymca.\nFresh test: scholarship-sd91, patient-ripley, camp-pearce, i9.\nW7/W4 are legacy diagnostics, excluded from operating-point selection and fresh test.\ncontextual-recall: Other loss weight 0.25; all other variants use weight 1.0.\n\n",
    );
    writeln!(
        report,
        "Reviewed correction forms: {}; 10 replays each, replacing augmentation slots only.\n",
        reviewed.len()
    )?;
    let validation: Vec<_> = public
        .iter()
        .filter(|f| f.split == "validation")
        .cloned()
        .collect();
    let mut predictions = String::from(
        "variant\tseed\ttrack\tsplit\tform\tfield\tlabel\tsection\texpected\tpredicted\tconfidence\taccepted\n",
    );
    let mut corrections =
        String::from("variant\tsplit\tform\tfield\tlabel\tsection\tpredicted\texpected\n");
    let mut policies = String::from("variant\tseed\tthreshold\tselection\n");
    let mut metrics = String::from(
        "variant\tseed\ttrack\tsplit\tfields\tcorrect\tpersonal\tpositives\tfalse_fills\tnegatives\taccepted_correct\taccepted\tthreshold\tmacro_recall\n",
    );
    let mut risk = String::from(
        "variant\tseed\ttrack\tthreshold\tvalidation_accepted_correct\tvalidation_accepted\tvalidation_positives\n",
    );
    let mut models = Vec::new();
    for (name, context, forms) in variants {
        for seed in [1, 7, 19] {
            let model = if name == "contextual-recall" {
                Model::train_on_weighted(Budget::PERSONAL, seed, &forms, 0.25)
            } else {
                Model::train_on(Budget::PERSONAL, seed, &forms)
            };
            let t = threshold(&model, &validation, context);
            fs::write(out.join(format!("{name}-{seed}.bin")), model.to_bytes())?;
            assert_eq!(Model::from_bytes(&model.to_bytes()), Some(model.clone()));
            writeln!(
                policies,
                "{name}\t{seed}\t{t}\tgold validation only; fixed grid, zero observed incorrect fills"
            )?;
            models.push((name.to_string(), seed, context, model, t));
        }
    }
    let legacy = Model::from_bytes(&fs::read(data.join("reference/personal-v1.bin"))?)
        .ok_or("invalid legacy model")?;
    let t = threshold(&legacy, &validation, false);
    writeln!(
        policies,
        "legacy-v1\t1\t{t}\tgold validation only; fixed grid, zero observed incorrect fills"
    )?;
    models.insert(0, ("legacy-v1".into(), 1, false, legacy, t));
    for (name, seed, context, model, t) in &models {
        writeln!(
            report,
            "{name} seed={seed} validation-selected threshold={t}"
        )?;
        for (track, forms) in [
            ("gold", &public),
            ("extracted", &extracted),
            ("synthetic", &synthetic),
        ] {
            let track_validation: Vec<_> = forms
                .iter()
                .filter(|f| f.split == "validation")
                .cloned()
                .collect();
            let track_t = threshold(model, &track_validation, *context);
            if track == "extracted" {
                writeln!(
                    policies,
                    "{name}\t{seed}\t{track_t}\textracted validation only; fixed grid, zero observed incorrect fills"
                )?;
            }
            for cutoff in [0., 0.5, 0.7, 0.8, 0.9, 0.95, 0.98, 0.99, 0.995, 0.999, 1.01] {
                let m = score(model, &track_validation, *context, cutoff);
                writeln!(
                    risk,
                    "{name}\t{seed}\t{track}\t{cutoff}\t{}\t{}\t{}",
                    m.safe, m.fills, m.positives
                )?;
            }
            for split in ["train", "validation", "test"] {
                let subset: Vec<_> = forms.iter().filter(|f| f.split == split).cloned().collect();
                let m = score(model, &subset, *context, track_t);
                writeln!(
                    metrics,
                    "{name}\t{seed}\t{track}\t{split}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{track_t}\t{:.6}",
                    m.fields,
                    m.correct,
                    m.personal,
                    m.positives,
                    m.false_fills,
                    m.negatives,
                    m.safe,
                    m.fills,
                    m.macro_recall()
                )?;
                writeln!(
                    report,
                    "  {track} {split}: {}",
                    score(model, &subset, *context, track_t).show()
                )?;
            }
            if *seed != 1 {
                continue;
            }
            for form in forms {
                let labels = input(form, *context);
                let refs: Vec<_> = labels.iter().map(String::as_str).collect();
                for (f, (said, p)) in form.fields.iter().zip(model.classify(&refs)) {
                    writeln!(
                        predictions,
                        "{name}\t{seed}\t{track}\t{}\t{}\t{}\t{}\t{}\t{:?}\t{said:?}\t{p:.6}\t{}",
                        form.split,
                        form.id,
                        f.id,
                        f.label,
                        f.section,
                        f.class,
                        said != Class::Other && p >= track_t
                    )?;
                    if track == "gold" && said != f.class {
                        writeln!(
                            corrections,
                            "{name}\t{}\t{}\t{}\t{}\t{}\t{said:?}\t{:?}",
                            form.split, form.id, f.id, f.label, f.section, f.class
                        )?;
                    }
                }
            }
        }
        if *seed == 1 {
            for form in &public {
                writeln!(
                    report,
                    "  family {} {}: {}",
                    form.split,
                    form.id,
                    score(model, std::slice::from_ref(form), *context, *t).show()
                )?;
            }
        }
    }
    // Keep legacy forms diagnostic after their errors guided this iteration.
    let mut diagnostic = corpus::parse(&fs::read_to_string(data.join("online.tsv"))?)?;
    for form in &mut diagnostic {
        for f in &mut form.fields {
            if form.id == "public-w4" && f.label == "First name and middle initial" {
                f.class = Class::FirstNameMiddleInitial;
            }
            if form.id == "public-w7"
                && f.label
                    == "Applicant mailing city or town, state or province, country and ZIP code"
            {
                f.class = Class::CityStateZipCountry;
            }
        }
    }
    for (name, seed, context, model, t) in &models {
        if *seed == 1 {
            for form in &diagnostic {
                writeln!(
                    report,
                    "legacy diagnostic {name} {}: {}",
                    form.id,
                    score(model, std::slice::from_ref(form), *context, *t).show()
                )?;
            }
        }
    }
    report.push_str("\nGold uses manually reviewed labels/sections. Extracted uses readable PDF tooltips or page geometry and heuristic section boundaries at known field slots; printable slots are manually located, so this is NOT automatic field detection.\nEach input track selects its threshold only on that track’s validation families, using the same fixed grid; test data never sets a threshold. It is not calibrated probability or a safety guarantee.\nSynthetic splits share grammar; public source families are disjoint. Test results do not select models, thresholds or seeds.\nOther includes third parties, work contacts, former/secondary addresses and unsupported conditional details.\n");
    fs::write(out.join("metrics.tsv"), metrics)?;
    fs::write(out.join("risk-coverage.tsv"), risk)?;
    fs::copy(out.join("contextual-1.bin"), out.join("personal.bin"))?;
    let contextual = &models
        .iter()
        .find(|(name, seed, _, _, _)| name == "contextual" && *seed == 1)
        .unwrap();
    let extracted_validation: Vec<_> = extracted
        .iter()
        .filter(|f| f.split == "validation")
        .cloned()
        .collect();
    let extracted_t = threshold(&contextual.3, &extracted_validation, true);
    fs::write(
        out.join("policy.json"),
        format!(
            "{{\"variant\":\"contextual\",\"seed\":1,\"gold_threshold\":{},\"extracted_threshold\":{},\"auto_fill_enabled\":false,\"note\":\"Review-only experiment; cutoff acceptance is not authorization for automatic filling.\"}}\n",
            contextual.4, extracted_t,
        ),
    )?;
    fs::write(out.join("report.txt"), &report)?;
    fs::write(out.join("predictions.tsv"), predictions)?;
    fs::write(out.join("corrections.tsv"), corrections)?;
    fs::write(out.join("policies.tsv"), policies)?;
    print!("{report}");
    Ok(())
}
