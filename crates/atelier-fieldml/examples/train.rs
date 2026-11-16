//! Trains the Personal model and says how it does on forms it has not
//! seen: `cargo run --release -p atelier-fieldml --example train [out]`.

use atelier_fieldml::{Budget, Model};

fn main() {
    let budget = Budget::PERSONAL;
    let start = std::time::Instant::now();
    let model = Model::train(budget, 1);
    let (share, wrong) = model.evaluate(2000, 99);
    println!(
        "{}: trained in {:.1} s, {:.2}% of fields right on unseen forms, {} bytes",
        budget.name,
        start.elapsed().as_secs_f32(),
        share * 100.0,
        model.to_bytes().len()
    );
    for (label, want, said) in wrong.iter().take(25) {
        println!("  {label:?}: wanted {want:?}, said {said:?}");
    }
    if let Some(path) = std::env::args().nth(1) {
        std::fs::write(&path, model.to_bytes()).expect("write the model");
        println!("wrote {path}");
    }
}
