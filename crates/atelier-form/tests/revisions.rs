use atelier_form::{Form, Markup, NewAnnot, Review, RevisionState as State, Session, Value};
use atelier_raster::Point;
use atelier_syntax::{Document, Object};
use std::sync::Arc;

fn document() -> Arc<Document> {
    Arc::new(Document::open(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DR << /Font << /Helv 6 0 R >> >> /DA (/Helv 12 Tf 0 g) >> >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R] >> endobj\n4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >> endobj\n5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (original) /Rect [10 100 180 130] /DA (/Helv 12 Tf 0 g) >> endobj\n6 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n%%EOF".to_vec()).unwrap())
}
fn value(doc: &Document) -> String {
    Form::load(doc).unwrap().unwrap().fields[0].value.display()
}
fn commit(s: &mut Session, doc: &mut Arc<Document>) {
    *doc = s.build_tracked(doc).unwrap().doc;
}
fn decide(s: &mut Session, doc: &mut Arc<Document>, id: u64, state: State) {
    s.decide_revision(doc, id, state, "Grace").unwrap();
    *doc = s.build().unwrap().doc;
}
#[test]
fn editing_sessions_group_changes_and_preserve_original_values() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    s.set_tracking(true, "Ada").unwrap();
    s.begin_revision_session();
    s.set(0, Value::Text("first".into()));
    commit(&mut s, &mut doc);
    s.set(0, Value::Text("second".into()));
    commit(&mut s, &mut doc);
    s.rotate(0, 90);
    commit(&mut s, &mut doc);
    assert_eq!(s.revisions().len(), 1);
    assert_eq!(s.revisions()[0].changes[0].before, "original");
    assert_eq!(s.revisions()[0].changes[0].after, "second");
    s.end_revision_session();
    let reopened = Arc::new(Document::open(doc.data().to_vec()).unwrap());
    s = Session::new(reopened.clone());
    doc = reopened;
    assert!(!s.revision_session_active());
    decide(&mut s, &mut doc, 1, State::Rejected);
    assert_eq!(value(&doc), "original");
    assert_eq!(doc.pages().unwrap()[0].rotate, 0);
}
#[test]
fn text_revisions_reopen_and_reject_without_erasing_dependent_edits() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    s.set_tracking(true, "Ada").unwrap();
    s.set(0, Value::Text("first".into()));
    commit(&mut s, &mut doc);
    s.set(0, Value::Text("second".into()));
    commit(&mut s, &mut doc);
    assert_eq!(s.revisions().len(), 2);
    assert_eq!(s.revisions()[0].changes[0].before, "original");
    assert_eq!(s.revisions()[0].changes[0].after, "first");
    assert_eq!(s.revisions()[0].author, "Ada");
    let reopened = Arc::new(Document::open(doc.data().to_vec()).unwrap());
    s = Session::new(reopened.clone());
    doc = reopened;
    assert!(s.tracking_enabled());
    assert_eq!(s.revisions().len(), 2);
    assert!(
        s.decide_revision(&doc, 1, State::Rejected, "Grace")
            .is_err()
    );
    assert_eq!(value(&doc), "second");
    decide(&mut s, &mut doc, 2, State::Rejected);
    assert_eq!(value(&doc), "first");
    decide(&mut s, &mut doc, 1, State::Rejected);
    assert_eq!(value(&doc), "original");
    decide(&mut s, &mut doc, 1, State::Accepted);
    assert_eq!(value(&doc), "first");
    assert_eq!(s.revisions()[0].events.last().unwrap().author, "Grace");
    s.saved(doc.clone());
    assert!(!s.is_dirty());
    decide(&mut s, &mut doc, 1, State::Undone);
    assert_eq!(value(&doc), "original");
    assert!(s.is_dirty());
    s.revert();
    assert_eq!(value(&s.build().unwrap().doc), "first");
    assert!(!s.is_dirty());
}
#[test]
fn undo_redo_and_new_edit_branches_survive_save_reopen() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    s.set_tracking(true, "Ada").unwrap();
    for text in ["one", "two"] {
        s.set(0, Value::Text(text.into()));
        commit(&mut s, &mut doc);
    }
    decide(&mut s, &mut doc, 2, State::Undone);
    decide(&mut s, &mut doc, 1, State::Undone);
    let reopened = Arc::new(Document::open(doc.data().to_vec()).unwrap());
    s = Session::new(reopened.clone());
    doc = reopened;
    assert_eq!(s.revision_redo(), Some((1, State::Pending)));
    decide(&mut s, &mut doc, 1, State::Pending);
    assert_eq!(value(&doc), "one");
    decide(&mut s, &mut doc, 2, State::Pending);
    assert_eq!(value(&doc), "two");
    decide(&mut s, &mut doc, 2, State::Undone);
    s.set(0, Value::Text("different".into()));
    commit(&mut s, &mut doc);
    assert_eq!(s.revisions()[1].state, State::Superseded);
    assert_eq!(s.revisions()[2].id, 3);
    assert!(s.revision_redo().is_none());
    assert!(
        s.decide_revision(&doc, 2, State::Accepted, "Grace")
            .is_err()
    );
    assert_eq!(value(&doc), "different");
}
#[test]
fn page_rotation_order_insertion_and_deletion_are_reversible_after_save() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    s.set_tracking(true, "Ada").unwrap();
    s.rotate(0, 90);
    commit(&mut s, &mut doc);
    s.move_pages(&[0], 2);
    commit(&mut s, &mut doc);
    s.insert_file(document(), 1).unwrap();
    commit(&mut s, &mut doc);
    s.delete_pages(&[0]).unwrap();
    commit(&mut s, &mut doc);
    assert_eq!(doc.pages().unwrap().len(), 3);
    let reopened = Arc::new(Document::open(doc.data().to_vec()).unwrap());
    s = Session::new(reopened.clone());
    doc = reopened;
    for id in (1..=4).rev() {
        decide(&mut s, &mut doc, id, State::Rejected);
    }
    assert_eq!(doc.pages().unwrap().len(), 2);
    assert_eq!(doc.pages().unwrap()[0].media_box.width(), 200.0);
    assert_eq!(doc.pages().unwrap()[0].rotate, 0);
    assert_eq!(value(&doc), "original");
}
#[test]
fn annotations_and_comment_edits_restore_the_original_appearance_and_text() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    s.set_tracking(true, "Ada").unwrap();
    let mut review = Review {
        text: "Original comment".into(),
        author: "Ada".into(),
        subject: String::new(),
        created: "D:20261102120000Z".into(),
        modified: "D:20261102120000Z".into(),
        state: None,
    };
    s.add_annot(
        1,
        NewAnnot {
            markup: Markup::Note {
                at: Point::new(30.0, 70.0),
                review: review.clone(),
            },
            color: [1.0, 0.8, 0.2],
        },
    );
    commit(&mut s, &mut doc);
    review.text = "Updated comment".into();
    s.set_added_review(0, review);
    commit(&mut s, &mut doc);
    decide(&mut s, &mut doc, 2, State::Rejected);
    let Object::Array(annots) = doc
        .get_key(&doc.pages().unwrap()[1].dict, b"Annots")
        .unwrap()
    else {
        panic!("missing comment")
    };
    let d = doc.resolve_dict(&annots[0]).unwrap();
    let Object::String(text) = doc.get_key(&d, b"Contents").unwrap() else {
        panic!("missing text")
    };
    assert_eq!(text.to_text(), "Original comment");
    decide(&mut s, &mut doc, 1, State::Rejected);
    assert!(matches!(
        doc.get_key(&doc.pages().unwrap()[1].dict, b"Annots")
            .unwrap(),
        Object::Null
    ));
}
#[test]
fn untracked_edits_block_unsafe_rejection_and_noop_edits_create_no_revision() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    s.set_tracking(true, "Ada").unwrap();
    s.set(0, Value::Text("original".into()));
    commit(&mut s, &mut doc);
    assert!(s.revisions().is_empty());
    s.set(0, Value::Text("tracked".into()));
    commit(&mut s, &mut doc);
    s.set_tracking(false, "Ada").unwrap();
    s.set(0, Value::Text("later untracked".into()));
    commit(&mut s, &mut doc);
    assert!(
        s.decide_revision(&doc, 1, State::Rejected, "Grace")
            .is_err()
    );
    assert_eq!(value(&doc), "later untracked");
    assert_eq!(s.revisions().len(), 1);
}
#[test]
fn encrypted_history_and_rejection_keep_the_password_protection() {
    let path = format!(
        "{}/tests/data/encrypted-form.pdf",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut doc =
        Arc::new(Document::open_with_password(std::fs::read(path).unwrap(), b"user").unwrap());
    let mut s = Session::new(doc.clone());
    let field = s
        .original()
        .unwrap()
        .fields
        .iter()
        .position(|f| f.name == "name")
        .unwrap();
    let original = Form::load(&doc).unwrap().unwrap().fields[field]
        .value
        .clone();
    s.set_tracking(true, "Ada").unwrap();
    s.set(field, Value::Text("Tracked encrypted value".into()));
    commit(&mut s, &mut doc);
    assert!(
        !doc.data()
            .windows(23)
            .any(|s| s == b"Tracked encrypted value")
    );
    assert!(Document::open(doc.data().to_vec()).is_err());
    let reopened = Arc::new(Document::open_with_password(doc.data().to_vec(), b"user").unwrap());
    s = Session::new(reopened.clone());
    doc = reopened;
    decide(&mut s, &mut doc, 1, State::Rejected);
    assert_eq!(
        Form::load(&doc).unwrap().unwrap().fields[field].value,
        original
    );
    assert!(Document::open(doc.data().to_vec()).is_err());
}

#[test]
fn author_only_comment_edits_are_tracked_and_page_extracts_start_new_history() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    s.set_tracking(true, "Ada").unwrap();
    let review = Review {
        text: "Same text".into(),
        author: "Ada".into(),
        subject: "Subject".into(),
        created: "D:20261102120000Z".into(),
        modified: "D:20261102120000Z".into(),
        state: None,
    };
    s.add_annot(
        1,
        NewAnnot {
            markup: Markup::Note {
                at: Point::new(30.0, 70.0),
                review: review.clone(),
            },
            color: [1.0, 0.8, 0.2],
        },
    );
    commit(&mut s, &mut doc);
    s.set_added_review(
        0,
        Review {
            author: "Grace".into(),
            ..review
        },
    );
    commit(&mut s, &mut doc);
    assert_eq!(s.revisions().len(), 2);
    assert!(s.revisions()[1].changes[0].before.contains("Ada"));
    assert!(s.revisions()[1].changes[0].after.contains("Grace"));
    let extracted = Document::open(s.extract(&[1]).unwrap()).unwrap();
    assert!(
        extracted
            .catalog()
            .unwrap()
            .get(b"AtelierRevisions")
            .is_none()
    );
    assert_eq!(extracted.pages().unwrap().len(), 1);
}

