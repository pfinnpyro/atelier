//! Builds small forms by hand, fills them, saves, reopens and checks the
//! values and what the page shows.

use atelier_form::{FieldKind, FillWarning, Form, Value, document_actions};
use atelier_render::{RenderOptions, render_page};
use atelier_syntax::{Document, ObjRef, Object};

/// A one-page, 200 x 200 point document. Objects 1-3 are the catalog,
/// pages and page; `extra` objects follow from 4. `fields` and `annots`
/// are written into the AcroForm and the page.
fn build(acroform: &str, annots: &str, extra: &[&str]) -> Vec<u8> {
    let mut objects: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm {acroform} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots {annots} >>"),
    ];
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

const HELV: &str =
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

/// A text field (4), a parent with two radio kids (5, 6, 7), a checkbox
/// without appearances (8), a list box (9) and the Helvetica font (10).
fn sample() -> Vec<u8> {
    build(
        "<< /Fields [4 0 R 5 0 R 8 0 R 9 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 10 0 R >> >> >>",
        "[4 0 R 6 0 R 7 0 R 8 0 R 9 0 R]",
        &[
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /TU (Your name) /V (old) /Rect [10 150 190 180] /DA (/Helv 12 Tf 0 g) /F 4 /MaxLen 20 >>",
            "<< /FT /Btn /Ff 49152 /T (choice) /V /Off /Kids [6 0 R 7 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [10 100 30 120] /AS /Off /MK << /BC [0] >> /AP << /N << /a 11 0 R /Off 12 0 R >> >> >>",
            "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [40 100 60 120] /AS /Off /AP << /N << /b 11 0 R /Off 12 0 R >> >> >>",
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /Rect [100 100 120 120] /MK << /BC [0] >> >>",
            "<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 2097152 /T (fruit) /Opt [(Apple) [(b) (Banana)] (Cherry)] /Rect [10 10 190 80] >>",
            HELV,
            "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 22 >>\nstream\n0 g 0 0 20 20 re f    \nendstream",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream",
        ],
    )
}

fn render(doc: &Document) -> atelier_raster::Pixmap {
    let page = &doc.pages().unwrap()[0];
    let options = RenderOptions {
        scale: 1.0,
        ..RenderOptions::default()
    };
    render_page(doc, page, &options).unwrap().pixmap
}

/// Number of dark pixels inside a user-space box.
fn ink(p: &atelier_raster::Pixmap, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let px = p.pixel(x, 199 - y);
            if px[0] < 128 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn reads_the_field_tree() {
    let doc = Document::open(sample()).unwrap();
    let form = Form::load(&doc).unwrap().unwrap();
    let names: Vec<_> = form
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("name", FieldKind::Text),
            ("choice", FieldKind::Radio),
            ("agree", FieldKind::Checkbox),
            ("fruit", FieldKind::ListBox),
        ]
    );
    let name = form.field("name").unwrap();
    assert_eq!(name.value, Value::Text("old".into()));
    assert_eq!(name.tooltip.as_deref(), Some("Your name"));
    assert_eq!(name.max_len, Some(20));
    assert_eq!(name.widgets[0].page, Some(0));
    let choice = form.field("choice").unwrap();
    assert_eq!(choice.widgets.len(), 2);
    assert_eq!(choice.states(), ["a", "b"]);
    assert_eq!(choice.value, Value::State("Off".into()));
    let fruit = form.field("fruit").unwrap();
    assert_eq!(fruit.options[1].export, "b");
    assert_eq!(fruit.options[1].display, "Banana");
    assert_eq!(form.field_of_widget(ObjRef::new(7, 0)), Some((1, 1)));
}

