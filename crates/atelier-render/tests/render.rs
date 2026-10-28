//! Renders small hand-written pages and checks pixels. Pages are 100 x 100
//! points rendered at 72 dpi, so user point (x, y) is device pixel
//! (x, 100 - y).

use atelier_raster::Pixmap;
use atelier_render::{RenderOptions, render_page};
use atelier_syntax::Document;

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nendstream");
    s
}

fn build(page_extra: &str, content: &[u8], resources: &str, extra: &[Vec<u8>]) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] {page_extra} /Resources {resources} /Contents 4 0 R >>")
            .into_bytes(),
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

fn render_with(page_extra: &str, content: &[u8], resources: &str, extra: &[Vec<u8>]) -> Pixmap {
    let doc = Document::open(build(page_extra, content, resources, extra)).unwrap();
    let page = &doc.pages().unwrap()[0];
    let options = RenderOptions {
        scale: 1.0,
        ..RenderOptions::default()
    };
    render_page(&doc, page, &options).unwrap().pixmap
}

fn render(content: &[u8]) -> Pixmap {
    render_with("", content, "<< >>", &[])
}

/// RGB at user-space point (x, y).
fn at(p: &Pixmap, x: usize, y: usize) -> [u8; 3] {
    let px = p.pixel(x, 99 - y);
    [px[0], px[1], px[2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];

#[test]
fn fills_a_rectangle() {
    let p = render(b"1 0 0 rg 10 10 30 20 re f");
    assert_eq!((p.width(), p.height()), (100, 100));
    assert_eq!(at(&p, 20, 20), RED);
    assert_eq!(at(&p, 50, 50), WHITE);
    assert_eq!(at(&p, 20, 35), WHITE);
}

#[test]
fn even_odd_leaves_a_hole() {
    let p = render(b"0 0 1 rg 10 10 80 80 re 30 30 40 40 re f*");
    assert_eq!(at(&p, 15, 15), BLUE);
    assert_eq!(at(&p, 50, 50), WHITE);
    let p = render(b"0 0 1 rg 10 10 80 80 re 30 30 40 40 re f");
    assert_eq!(at(&p, 50, 50), BLUE);
}

#[test]
fn clip_limits_painting_and_q_restores_it() {
    let p = render(b"q 0 0 50 100 re W n 1 0 0 rg 0 0 100 100 re f Q 0 0 1 rg 60 60 10 10 re f");
    assert_eq!(at(&p, 25, 50), RED);
    assert_eq!(at(&p, 75, 20), WHITE);
    assert_eq!(at(&p, 65, 65), BLUE);
}

#[test]
fn stroke_width_and_color() {
    let p = render(b"0 0 1 RG 10 w 10 50 m 90 50 l S");
    assert_eq!(at(&p, 50, 50), BLUE);
    assert_eq!(at(&p, 50, 53), BLUE);
    assert_eq!(at(&p, 50, 57), WHITE);
    // Butt caps: nothing beyond the endpoints.
    assert_eq!(at(&p, 5, 50), WHITE);
}

#[test]
fn ctm_and_q_q_nesting() {
    let p = render(b"q 2 0 0 2 0 0 cm 1 0 0 rg 0 0 10 10 re f Q 0 0 1 rg 50 50 10 10 re f");
    assert_eq!(at(&p, 15, 15), RED);
    assert_eq!(at(&p, 25, 25), WHITE);
    assert_eq!(at(&p, 55, 55), BLUE);
}

#[test]
fn constant_alpha_blends_with_white() {
    let p = render_with(
        "",
        b"/G1 gs 1 0 0 rg 0 0 100 100 re f",
        "<< /ExtGState << /G1 << /ca 0.5 >> >> >>",
        &[],
    );
    let [r, g, b] = at(&p, 50, 50);
    assert_eq!(r, 255);
    assert!((126..=129).contains(&g) && g == b, "{g} {b}");
}

#[test]
fn rotation_moves_content() {
    // Rotate 90: the page's bottom-left corner ends up at the top-left.
    let doc = Document::open(build(
        "/MediaBox [0 0 100 50] /Rotate 90",
        b"1 0 0 rg 0 0 10 10 re f",
        "<< >>",
        &[],
    ))
    .unwrap();
    let page = &doc.pages().unwrap()[0];
    let p = render_page(
        &doc,
        page,
        &RenderOptions {
            scale: 1.0,
            ..RenderOptions::default()
        },
    )
    .unwrap()
    .pixmap;
    assert_eq!((p.width(), p.height()), (50, 100));
    assert_eq!(&p.pixel(5, 5)[..3], &RED);
    assert_eq!(&p.pixel(45, 95)[..3], &WHITE);
}

#[test]
fn form_xobject_is_clipped_to_its_bbox() {
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 20 20] /Matrix [1 0 0 1 10 10]",
        b"1 0 0 rg 0 0 60 60 re f",
    );
    let p = render_with("", b"/F Do", "<< /XObject << /F 5 0 R >> >>", &[form]);
    assert_eq!(at(&p, 20, 20), RED);
    assert_eq!(at(&p, 40, 40), WHITE);
}

