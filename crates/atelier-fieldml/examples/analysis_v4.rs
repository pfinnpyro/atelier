//! Label analysis (v4): kind / group / role, from text and page layout.
//!
//!     cargo run --release -p atelier-fieldml --example analysis_v4 -- dev DATA OUT
//!     cargo run --release -p atelier-fieldml --example analysis_v4 -- holdout DATA OUT HOLDOUT.tsv
//!     cargo run --release -p atelier-fieldml --example analysis_v4 -- ship DATA OUT
//!
//! `dev` is leave-one-form-out over every reviewed form: `v4/corpus.tsv` and
//! the spent first holdout `v4/holdout-corpus.tsv`. Every form is scored by
//! a model that never saw it. `holdout` trains on all of them and scores a
//! fresh corpus file once; do not tune on what it prints. `ship` trains on
//! every reviewed form, the spent v5 holdout included, with the defaults,
//! and writes the bundled model (`data/analysis.bin`, see
//! `AnalysisModel::bundled`).
//!
//! Layout comes from `v4/layout.tsv` (and `<HOLDOUT>-layout.tsv` beside a
//! holdout file). `DECODE=vote`, `EPOCHS=n`, `SYNTHETIC=n`, `KIND_DECAY=x`,
//! `ROLE_DECAY=x`, `BLOCK_WEIGHT=x`, `JOIN=x`, `CUT=x`, `SEED=n`, `MEMBERS=n` and `NO_LAYOUT=1`
//! override the defaults for development comparisons only.
use atelier_fieldml::analysis::{
    self, Analysis, AnalysisModel, Decode, Form, KINDS, ROLES, Score, Settings,
};
use std::{fmt::Write, fs, path::PathBuf};

type Scored<'a> = Vec<(&'a Form, [Vec<Analysis>; 2])>;

fn report(scored: &Scored, title: &str) -> Result<(String, String), std::fmt::Error> {
    let mut text = format!("{title}\n");
    let mut rows = String::from(
        "# track\tform\tfield\tlabel\tsection\tkind\tsaid_kind\tkind_p\trole\tsaid_role\trole_p\tgroup\tsaid_group\n",
    );
    for (t, track) in ["gold", "extracted"].into_iter().enumerate() {
        let mut total = Score::default();
        for (form, said) in scored {
            let s = Score::of(form, &said[t]);
            writeln!(text, "{track} {:<20} {}", form.id, s.line())?;
            total.add(&s);
            for (f, a) in form.fields.iter().zip(&said[t]) {
                let input = if t == 0 { &f.gold } else { &f.extracted };
                writeln!(
                    rows,
                    "{track}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.3}\t{}\t{}\t{:.3}\t{}\t{}",
                    form.id,
                    f.id,
                    input.label,
                    input.section,
                    KINDS[f.kind],
                    a.kind,
                    a.kind_confidence,
                    ROLES[f.role],
                    a.role,
                    a.role_confidence,
                    f.group,
                    a.group
                )?;
            }
        }
        writeln!(text, "{track} {:<20} {}\n", "TOTAL", total.line())?;
    }
    Ok((text, rows))
}

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn load(corpus: &PathBuf, layout: &PathBuf) -> Result<Vec<Form>, Box<dyn std::error::Error>> {
    let mut forms = analysis::parse(&fs::read_to_string(corpus)?)?;
    if std::env::var_os("NO_LAYOUT").is_none() && layout.exists() {
        analysis::attach_layout(&fs::read_to_string(layout)?, &mut forms)?;
    }
    Ok(forms)
}

