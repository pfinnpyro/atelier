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
