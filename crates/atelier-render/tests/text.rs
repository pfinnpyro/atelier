//! Text drawing and extraction with embedded fonts (so no system fonts are
//! needed). Pages are 200 x 100 points rendered at 72 dpi, so user point
//! (x, y) is device pixel (x, 100 - y).

use atelier_raster::Pixmap;
use atelier_render::{PageText, RenderOptions, extract_text, render_page};
use atelier_syntax::Document;

const LIBERATION: &[u8] = include_bytes!("../../atelier-font/tests/data/liberation.ttf");
const CHARTER: &[u8] = include_bytes!("../../atelier-font/tests/data/charter.pfb");
const INTER_CFF: &[u8] = include_bytes!("../../atelier-font/tests/data/inter.cff");

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nendstream");
    s
}

/// A one-page file. Objects 5 and up are `extra`; the page's /F1 is 5 0 R.
fn build(content: &[u8], extra: &[Vec<u8>]) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        stream("", content),
    ];
    objects.extend_from_slice(extra);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
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

/// Liberation Sans as an embedded TrueType font with WinAnsiEncoding.
fn truetype_font() -> Vec<Vec<u8>> {
    vec![
        b"<< /Type /Font /Subtype /TrueType /BaseFont /LiberationSans /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /LiberationSans /Flags 32 /FontFile2 7 0 R >>".to_vec(),
        stream("", LIBERATION),
    ]
}

fn open(content: &[u8], extra: &[Vec<u8>]) -> Document {
    Document::open(build(content, extra)).unwrap()
}

fn render(doc: &Document) -> (Pixmap, Vec<String>) {
    let page = &doc.pages().unwrap()[0];
    let options = RenderOptions {
        scale: 1.0,
        ..RenderOptions::default()
    };
    let r = render_page(doc, page, &options).unwrap();
    (r.pixmap, r.warnings)
}

fn text(doc: &Document) -> PageText {
    extract_text(doc, &doc.pages().unwrap()[0]).unwrap()
}

