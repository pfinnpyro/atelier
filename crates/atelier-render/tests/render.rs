//! Renders small hand-written pages and checks pixels. Pages are 100 x 100
//! points rendered at 72 dpi, so user point (x, y) is device pixel
//! (x, 100 - y).

use atelier_raster::Pixmap;
use atelier_render::{RenderOptions, render_page, render_region};
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

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 8)
}

#[test]
fn jpeg_image_with_jpeg_soft_mask() {
    // 16x16 JPEG: red left half, blue right half. Its soft mask is a gray
    // JPEG, opaque on top and clear at the bottom. The dictionary's wrong
    // Width and missing ColorSpace are overridden by the JPEG itself.
    let jpeg = include_bytes!("data/red_blue.jpg");
    let mask = include_bytes!("data/half_mask.jpg");
    let image = stream(
        "/Type /XObject /Subtype /Image /Width 3 /Height 16 /BitsPerComponent 8 \
         /Filter /DCTDecode /SMask 6 0 R",
        jpeg,
    );
    let smask = stream(
        "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Filter /DCTDecode",
        mask,
    );
    let p = render_with(
        "",
        b"q 100 0 0 100 0 0 cm /I Do Q",
        "<< /XObject << /I 5 0 R >> >>",
        &[image, smask],
    );
    assert!(near(at(&p, 20, 80), RED), "{:?}", at(&p, 20, 80));
    assert!(near(at(&p, 80, 80), BLUE), "{:?}", at(&p, 80, 80));
    assert!(near(at(&p, 20, 20), WHITE), "{:?}", at(&p, 20, 20));
    assert!(near(at(&p, 80, 20), WHITE), "{:?}", at(&p, 80, 20));
}

#[test]
fn broken_jpeg_is_skipped_with_a_warning() {
    let image = stream(
        "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB \
         /BitsPerComponent 8 /Filter /DCTDecode",
        b"\xff\xd8not a jpeg",
    );
    let doc = Document::open(build(
        "",
        b"q 100 0 0 100 0 0 cm /I Do Q",
        "<< /XObject << /I 5 0 R >> >>",
        &[image],
    ))
    .unwrap();
    let page = &doc.pages().unwrap()[0];
    let options = RenderOptions {
        scale: 1.0,
        ..RenderOptions::default()
    };
    let r = render_page(&doc, page, &options).unwrap();
    assert!(
        r.warnings.iter().any(|w| w.contains("JPEG")),
        "{:?}",
        r.warnings
    );
    assert_eq!(at(&r.pixmap, 50, 50), WHITE);
}

#[test]
fn region_matches_the_same_area_of_the_full_page() {
    // A diagonal stroke and a circle-ish curve, on a rotated page, at a
    // scale that does not divide evenly.
    let content = b"0 0 1 rg 10 10 m 90 30 l 50 95 l f 1 0 0 RG 3 w 5 5 m 95 95 l S \
                    0 g 50 50 m 80 50 80 80 50 80 c 20 80 20 50 50 50 c f";
    for rotate in ["", "/Rotate 90"] {
        let doc = Document::open(build(rotate, content, "<< >>", &[])).unwrap();
        let page = &doc.pages().unwrap()[0];
        let options = RenderOptions {
            scale: 1.7,
            ..RenderOptions::default()
        };
        let full = render_page(&doc, page, &options).unwrap().pixmap;
        for (x, y, w, h) in [(0, 0, 60, 60), (60, 60, 40, 40), (13, 97, 50, 73)] {
            let tile = render_region(&doc, page, &options, (x, y, w, h))
                .unwrap()
                .pixmap;
            assert_eq!((tile.width(), tile.height()), (w, h));
            for ty in 0..h {
                for tx in 0..w {
                    assert_eq!(
                        tile.pixel(tx, ty),
                        full.pixel(x + tx, y + ty),
                        "{rotate} tile at ({x}, {y}), pixel ({tx}, {ty})"
                    );
                }
            }
        }
    }
}

const AXIAL_RED_BLUE: &str = "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [20 0 80 0] \
     /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>";

#[test]
fn axial_shading_blends_and_extends() {
    let p = render_with(
        "",
        b"/S sh",
        &format!("<< /Shading << /S {AXIAL_RED_BLUE} >> >>"),
        &[],
    );
    // Without Extend nothing is painted outside the axis.
    assert_eq!(at(&p, 10, 50), WHITE);
    assert_eq!(at(&p, 90, 50), WHITE);
    assert!(near(at(&p, 21, 50), [251, 0, 4]));
    assert!(near(at(&p, 50, 50), [128, 0, 127]));
    assert!(near(at(&p, 79, 50), [4, 0, 251]));

    let extended = AXIAL_RED_BLUE.replace(">> >>", ">> /Extend [true true] >>");
    let p = render_with(
        "",
        b"/S sh",
        &format!("<< /Shading << /S {extended} >> >>"),
        &[],
    );
    assert_eq!(at(&p, 5, 50), RED);
    assert_eq!(at(&p, 95, 50), BLUE);
}

