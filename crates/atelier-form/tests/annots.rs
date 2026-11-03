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
                review: atelier_form::Review {
                    text: "Check this — ✓".into(),
                    author: "Ada Reviewer".into(),
                    subject: "Review".into(),
                    created: "D:20261101120000Z".into(),
                    modified: "D:20261101120000Z".into(),
                    state: None,
                },
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
    for (key, expected) in [
        (b"T".as_slice(), "Ada Reviewer"),
        (b"CreationDate", "D:20261101120000Z"),
        (b"M", "D:20261101120000Z"),
    ] {
        let Object::String(value) = saved.get_key(&note, key).unwrap() else {
            panic!("missing review metadata")
        };
        assert_eq!(value.to_text(), expected);
    }
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

#[test]
fn review_lines_save_standard_subtypes_and_visible_appearances() {
    let base = doc();
    let p = Point::new;
    let quads = vec![[p(20.0, 40.0), p(120.0, 40.0), p(120.0, 60.0), p(20.0, 60.0)]];
    for (strike, subtype, y) in [(false, "Underline", 41), (true, "StrikeOut", 49)] {
        let mut session = Session::new(base.clone());
        session.add_annot(
            0,
            NewAnnot {
                markup: Markup::TextLine {
                    quads: quads.clone(),
                    strike,
                },
                color: [0.8, 0.0, 0.0],
            },
        );
        let saved = Document::open(session.build().unwrap().doc.data().to_vec()).unwrap();
        let page = &saved.pages().unwrap()[0];
        let Object::Array(annots) = saved.get_key(&page.dict, b"Annots").unwrap() else {
            panic!("no annotations")
        };
        let annot = saved.resolve_dict(&annots[0]).unwrap();
        assert_eq!(
            saved
                .get_key(&annot, b"Subtype")
                .unwrap()
                .as_name()
                .unwrap()
                .as_str_lossy(),
            subtype
        );
        assert_eq!(
            saved
                .get_key(&annot, b"QuadPoints")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            8
        );
        let pixel = colour_at(&saved, 70, y);
        assert!(pixel[0] > pixel[1] + 20, "{subtype}: {pixel:?}");
    }
}

