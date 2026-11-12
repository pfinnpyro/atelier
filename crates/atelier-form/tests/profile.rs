//! Fills a two-page form from a profile, one page and then the whole
//! document, saves it and reads the values back.

use std::sync::Arc;

use atelier_form::{Entry, EntryKind, Form, Profile, Session, Standard, Value, plan};
use atelier_syntax::Document;

/// Two pages of 200 x 200 points. Objects 1-4 are the catalog, the pages
/// and the two pages; `fields` follow from 5, the first `on_first` of
/// them on the first page.
fn two_pages(fields: &[&str], on_first: usize) -> Vec<u8> {
    let refs = |range: std::ops::Range<usize>| {
        range
            .map(|i| format!("{} 0 R", i + 5))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let font = fields.len() + 5;
    let mut objects: Vec<String> = vec![
        format!(
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{}] /DA (/Helv 10 Tf 0 g) \
             /DR << /Font << /Helv {font} 0 R >> >> >> >>",
            refs(0..fields.len())
        ),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{}] >>",
            refs(0..on_first)
        ),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{}] >>",
            refs(on_first..fields.len())
        ),
    ];
    objects.extend(fields.iter().enumerate().map(|(i, f)| {
        let y = 170 - 25 * (i % 6);
        format!(
            "<< /Type /Annot /Subtype /Widget /Rect [10 {y} 190 {}] /F 4 {f} >>",
            y + 20
        )
    }));
    objects.push(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    );
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

fn profile() -> Profile {
    let entry = |kind, value: &str| Entry {
        kind,
        value: value.into(),
    };
    Profile {
        name: "Home".into(),
        entries: vec![
            entry(EntryKind::Standard(Standard::Name), "Ada Lovelace"),
            entry(EntryKind::Standard(Standard::City), "London"),
            entry(EntryKind::Standard(Standard::State), "TX"),
            entry(EntryKind::Standard(Standard::Phone), "555 0100"),
            entry(EntryKind::Custom("Member number".into()), "M-4471"),
        ],
    }
}

fn sample() -> Vec<u8> {
    two_pages(
        &[
            // The first page.
            "/FT /Tx /T (f1) /TU (Name of applicant)",
            "/FT /Tx /T (City) /V (Paris)",
            "/FT /Ch /Ff 131072 /T (State) /Opt [(CA) (TX)]",
            "/FT /Tx /T (Employer name)",
            // The second.
            "/FT /Tx /T (Name)",
            "/FT /Tx /T (Telephone)",
            "/FT /Tx /T (MemberNumber)",
            "/FT /Btn /T (Phone)",
        ],
        4,
    )
}

/// Fills what the profile matches on one page, or everywhere.
fn fill(session: &mut Session, form: &Form, page: Option<usize>, replace: bool) -> usize {
    let planned = plan(form, &profile(), replace, |field| {
        page.is_none_or(|p| session.pages_of(field).contains(&p))
    });
    for p in &planned {
        assert_eq!(p.skipped, None, "{}", p.label);
        session.set(p.field, p.value.clone());
    }
    planned.len()
}

fn values(form: &Form) -> Vec<String> {
    form.fields.iter().map(|f| f.value.display()).collect()
}

#[test]
fn a_page_or_the_whole_document_is_filled_and_saved() {
    let doc = Arc::new(Document::open(sample()).unwrap());
    let mut session = Session::new(doc.clone());
    let form = Form::load(&doc).unwrap().unwrap();

    // The second page alone: the first is left as it was.
    assert_eq!(fill(&mut session, &form, Some(1), false), 3);
    let built = session.build().unwrap();
    assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    let form = built.form.unwrap();
    assert_eq!(
        values(&form),
        [
            "",
            "Paris",
            "",
            "",
            "Ada Lovelace",
            "555 0100",
            "M-4471",
            ""
        ]
    );

    // Then the whole document: what is filled in already stays.
    assert_eq!(fill(&mut session, &form, None, false), 2);
    let built = session.build().unwrap();
    let form = built.form.unwrap();
    assert_eq!(
        values(&form),
        [
            "Ada Lovelace",
            "Paris",
            "TX",
            "",
            "Ada Lovelace",
            "555 0100",
            "M-4471",
            ""
        ]
    );
    // Filling again finds nothing left to do.
    assert_eq!(fill(&mut session, &form, None, false), 0);

    // Replacing takes the city too.
    assert_eq!(fill(&mut session, &form, Some(0), true), 1);
    let built = session.build().unwrap();
    assert_eq!(
        built.form.as_ref().unwrap().fields[1].value,
        Value::Text("London".into())
    );

    // The saved file reads back the same.
    let reopened = Document::open(built.doc.data().to_vec()).unwrap();
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(
        values(&form),
        [
            "Ada Lovelace",
            "London",
            "TX",
            "",
            "Ada Lovelace",
            "555 0100",
            "M-4471",
            ""
        ]
    );
}
