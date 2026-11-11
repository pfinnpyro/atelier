//! Finds the blanks of a printed form, makes fields of them, saves,
//! reopens and fills them in.

use std::sync::Arc;

use atelier_form::{
    FieldKind, Form, NewField, NewFieldKind, Session, Source, Value, detect_page, flags,
};
use atelier_render::{RenderOptions, extract_text, render_page};
use atelier_syntax::{Document, Object, Rect};

/// A file whose pages draw `contents`, each 400 x 300 points, set in
/// Helvetica as /F1. `catalog` is added to the catalog, and `extra`
/// objects follow the pages.
fn build(contents: &[&str], catalog: &str, extra: &[&str]) -> Vec<u8> {
    let n = contents.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    let mut objects: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    for (i, c) in contents.iter().enumerate() {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        objects.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
    }
    objects.extend(extra.iter().map(|s| s.to_string()));
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

/// A printed form: a name to write on a line, a boxed comment, a square
/// to tick and a small table of amounts.
const FORM: &str = "\
BT /F1 10 Tf 40 262 Td (Name:) Tj ET
0.5 w 80 260 m 300 260 l S
BT /F1 10 Tf 60 232 Td (I agree) Tj ET
40 230 10 10 re S
0.9 0.9 0.6 rg 40 150 300 60 re f
0 g
BT /F1 10 Tf 44 142 Td (Comments) Tj ET
40 40 m 340 40 l S 40 60 m 340 60 l S 40 80 m 340 80 l S 40 100 m 340 100 l S
40 40 m 40 100 l S 140 40 m 140 100 l S 240 40 m 240 100 l S 340 40 m 340 100 l S
BT /F1 10 Tf 44 86 Td (Item) Tj 100 0 Td (Net) Tj 100 0 Td (Tax) Tj ET
BT /F1 10 Tf 44 66 Td (Rent) Tj 0 -20 Td (Fuel) Tj ET";

fn open(bytes: Vec<u8>) -> Arc<Document> {
    Arc::new(Document::open(bytes).unwrap())
}

fn detect(doc: &Document, page: usize, taken: &[Rect]) -> atelier_form::Detected {
    let p = &doc.pages().unwrap()[page];
    let text = extract_text(doc, p).unwrap();
    detect_page(doc, p, &text, taken)
}

/// Dark pixels inside a user-space box of a 400 x 300 page drawn at 72 dpi.
fn ink(doc: &Document, page: usize, r: &Rect) -> usize {
    let options = RenderOptions {
        scale: 1.0,
        ..RenderOptions::default()
    };
    let p = render_page(doc, &doc.pages().unwrap()[page], &options)
        .unwrap()
        .pixmap;
    let mut n = 0;
    for y in r.y0.ceil() as usize..r.y1.floor() as usize {
        for x in r.x0.ceil() as usize..r.x1.floor() as usize {
            if p.pixel(x, 299 - y)[0] < 128 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn finds_the_blanks_of_a_printed_form() {
    let doc = open(build(&[FORM], "", &[]));
    let found = detect(&doc, 0, &[]);
    let got: Vec<(&str, NewFieldKind, Source)> = found
        .fields
        .iter()
        .map(|s| (s.field.name.as_str(), s.field.kind, s.source))
        .collect();
    use NewFieldKind::{Checkbox, Multiline, Text};
    assert_eq!(
        got,
        [
            ("Name", Text, Source::Underline),
            ("I agree", Checkbox, Source::CheckBox),
            ("Comments", Multiline, Source::Box),
            ("Rent Net", Text, Source::TableCell),
            ("Rent Tax", Text, Source::TableCell),
            ("Fuel Net", Text, Source::TableCell),
            ("Fuel Tax", Text, Source::TableCell),
        ]
    );
    assert_eq!(found.tables.len(), 1);
    let table = &found.tables[0];
    assert_eq!((table.rows, table.cols), (3, 3));
    assert_eq!(table.to_tsv(), "Item\tNet\tTax\nRent\t\t\nFuel\t\t");
    // The line to write the name on, and the room over it.
    let name = found.fields[0].field.rect;
    assert_eq!((name.x0, name.x1, name.y0), (80.0, 300.0, 260.5));
    // The tinted box, inside its edges.
    let comments = found.fields[2].field.rect;
    assert_eq!(
        (comments.x0, comments.y0, comments.x1, comments.y1),
        (41.0, 151.0, 339.0, 209.0)
    );
}

#[test]
fn suggested_fields_are_saved_and_can_be_filled() {
    let doc = open(build(&[FORM], "", &[]));
    let found = detect(&doc, 0, &[]);
    let mut s = Session::new(doc.clone());
    assert!(s.original().is_none());
    for f in &found.fields {
        s.add_field(0, f.field.clone()).unwrap();
    }
    assert!(s.is_dirty());
    assert_eq!(s.added_field_count(), 7);
    assert!(s.is_added_field(0) && s.is_added_field(6) && !s.is_added_field(7));
    assert_eq!(s.pages_of(3), [0]);

    // Filled in before saving, like any field.
    s.set(0, Value::Text("Ada Lovelace".into()));
    s.set(1, Value::State("Yes".into()));
    s.set(2, Value::Text("First line\nSecond line".into()));
    let built = s.build().unwrap();
    assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    let form = built.form.as_ref().unwrap();
    // What the session listed is what was written.
    let listed = s.original().unwrap();
    for (a, b) in listed.fields.iter().zip(&form.fields) {
        assert_eq!((a.obj, &a.name), (b.obj, &b.name));
    }

    let reopened = open(built.doc.data().to_vec());
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(form.fields.len(), 7);
    assert_eq!(form.fields[0].name, "Name");
    assert_eq!(form.fields[0].kind, FieldKind::Text);
    assert_eq!(form.fields[0].tooltip.as_deref(), Some("Name:"));
    assert_eq!(form.fields[0].value, Value::Text("Ada Lovelace".into()));
    assert_eq!(form.fields[1].kind, FieldKind::Checkbox);
    assert_eq!(form.fields[1].value, Value::State("Yes".into()));
    assert_eq!(form.fields[1].widgets[0].on_state.as_deref(), Some("Yes"));
    assert!(form.fields[2].has_flag(flags::MULTILINE));
    for f in &form.fields {
        assert_eq!(f.widgets.len(), 1);
        assert_eq!(f.widgets[0].page, Some(0), "{}", f.name);
        assert!(f.widgets[0].annot_index.is_some());
    }
    // Tab order follows the page, top to bottom.
    assert_eq!(form.tab_order(&reopened).len(), 7);
    // The form has what viewers need to draw new values.
    let catalog = reopened.catalog().unwrap();
    let acro = reopened
        .resolve_dict(catalog.get(b"AcroForm").unwrap())
        .unwrap();
    assert!(matches!(
        reopened.get_key(&acro, b"DA"),
        Ok(Object::String(_))
    ));
    let dr = reopened.resolve_dict(acro.get(b"DR").unwrap()).unwrap();
    let fonts = reopened.resolve_dict(dr.get(b"Font").unwrap()).unwrap();
    assert!(fonts.contains_key(b"Helv"));

    // The values show on the page: the name over its line, a tick in the
    // square, and nothing yet in an empty cell.
    let name = &form.fields[0].widgets[0].rect;
    let tick = &form.fields[1].widgets[0].rect;
    let cell = &form.fields[3].widgets[0].rect;
    assert_eq!(ink(&doc, 0, name), 0);
    assert!(ink(&reopened, 0, name) > 40);
    let (ticked, empty) = (ink(&reopened, 0, tick), ink(&doc, 0, tick));
    assert!(ticked > empty + 3, "{ticked} {empty}");
    assert_eq!(ink(&reopened, 0, cell), 0);

    // Saved, the fields are the file's own; detection leaves them be.
    let mut s = Session::new(reopened.clone());
    assert!(!s.is_dirty() && !s.is_added_field(0));
    let taken: Vec<Rect> = form
        .fields
        .iter()
        .flat_map(|f| f.widgets.iter().map(|w| w.rect))
        .collect();
    assert!(detect(&reopened, 0, &taken).fields.is_empty());
    s.set(3, Value::Text("120.00".into()));
    let doc = s.build().unwrap().doc;
    assert!(ink(&doc, 0, cell) > 20);
}

#[test]
fn added_fields_can_be_renamed_and_removed_until_saved() {
    let doc = open(build(&[FORM], "", &[]));
    let mut s = Session::new(doc.clone());
    let field = |name: &str, y: f64| NewField {
        name: name.into(),
        kind: NewFieldKind::Text,
        rect: Rect {
            x0: 50.0,
            y0: y,
            x1: 150.0,
            y1: y + 14.0,
        },
        tooltip: None,
        mark: None,
        font_size: 0.0,
    };
    assert_eq!(s.add_field(0, field("a", 200.0)).unwrap(), 0);
    assert_eq!(s.add_field(0, field("b.c", 180.0)).unwrap(), 1);
    // Names are unique, and have no dots.
    assert_eq!(s.add_field(0, field("a", 160.0)).unwrap(), 2);
    let names = |s: &Session| -> Vec<String> {
        s.original()
            .map(|f| f.fields.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default()
    };
    assert_eq!(names(&s), ["a", "b c", "a 2"]);
    assert_eq!(s.rename_added_field(2, "b c").unwrap(), "b c 2");
    assert_eq!(s.rename_added_field(2, "total").unwrap(), "total");
    // Renaming a field to its own name is fine.
    assert_eq!(s.rename_added_field(0, "a").unwrap(), "a");

    s.set(1, Value::Text("one".into()));
    s.set(2, Value::Text("two".into()));
    let mut moved = s.widget_bounds(2, 0).unwrap();
    moved.x1 = 250.0;
    s.set_widget_bounds(2, 0, moved).unwrap();
    // Taking the middle one away keeps what the others hold.
    assert_eq!(s.remove_added_field(1), Some(0));
    assert_eq!(names(&s), ["a", "total"]);
    let built = s.build().unwrap();
    let form = built.form.unwrap();
    assert_eq!(form.fields.len(), 2);
    assert_eq!(form.fields[1].value, Value::Text("two".into()));
    assert_eq!(form.fields[1].widgets[0].rect.x1, 250.0);
    let reopened = open(built.doc.data().to_vec());
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(form.fields[1].name, "total");
    assert_eq!(form.fields[1].widgets[0].rect.x1, 250.0);

    // With the last one gone the file is as it was.
    assert_eq!(s.remove_added_field(1), Some(0));
    assert_eq!(s.remove_added_field(0), Some(0));
    assert!(s.remove_added_field(0).is_none());
    assert!(s.original().is_none() && !s.is_dirty());
    assert_eq!(s.build().unwrap().doc.data(), doc.data());

    // Bad boxes and pages are refused, and change nothing.
    let mut flat = field("flat", 100.0);
    flat.rect.y1 = flat.rect.y0 + 0.2;
    assert!(s.add_field(0, flat).is_err());
    assert!(s.add_field(9, field("off", 100.0)).is_err());
    assert!(!s.is_dirty());
}

#[test]
fn fields_join_a_form_the_file_already_has() {
    // The form's field list and resources are objects of their own, and
    // its font is not Helvetica.
    let bytes = build(
        &[FORM],
        "/AcroForm 6 0 R",
        &[
            "<< /Fields 7 0 R /DR 9 0 R /DA (/Cour 9 Tf 0 g) >>",
            "[8 0 R]",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (kept) /Rect [80 270 300 284] /P 4 0 R >>",
            "<< /Font << /Cour 10 0 R >> >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
        ],
    );
    let doc = open(bytes);
    let mut s = Session::new(doc.clone());
    assert_eq!(s.original().unwrap().fields.len(), 1);
    let index = s
        .add_field(
            0,
            NewField {
                name: "Name".into(),
                kind: NewFieldKind::Checkbox,
                rect: Rect {
                    x0: 40.5,
                    y0: 230.5,
                    x1: 49.5,
                    y1: 239.5,
                },
                tooltip: Some("I agree".into()),
                mark: None,
                font_size: 0.0,
            },
        )
        .unwrap();
    assert_eq!(index, 1);
    assert!(!s.is_added_field(0) && s.is_added_field(1));
    assert_eq!(s.original().unwrap().fields[1].name, "Name 2");
    assert!(s.remove_added_field(0).is_none());
    assert!(s.rename_added_field(0, "x").is_err());
    s.set(0, Value::Text("changed".into()));
    let reopened = open(s.build().unwrap().doc.data().to_vec());
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(form.fields.len(), 2);
    assert_eq!(form.fields[0].value, Value::Text("changed".into()));
    assert_eq!(form.fields[1].kind, FieldKind::Checkbox);
    assert_eq!(form.da.as_deref(), Some("/Cour 9 Tf 0 g"));
    let fonts = reopened
        .resolve_dict(form.resources.get(b"Font").unwrap())
        .unwrap();
    assert!(fonts.contains_key(b"Cour") && fonts.contains_key(b"Helv"));
}

#[test]
fn added_fields_follow_their_pages() {
    let doc = open(build(&[FORM, "0 0 m 10 10 l S", FORM], "", &[]));
    let mut s = Session::new(doc);
    let field = |name: &str| NewField {
        name: name.into(),
        kind: NewFieldKind::Text,
        rect: Rect {
            x0: 80.0,
            y0: 260.5,
            x1: 300.0,
            y1: 274.0,
        },
        tooltip: None,
        mark: None,
        font_size: 0.0,
    };
    s.add_field(0, field("first")).unwrap();
    s.add_field(2, field("last")).unwrap();
    s.set(1, Value::Text("kept".into()));
    // The last page moves to the front.
    s.move_pages(&[2], 0);
    assert_eq!(s.pages_of(1), [0]);
    assert_eq!(s.pages_of(0), [1]);
    let built = s.build().unwrap();
    let form = built.form.unwrap();
    assert_eq!(form.fields[0].widgets[0].page, Some(1));
    assert_eq!(form.fields[1].widgets[0].page, Some(0));
    // Deleting a page takes its field along.
    s.delete_pages(&[1]).unwrap();
    assert_eq!(s.added_field_count(), 1);
    let built = s.build().unwrap();
    let reopened = open(built.doc.data().to_vec());
    assert_eq!(reopened.pages().unwrap().len(), 2);
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(form.fields.len(), 1);
    assert_eq!(form.fields[0].name, "last");
    assert_eq!(form.fields[0].value, Value::Text("kept".into()));
    assert_eq!(form.fields[0].widgets[0].page, Some(0));
}

#[test]
fn turned_pages_get_fields_that_read_the_way_the_page_does() {
    let bytes = String::from_utf8(build(&[FORM], "", &[])).unwrap().replace(
        "/MediaBox [0 0 400 300]",
        "/Rotate 90 /MediaBox [0 0 400 300]",
    );
    // The replacement moved every object after the page; let the reader
    // find them again.
    let doc = open(bytes.into_bytes());
    assert_eq!(doc.pages().unwrap()[0].rotate, 90);
    let mut s = Session::new(doc);
    s.add_field(
        0,
        NewField {
            name: "side".into(),
            kind: NewFieldKind::Text,
            rect: Rect {
                x0: 350.0,
                y0: 40.0,
                x1: 364.0,
                y1: 200.0,
            },
            tooltip: None,
            mark: None,
            font_size: 0.0,
        },
    )
    .unwrap();
    let form = s.build().unwrap().form.unwrap();
    assert_eq!(form.fields[0].widgets[0].rotation, 90);
}

/// A one-page file that is nothing but a picture: an 800 x 600 gray image
/// over the whole 400 x 300 point page, as a scanner makes.
fn scan(pixels: &[u8]) -> Vec<u8> {
    let content = "q 400 0 0 300 0 0 cm /Im0 Do Q";
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
    ];
    let mut image = format!(
        "<< /Type /XObject /Subtype /Image /Width 800 /Height 600 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} >>\nstream\n",
        pixels.len()
    )
    .into_bytes();
    image.extend_from_slice(pixels);
    image.extend_from_slice(b"\nendstream");
    objects.push(image);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f\r\n");
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

#[test]
fn a_scan_is_read_from_its_picture() {
    let mut pixels = vec![255u8; 800 * 600];
    // Paints a box given in points, two pixels to the point, y up.
    let mut paint = |x0: f64, y0: f64, x1: f64, y1: f64| {
        for y in (600.0 - 2.0 * y1) as usize..(600.0 - 2.0 * y0) as usize {
            for x in (2.0 * x0) as usize..(2.0 * x1) as usize {
                pixels[y * 800 + x] = 0;
            }
        }
    };
    // Something printed: upright strokes, like a word.
    let mut print = |x: f64, y: f64| {
        for i in 0..8 {
            paint(
                x + 4.0 * f64::from(i),
                y,
                x + 4.0 * f64::from(i) + 1.0,
                y + 7.0,
            );
        }
    };
    print(56.0, 141.0);
    print(56.0, 268.0);
    // A table of two rows and two columns, its top left cell printed in.
    for x in [50.0, 150.0, 250.0] {
        paint(x - 0.5, 100.0, x + 0.5, 160.0);
    }
    for y in [100.0, 130.0, 160.0] {
        paint(50.0, y - 0.5, 250.0, y + 0.5);
    }
    // A tall box with a caption along its top.
    for x in [50.0, 250.0] {
        paint(x - 0.5, 200.0, x + 0.5, 280.0);
    }
    for y in [200.0, 280.0] {
        paint(50.0, y - 0.5, 250.0, y + 0.5);
    }
    let doc = open(scan(&pixels));
    let found = detect(&doc, 0, &[]);
    assert_eq!(found.tables.len(), 1);
    assert_eq!((found.tables[0].rows, found.tables[0].cols), (2, 2));
    let got: Vec<(&str, NewFieldKind, [i64; 4])> = found
        .fields
        .iter()
        .map(|s| {
            let r = s.field.rect;
            (
                s.field.name.as_str(),
                s.field.kind,
                [r.x0, r.y0, r.x1, r.y1].map(|v| v.round() as i64),
            )
        })
        .collect();
    use NewFieldKind::{Multiline, Text};
    assert_eq!(
        got,
        [
            // The box keeps the room under its caption.
            ("Text 1", Multiline, [51, 201, 249, 267]),
            // The cell beside the printed one, then the row under them.
            ("Text 2", Text, [151, 131, 249, 159]),
            ("Text 3", Text, [51, 101, 149, 129]),
            ("Text 4", Text, [151, 101, 249, 129]),
        ]
    );
    // With the fields made, the scan is a form.
    let mut s = Session::new(doc);
    for f in &found.fields {
        s.add_field(0, f.field.clone()).unwrap();
    }
    s.set(1, Value::Text("42".into()));
    let reopened = open(s.build().unwrap().doc.data().to_vec());
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(form.fields.len(), 4);
    assert_eq!(form.fields[1].value, Value::Text("42".into()));
}

/// Lines to sign, initial and date, and a square to tick.
const SIGN: &str = "\
BT /F1 10 Tf 40 262 Td (Signature:) Tj ET
0.5 w 100 260 m 300 260 l S
BT /F1 10 Tf 40 202 Td (Initials:) Tj ET
90 200 m 140 200 l S
BT /F1 10 Tf 40 162 Td (Date signed:) Tj ET
110 160 m 250 160 l S
40 110 10 10 re S
BT /F1 10 Tf 56 111 Td (Agree) Tj ET";

#[test]
fn signatures_and_initials_are_suggested_as_such() {
    use atelier_form::CheckMark;
    let doc = open(build(&[SIGN], "", &[]));
    let found = detect(&doc, 0, &[]);
    let got: Vec<(&str, NewFieldKind)> = found
        .fields
        .iter()
        .map(|s| (s.field.name.as_str(), s.field.kind))
        .collect();
    assert_eq!(
        got,
        [
            ("Signature", NewFieldKind::Signature),
            ("Initials", NewFieldKind::Initials),
            // Typed, though it speaks of signing.
            ("Date signed", NewFieldKind::Text),
            ("Agree", NewFieldKind::Checkbox),
        ]
    );
    // A signature gets more room over its line than a line of type.
    let sign = found.fields[0].field.rect;
    assert_eq!((sign.y0, sign.y1), (260.5, 286.0));

    let mut s = Session::new(doc.clone());
    for f in &found.fields {
        let mut field = f.field.clone();
        if field.kind == NewFieldKind::Checkbox {
            field.mark = Some(CheckMark::Square);
        }
        s.add_field(0, field).unwrap();
    }
    assert_eq!(s.check_mark(3), Some(CheckMark::Square));
    // A signature field has no mark.
    assert_eq!(s.check_mark(0), None);
    s.set(1, Value::Text("AL".into()));
    s.set(3, Value::State("Yes".into()));
    let built = s.build().unwrap();
    let reopened = open(built.doc.data().to_vec());
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(form.fields[0].kind, FieldKind::Signature);
    assert_eq!(form.fields[0].value, Value::None);
    assert_eq!(form.sig_flags & 1, 1);
    assert_eq!(form.fields[1].kind, FieldKind::Text);
    assert_eq!(form.fields[1].quadding, 1);
    assert_eq!(form.fields[2].kind, FieldKind::Text);
    // Only what can be typed or ticked is in the Tab order.
    assert_eq!(form.tab_order(&reopened).len(), 3);
    // The filled square covers more of the box than a check does.
    let tick = form.fields[3].widgets[0].rect;
    let square = ink(&reopened, 0, &tick);
    assert!(square > 20, "{square}");

    // The mark of a saved check box can still be changed; it stays ticked.
    let mut s = Session::new(reopened.clone());
    assert_eq!(s.check_mark(3), Some(CheckMark::Square));
    assert!(s.set_check_mark(1, CheckMark::Cross).is_err());
    s.set_check_mark(3, CheckMark::Check).unwrap();
    assert!(s.is_dirty());
    let again = open(s.build().unwrap().doc.data().to_vec());
    let form = Form::load(&again).unwrap().unwrap();
    assert_eq!(form.fields[3].value, Value::State("Yes".into()));
    let check = ink(&again, 0, &tick);
    assert!(check > 3 && check < square, "{check} {square}");
    let mut s = Session::new(again);
    assert_eq!(s.check_mark(3), Some(CheckMark::Check));
    assert_eq!(s.set_all_check_marks(CheckMark::Dot), 1);
    assert_eq!(s.set_all_check_marks(CheckMark::Dot), 0);
    assert_eq!(s.check_mark(3), Some(CheckMark::Dot));
}

#[test]
fn a_document_keeps_its_own_mark_for_new_check_boxes() {
    use atelier_form::CheckMark;
    let doc = open(build(&[SIGN], "", &[]));
    let mut s = Session::new(doc);
    assert_eq!(s.document_mark(), None);
    s.set_document_mark(Some(CheckMark::Cross));
    assert!(s.is_dirty());
    let tick = Rect {
        x0: 40.5,
        y0: 110.5,
        x1: 49.5,
        y1: 119.5,
    };
    // A box that names no mark takes the document's; one that does keeps it.
    s.add_field(0, NewField::new("a", NewFieldKind::Checkbox, tick))
        .unwrap();
    let mut star = NewField::new("b", NewFieldKind::Checkbox, tick);
    star.mark = Some(CheckMark::Star);
    s.add_field(0, star).unwrap();
    assert_eq!(s.check_mark(0), Some(CheckMark::Cross));
    assert_eq!(s.check_mark(1), Some(CheckMark::Star));
    let saved = open(s.build().unwrap().doc.data().to_vec());
    let mut s = Session::new(saved.clone());
    assert_eq!(s.document_mark(), Some(CheckMark::Cross));
    assert_eq!(s.check_mark(0), Some(CheckMark::Cross));
    assert!(!s.is_dirty());
    // Dropping the setting is a change too, and is saved.
    s.set_document_mark(None);
    assert!(s.is_dirty());
    let cleared = open(s.build().unwrap().doc.data().to_vec());
    assert_eq!(Session::new(cleared).document_mark(), None);
    s.saved(saved);
    assert_eq!(s.document_mark(), Some(CheckMark::Cross));
}

#[test]
fn text_is_sized_to_fit_unless_a_size_is_set() {
    let doc = open(build(&[SIGN], "", &[]));
    let mut s = Session::new(doc.clone());
    let narrow = Rect {
        x0: 300.0,
        y0: 100.0,
        x1: 360.0,
        y1: 120.0,
    };
    s.add_field(0, NewField::new("auto", NewFieldKind::Text, narrow))
        .unwrap();
    let mut fixed = NewField::new("fixed", NewFieldKind::Text, narrow);
    fixed.rect.y0 = 60.0;
    fixed.rect.y1 = 80.0;
    fixed.font_size = 7.0;
    s.add_field(0, fixed).unwrap();
    assert_eq!(s.field_style(0).unwrap().appearance.size, 0.0);
    assert_eq!(s.field_style(1).unwrap().appearance.size, 7.0);
    // The height of the dark rows of a box says how large its text is.
    let tall = |doc: &Document, r: &Rect| {
        let options = RenderOptions {
            scale: 4.0,
            ..RenderOptions::default()
        };
        let p = render_page(doc, &doc.pages().unwrap()[0], &options)
            .unwrap()
            .pixmap;
        let rows = ((1200.0 - 4.0 * r.y1) as usize..(1200.0 - 4.0 * r.y0) as usize)
            .filter(|&y| {
                ((4.0 * r.x0) as usize..(4.0 * r.x1) as usize).any(|x| p.pixel(x, y)[0] < 128)
            })
            .count();
        rows as f64 / 4.0
    };
    s.set(0, Value::Text("XH".into()));
    let short = tall(&s.build().unwrap().doc, &narrow);
    // A long value shrinks to stay inside the box.
    s.set(0, Value::Text("XHXHXHXHXHXHXHXHXHXH".into()));
    let long = tall(&s.build().unwrap().doc, &narrow);
    assert!(
        short > 8.0 && long < short * 0.6 && long > 2.0,
        "{short} {long}"
    );
    // A set size is kept, and can be changed or handed back to fit.
    let mut style = s.field_style(1).unwrap();
    style.appearance.size = 12.0;
    s.set_field_style(1, style.clone()).unwrap();
    s.set(1, Value::Text("XH".into()));
    let reopened = open(s.build().unwrap().doc.data().to_vec());
    let form = Form::load(&reopened).unwrap().unwrap();
    assert!(
        form.fields[1]
            .da
            .as_deref()
            .unwrap()
            .contains("/Helv 12 Tf")
    );
    let mut s = Session::new(reopened);
    style.appearance.size = 0.0;
    s.set_field_style(1, style).unwrap();
    assert_eq!(s.field_style(1).unwrap().appearance.size, 0.0);
    s.build().unwrap();
}