fn both(model: &AnalysisModel, form: &Form) -> [Vec<Analysis>; 2] {
    [
        model.analyze(&form.document(false)),
        model.analyze(&form.document(true)),
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str);
    if args.len() < 4
        || !matches!(mode, Some("dev" | "holdout" | "ship"))
        || (mode == Some("holdout") && args.len() < 5)
    {
        return Err("dev DATA OUT | holdout DATA OUT HOLDOUT.tsv | ship DATA OUT".into());
    }
    let data = PathBuf::from(&args[2]);
    let out = PathBuf::from(&args[3]);
    fs::create_dir_all(&out)?;
    let defaults = Settings::default();
    let settings = Settings {
        epochs: env("EPOCHS", defaults.epochs),
        synthetic: env("SYNTHETIC", defaults.synthetic),
        kind_decay: env("KIND_DECAY", defaults.kind_decay),
        role_decay: env("ROLE_DECAY", defaults.role_decay),
        block_weight: env("BLOCK_WEIGHT", defaults.block_weight),
        join: env("JOIN", defaults.join),
        cut: env("CUT", defaults.cut),
        seed: env("SEED", defaults.seed),
        members: env("MEMBERS", defaults.members),
        ..defaults
    };
    let decode = match std::env::var("DECODE").as_deref() {
        Ok("vote") => Decode::Vote,
        _ => Decode::Subject,
    };
    let layout = data.join("v4/layout.tsv");
    let mut forms = load(&data.join("v4/corpus.tsv"), &layout)?;
    forms.extend(load(&data.join("v4/holdout-corpus.tsv"), &layout)?);
    if mode == Some("ship") {
        let v5 = data.join("v5/holdout-corpus.tsv");
        forms.extend(load(&v5, &data.join("v5/holdout-corpus-layout.tsv"))?);
        let model = AnalysisModel::train(&forms, Settings::default());
        let bytes = model.to_bytes();
        fs::write(out.join("analysis.bin"), &bytes)?;
        let model = AnalysisModel::from_bytes(&bytes).ok_or("the written model reads back")?;
        let mut total = Score::default();
        let timer = std::time::Instant::now();
        for form in &forms {
            total.add(&Score::of(form, &model.analyze(&form.document(false))));
        }
        let ms = timer.elapsed().as_secs_f64() * 1e3;
        let fields: usize = forms.iter().map(|f| f.fields.len()).sum();
        println!(
            "trained on {} forms; {} bytes; {:.2} ms per field; on its own training forms: {}",
            forms.len(),
            bytes.len(),
            ms / fields as f64,
            total.line()
        );
        return Ok(());
    }
    let laid = forms.iter().filter(|f| !f.pages.is_empty()).count();
    let start = std::time::Instant::now();
    let holdout;
    let mut scored: Scored = Vec::new();
    let title = if mode == Some("dev") {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let mut results: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    let forms = &forms;
                    s.spawn(move || {
                        let mut mine = Vec::new();
                        for n in (t..forms.len()).step_by(threads) {
                            let train: Vec<_> = forms
                                .iter()
                                .enumerate()
                                .filter(|(m, _)| *m != n)
                                .map(|(_, f)| f.clone())
                                .collect();
                            let mut model = AnalysisModel::train(&train, settings);
                            model.decode = decode;
                            mine.push((n, both(&model, &forms[n])));
                        }
                        mine
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });
        results.sort_by_key(|r| r.0);
        for (n, said) in results {
            scored.push((&forms[n], said));
        }
        format!(
            "v4 development, leave one form out over {} forms ({laid} with layout); {settings:?}; {decode:?}",
            forms.len()
        )
    } else {
        let file = PathBuf::from(&args[4]);
        let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
        holdout = load(&file, &file.with_file_name(format!("{stem}-layout.tsv")))?;
        let mut model = AnalysisModel::train(&forms, settings);
        model.decode = decode;
        for form in &holdout {
            scored.push((form, both(&model, form)));
        }
        let bytes = model.to_bytes();
        fs::write(out.join("analysis.bin"), &bytes)?;
        let timer = std::time::Instant::now();
        for form in &holdout {
            model.analyze(&form.document(true));
        }
        let fields: usize = holdout.iter().map(|f| f.fields.len()).sum();
        eprintln!(
            "model {} bytes; analysis of {} fields took {:.1} ms",
            bytes.len(),
            fields,
            timer.elapsed().as_secs_f64() * 1e3
        );
        format!(
            "v4 HOLDOUT {} (trained on all {} development forms, run once); {settings:?}; {decode:?}",
            file.display(),
            forms.len()
        )
    };
    let (text, rows) = report(&scored, &title)?;
    fs::write(out.join("report.txt"), &text)?;
    fs::write(out.join("predictions.tsv"), rows)?;
    print!("{text}");
    eprintln!("{:.1} s", start.elapsed().as_secs_f32());
    Ok(())
}
