//! Reproducible baseline, mixed-corpus training, predictions and correction queue.
//! cargo run --release -p atelier-fieldml --example first_run -- DATA_DIR OUT_DIR
use atelier_fieldml::{Budget, Class, Model, corpus, synth};
use std::{fmt::Write, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if !(3..=4).contains(&args.len()) {
        return Err("expected DATA_DIR OUT_DIR [REVIEWED_TRAINING_TSV]".into());
    }
    let data = PathBuf::from(&args[1]);
    let out = PathBuf::from(&args[2]);
    fs::create_dir_all(&out)?;
    let public = corpus::parse(&fs::read_to_string(data.join("online.tsv"))?)?;
    let mut generated = String::from("# split\tform\tfield\tclass\tlabel\n");
    for (split, count, seed) in [
        ("train", 4000, 1),
        ("validation", 500, 98),
        ("test", 500, 99),
    ] {
        let mut rng = synth::Rng::new(seed);
        for n in 0..count {
            for (i, (label, class)) in synth::form(&mut rng).iter().enumerate() {
                writeln!(
                    generated,
                    "{split}\tsynth-{split}-{n:04}\tf{i:03}\t{class:?}\t{label}"
                )?;
            }
        }
    }
    fs::write(out.join("synthetic.tsv"), &generated)?;
    let synthetic = corpus::parse(&generated)?;
    let baseline = Model::train(Budget::PERSONAL, 1);
    fs::write(out.join("baseline.bin"), baseline.to_bytes())?;
    let mut external: Vec<_> = public
        .iter()
        .filter(|f| f.split == "train")
        .map(|f| f.labels())
        .collect();
    if let Some(path) = args.get(3) {
        let reviewed = corpus::parse(&fs::read_to_string(path)?)?;
        for form in reviewed {
            if form.split != "train"
                || public
                    .iter()
                    .chain(&synthetic)
                    .any(|p| p.id == form.id && p.split != "train")
            {
                return Err(
                    "corrections must be training-only; held-out forms cannot be replayed".into(),
                );
            }
            external.push(form.labels());
        }
    }
    if external.is_empty() {
        return Err("no external training forms".into());
    }
    // 5% replay of reviewed external forms, 95% synthetic, same total budget.
    let mut training: Vec<_> = synthetic
        .iter()
        .filter(|f| f.split == "train")
        .take(3800)
        .map(|f| f.labels())
        .collect();
    for i in 0..200 {
        training.push(external[i % external.len()].clone());
    }
    let start = std::time::Instant::now();
    let model = Model::train_on(Budget::PERSONAL, 1, &training);
    fs::write(out.join("personal.bin"), model.to_bytes())?;
    assert_eq!(
        Model::from_bytes(&fs::read(out.join("personal.bin"))?),
        Some(model.clone())
    );
    let mut report = format!(
        "Personal first run\n26 classes, 8192 buckets, 4000 forms, 6 epochs, seed 1\nMixed: 3800 synthetic + 200 replays of {} public/reviewed training form(s).\nMixed training seconds: {:.3}\nModel bytes: {}\n\n",
        external.len(),
        start.elapsed().as_secs_f64(),
        model.to_bytes().len()
    );
    let mut predictions =
        String::from("split\tform\tfield\tlabel\texpected\tbaseline\tpredicted\tconfidence\n");
    let mut corrections = String::from(
        "# Review queue only; never automatically train on validation/test errors.\nsplit\tform\tfield\tlabel\tpredicted\texpected\n",
    );
    for (name, forms) in [
        ("synthetic", &synthetic),
        ("online-visible-labels", &public),
    ] {
        for split in ["train", "validation", "test"] {
            let (
                mut count,
                mut base_right,
                mut right,
                mut positives,
                mut positive_right,
                mut false_fills,
                mut negatives,
            ) = (0, 0, 0, 0, 0, 0, 0);
            let mut per_class = [(0usize, 0usize); Class::ALL.len()];
            for form in forms.iter().filter(|f| f.split == split) {
                let labels: Vec<_> = form.fields.iter().map(|f| f.label.as_str()).collect();
                for ((f, (base, _)), (said, confidence)) in form
                    .fields
                    .iter()
                    .zip(baseline.classify(&labels))
                    .zip(model.classify(&labels))
                {
                    count += 1;
                    base_right += usize::from(base == f.class);
                    right += usize::from(said == f.class);
                    let class = Class::ALL.iter().position(|c| *c == f.class).unwrap();
                    per_class[class].0 += usize::from(said == f.class);
                    per_class[class].1 += 1;
                    if f.class == Class::Other {
                        negatives += 1;
                        false_fills += usize::from(said != Class::Other);
                    } else {
                        positives += 1;
                        positive_right += usize::from(said == f.class);
                    }
                    writeln!(
                        predictions,
                        "{}\t{}\t{}\t{}\t{:?}\t{base:?}\t{said:?}\t{confidence:.6}",
                        form.split, form.id, f.id, f.label, f.class
                    )?;
                    if said != f.class {
                        writeln!(
                            corrections,
                            "{}\t{}\t{}\t{}\t{said:?}\t{:?}",
                            form.split, form.id, f.id, f.label, f.class
                        )?;
                    }
                }
            }
            let present: Vec<_> = per_class.iter().filter(|(_, n)| *n > 0).collect();
            let macro_recall = present
                .iter()
                .map(|(r, n)| *r as f64 / *n as f64)
                .sum::<f64>()
                / present.len().max(1) as f64;
            writeln!(
                report,
                "{name} {split}: fields={count}, baseline={:.2}%, mixed={:.2}%, macro recall={:.2}%, personal={positive_right}/{positives}, false fills={false_fills}/{negatives}",
                100.0 * base_right as f64 / count.max(1) as f64,
                100.0 * right as f64 / count.max(1) as f64,
                100.0 * macro_recall
            )?;
        }
    }
    // Actual metadata path: these IRS PDFs have opaque /T names and no /TU.
    for form in &public {
        let raw: Vec<_> = form.fields.iter().map(|f| f.id.as_str()).collect();
        let predictions = model.classify(&raw);
        let right = form
            .fields
            .iter()
            .zip(&predictions)
            .filter(|(f, (said, _))| f.class == *said)
            .count();
        let positives = form
            .fields
            .iter()
            .filter(|f| f.class != Class::Other)
            .count();
        let personal_right = form
            .fields
            .iter()
            .zip(&predictions)
            .filter(|(f, (said, _))| f.class != Class::Other && f.class == *said)
            .count();
        let false_fills = form
            .fields
            .iter()
            .zip(&predictions)
            .filter(|(f, (said, _))| f.class == Class::Other && *said != Class::Other)
            .count();
        writeln!(
            report,
            "online-raw-metadata {} {}: {right}/{} correct; personal={personal_right}/{positives}; false fills={false_fills}",
            form.split,
            form.id,
            form.fields.len()
        )?;
    }
    report.push_str("\nVisible labels are human annotations, not automatic label extraction. Raw metadata results expose the PDF extraction gap.\nSynthetic splits share a generator grammar; different seeds are not unseen template families.\nOnly three public form families: held-out results are diagnostic, not production accuracy.\nOther includes unsupported composites and historical/third-party details: leave blank.\nPredictions are classification outputs, not calibrated fill permissions.\n");
    fs::write(out.join("report.txt"), &report)?;
    fs::write(out.join("predictions.tsv"), predictions)?;
    fs::write(out.join("corrections.tsv"), corrections)?;
    print!("{report}");
    Ok(())
}