#[test]
fn self_referencing_form_terminates() {
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /XObject << /F 5 0 R >> >>",
        b"1 0 0 rg 0 0 10 10 re f /F Do",
    );
    let p = render_with("", b"/F Do", "<< /XObject << /F 5 0 R >> >>", &[form]);
    assert_eq!(at(&p, 5, 5), RED);
}

#[test]
fn rgb_image_and_stencil_mask() {
    // 2x1 RGB image: red then blue, drawn over the left half.
    let image = stream(
        "/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[255, 0, 0, 0, 0, 255],
    );
    // 1x2 stencil: top row paints (0), bottom row does not (1).
    let mask = stream(
        "/Type /XObject /Subtype /Image /Width 1 /Height 2 /ImageMask true",
        &[0x00, 0x80],
    );
    let p = render_with(
        "",
        b"q 50 0 0 100 0 0 cm /I Do Q 0 1 0 rg q 50 0 0 100 50 0 cm /M Do Q",
        "<< /XObject << /I 5 0 R /M 6 0 R >> >>",
        &[image, mask],
    );
    assert_eq!(at(&p, 10, 50), RED);
    assert_eq!(at(&p, 40, 50), BLUE);
    assert_eq!(at(&p, 75, 75), [0, 255, 0]);
    assert_eq!(at(&p, 75, 25), WHITE);
}

#[test]
fn inline_image() {
    let p = render(b"q 100 0 0 100 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /RGB ID \xff\x00\x00 EI Q");
    assert_eq!(at(&p, 50, 50), RED);
}

#[test]
fn malformed_content_still_renders_the_rest() {
    let p = render(b"1 0 0 rg 0 0 m ] >> 5 re f 1 2 3 cm (oops 10 10 20 20 re f");
    assert_eq!(at(&p, 50, 50), WHITE);
    let p = render(b"1 0 0 rg } 10 10 20 20 re /Junk 1 2 3 f");
    assert_eq!(at(&p, 20, 20), RED);
}

/// Random operator soup must never panic or hang.
#[test]
fn random_content_never_panics() {
    let words: [&[u8]; 40] = [
        b"q",
        b"Q",
        b"cm",
        b"m",
        b"l",
        b"c",
        b"v",
        b"y",
        b"h",
        b"re",
        b"S",
        b"s",
        b"f",
        b"f*",
        b"B",
        b"b*",
        b"n",
        b"W",
        b"W*",
        b"w",
        b"J",
        b"j",
        b"M",
        b"d",
        b"rg",
        b"RG",
        b"k",
        b"g",
        b"cs",
        b"scn",
        b"Do",
        b"0",
        b"1",
        b"-3.5",
        b"1e9",
        b"500",
        b"[1 2]",
        b"/F",
        b"/DeviceRGB",
        b"0.001",
    ];
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /XObject << /F 5 0 R >> >>",
        b"0 0 m 50 50 l S /F Do",
    );
    let mut state = 0x1234_5678_u32;
    for _ in 0..300 {
        let mut content = Vec::new();
        for _ in 0..200 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            content.extend_from_slice(words[state as usize % words.len()]);
            content.push(b' ');
        }
        render_with(
            "",
            &content,
            "<< /XObject << /F 5 0 R >> >>",
            std::slice::from_ref(&form),
        );
    }
}