#[test]
fn filled_values_survive_saving_and_show_on_the_page() {
    let doc = Document::open(sample()).unwrap();
    let before = render(&doc);
    let mut form = Form::load(&doc).unwrap().unwrap();
    let mut update = doc.update();
    let warnings = form
        .set_value(&mut update, 0, Value::Text("Ada Lovelace".into()))
        .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    form.set_value(&mut update, 1, Value::State("b".into()))
        .unwrap();
    form.set_value(&mut update, 2, Value::State("Yes".into()))
        .unwrap();
    form.set_value(
        &mut update,
        3,
        Value::Choices(vec!["b".into(), "Cherry".into()]),
    )
    .unwrap();
    let saved = update.to_bytes().unwrap();
    assert!(
        saved.starts_with(doc.data()),
        "saving must append, not rewrite"
    );

    let doc = Document::open(saved).unwrap();
    let form = Form::load(&doc).unwrap().unwrap();
    assert_eq!(form.fields[0].value, Value::Text("Ada Lovelace".into()));
    assert_eq!(form.fields[1].value, Value::State("b".into()));
    assert_eq!(form.fields[2].value, Value::State("Yes".into()));
    assert_eq!(
        form.fields[3].value,
        Value::Choices(vec!["b".into(), "Cherry".into()])
    );
    // The radio widgets switched state; the checkbox got drawn appearances.
    let state = |r: u32| match doc.get(ObjRef::new(r, 0)).unwrap() {
        Object::Dict(d) => d
            .get(b"AS")
            .and_then(|a| a.as_name().map(|n| n.as_str_lossy().into_owned())),
        _ => None,
    };
    assert_eq!(state(6).as_deref(), Some("Off"));
    assert_eq!(state(7).as_deref(), Some("b"));
    assert_eq!(state(8).as_deref(), Some("Yes"));
    assert_eq!(form.fields[2].states(), ["Yes"]);

    let after = render(&doc);
    // Text in the text field, the second radio filled, a check mark.
    assert!(ink(&after, 12, 152, 188, 178) > ink(&before, 12, 152, 188, 178) + 50);
    assert!(ink(&after, 42, 102, 58, 118) > 200);
    assert!(ink(&after, 12, 102, 28, 118) < 10);
    assert!(ink(&after, 102, 102, 118, 118) > 20);
}

#[test]
fn turning_a_checkbox_off_and_resetting() {
    let doc = Document::open(sample()).unwrap();
    let mut form = Form::load(&doc).unwrap().unwrap();
    let mut update = doc.update();
    form.set_value(&mut update, 1, Value::State("a".into()))
        .unwrap();
    form.set_value(&mut update, 1, Value::None).unwrap();
    assert_eq!(form.fields[1].value, Value::State("Off".into()));
    form.set_value(&mut update, 0, Value::Text("x".into()))
        .unwrap();
    form.reset(&mut update, 0).unwrap();
    assert_eq!(form.fields[0].value, Value::None);
}

#[test]
fn long_values_are_cut_and_missing_characters_reported() {
    let doc = Document::open(sample()).unwrap();
    let mut form = Form::load(&doc).unwrap().unwrap();
    let mut update = doc.update();
    let w = form
        .set_value(&mut update, 0, Value::Text("x".repeat(30)))
        .unwrap();
    assert_eq!(
        w,
        [FillWarning::Truncated {
            field: "name".into()
        }]
    );
    assert_eq!(form.fields[0].value, Value::Text("x".repeat(20)));
    // Helvetica with WinAnsi has no Greek.
    let w = form
        .set_value(&mut update, 0, Value::Text("αβγ".into()))
        .unwrap();
    assert_eq!(
        w,
        [FillWarning::MissingCharacters {
            field: "name".into()
        }]
    );
    // The value itself keeps the real characters.
    let doc2 = Document::open(update.to_bytes().unwrap()).unwrap();
    let form2 = Form::load(&doc2).unwrap().unwrap();
    assert_eq!(form2.fields[0].value, Value::Text("αβγ".into()));
}

