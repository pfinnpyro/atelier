//! Calibrate on reserved calibration families, then evaluate a frozen test once.
//! DATA MODEL OUT [calibrate|test]
use atelier_fieldml::{Class, corpus, personal::PersonalModel};
use std::{fmt::Write, fs, path::PathBuf};
#[derive(Clone, Default)]
struct Metrics {
    n: usize,
    right: usize,
    pos: usize,
    personal: usize,
    neg: usize,
    bad: usize,
    hard_neg: usize,
    hard_bad: usize,
    accepted: usize,
    safe: usize,
    confidence: f64,
    brier: f64,
    classes: [(usize, usize); 26],
    bins: [(usize, usize, f64); 10],
}
impl Metrics {
    fn add(&mut self, want: Class, said: Class, p: f32, hard: bool) {
        self.n += 1;
        if hard {
            self.hard_neg += 1;
            self.hard_bad += usize::from(said != Class::Other);
        }
        self.right += usize::from(want == said);
        self.brier += (f64::from(p) - f64::from(u8::from(want == said))).powi(2);
        let i = Class::ALL.iter().position(|c| *c == want).unwrap();
        self.classes[i].0 += usize::from(want == said);
        self.classes[i].1 += 1;
        if want == Class::Other {
            self.neg += 1;
            self.bad += usize::from(said != Class::Other);
        } else {
            self.pos += 1;
            self.personal += usize::from(want == said);
        }
        if said != Class::Other && p > 0.90 {
            self.accepted += 1;
            self.safe += usize::from(want == said);
            self.confidence += f64::from(p);
        }
        let b = ((p * 10.) as usize).min(9);
        self.bins[b].0 += 1;
        self.bins[b].1 += usize::from(want == said);
        self.bins[b].2 += f64::from(p);
    }
    fn ratio(a: usize, b: usize) -> f64 {
        a as f64 / b.max(1) as f64
    }
    fn macro_recall(&self) -> f64 {
        let present: Vec<_> = self.classes.iter().filter(|(_, n)| *n > 0).collect();
        present
            .iter()
            .map(|(r, n)| Self::ratio(*r, *n))
            .sum::<f64>()
            / present.len().max(1) as f64
    }
    fn ece(&self) -> f64 {
        self.bins
            .iter()
            .filter(|(n, _, _)| *n > 0)
            .map(|(n, r, p)| (Self::ratio(*r, *n) - p / *n as f64).abs() * *n as f64)
            .sum::<f64>()
            / self.n.max(1) as f64
    }
    fn qualifies(&self) -> bool {
        Self::ratio(self.right, self.n) > 0.90
            && Self::ratio(self.personal, self.pos) > 0.90
            && self.macro_recall() > 0.90
            && Self::ratio(self.bad, self.neg) < 0.10
            && self.hard_neg > 0
            && Self::ratio(self.hard_bad, self.hard_neg) < 0.10
            && Self::ratio(self.safe, self.accepted) > 0.90
            && Self::ratio(self.safe, self.pos) > 0.90
            && self.confidence / self.accepted.max(1) as f64 > 0.90
            && self.ece() < 0.10
            && self.accepted >= 20
    }
    fn show(&self) -> String {
        format!(
            "accuracy {:.2}%; recall {}/{} ({:.2}%); macro {:.2}%; falsefills {}/{} ({:.2}%); accepted {}/{} precision {:.2}%; accepted recall {:.2}%; mean accepted confidence {:.2}%; ECE {:.2}%; hard-negative falsefills {}/{}; qualifies {}",
            100. * Self::ratio(self.right, self.n),
            self.personal,
            self.pos,
            100. * Self::ratio(self.personal, self.pos),
            100. * self.macro_recall(),
            self.bad,
            self.neg,
            100. * Self::ratio(self.bad, self.neg),
            self.safe,
            self.accepted,
            100. * Self::ratio(self.safe, self.accepted),
            100. * Self::ratio(self.safe, self.pos),
            100. * self.confidence / self.accepted.max(1) as f64,
            100. * self.ece(),
            self.hard_bad,
            self.hard_neg,
            self.qualifies()
        )
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 {
        return Err("DATA_DIR MODEL_FILE OUT_DIR calibrate|test".into());
    }
    let data = PathBuf::from(&args[1]);
    let path = PathBuf::from(&args[2]);
    let out = PathBuf::from(&args[3]);
    fs::create_dir_all(&out)?;
    let mut model = PersonalModel::from_bytes(&fs::read(path)?).ok_or("invalid personal model")?;
    let actual = corpus::parse(&fs::read_to_string(data.join("v3/fresh-extracted.tsv"))?)?;
    let gold = corpus::parse(&fs::read_to_string(data.join("v3/fresh-gold.tsv"))?)?;
    if args[4] == "calibrate" {
        let calibration: Vec<_> = actual
            .iter()
            .filter(|f| f.split == "validation")
            .map(|f| f.labels())
            .collect();
        let before = model.to_bytes();
        let t = model.calibrate(&calibration);
        let after = model.to_bytes();
        assert_eq!(
            &before[12..],
            &after[12..],
            "calibration cannot modify learned heads"
        );
        fs::write(out.join("calibrated.bin"), after)?;
        println!("Calibrated only on reserved calibration families; temperature {t}");
    } else if args[4] != "test" {
        return Err("expected calibrate or test".into());
    }
    let hard_ids: std::collections::HashSet<_> = gold
        .iter()
        .flat_map(|form| {
            form.fields
                .iter()
                .filter(|f| {
                    f.class == Class::Other
                        && atelier_fieldml::tokens(&f.label).iter().any(|w| {
                            [
                                "name",
                                "surname",
                                "first",
                                "last",
                                "initial",
                                "birth",
                                "birthdate",
                                "dob",
                                "address",
                                "city",
                                "state",
                                "zip",
                                "postcode",
                                "postal",
                                "country",
                                "phone",
                                "telephone",
                                "tel",
                                "email",
                                "mail",
                            ]
                            .contains(&w.as_str())
                        })
                })
                .map(|f| (form.id.clone(), f.id.clone()))
        })
        .collect();
    let split = if args[4] == "calibrate" {
        "validation"
    } else {
        "test"
    };
    let mut report = format!(
        "V3 {split}; fixed acceptance cutoff >0.90; minimum 20 accepted fields; all rates above 90%, false-fill and ECE below 10%.\n"
    );
    let mut predictions = String::from(
        "track\tsplit\tform\tfield\tlabel\tsection\texpected\tpredicted\tconfidence\taccepted\n",
    );
    let mut summary = String::from(
        "track\tsplit\tfields\tcorrect\tpersonal\tpositives\tfalse_fills\tnegatives\taccepted_correct\taccepted\tmacro_recall\tmean_accepted_confidence\tece\thard_false_fills\thard_negatives\tqualifies\n",
    );
    let mut qualify = true;
    for (track, forms) in [("gold", gold), ("extracted", actual)] {
        let mut total = Metrics::default();
        for form in forms.iter().filter(|f| f.split == split) {
            let inputs = form.labels();
            let labels: Vec<_> = inputs.iter().map(|(l, _)| l.as_str()).collect();
            let mut family = Metrics::default();
            for (f, (said, p)) in form.fields.iter().zip(model.classify(&labels)) {
                let hard = hard_ids.contains(&(form.id.clone(), f.id.clone()));
                family.add(f.class, said, p, hard);
                total.add(f.class, said, p, hard);
                writeln!(
                    predictions,
                    "{track}\t{split}\t{}\t{}\t{}\t{}\t{:?}\t{said:?}\t{p:.7}\t{}",
                    form.id,
                    f.id,
                    f.label,
                    f.section,
                    f.class,
                    said != Class::Other && p > 0.90
                )?;
            }
            writeln!(report, "{track} family {}: {}", form.id, family.show())?;
        }
        writeln!(report, "{track} TOTAL: {}", total.show())?;
        for (i, (r, n)) in total
            .classes
            .iter()
            .enumerate()
            .filter(|(_, (_, n))| *n > 0)
        {
            writeln!(report, "  {:?}: {r}/{n}", Class::ALL[i])?;
        }
        writeln!(
            summary,
            "{track}\t{split}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{:.6}\t{}\t{}\t{}",
            total.n,
            total.right,
            total.personal,
            total.pos,
            total.bad,
            total.neg,
            total.safe,
            total.accepted,
            total.macro_recall(),
            total.confidence / total.accepted.max(1) as f64,
            total.ece(),
            total.hard_bad,
            total.hard_neg,
            total.qualifies()
        )?;
        qualify &= total.qualifies();
    }
    writeln!(report, "BOTH TRACKS QUALIFY: {qualify}")?;
    fs::write(out.join(format!("{split}-report.txt")), &report)?;
    fs::write(out.join(format!("{split}-predictions.tsv")), predictions)?;
    fs::write(out.join(format!("{split}-metrics.tsv")), summary)?;
    print!("{report}");
    Ok(())
}