#[test]
fn radial_shading_is_round() {
    let shading = "<< /ShadingType 3 /ColorSpace /DeviceGray /Coords [50 50 0 50 50 40] \
         /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >>";
    let p = render_with(
        "",
        b"/S sh",
        &format!("<< /Shading << /S {shading} >> >>"),
        &[],
    );
    assert!(near(at(&p, 50, 50), [0, 0, 0]));
    // Same distance from the centre, same colour.
    assert_eq!(at(&p, 70, 50), at(&p, 50, 70));
    assert!(near(at(&p, 70, 50), [128, 128, 128]));
    // Outside the end circle, unextended.
    assert_eq!(at(&p, 95, 95), WHITE);
}

#[test]
fn shading_respects_the_clip() {
    let extended = AXIAL_RED_BLUE.replace(">> >>", ">> /Extend [true true] >>");
    let p = render_with(
        "",
        b"q 40 40 20 20 re W n /S sh Q",
        &format!("<< /Shading << /S {extended} >> >>"),
        &[],
    );
    assert_eq!(at(&p, 30, 50), WHITE);
    assert_eq!(at(&p, 50, 30), WHITE);
    assert!(near(at(&p, 50, 50), [128, 0, 127]));
}

#[test]
fn triangle_mesh_interpolates_vertex_colours() {
    // Free-form mesh: 8-bit coordinates over [0 100], 8-bit RGB.
    // Each vertex is a flag byte, x, y, then r, g, b.
    let data = [
        0u8, 0, 0, 255, 0, 0, // red at (0, 0)
        0, 255, 0, 0, 255, 0, // green at (100, 0)
        0, 0, 255, 0, 0, 255, // blue at (0, 100)
    ];
    let mesh = stream(
        "/ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 \
         /BitsPerFlag 8 /Decode [0 100 0 100 0 1 0 1 0 1]",
        &data,
    );
    let p = render_with("", b"/M sh", "<< /Shading << /M 5 0 R >> >>", &[mesh]);
    assert!(near(at(&p, 2, 2), [250, 4, 4]));
    assert!(near(at(&p, 96, 2), [10, 245, 0]));
    assert!(near(at(&p, 2, 96), [10, 0, 245]));
    // The other half of the square is outside the triangle.
    assert_eq!(at(&p, 80, 80), WHITE);
}

#[test]
fn coloured_tiling_pattern_repeats() {
    // 10 x 10 cell: red square in the lower left quarter.
    let cell = stream(
        "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 \
         /Resources << >>",
        b"1 0 0 rg 0 0 5 5 re f",
    );
    let p = render_with(
        "",
        b"/Pattern cs /P scn 0 0 100 100 re f",
        "<< /Pattern << /P 5 0 R >> >>",
        &[cell],
    );
    for (x, y) in [(2, 2), (32, 72), (92, 12)] {
        assert_eq!(at(&p, x, y), RED, "({x}, {y})");
    }
    for (x, y) in [(7, 2), (2, 7), (37, 77)] {
        assert_eq!(at(&p, x, y), WHITE, "({x}, {y})");
    }
}

#[test]
fn uncoloured_pattern_takes_the_fill_colour_and_matrix() {
    // Stencil cell drawn in whatever colour is set; the matrix moves it by 5.
    let cell = stream(
        "/PatternType 1 /PaintType 2 /TilingType 1 /BBox [0 0 10 10] /XStep 20 /YStep 20 \
         /Matrix [1 0 0 1 5 5] /Resources << >>",
        b"1 0 0 rg 0 0 10 10 re f",
    );
    let p = render_with(
        "",
        b"/P2 cs 0 0 1 /P scn 0 0 100 100 re f",
        "<< /Pattern << /P 5 0 R >> /ColorSpace << /P2 [/Pattern /DeviceRGB] >> >>",
        &[cell],
    );
    // The cell's own red is ignored.
    assert_eq!(at(&p, 10, 10), BLUE);
    assert_eq!(at(&p, 50, 30), BLUE);
    assert_eq!(at(&p, 2, 2), WHITE);
    assert_eq!(at(&p, 20, 10), WHITE);
}