#[test]
fn a_wrong_kind_of_value_changes_nothing() {
    let doc = Document::open(sample()).unwrap();
    let mut form = Form::load(&doc).unwrap().unwrap();
    let mut update = doc.update();
    let w = form
        .set_value(&mut update, 1, Value::Text("no".into()))
        .unwrap();
    assert_eq!(
        w,
        [FillWarning::WrongKind {
            field: "choice".into()
        }]
    );
    assert!(update.is_empty());
}

#[test]
fn documents_without_a_form() {
    let doc = Document::open(build("<< /Fields [] >>", "[]", &[])).unwrap();
    assert!(Form::load(&doc).unwrap().is_none());
}

#[test]
fn field_cycles_terminate() {
    let doc = Document::open(build(
        "<< /Fields [4 0 R] >>",
        "[]",
        &[
            "<< /T (a) /FT /Tx /Kids [5 0 R] >>",
            "<< /T (b) /Kids [4 0 R] >>",
        ],
    ))
    .unwrap();
    let form = Form::load(&doc).unwrap();
    assert!(form.is_none_or(|f| f.fields.len() <= 2));
}

#[test]
fn actions_are_listed_not_run() {
    let doc = Document::open(build(
        "<< /Fields [4 0 R] >>",
        "[4 0 R]",
        &[
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (total) /Rect [0 0 10 10] /AA << /C << /S /JavaScript /JS (event.value = 1;) >> /K << /S /JavaScript /JS 5 0 R >> >> /A << /S /URI /URI (https://example.com) /Next << /S /Launch /F (calc.exe) >> >> >>",
            "<< /Length 12 >>\nstream\napp.alert(1)\nendstream",
        ],
    ))
    .unwrap();
    let actions = document_actions(&doc);
    let summary: Vec<(String, String)> = actions
        .iter()
        .map(|a| (a.location.clone(), a.kind.clone()))
        .collect();
    assert!(summary.contains(&("Page 1, widget total, activate".into(), "URI".into())));
    assert!(summary.contains(&("Page 1, widget total, activate".into(), "Launch".into())));
    assert!(summary.contains(&("Field total, calculate".into(), "JavaScript".into())));
    let keystroke = actions
        .iter()
        .find(|a| a.location == "Field total, keystroke")
        .unwrap();
    assert_eq!(keystroke.javascript.as_deref(), Some("app.alert(1)"));
    let launch = actions.iter().find(|a| a.kind == "Launch").unwrap();
    assert_eq!(launch.target.as_deref(), Some("calc.exe"));
}

#[test]
fn a_session_rebuilds_from_the_original_each_time() {
    use atelier_form::Session;
    use std::sync::Arc;
    let doc = Arc::new(Document::open(sample()).unwrap());
    let mut s = Session::new(doc.clone());
    assert!(!s.is_dirty());
    // Nothing changed: the original document comes back as is.
    assert!(Arc::ptr_eq(&s.build().unwrap().doc, &doc));

    s.set(0, Value::Text("first".into()));
    s.set(0, Value::Text("second".into()));
    s.set(2, Value::State("Yes".into()));
    assert!(s.is_dirty());
    let built = s.build().unwrap();
    assert_eq!(
        built.form.unwrap().fields[0].value,
        Value::Text("second".into())
    );
    // One appended section on top of the original, not one per change.
    let data = built.doc.data();
    assert!(data.starts_with(doc.data()));
    assert_eq!(count(data, b"%%EOF"), 2);

    // Setting a field back to how it was drops the change.
    s.set(0, Value::Text("old".into()));
    s.set(2, Value::State("Off".into()));
    assert!(!s.is_dirty());
    assert_eq!(s.pages_of(1), [0]);
}

#[test]
fn scripts_can_hide_and_show_fields() {
    use atelier_form::Session;
    use std::sync::Arc;
    let doc = Arc::new(Document::open(sample()).unwrap());
    let mut s = Session::new(doc);
    assert!(!s.is_hidden(0));
    s.set_hidden(0, true);
    assert!(s.is_dirty() && s.is_hidden(0));
    let built = s.build().unwrap();
    let form = built.form.unwrap();
    assert!(form.fields[0].widgets.iter().all(|w| w.is_hidden()));
    // The flag is in the saved file, so other viewers hide it too.
    let again = Form::load(&built.doc).unwrap().unwrap();
    assert!(again.fields[0].widgets.iter().all(|w| w.is_hidden()));
    // Showing it again is no change at all.
    s.set_hidden(0, false);
    assert!(!s.is_dirty() && !s.is_hidden(0));
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

#[test]
fn tab_order_and_hit_testing() {
    let doc = Document::open(sample()).unwrap();
    let form = Form::load(&doc).unwrap().unwrap();
    // /Annots order: name, both radios, the checkbox, the list.
    assert_eq!(
        form.tab_order(&doc),
        [(0, 0), (1, 0), (1, 1), (2, 0), (3, 0)]
    );
    assert_eq!(form.widget_at(0, 50.0, 110.0), Some((1, 1)));
    assert_eq!(form.widget_at(0, 100.0, 165.0), Some((0, 0)));
    assert_eq!(form.widget_at(0, 100.0, 90.0), None);
    assert_eq!(form.widget_at(1, 100.0, 165.0), None);

    // Rows order on a page with /Tabs /R: top to bottom, then left to right.
    let data = String::from_utf8_lossy(&sample()).replace(
        "/Annots [4 0 R 6 0 R 7 0 R 8 0 R 9 0 R]",
        "/Annots [9 0 R 8 0 R 7 0 R 6 0 R 4 0 R] /Tabs /R",
    );
    let doc = Document::open(data.into_bytes()).unwrap();
    let form = Form::load(&doc).unwrap().unwrap();
    assert_eq!(
        form.tab_order(&doc),
        [(0, 0), (1, 0), (1, 1), (2, 0), (3, 0)]
    );
}

#[test]
fn encrypted_forms_fill_and_stay_encrypted() {
    use atelier_form::Session;
    use std::sync::Arc;
    let path = format!(
        "{}/tests/data/encrypted-form.pdf",
        env!("CARGO_MANIFEST_DIR")
    );
    let data = std::fs::read(path).unwrap();
    let doc = Arc::new(Document::open_with_password(data, b"user").unwrap());
    let mut s = Session::new(doc);
    let name = s
        .original()
        .unwrap()
        .fields
        .iter()
        .position(|f| f.name == "name")
        .unwrap();
    let colour = s
        .original()
        .unwrap()
        .fields
        .iter()
        .position(|f| f.name == "colour")
        .unwrap();
    s.set(name, Value::Text("Grace Hopper".into()));
    s.set(colour, Value::Text("Blue".into()));
    let built = s.build().unwrap();
    let saved = built.doc.data().to_vec();
    assert!(
        !saved.windows(12).any(|w| w == b"Grace Hopper"),
        "the value must be encrypted"
    );
    assert!(matches!(
        Document::open(saved.clone()),
        Err(atelier_syntax::Error::Password)
    ));
    let reopened = Document::open_with_password(saved, b"user").unwrap();
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(form.fields[name].value, Value::Text("Grace Hopper".into()));
    assert_eq!(form.fields[colour].value, Value::Text("Blue".into()));
    let page = render(&reopened);
    assert!(page.width() > 0);
}

#[test]
fn rotations_are_saved_with_the_fields() {
    use atelier_form::Session;
    use std::sync::Arc;
    let doc = Arc::new(Document::open(sample()).unwrap());
    let mut s = Session::new(doc.clone());
    assert_eq!(s.rotation(0), 0);
    s.rotate(0, -90);
    assert_eq!(s.rotation(0), 270);
    assert!(s.is_dirty());
    s.set(0, Value::Text("turned".into()));
    let built = s.build().unwrap();
    let page = &built.doc.pages().unwrap()[0];
    assert_eq!(page.rotate, 270);
    assert_eq!(
        built.form.unwrap().fields[0].value,
        Value::Text("turned".into())
    );
    // Turning back to the original drops the change.
    s.rotate(0, 360);
    s.set(0, Value::Text("old".into()));
    assert!(!s.is_dirty());

    // Documents without a form still take rotations.
    let plain = Arc::new(Document::open(build("<< >>", "[]", &[])).unwrap());
    let mut s = Session::new(plain);
    assert!(s.original().is_none());
    s.rotate(0, 180);
    assert_eq!(s.build().unwrap().doc.pages().unwrap()[0].rotate, 180);

    // Turns made quietly (auto-orient on open) are kept but not unsaved.
    s.quiet_rotations();
    assert!(!s.is_dirty());
    assert_eq!(s.build().unwrap().doc.pages().unwrap()[0].rotate, 180);
    s.rotate(0, 90);
    assert!(s.is_dirty());
}

#[test]
fn ocr_text_layers_are_saved_as_invisible_text() {
    use atelier_raster::Point;
    use atelier_render::{PageText, TextGlyph, extract_text};
    use std::sync::Arc;
    let doc = Arc::new(Document::open(sample()).unwrap());
    // Two words along y = 500, each letter 10 wide and 12 tall.
    let mut glyphs = Vec::new();
    for (i, c) in "Hello world".chars().enumerate() {
        let x = 100.0 + 10.0 * i as f64;
        let p = |x, y| Point::new(x, y);
        let quad = [
            p(x, 500.0),
            p(x + 10.0, 500.0),
            p(x + 10.0, 512.0),
            p(x, 512.0),
        ];
        glyphs.push(TextGlyph {
            text: c.to_string(),
            quad,
            baseline: (quad[0], quad[1]),
            size: 12.0,
            font: "OCR".into(),
            invisible: true,
        });
    }
    let mut session = atelier_form::Session::new(doc.clone());
    session.set_text_layer(0, Arc::new(PageText { glyphs }));
    assert!(!session.is_dirty());
    let built = session.build().unwrap();
    let page = &built.doc.pages().unwrap()[0];
    let text = extract_text(&built.doc, page).unwrap();
    assert_eq!(text.plain_text().trim(), "Hello world");
    assert!(text.glyphs.iter().all(|g| g.invisible));
    // The words keep their place: "world" starts near x = 160.
    let w = text.glyphs.iter().find(|g| g.text == "w").unwrap();
    assert!((w.quad[0].x - 160.0).abs() < 1.0, "{:?}", w.quad[0]);
    // And the page looks the same.
    assert_eq!(render(&built.doc).data(), render(&doc).data());
}

#[test]
fn xfa_is_found_and_judged_static_or_dynamic() {
    use atelier_form::xfa;
    let template = "<template><subform name=\"f\"><field name=\"a\"><event activity=\"click\"><script contentType=\"application/x-javascript\">app.alert(1)</script></event></field></subform></template>";
    let stream = format!(
        "<< /Length {} >>\nstream\n{template}\nendstream",
        template.len() + 1
    );
    // An AcroForm copy of the fields: static.
    let doc = Document::open(build(
        "<< /Fields [4 0 R] /XFA [(template) 5 0 R] >>",
        "[4 0 R]",
        &[
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (a) /Rect [0 0 10 10] >>",
            &stream,
        ],
    ))
    .unwrap();
    let x = xfa(&doc).unwrap();
    assert!(!x.dynamic);
    assert_eq!(x.packets[0].0, "template");
    assert_eq!(x.scripts[0].location, "f.a, click");
    // No fields: the pages cannot hold the form.
    let doc = Document::open(build(
        "<< /Fields [] /XFA [(template) 4 0 R] >>",
        "[]",
        &[&stream],
    ))
    .unwrap();
    assert!(xfa(&doc).unwrap().dynamic);
    // No XFA at all.
    assert!(xfa(&Document::open(sample()).unwrap()).is_none());
}

#[test]
fn hotspots_tie_actions_to_what_is_clicked() {
    use atelier_form::hotspots;
    let doc = Document::open(build(
        "<< /Fields [4 0 R] >>",
        "[5 0 R 6 0 R]",
        &[
            "<< /FT /Tx /T (total) /Kids [5 0 R] /AA << /C << /S /JavaScript /JS (event.value = 1;) >> >> >>",
            "<< /Type /Annot /Subtype /Widget /Parent 4 0 R /Rect [0 0 50 20] /AA << /U << /S /JavaScript /JS (app.alert(2)) >> >> >>",
            "<< /Type /Annot /Subtype /Link /Rect [100 100 150 120] /F 2 /A << /S /JavaScript /JS (hidden()) >> >>",
        ],
    ))
    .unwrap();
    let actions = document_actions(&doc);
    let spots = hotspots(&doc, &actions);
    // The hidden link is left out.
    assert_eq!(spots.len(), 1);
    let s = &spots[0];
    assert!(s.contains(10.0, 10.0) && !s.contains(60.0, 10.0));
    let tied: Vec<(&str, bool)> = s
        .actions
        .iter()
        .map(|&i| (actions[i].location.as_str(), actions[i].on_click))
        .collect();
    assert_eq!(
        tied,
        [
            ("Page 1, widget annotation, mouse up", true),
            ("Field total, calculate", false)
        ]
    );
}

#[test]
fn cyclic_action_chains_are_bounded() {
    let doc = Document::open(build(
        "<< /Fields [] >>",
        "[4 0 R]",
        &[
            "<< /Subtype /Link /A 5 0 R >>",
            "<< /S /JavaScript /JS (while(true){}) /Next 5 0 R >>",
        ],
    ))
    .unwrap();
    let actions = document_actions(&doc);
    assert!(!actions.is_empty());
    assert!(actions.len() <= 33);
    assert!(
        actions
            .iter()
            .all(|a| a.javascript.as_deref() == Some("while(true){}"))
    );
}

#[test]
fn action_arrays_preserve_external_targets_without_running_them() {
    let doc = Document::open(build(
        "<< /Fields [] >>", "[4 0 R]",
        &["<< /Subtype /Link /A << /S /JavaScript /JS (app.alert(1)) /Next [<< /S /SubmitForm /F << /UF (https://example.invalid/submit) >> >> << /S /Launch /F (do-not-run.exe) >>] >> >>"],
    )).unwrap();
    let actions = document_actions(&doc);
    assert_eq!(
        actions.iter().map(|a| a.kind.as_str()).collect::<Vec<_>>(),
        ["JavaScript", "SubmitForm", "Launch"]
    );
    assert_eq!(
        actions[1].target.as_deref(),
        Some("https://example.invalid/submit")
    );
    assert_eq!(actions[2].target.as_deref(), Some("do-not-run.exe"));
}

#[test]
fn hybrid_xfa_is_detected_and_preserved_when_filling_acroform() {
    for xfa in ["5 0 R", "[(template) 5 0 R]"] {
        let data = build(
            &format!("<< /Fields [4 0 R] /XFA {xfa} >>"),
            "[4 0 R]",
            &[
                "<< /Subtype /Widget /FT /Tx /T (name) /Rect [0 0 100 20] >>",
                "<< /Length 11 >>\nstream\n<template/>\nendstream",
            ],
        );
        let doc = Document::open(data).unwrap();
        let mut form = Form::load(&doc).unwrap().unwrap();
        assert!(form.has_xfa);
        let mut update = doc.update();
        form.set_value(&mut update, 0, Value::Text("QA".into()))
            .unwrap();
        let reopened = Document::open(update.to_bytes().unwrap()).unwrap();
        let filled = Form::load(&reopened).unwrap().unwrap();
        assert!(filled.has_xfa);
        assert_eq!(filled.fields[0].value, Value::Text("QA".into()));
        let catalog = reopened.catalog().unwrap();
        let acro = reopened.get_key(&catalog, b"AcroForm").unwrap();
        assert!(acro.as_dict().unwrap().get(b"XFA").is_some());
    }
}

#[test]
fn signature_fields_are_not_editable_as_text() {
    let doc = Document::open(build(
        "<< /Fields [4 0 R] /SigFlags 3 >>",
        "[4 0 R]",
        &["<< /Subtype /Widget /FT /Sig /T (approval) /Rect [0 0 100 20] >>"],
    ))
    .unwrap();
    let mut form = Form::load(&doc).unwrap().unwrap();
    assert_eq!(form.fields[0].kind, FieldKind::Signature);
    assert_eq!(form.sig_flags, 3);
    let mut update = doc.update();
    let warnings = form
        .set_value(&mut update, 0, Value::Text("QA".into()))
        .unwrap();
    assert!(!warnings.is_empty());
    assert!(update.is_empty());
}

#[test]
fn xfa_detection_does_not_depend_on_acroform_fields() {
    for xfa in ["4 0 R", "[(template) 4 0 R]"] {
        let doc = Document::open(build(
            &format!("<< /Fields [] /XFA {xfa} >>"),
            "[]",
            &["<< /Length 11 >>\nstream\n<template/>\nendstream"],
        ))
        .unwrap();
        assert!(atelier_form::document_has_xfa(&doc));
        assert!(Form::load(&doc).unwrap().is_none());
    }
    for acro in [
        "<< /Fields [] >>",
        "<< /Fields [] /XFA null >>",
        "<< /Fields [] /XFA 99 0 R >>",
    ] {
        let doc = Document::open(build(acro, "[]", &[])).unwrap();
        assert!(!atelier_form::document_has_xfa(&doc));
    }
}

#[test]
fn saved_comment_edits_and_script_visibility_changes_survive_together() {
    use atelier_form::{Markup, NewAnnot, Review, Session};
    use std::sync::Arc;
    let mut session = Session::new(Arc::new(Document::open(sample()).unwrap()));
    let review = Review {
        text: "Before".into(),
        author: "Ada".into(),
        subject: "Review".into(),
        created: "D:20261101120000Z".into(),
        modified: "D:20261101120000Z".into(),
    };
    session.add_annot(
        0,
        NewAnnot {
            markup: Markup::Note {
                at: atelier_raster::Point::new(100.0, 180.0),
                review: review.clone(),
            },
            color: [1.0, 0.8, 0.2],
        },
    );
    let saved = session.build().unwrap().doc;
    let Object::Array(annots) = saved
        .get_key(&saved.pages().unwrap()[0].dict, b"Annots")
        .unwrap()
    else {
        panic!("missing annotations")
    };
    let ordinal = annots.len() - 1;
    session.saved(saved);
    session.set_review(
        0,
        ordinal,
        Review {
            text: "After".into(),
            ..review
        },
    );
    session.set_hidden(0, true);
    assert!(session.is_dirty());
    let built = session.build().unwrap();
    assert!(built.form.as_ref().unwrap().fields[0].widgets[0].is_hidden());
    let Object::Array(annots) = built
        .doc
        .get_key(&built.doc.pages().unwrap()[0].dict, b"Annots")
        .unwrap()
    else {
        panic!("missing annotations")
    };
    let note = built.doc.resolve_dict(&annots[ordinal]).unwrap();
    let Object::String(text) = built.doc.get_key(&note, b"Contents").unwrap() else {
        panic!("missing text")
    };
    assert_eq!(text.to_text(), "After");
    session.saved(built.doc);
    assert!(!session.is_dirty());
    assert!(session.is_hidden(0));
    session.set_hidden(0, false);
    session.revert();
    assert!(!session.is_dirty());
    assert!(session.is_hidden(0));
}