fn field_appearance(doc: &Document) -> String {
    let form = Form::load(doc).unwrap().unwrap();
    let Object::Dict(widget) = doc.get(form.fields[0].widgets[0].obj).unwrap() else {
        panic!()
    };
    let ap = doc.resolve_dict(widget.get(b"AP").unwrap()).unwrap();
    let Object::Stream(stream) = doc.resolve(ap.get(b"N").unwrap()).unwrap() else {
        panic!()
    };
    String::from_utf8(doc.decode_stream(&stream).unwrap().data).unwrap()
}

#[test]
fn formatting_spacing_and_bounds_survive_reopen_and_reject_as_one_session() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    s.set_tracking(true, "Ada").unwrap();
    s.begin_revision_session();
    let mut style = s.field_style(0).unwrap();
    style.appearance.size = 18.0;
    style.appearance.color = "0 0 1 rg".into();
    style.appearance.character_spacing = 2.0;
    style.appearance.word_spacing = 4.0;
    style.appearance.leading = 25.0;
    style.alignment = 2;
    s.set_field_style(0, style.clone()).unwrap();
    commit(&mut s, &mut doc);
    let mut rect = s.widget_bounds(0, 0).unwrap();
    rect.x0 = 20.0;
    rect.x1 = 190.0;
    rect.y1 = 160.0;
    s.set_widget_bounds(0, 0, rect).unwrap();
    commit(&mut s, &mut doc);
    style.appearance.size = 20.0;
    s.set_field_style(0, style.clone()).unwrap();
    commit(&mut s, &mut doc);
    assert_eq!(s.revisions().len(), 1);
    let size = s.revisions()[0]
        .changes
        .iter()
        .find(|c| c.target.ends_with(" / font-size"))
        .unwrap();
    assert_eq!(size.before, "12");
    assert_eq!(size.after, "20");
    assert!(
        s.revisions()[0]
            .changes
            .iter()
            .any(|c| c.target.ends_with(" / height"))
    );
    let appearance = field_appearance(&doc);
    assert!(appearance.contains("20 Tf"));
    assert!(appearance.contains("2 Tc 4 Tw"));
    assert!(appearance.contains("0 0 1 rg"));
    let reopened = Arc::new(Document::open(doc.data().to_vec()).unwrap());
    s = Session::new(reopened.clone());
    doc = reopened;
    assert_eq!(s.field_style(0).unwrap(), style);
    assert_eq!(s.widget_bounds(0, 0).unwrap(), rect);
    assert_eq!(value(&doc), "original");
    decide(&mut s, &mut doc, 1, State::Rejected);
    assert_eq!(s.field_style(0).unwrap().appearance.size, 12.0);
    assert_eq!(s.widget_bounds(0, 0).unwrap().y1, 130.0);
    decide(&mut s, &mut doc, 1, State::Accepted);
    assert_eq!(s.field_style(0).unwrap(), style);
    assert_eq!(s.widget_bounds(0, 0).unwrap(), rect);
}