#[test]
fn shading_pattern_fills_and_strokes() {
    let extended = AXIAL_RED_BLUE.replace(">> >>", ">> /Extend [true true] >>");
    let p = render_with(
        "",
        b"/Pattern cs /P scn 0 0 100 40 re f /Pattern CS /P SCN 10 w 0 70 m 100 70 l S",
        &format!("<< /Pattern << /P << /PatternType 2 /Shading {extended} >> >> >>"),
        &[],
    );
    assert_eq!(at(&p, 5, 20), RED);
    assert_eq!(at(&p, 95, 20), BLUE);
    assert!(near(at(&p, 50, 70), [128, 0, 127]));
    assert_eq!(at(&p, 50, 50), WHITE);
}

#[test]
fn separation_uses_its_tint_transform() {
    // Tint t maps to RGB (t, 0, 1 - t).
    let resources = "<< /ColorSpace << /Spot [/Separation /Spot /DeviceRGB \
         << /FunctionType 2 /Domain [0 1] /C0 [0 0 1] /C1 [1 0 0] /N 1 >>] >> >>";
    let p = render_with(
        "",
        b"/Spot cs 1 sc 0 0 50 100 re f 0 sc 50 0 50 100 re f",
        resources,
        &[],
    );
    assert_eq!(at(&p, 25, 50), RED);
    assert_eq!(at(&p, 75, 50), BLUE);
}

#[test]
fn multiply_blend_mode_darkens() {
    // Red, then 50% grey multiplied over half of it.
    let p = render_with(
        "",
        b"1 0 0 rg 0 0 100 100 re f /M gs 0.5 g 50 0 50 100 re f",
        "<< /ExtGState << /M << /BM /Multiply >> >> >>",
        &[],
    );
    assert_eq!(at(&p, 25, 50), RED);
    assert!(near(at(&p, 75, 50), [128, 0, 0]), "{:?}", at(&p, 75, 50));
}

#[test]
fn blend_mode_array_and_q_restore() {
    // The first known mode in the array wins; Q goes back to Normal.
    let p = render_with(
        "",
        b"0 0 1 rg 0 0 100 100 re f q /S gs 1 0 0 rg 0 0 50 100 re f Q 1 0 0 rg 50 0 50 50 re f",
        "<< /ExtGState << /S << /BM [/NoSuchMode /Screen] >> >> >>",
        &[],
    );
    assert_eq!(at(&p, 25, 50), [255, 0, 255]);
    assert_eq!(at(&p, 75, 25), RED);
    assert_eq!(at(&p, 75, 75), BLUE);
}

#[test]
fn luminosity_soft_mask_fades_painting() {
    // The mask group is white on the left half, black on the right, and
    // 50% grey in the top right corner.
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] \
         /Group << /S /Transparency /CS /DeviceGray >>",
        b"1 g 0 0 50 100 re f 0.5 g 50 50 50 50 re f",
    );
    let p = render_with(
        "",
        b"/SM gs 0 0 1 rg 0 0 100 100 re f",
        "<< /ExtGState << /SM << /SMask << /S /Luminosity /G 5 0 R >> >> >> >>",
        &[group],
    );
    assert_eq!(at(&p, 25, 50), BLUE);
    assert_eq!(at(&p, 75, 25), WHITE);
    assert!(
        near(at(&p, 75, 75), [127, 127, 255]),
        "{:?}",
        at(&p, 75, 75)
    );
}

#[test]
fn alpha_soft_mask_and_none_turns_it_off() {
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Group << /S /Transparency >>",
        b"0 0 0 rg 0 0 100 50 re f",
    );
    let p = render_with(
        "",
        b"q /SM gs 1 0 0 rg 0 0 100 100 re f Q /Off gs 0 0 1 rg 0 0 20 20 re f",
        "<< /ExtGState << /SM << /SMask << /S /Alpha /G 5 0 R >> >> \
           /Off << /SMask /None >> >> >>",
        &[group],
    );
    assert_eq!(at(&p, 50, 25), RED);
    assert_eq!(at(&p, 50, 75), WHITE);
    assert_eq!(at(&p, 10, 10), BLUE);
}

#[test]
fn group_opacity_applies_to_the_group_as_a_whole() {
    // Two overlapping opaque squares in a group drawn at 50%: the overlap
    // is no darker than the rest.
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Group << /S /Transparency >>",
        b"0 0 1 rg 10 10 50 50 re f 40 40 50 50 re f",
    );
    let p = render_with(
        "",
        b"/H gs /G Do",
        "<< /ExtGState << /H << /ca 0.5 >> >> /XObject << /G 5 0 R >> >>",
        &[group],
    );
    let half = at(&p, 20, 20);
    assert!(near(half, [127, 127, 255]), "{half:?}");
    assert_eq!(at(&p, 50, 50), half);
    assert_eq!(at(&p, 80, 80), half);
    assert_eq!(at(&p, 80, 20), WHITE);
}

