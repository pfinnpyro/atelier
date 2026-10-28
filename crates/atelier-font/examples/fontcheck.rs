//! Prints each glyph's advance, outline area and bounds, for comparing with
//! another font library. Usage: fontcheck FONT [MAX_GLYPHS]

use atelier_font::FontProgram;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).expect("read font");
    let max: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let font = match FontProgram::parse(&data) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let names = match &font {
        FontProgram::Type1(t) => Some(t),
        _ => None,
    };
    for gid in 0..font.glyph_count().min(max) as u32 {
        let label = names
            .and_then(|t| t.glyph_name(gid))
            .map_or(gid.to_string(), str::to_string);
        let adv = font.advance(gid).unwrap_or(f64::NAN);
        let Some(path) = font.outline(gid) else {
            println!("{label} {adv:.6} none");
            continue;
        };
        let polys = path.flatten(0.00002);
        let mut area = 0.0;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (poly, _) in &polys {
            for (i, p) in poly.iter().enumerate() {
                let q = poly[(i + 1) % poly.len()];
                area += p.x * q.y - q.x * p.y;
                x0 = x0.min(p.x);
                y0 = y0.min(p.y);
                x1 = x1.max(p.x);
                y1 = y1.max(p.y);
            }
        }
        if polys.is_empty() {
            println!("{label} {adv:.6} empty");
        } else {
            println!("{label} {adv:.6} {:.6} {x0:.5} {y0:.5} {x1:.5} {y1:.5}", area / 2.0);
        }
    }
}