/// Number of dark pixels in a device rectangle.
fn ink(p: &Pixmap, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if p.pixel(x, y)[0] < 128 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn truetype_text_draws_and_extracts() {
    let doc = open(b"BT /F1 40 Tf 10 30 Td (AWM) Tj ET", &truetype_font());
    let (p, warnings) = render(&doc);
    assert!(warnings.is_empty(), "{warnings:?}");
    // Glyphs sit between the baseline (y 30 -> row 70) and cap height.
    assert!(ink(&p, 10, 40, 110, 70) > 400);
    assert_eq!(ink(&p, 0, 75, 200, 100), 0, "nothing below the baseline");
    assert_eq!(ink(&p, 120, 0, 200, 100), 0, "nothing past the text");

    let t = text(&doc);
    assert_eq!(t.plain_text(), "AWM");
    let q = &t.glyphs[0];
    assert!((q.baseline.0.x - 10.0).abs() < 1e-9 && (q.baseline.0.y - 30.0).abs() < 1e-9);
    // A's advance in Liberation Sans is 1366/2048 em.
    let adv = q.baseline.1.x - q.baseline.0.x;
    assert!((adv - 40.0 * 1366.0 / 2048.0).abs() < 0.01, "{adv}");
    assert!((q.size - 40.0).abs() < 1e-9);
}

#[test]
fn spacing_scaling_and_tj_adjustments_move_glyphs() {
    let doc = open(
        b"BT /F1 10 Tf 0 0 Td 2 Tc 5 Tw 200 Tz [(Q) -1000 (Q Q)] TJ ET",
        &truetype_font(),
    );
    let g = text(&doc).glyphs;
    let w = 10.0 * 1593.0 / 2048.0;
    let space = 10.0 * 569.0 / 2048.0;
    let x: Vec<f64> = g.iter().map(|g| g.baseline.0.x).collect();
    // Each advance: (w + Tc [+ Tw for spaces]) * 2, and TJ -1000 adds 10 * 2.
    assert!((x[1] - ((w + 2.0) * 2.0 + 20.0)).abs() < 1e-6, "{x:?}");
    assert!((x[2] - (x[1] + (w + 2.0) * 2.0)).abs() < 1e-6, "{x:?}");
    assert!(
        (x[3] - (x[2] + (space + 2.0 + 5.0) * 2.0)).abs() < 1e-6,
        "{x:?}"
    );
}

#[test]
fn render_modes_and_text_clipping() {
    // Mode 7 adds the glyphs to the clip; the red fill then shows only
    // inside them. Mode 3 text is invisible but still extracted.
    let doc = open(
        b"q BT /F1 60 Tf 10 20 Td 7 Tr (MM) Tj ET 1 0 0 rg 0 0 200 100 re f Q \
          BT /F1 20 Tf 150 80 Td 3 Tr (Q) Tj ET",
        &truetype_font(),
    );
    let (p, _) = render(&doc);
    let red = |x: usize, y: usize| {
        let px = p.pixel(x, y);
        px[0] > 200 && px[1] < 60
    };
    // Inside the left stem of the first M, and well outside the text.
    assert!(red(18, 60));
    assert!(!red(190, 90));
    assert!(!red(100, 5));
    // The invisible Q left no ink.
    assert_eq!(ink(&p, 140, 0, 200, 25), 0);
    let t = text(&doc);
    assert_eq!(t.plain_text(), "MM\nQ");
    assert!(t.glyphs[2].invisible);
}

#[test]
fn type1_and_cff_fonts() {
    let pfb = CHARTER;
    // PFB to the clear + binary layout PDF uses.
    let len = |at: usize| u32::from_le_bytes(pfb[at + 2..at + 6].try_into().unwrap()) as usize;
    let l1 = len(0);
    let clear = &pfb[6..6 + l1];
    let l2 = len(6 + l1);
    let binary = &pfb[12 + l1..12 + l1 + l2];
    let mut file = clear.to_vec();
    file.extend_from_slice(binary);
    let extra = vec![
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Charter /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /Charter /Flags 34 /FontFile 7 0 R >>".to_vec(),
        stream(&format!("/Length1 {} /Length2 {} /Length3 0", clear.len(), binary.len()), &file),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Inter /FontDescriptor 9 0 R /Encoding << /Differences [65 /M] >> >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /Inter /Flags 32 /FontFile3 10 0 R >>".to_vec(),
        stream("/Subtype /Type1C", INTER_CFF),
    ];
    let content = b"BT /F1 30 Tf 10 50 Td (Hello) Tj /F2 30 Tf 0 -40 Td (AW) Tj ET";
    let mut doc_bytes = build(content, &extra);
    // Point /F2 at object 8.
    let page = b"/Font << /F1 5 0 R >>";
    let at = doc_bytes
        .windows(page.len())
        .position(|w| w == page)
        .unwrap();
    doc_bytes.splice(
        at..at + page.len(),
        b"/Font << /F1 5 0 R /F2 8 0 R >>".iter().copied(),
    );
    let doc = Document::open(doc_bytes).unwrap();
    let (p, warnings) = render(&doc);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(ink(&p, 10, 25, 100, 50) > 200, "Charter line drawn");
    assert!(ink(&p, 10, 65, 100, 90) > 200, "Inter line drawn");
    // Differences maps code 65 to M; the built-in standard names give W.
    assert_eq!(text(&doc).plain_text(), "Hello\nMW");
}

#[test]
fn cid_font_with_to_unicode() {
    // Identity-H codes are glyph ids in the TrueType font; ToUnicode
    // gives the text.
    let to_unicode = b"/CIDInit /ProcSet findresource begin begincmap 1 begincodespacerange <0000> <FFFF> endcodespacerange 2 beginbfrange <0001> <0003> <0041> <0004> <0004> <00660069> endbfrange endcmap end";
    let extra = vec![
        b"<< /Type /Font /Subtype /Type0 /BaseFont /Lib /Encoding /Identity-H /DescendantFonts [6 0 R] /ToUnicode 9 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Lib /FontDescriptor 7 0 R /DW 500 /W [2 [250]] >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /Lib /Flags 4 /FontFile2 8 0 R >>".to_vec(),
        stream("", LIBERATION),
        stream("", to_unicode),
    ];
    let doc = open(b"BT /F1 10 Tf 0 0 Td <0001000200030004> Tj ET", &extra);
    let t = text(&doc);
    assert_eq!(t.plain_text(), "ABCfi");
    let x: Vec<f64> = t.glyphs.iter().map(|g| g.baseline.0.x).collect();
    assert_eq!(x, vec![0.0, 5.0, 7.5, 12.5]);
}

#[test]
fn type3_glyphs_run_their_procedures() {
    let extra = vec![
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /box 6 0 R >> /Encoding << /Differences [97 /box] >> /FirstChar 97 /LastChar 97 /Widths [1000] >>".to_vec(),
        stream("", b"1000 0 0 0 1000 1000 d1 0 0 1000 1000 re f"),
    ];
    let doc = open(b"0 0 1 rg BT /F1 20 Tf 10 10 Td (aa) Tj ET", &extra);
    let (p, _) = render(&doc);
    // Two 20 x 20 boxes in the fill colour, side by side.
    assert_eq!(p.pixel(20, 80)[..3], [0, 0, 255]);
    assert_eq!(p.pixel(40, 80)[..3], [0, 0, 255]);
    assert_eq!(p.pixel(55, 80)[..3], [255, 255, 255]);
    let g = text(&doc).glyphs;
    assert!((g[1].baseline.0.x - 30.0).abs() < 1e-9);
}

#[test]
fn hostile_text_does_not_hang_or_panic() {
    let mut extra = truetype_font();
    // A Type 3 glyph that shows itself again.
    extra.push(b"<< /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 9 0 R >> /Encoding << /Differences [97 /a] >> /Resources << /Font << /F9 8 0 R >> >> >>".to_vec());
    extra.push(stream("", b"BT /F9 1000 Tf (aaaa) Tj ET"));
    let content = b"BT /Missing 10 Tf (no font) Tj /F1 1e30 Tf 10 Tz 99 Tr (Q) Tj /F1 10 Tf <> Tj [1 2 (x) /Name] TJ ET \
                    BT /F1 10 Tf 0 0 0 0 0 0 Tm (degenerate) Tj ET";
    let mut bytes = build(content, &extra);
    let page = b"/Font << /F1 5 0 R >>";
    let at = bytes.windows(page.len()).position(|w| w == page).unwrap();
    bytes.splice(
        at..at + page.len(),
        b"/Font << /F1 5 0 R /F9 8 0 R >>".iter().copied(),
    );
    let doc = Document::open(bytes).unwrap();
    let _ = render(&doc);
    let doc2 = open(b"BT /F9 10 Tf (a) Tj ET", &[]);
    let _ = render(&doc2);
    let _ = text(&doc);
}

#[test]
fn predefined_cjk_cmaps_give_cids_and_text() {
    // Shift-JIS through 90ms-RKSJ-H, with no ToUnicode: the text comes from
    // the Adobe-Japan1 collection. The font is embedded with Identity
    // CIDToGIDMap so no system font is involved; CID widths prove the CIDs.
    let extra = vec![
        b"<< /Type /Font /Subtype /Type0 /BaseFont /Lib /Encoding /90ms-RKSJ-H /DescendantFonts [6 0 R] >>".to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Lib /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 6 >> /FontDescriptor 7 0 R /DW 1000 /W [843 [500]] >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /Lib /Flags 4 /FontFile2 8 0 R >>".to_vec(),
        stream("", LIBERATION),
    ];
    // あ (82A0, CID 843), 日本 (93FA 967B), then an ASCII A.
    let doc = open(b"BT /F1 10 Tf 0 0 Td <82A093FA967B41> Tj ET", &extra);
    let t = text(&doc);
    assert_eq!(t.plain_text(), "あ日本A");
    let x: Vec<f64> = t.glyphs.iter().map(|g| g.baseline.0.x).collect();
    assert_eq!(x, vec![0.0, 5.0, 15.0, 25.0]);
}
