//! Prints what the layout reader made of reviewed forms:
//! `cargo run -p atelier-fieldml --example layout_debug -- DATA [FORM...]`.
use atelier_fieldml::analysis;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let data = std::path::PathBuf::from(&args[0]);
    let mut forms = analysis::parse(&fs::read_to_string(data.join("v4/corpus.tsv"))?)?;
    forms.extend(analysis::parse(&fs::read_to_string(
        data.join("v4/holdout-corpus.tsv"),
    )?)?);
    analysis::attach_layout(&fs::read_to_string(data.join("v4/layout.tsv"))?, &mut forms)?;
    for form in forms
        .iter()
        .filter(|f| args.len() < 2 || args[1..].contains(&f.id))
    {
        println!("=== {}", form.id);
        print!("{}", analysis::describe_layout(&form.document(true)));
    }
    Ok(())
}
