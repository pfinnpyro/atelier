//! Development evaluation only; independent test is run after fitting freezes.
use atelier_fieldml::{Class, Model, corpus, personal::PersonalModel, synth};
use std::{fmt::Write, fs, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("DATA_DIR OUT_DIR".into());
    }
    let data = PathBuf::from(&args[1]);
    let out = PathBuf::from(&args[2]);
    fs::create_dir_all(&out)?;
    let gold = corpus::parse(&fs::read_to_string(data.join("v3/development-gold.tsv"))?)?;
    let extracted = corpus::parse(&fs::read_to_string(
        data.join("v3/development-extracted.tsv"),
    )?)?;
    let teacher = Model::from_bytes(&fs::read(data.join("reference/personal-v1.bin"))?).unwrap();
    let mut rng = synth::Rng::new(1);
    let mut train: Vec<_> = (0..3000).map(|_| synth::form(&mut rng)).collect();
    let mut rng = synth::Rng::new(2026);
    train.extend((0..3000).map(|_| synth::application(&mut rng)));
    for (a, b) in gold
        .iter()
        .zip(&extracted)
        .filter(|(f, _)| f.split == "train")
    {
        for _ in 0..30 {
            train.push(a.labels());
            train.push(b.labels());
        }
    }
    let mut model = PersonalModel::train(&train, &teacher, 20, 1);
    let validation: Vec<_> = extracted
        .iter()
        .filter(|f| f.split == "validation")
        .map(|f| f.labels())
        .collect();
    let t = model.calibrate(&validation);
    fs::write(out.join("personal.bin"), model.to_bytes())?;
    assert_eq!(
        PersonalModel::from_bytes(&model.to_bytes()),
        Some(model.clone())
    );
    let mut report = format!(
        "Factorized v3 development; seed1; training forms {}; temperature {t}; bytes {}\n",
        train.len(),
        model.to_bytes().len()
    );
    let mut predictions = String::from(
        "track\tsplit\tform\tfield\tlabel\tsection\texpected\tpredicted\tconfidence\n",
    );
    for (track, forms) in [("gold", gold), ("extracted", extracted)] {
        for form in forms {
            let input = form.labels();
            let labels: Vec<_> = input.iter().map(|(l, _)| l.as_str()).collect();
            let p = model.classify(&labels);
            let mut right = 0;
            let mut personal = 0;
            let mut positives = 0;
            let mut bad = 0;
            let mut negatives = 0;
            let mut accepted = 0;
            let mut safe = 0;
            for (f, (c, p)) in form.fields.iter().zip(p) {
                right += usize::from(f.class == c);
                if f.class == Class::Other {
                    negatives += 1;
                    bad += usize::from(c != Class::Other);
                } else {
                    positives += 1;
                    personal += usize::from(f.class == c);
                }
                if c != Class::Other && p >= 0.9 {
                    accepted += 1;
                    safe += usize::from(f.class == c);
                }
                writeln!(
                    predictions,
                    "{track}\t{}\t{}\t{}\t{}\t{}\t{:?}\t{c:?}\t{p:.6}",
                    form.split, form.id, f.id, f.label, f.section, f.class
                )?;
            }
            writeln!(
                report,
                "{track} {} {}: {right}/{} correct; personal {personal}/{positives}; falsefills {bad}/{negatives}; accepted {safe}/{accepted}",
                form.split,
                form.id,
                form.fields.len()
            )?;
        }
    }
    fs::write(out.join("predictions.tsv"), predictions)?;
    fs::write(out.join("report.txt"), &report)?;
    print!("{report}");
    Ok(())
}