#[test]
fn invalid_style_and_bounds_are_atomic_and_identical_edits_are_clean() {
    let doc = document();
    let mut s = Session::new(doc);
    let original = s.field_style(0).unwrap();
    s.set_field_style(0, original.clone()).unwrap();
    s.set_widget_bounds(0, 0, s.widget_bounds(0, 0).unwrap())
        .unwrap();
    assert!(!s.is_dirty());
    for value in [f64::NAN, f64::INFINITY, -1.0] {
        let mut style = original.clone();
        style.appearance.size = value;
        assert!(s.set_field_style(0, style).is_err());
    }
    let mut style = original.clone();
    style.appearance.color = "0 g 0 0 m".into();
    assert!(s.set_field_style(0, style).is_err());
    let mut style = original.clone();
    style.appearance.font = Some(b"MissingFont".to_vec());
    assert!(s.set_field_style(0, style).is_err());
    let mut rect = s.widget_bounds(0, 0).unwrap();
    rect.x1 = rect.x0;
    assert!(s.set_widget_bounds(0, 0, rect).is_err());
    assert!(!s.is_dirty());
    assert_eq!(s.field_style(0).unwrap(), original);
}

#[test]
fn multiline_spacing_changes_baselines_and_character_spacing_changes_wrap() {
    let source = String::from_utf8(document().data().to_vec())
        .unwrap()
        .replace("/FT /Tx", "/FT /Tx /Ff 4096");
    let mut doc = Arc::new(Document::open(source.into_bytes()).unwrap());
    let mut s = Session::new(doc.clone());
    s.set(0, Value::Text("hello world hello world".into()));
    let mut style = s.field_style(0).unwrap();
    style.appearance.size = 12.0;
    style.appearance.leading = 20.0;
    s.set_field_style(0, style.clone()).unwrap();
    commit(&mut s, &mut doc);
    let normal = field_appearance(&doc);
    let lines = |ap: &str| ap.lines().filter(|l| l.contains(" Tm ")).count();
    style.appearance.character_spacing = 8.0;
    s.set_field_style(0, style).unwrap();
    commit(&mut s, &mut doc);
    let spaced = field_appearance(&doc);
    assert!(lines(&spaced) > lines(&normal));
    let ys: Vec<f64> = spaced
        .lines()
        .filter(|l| l.contains(" Tm "))
        .map(|l| l.split_whitespace().nth(5).unwrap().parse().unwrap())
        .collect();
    assert!((ys[0] - ys[1] - 20.0).abs() < 0.001);
}