#[test]
fn inline_comment_metadata_can_be_edited_after_save() {
    let base = doc();
    let p = Point::new;
    let review = atelier_form::Review {
        text: "Original note".into(),
        author: "Ada".into(),
        subject: "Verify amount".into(),
        created: "D:20261101120000Z".into(),
        modified: "D:20261101120000Z".into(),
        state: None,
    };
    let mut session = Session::new(base);
    session.add_annot(
        0,
        NewAnnot {
            markup: Markup::Comment {
                quads: vec![[p(18.0, 96.0), p(90.0, 96.0), p(90.0, 112.0), p(18.0, 112.0)]],
                review: review.clone(),
            },
            color: [1.0, 0.9, 0.0],
        },
    );
    let saved = session.build().unwrap().doc;
    let mut session = Session::new(saved);
    let edited = atelier_form::Review {
        text: "Checked against source".into(),
        subject: "Verified".into(),
        modified: "D:20261101130000Z".into(),
        ..review.clone()
    };
    session.set_review(0, 0, edited.clone());
    assert!(session.is_dirty());
    let rebuilt = session.build().unwrap().doc;
    let reopened = Document::open(rebuilt.data().to_vec()).unwrap();
    let Object::Array(annots) = reopened
        .get_key(&reopened.pages().unwrap()[0].dict, b"Annots")
        .unwrap()
    else {
        panic!("missing annotation")
    };
    assert_eq!(
        annots.len(),
        1,
        "editing must not create another annotation"
    );
    let annot = reopened.resolve_dict(&annots[0]).unwrap();
    for (key, expected) in [
        (b"Contents".as_slice(), edited.text.as_str()),
        (b"T", "Ada"),
        (b"Subj", "Verified"),
        (b"CreationDate", review.created.as_str()),
        (b"M", edited.modified.as_str()),
    ] {
        let Object::String(value) = reopened.get_key(&annot, key).unwrap() else {
            panic!("missing metadata")
        };
        assert_eq!(value.to_text(), expected);
    }
    assert_eq!(
        reopened
            .get_key(&annot, b"Subtype")
            .unwrap()
            .as_name()
            .unwrap()
            .as_str_lossy(),
        "Highlight"
    );
    assert_eq!(
        reopened
            .get_key(&annot, b"QuadPoints")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let pixel = colour_at(&reopened, 85, 109);
    assert!(pixel[0] > 200 && pixel[2] < 80);
    session.saved(rebuilt);
    assert!(!session.is_dirty());
    session.set_review(0, 0, review);
    assert!(session.is_dirty());
    session.revert();
    assert!(!session.is_dirty());
}

#[test]
fn unsaved_comment_edits_survive_rebuild_without_duplicate_notes() {
    let mut session = Session::new(doc());
    let review = atelier_form::Review {
        text: "Draft".into(),
        author: "Reviewer".into(),
        subject: "Question".into(),
        created: "D:20261101120000Z".into(),
        modified: "D:20261101120000Z".into(),
        state: None,
    };
    session.add_annot(
        0,
        NewAnnot {
            markup: Markup::Note {
                at: Point::new(100.0, 180.0),
                review: review.clone(),
            },
            color: [1.0, 0.8, 0.2],
        },
    );
    assert!(session.set_added_review(
        0,
        atelier_form::Review {
            text: "Updated draft".into(),
            ..review
        }
    ));
    assert!(!session.set_added_review(
        9,
        atelier_form::Review {
            text: String::new(),
            author: String::new(),
            subject: String::new(),
            created: String::new(),
            modified: String::new(),
            state: None,
        }
    ));
    let saved = session.build().unwrap().doc;
    let Object::Array(annots) = saved
        .get_key(&saved.pages().unwrap()[0].dict, b"Annots")
        .unwrap()
    else {
        panic!("missing note")
    };
    assert_eq!(annots.len(), 1);
    let note = saved.resolve_dict(&annots[0]).unwrap();
    let Object::String(text) = saved.get_key(&note, b"Contents").unwrap() else {
        panic!("missing text")
    };
    assert_eq!(text.to_text(), "Updated draft");
}

#[test]
fn saved_annotation_removal_tracks_ordinals_and_undo_restores_it() {
    let mut session = Session::new(doc());
    for x in [20.0, 60.0, 100.0] {
        session.add_annot(
            0,
            NewAnnot {
                markup: Markup::Ink {
                    strokes: vec![vec![Point::new(x, 20.0), Point::new(x + 10.0, 30.0)]],
                    width: 2.0,
                },
                color: [0.0, 0.0, 1.0],
            },
        );
    }
    let saved = session.build().unwrap().doc;
    session.saved(saved);
    session.remove_saved_annot(0, 0);
    assert_eq!(session.original_annot_ordinal(0, 0), Some(1));
    assert_eq!(session.original_annot_ordinal(0, 1), Some(2));
    session.remove_saved_annot(0, 1);
    let built = session.build().unwrap().doc;
    let Object::Array(list) = built
        .get_key(&built.pages().unwrap()[0].dict, b"Annots")
        .unwrap()
    else {
        panic!("no annotations")
    };
    assert_eq!(list.len(), 1);
    let remaining = built.resolve_dict(&list[0]).unwrap();
    let rect = remaining.get(b"Rect").unwrap().as_array().unwrap();
    assert!(rect[0].as_f64().unwrap() > 90.0);
    assert_eq!(session.undo_annot(), Some(0));
    assert_eq!(session.original_annot_ordinal(0, 0), Some(1));
    let restored = session.build().unwrap().doc;
    assert_eq!(
        restored
            .get_key(&restored.pages().unwrap()[0].dict, b"Annots")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(session.undo_annot(), Some(0));
    assert!(!session.is_dirty());
}

#[test]
fn review_decisions_survive_save_reopen_and_reversal() {
    let mut session = Session::new(doc());
    let mut review = atelier_form::Review {
        text: "Check this".into(),
        author: "Ada".into(),
        subject: "Review".into(),
        created: "D:20261102120000Z".into(),
        modified: "D:20261102120000Z".into(),
        state: Some("Accepted".into()),
    };
    session.add_annot(
        0,
        NewAnnot {
            markup: Markup::Note {
                at: Point::new(30.0, 100.0),
                review: review.clone(),
            },
            color: [1.0, 0.8, 0.2],
        },
    );
    review.state = Some("Rejected".into());
    assert!(session.set_added_review(0, review.clone()));
    let added = session.build().unwrap().doc;
    let Object::Array(list) = added
        .get_key(&added.pages().unwrap()[0].dict, b"Annots")
        .unwrap()
    else {
        panic!("missing annotations")
    };
    let annot = added.resolve_dict(&list[0]).unwrap();
    let Object::String(state) = added.get_key(&annot, b"State").unwrap() else {
        panic!("missing state")
    };
    assert_eq!(state.to_text(), "Rejected");
    session = Session::new(added);
    for state in ["Accepted", "Rejected", "Accepted"] {
        review.state = Some(state.into());
        session.set_review(0, 0, review.clone());
        // First build writes the new annotation; subsequent builds edit the saved one.
        let built = session.build().unwrap().doc;
        let reopened = Document::open(built.data().to_vec()).unwrap();
        let Object::Array(annots) = reopened
            .get_key(&reopened.pages().unwrap()[0].dict, b"Annots")
            .unwrap()
        else {
            panic!("missing annotations")
        };
        assert_eq!(annots.len(), 1);
        let annot = reopened.resolve_dict(&annots[0]).unwrap();
        for (key, expected) in [
            (b"State".as_slice(), state),
            (b"StateModel".as_slice(), "Review"),
            (b"Contents".as_slice(), "Check this"),
        ] {
            let Object::String(value) = reopened.get_key(&annot, key).unwrap() else {
                panic!("missing review metadata")
            };
            assert_eq!(value.to_text(), expected);
        }
        session = Session::new(std::sync::Arc::new(reopened));
    }
}
