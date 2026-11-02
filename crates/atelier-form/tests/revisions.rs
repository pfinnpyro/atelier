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