#[test]
fn isolated_group_blends_against_nothing() {
    // Inside an isolated group, Difference has nothing to mix with, so the
    // green square keeps its colour over the red page.
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] \
         /Group << /S /Transparency /I true >> /Resources << /ExtGState << /D << /BM /Difference >> >> >>",
        b"/D gs 0 1 0 rg 20 20 60 60 re f",
    );
    let p = render_with(
        "",
        b"1 0 0 rg 0 0 100 100 re f /G Do",
        "<< /XObject << /G 5 0 R >> >>",
        &[group],
    );
    assert_eq!(at(&p, 50, 50), [0, 255, 0]);
    assert_eq!(at(&p, 10, 10), RED);
}

#[test]
fn group_is_clipped_by_the_page_clip() {
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Group << /S /Transparency >>",
        b"0 0 1 rg 0 0 100 100 re f",
    );
    let p = render_with(
        "",
        b"0 0 50 100 re W n /H gs /G Do",
        "<< /ExtGState << /H << /ca 0.5 >> >> /XObject << /G 5 0 R >> >>",
        &[group],
    );
    assert!(near(at(&p, 25, 50), [127, 127, 255]));
    assert_eq!(at(&p, 75, 50), WHITE);
}

#[test]
fn soft_mask_that_uses_itself_terminates() {
    // The mask group sets the same soft mask while it is being drawn.
    let group = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Group << /S /Transparency >> \
         /Resources << /ExtGState << /SM 6 0 R >> >>",
        b"/SM gs 0 g 0 0 100 100 re f",
    );
    let p = render_with(
        "",
        b"/SM gs 1 0 0 rg 0 0 100 100 re f",
        "<< /ExtGState << /SM 6 0 R >> >>",
        &[group, b"<< /SMask << /S /Alpha /G 5 0 R >> >>".to_vec()],
    );
    assert_eq!(at(&p, 50, 50), RED);
}

#[test]
fn ccitt_group_4_image() {
    // 8 x 2 pixels, both rows white-white-black-black-black-white-white-
    // white: H(2 white, 3 black) V0 on the first row, V0 V0 V0 on the
    // second.
    let image = stream(
        "/Type /XObject /Subtype /Image /Width 8 /Height 2 /ColorSpace /DeviceGray \
         /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 8 >>",
        &[0b0010_1111, 0b0111_1000],
    );
    let p = render_with(
        "",
        b"100 0 0 100 0 0 cm /Im Do",
        "<< /XObject << /Im 5 0 R >> >>",
        &[image],
    );
    assert_eq!(at(&p, 10, 50), WHITE);
    assert_eq!(at(&p, 40, 25), [0, 0, 0]);
    assert_eq!(at(&p, 40, 75), [0, 0, 0]);
    assert_eq!(at(&p, 80, 50), WHITE);
}

#[test]
fn jbig2_image_with_globals() {
    // jbig2enc's symbol coding of a 400 x 120 page, with the symbol
    // dictionary in a JBIG2Globals stream. Drawn at one pixel per unit, so
    // the top left 100 x 100 must match the source bitmap exactly.
    let data = |name: &str| {
        std::fs::read(format!(
            "{}/../atelier-filters/tests/data/jbig2/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    };
    let image = stream(
        "/Type /XObject /Subtype /Image /Width 400 /Height 120 /ColorSpace /DeviceGray \
         /BitsPerComponent 1 /Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 6 0 R >>",
        &data("symbol.jb2"),
    );
    let globals = stream("", &data("symbol-globals.jb2"));
    let p = render_with(
        "",
        b"400 0 0 120 0 -20 cm /Im Do",
        "<< /XObject << /Im 5 0 R >> >>",
        &[image, globals],
    );
    let pbm = data("symbol.pbm");
    let pixels = &pbm[pbm.len() - 50 * 120..];
    let mut black = 0;
    for y in 0..100 {
        for x in 0..100 {
            let bit = (pixels[y * 50 + x / 8] >> (7 - x % 8)) & 1;
            let expected = if bit == 1 { [0, 0, 0] } else { WHITE };
            assert_eq!(at(&p, x, 99 - y), expected, "({x}, {y})");
            black += usize::from(bit);
        }
    }
    assert!(black > 500, "the test area should hold text");
}
