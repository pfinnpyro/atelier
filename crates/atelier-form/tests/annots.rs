//! Highlights, comments and ink: added, saved, read back and drawn.

use std::sync::Arc;

use atelier_form::{Markup, NewAnnot, Session};
use atelier_raster::Point;
use atelier_render::{RenderOptions, render_page};
use atelier_syntax::{Document, Object};

/// A one-page, 200 x 200 point document with some text.
fn doc() -> Arc<Document> {
    let content = "BT /F 12 Tf 20 100 Td (Hello there) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
         /Resources << /Font << /F 5 0 R >> >> >>"
            .into(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f\r\n");
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    Arc::new(Document::open(out).unwrap())
}

/// The colour of a point given in user space.
fn colour_at(doc: &Document, x: usize, y: usize) -> [u8; 4] {
    let page = &doc.pages().unwrap()[0];
    let options = RenderOptions {
        scale: 1.0,
        ..RenderOptions::default()
    };
    render_page(doc, page, &options)
        .unwrap()
        .pixmap
        .pixel(x, 199 - y)
}

#[test]
fn annotations_are_saved_and_drawn() {
    let base = doc();
    let mut s = Session::new(base.clone());
    let p = Point::new;
    s.add_annot(
        0,
        NewAnnot {
            markup: Markup::Highlight {
                quads: vec![[p(18.0, 96.0), p(90.0, 96.0), p(90.0, 112.0), p(18.0, 112.0)]],
            },
            color: [1.0, 0.9, 0.0],
        },
    );
    s.add_annot(
        0,
        NewAnnot {
            markup: Markup::Note {
                at: p(150.0, 180.0),
                text: "Check this — ✓".into(),
            },
            color: [1.0, 0.8, 0.2],
        },
    );
    s.add_annot(
        0,
        NewAnnot {
            markup: Markup::Ink {
                strokes: vec![vec![p(20.0, 40.0), p(120.0, 40.0)]],
                width: 4.0,
            },
            color: [0.0, 0.0, 0.8],
        },
    );
    assert!(s.is_dirty());
    let saved = Document::open(s.build().unwrap().doc.data().to_vec()).unwrap();
    let page = &saved.pages().unwrap()[0];
    let Ok(Object::Array(annots)) = saved.get_key(&page.dict, b"Annots") else {
        panic!("no annotations");
    };
    let kinds: Vec<String> = annots
        .iter()
        .map(|a| {
            let d = saved.resolve_dict(a).unwrap();
            let k = saved.get_key(&d, b"Subtype").unwrap();
            String::from_utf8_lossy(&k.as_name().unwrap().0).into_owned()
        })
        .collect();
    assert_eq!(kinds, ["Highlight", "Text", "Ink"]);
    let note = saved.resolve_dict(&annots[1]).unwrap();
    let Ok(Object::String(text)) = saved.get_key(&note, b"Contents") else {
        panic!("no comment text");
    };
    assert_eq!(text.to_text(), "Check this — ✓");
    // The highlight is yellow over white, the ink blue, the note filled.
    let hl = colour_at(&saved, 85, 109);
    assert!(hl[0] > 200 && hl[1] > 180 && hl[2] < 80, "{hl:?}");
    let ink = colour_at(&saved, 70, 40);
    assert!(ink[2] > 150 && ink[0] < 60, "{ink:?}");
    let note = colour_at(&saved, 153, 163);
    assert!(note[0] > 200 && note[2] < 120, "{note:?}");
    // Undo takes the newest back.
    assert_eq!(s.undo_annot(), Some(0));
    assert_eq!(s.added_annots().len(), 2);
}

#[test]
fn selections_become_one_box_per_line() {
    let base = doc();
    let text = atelier_render::extract_text(&base, &base.pages().unwrap()[0]).unwrap();
    let quads = atelier_form::line_quads(&text.glyphs);
    assert_eq!(quads.len(), 1);
    assert!(quads[0][1].x > 70.0 && quads[0][0].x < 25.0, "{quads:?}");
}
