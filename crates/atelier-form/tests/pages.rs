//! Moving, removing, adding and extracting pages, then saving and reading
//! the result back.

use std::sync::Arc;

use atelier_form::Session;
use atelier_syntax::{Document, Object};

/// Writes numbered objects (from 1) as a PDF with object 1 as the catalog.
fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Pages whose widths are `widths`, in a two-level tree: the first two
/// sit under an inner node that gives them their height, rotation and
/// resources, so moving them out has to keep those. Each page draws with
/// a font named after its width.
fn pages(widths: &[u32]) -> Vec<u8> {
    let n = widths.len();
    // 1 catalog, 2 root, 3 inner node, then a page and a content stream
    // for each width, then a font.
    let page = |i: usize| 4 + 2 * i;
    let font = 4 + 2 * n;
    let inner: Vec<String> = (0..n.min(2)).map(|i| format!("{} 0 R", page(i))).collect();
    let mut kids = vec!["3 0 R".to_string()];
    kids.extend((2..n).map(|i| format!("{} 0 R", page(i))));
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 100 300] >>",
            kids.join(" ")
        ),
        format!(
            "<< /Type /Pages /Parent 2 0 R /Kids [{}] /Count {} /MediaBox [0 0 100 400] /Rotate 90 \
             /Resources << /Font << /F {font} 0 R >> >> >>",
            inner.join(" "),
            inner.len()
        ),
    ];
    for (i, w) in widths.iter().enumerate() {
        let parent = if i < 2 { 3 } else { 2 };
        objects.push(format!(
            "<< /Type /Page /Parent {parent} 0 R /CropBox [0 0 {w} 200] /Contents {} 0 R >>",
            page(i) + 1
        ));
        let text = format!("BT /F 12 Tf 10 10 Td (page{w}) Tj ET");
        objects.push(format!(
            "<< /Length {} >>\nstream\n{text}\nendstream",
            text.len()
        ));
    }
    objects.push(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    );
    pdf(&objects)
}

fn open(bytes: Vec<u8>) -> Arc<Document> {
    Arc::new(Document::open(bytes).unwrap())
}

/// Each page's crop width, rotation and media height.
fn shape(doc: &Document) -> Vec<(u32, u16, u32)> {
    doc.pages()
        .unwrap()
        .iter()
        .map(|p| (p.crop_box.x1 as u32, p.rotate, p.media_box.y1 as u32))
        .collect()
}

/// Saves the session's document and reads it back.
fn saved(session: &Session) -> Arc<Document> {
    let built = session.build().unwrap();
    open(built.doc.data().to_vec())
}

#[test]
fn moving_pages_keeps_what_they_inherit() {
    let base = open(pages(&[10, 20, 30, 40]));
    let mut s = Session::new(base);
    assert!(!s.is_dirty());
    // The first two to the end.
    let old = s.move_pages(&[0, 1], 4);
    assert_eq!(old, vec![2, 3, 0, 1]);
    assert!(s.is_dirty());
    let doc = saved(&s);
    assert_eq!(
        shape(&doc),
        vec![(30, 0, 300), (40, 0, 300), (10, 90, 400), (20, 90, 400)]
    );
    // The moved pages still find their font.
    let page = &doc.pages().unwrap()[2];
    assert!(page.resources.get(b"Font").is_some());
    // Moving back to the start puts things as they were.
    assert_eq!(s.move_pages(&[2, 3], 0), vec![2, 3, 0, 1]);
    assert!(!s.is_dirty());
}

#[test]
fn turns_follow_their_page() {
    let mut s = Session::new(open(pages(&[10, 20, 30])));
    s.rotate(2, 180);
    s.move_pages(&[2], 0);
    assert_eq!(s.rotation(0), 180);
    assert_eq!(s.rotation(1), 90);
    let doc = saved(&s);
    assert_eq!(shape(&doc)[0], (30, 180, 300));
}

#[test]
fn deleting_pages_keeps_at_least_one() {
    let mut s = Session::new(open(pages(&[10, 20, 30])));
    assert_eq!(s.delete_pages(&[0, 1, 2]), None);
    assert_eq!(s.page_count(), 3);
    assert_eq!(s.delete_pages(&[1]), Some(vec![0, 2]));
    let doc = saved(&s);
    assert_eq!(shape(&doc), vec![(10, 90, 400), (30, 0, 300)]);
    // The removed page is gone from the bytes, not just from the list.
    let text = String::from_utf8_lossy(doc.data());
    assert!(!text.contains("page20") && text.contains("page30"));
    s.revert();
    assert_eq!(s.page_count(), 3);
    assert!(!s.is_dirty());
}