#[test]
fn styled_resized_widget_renders_color_in_its_new_bounds() {
    let mut doc = document();
    let mut s = Session::new(doc.clone());
    let mut style = s.field_style(0).unwrap();
    style.appearance.size = 20.0;
    style.appearance.color = "0 0 1 rg".into();
    s.set_field_style(0, style).unwrap();
    let mut bounds = s.widget_bounds(0, 0).unwrap();
    bounds.y0 = 30.0;
    bounds.y1 = 65.0;
    s.set_widget_bounds(0, 0, bounds).unwrap();
    commit(&mut s, &mut doc);
    let image = atelier_render::render_page(
        &doc,
        &doc.pages().unwrap()[0],
        &atelier_render::RenderOptions {
            scale: 1.0,
            ..Default::default()
        },
    )
    .unwrap()
    .pixmap;
    let mut blue = 0;
    let mut old_ink = 0;
    for y in 0..200 {
        for x in 0..200 {
            let px = image.pixel(x, 199 - y);
            if px[2] > 150 && px[0] < 100 && (30..65).contains(&y) {
                blue += 1;
            }
            if px[0] < 100 && (100..130).contains(&y) {
                old_ink += 1;
            }
        }
    }
    assert!(blue > 50, "expected blue text in relocated widget");
    assert_eq!(old_ink, 0, "old widget location must be empty");
}