#[test]
fn pages_from_another_file_are_copied_in() {
    let mut s = Session::new(open(pages(&[10, 20])));
    let other = open(pages(&[50, 60, 70]));
    let old = s.insert_file(other, 1).unwrap();
    assert_eq!(old, vec![Some(0), None, None, None, Some(1)]);
    s.rotate(2, 270);
    let doc = saved(&s);
    assert_eq!(
        shape(&doc),
        vec![
            (10, 90, 400),
            (50, 90, 400),
            (60, 270, 400),
            (70, 0, 300),
            (20, 90, 400)
        ]
    );
    // The copied pages draw their own words with their own font.
    for (i, w) in [(1, 50), (3, 70)] {
        let page = &doc.pages().unwrap()[i];
        let contents = doc.get_key(&page.dict, b"Contents").unwrap();
        let Object::Stream(stream) = contents else {
            panic!("no contents")
        };
        let data = doc.decode_stream(&stream).unwrap();
        assert!(String::from_utf8_lossy(&data.data).contains(&format!("page{w}")));
    }
    // The font the first copied page inherited came along.
    assert!(doc.pages().unwrap()[1].resources.get(b"Font").is_some());
}

#[test]
fn extracting_leaves_the_other_pages_out() {
    let mut s = Session::new(open(pages(&[10, 20, 30, 40])));
    s.rotate(3, 90);
    let bytes = s.extract(&[3, 0]).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("page40") && text.contains("page10"));
    assert!(!text.contains("page20") && !text.contains("page30"));
    let doc = open(bytes);
    assert_eq!(shape(&doc), vec![(40, 90, 300), (10, 90, 400)]);
    // Extracting changes nothing in the session.
    assert_eq!(s.page_count(), 4);
}

#[test]
fn fields_follow_their_page() {
    // Two pages; a text field on the second.
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) \
         /DR << /Font << /Helv 6 0 R >> >> >> >>"
            .to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>".into(),
        "<< /FT /Tx /T (name) /Type /Annot /Subtype /Widget /Rect [10 10 150 40] /P 4 0 R >>"
            .into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    let mut s = Session::new(open(pdf(&objects)));
    assert_eq!(s.pages_of(0), vec![1]);
    s.move_pages(&[1], 0);
    assert_eq!(s.pages_of(0), vec![0]);
    s.set(0, atelier_form::Value::Text("Ada".into()));
    let built = s.build().unwrap();
    let form = built.form.unwrap();
    assert_eq!(form.fields[0].widgets[0].page, Some(0));
    assert!(
        form.fields[0]
            .value
            .same_as(&atelier_form::Value::Text("Ada".into()))
    );
}

#[test]
fn saved_review_edits_follow_pages_through_reordering() {
    use atelier_form::{Markup, NewAnnot, Review};
    let mut session = Session::new(open(pages(&[50, 80])));
    let review = Review {
        text: "Before".into(),
        author: "Ada".into(),
        subject: "Amount".into(),
        created: "D:20261101120000Z".into(),
        modified: "D:20261101120000Z".into(),
        state: None,
    };
    session.add_annot(
        0,
        NewAnnot {
            markup: Markup::Note {
                at: atelier_raster::Point::new(30.0, 100.0),
                review: review.clone(),
            },
            color: [1.0, 0.8, 0.2],
        },
    );
    let mut session = Session::new(session.build().unwrap().doc);
    session.set_review(
        0,
        0,
        Review {
            text: "After".into(),
            ..review
        },
    );
    session.move_pages(&[0], 2);
    let doc = session.build().unwrap().doc;
    assert_eq!(
        shape(&doc).iter().map(|s| s.0).collect::<Vec<_>>(),
        [80, 50]
    );
    let page = &doc.pages().unwrap()[1];
    let Object::Array(annots) = doc.get_key(&page.dict, b"Annots").unwrap() else {
        panic!("missing notes")
    };
    assert_eq!(annots.len(), 1);
    let note = doc.resolve_dict(&annots[0]).unwrap();
    let Object::String(text) = doc.get_key(&note, b"Contents").unwrap() else {
        panic!("missing text")
    };
    assert_eq!(text.to_text(), "After");
}
