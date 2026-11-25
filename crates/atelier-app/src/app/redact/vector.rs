use std::collections::HashSet;
use std::sync::Arc;

use atelier_syntax::{Dict, Document, Error, ObjRef, Object, Page, Rect, Result, Stream};
use eframe::egui;

use super::{Marks, empty_pdf, name, only_keys, set_page_tree};

struct ExpectedPage {
    page: ObjRef,
    dictionary: Dict,
    streams: Vec<ExpectedStream>,
    regions: Vec<Rect>,
}

struct ExpectedStream {
    reference: ObjRef,
    dictionary: Dict,
    length: usize,
    digest: [u8; 32],
}

impl ExpectedStream {
    fn new(reference: ObjRef, stream: &Stream) -> Self {
        Self {
            reference,
            dictionary: stream.dict.clone(),
            length: stream.data.len(),
            digest: atelier_crypto::sha256(&stream.data),
        }
    }

    fn matches(&self, copy: &Document) -> Result<bool> {
        let Object::Stream(mut stream) = copy.get(self.reference)? else {
            return Ok(false);
        };
        Ok(
            stream.dict.remove(b"Length") == Some(Object::Integer(self.length as i64))
                && stream.dict.to_string() == self.dictionary.to_string()
                && stream.data.len() == self.length
                && atelier_crypto::sha256(&stream.data) == self.digest,
        )
    }
}

fn region(page: &Page, mark: egui::Rect) -> Result<Rect> {
    if !mark.is_finite()
        || mark.width() <= 0.0
        || mark.height() <= 0.0
        || mark.min.x < 0.0
        || mark.min.y < 0.0
        || mark.max.x > 1.0
        || mark.max.y > 1.0
    {
        return Err(Error::Structure("Invalid redaction region".into()));
    }
    let crop = page.crop_box;
    let user = |point: egui::Pos2| {
        let (u, v) = (f64::from(point.x), f64::from(point.y));
        let (x, y) = match page.rotate {
            90 => (v, 1.0 - u),
            180 => (1.0 - u, 1.0 - v),
            270 => (1.0 - v, u),
            _ => (u, v),
        };
        (crop.x0 + x * crop.width(), crop.y1 - y * crop.height())
    };
    let a = user(mark.min);
    let b = user(mark.max);
    // Allow for thumbnail sampling and PDF coordinate rounding at the mask edge.
    Rect {
        x0: a.0.min(b.0) - 1.0,
        x1: a.0.max(b.0) + 1.0,
        y0: a.1.min(b.1) - 1.0,
        y1: a.1.max(b.1) + 1.0,
    }
    .intersect(&crop)
    .ok_or_else(|| Error::Structure("The redaction is outside the page".into()))
}

fn verify(copy: &Document, expected: &[ExpectedPage]) -> Result<()> {
    let fail =
        || Error::Structure("The vector redaction failed verification. No file was saved.".into());
    let root = ObjRef::new(1, 0);
    let parent = ObjRef::new(2, 0);
    let ids: Vec<_> = [root.num, parent.num]
        .into_iter()
        .chain(expected.iter().flat_map(|p| {
            p.streams
                .iter()
                .map(|s| s.reference.num)
                .chain([p.page.num])
        }))
        .collect();
    if copy.was_repaired()
        || !only_keys(copy.trailer(), &[b"Root", b"Size"])
        || copy.trailer().get(b"Root") != Some(&Object::Reference(root))
        || copy.object_numbers() != ids
    {
        return Err(fail());
    }
    let catalog = copy.catalog()?;
    if !only_keys(&catalog, &[b"Type", b"Pages"])
        || !catalog.is_type(b"Catalog")
        || catalog.get(b"Pages") != Some(&Object::Reference(parent))
    {
        return Err(fail());
    }
    let Object::Dict(tree) = copy.get(parent)? else {
        return Err(fail());
    };
    let kids: Vec<_> = expected.iter().map(|p| Object::Reference(p.page)).collect();
    if !only_keys(&tree, &[b"Type", b"Count", b"Kids"])
        || !tree.is_type(b"Pages")
        || tree.get(b"Count") != Some(&Object::Integer(expected.len() as i64))
        || tree.get(b"Kids") != Some(&Object::Array(kids))
    {
        return Err(fail());
    }
    let pages = copy.pages()?;
    if pages.len() != expected.len() {
        return Err(fail());
    }
    for (page, expected) in pages.iter().zip(expected) {
        // Compare canonical PDF objects so integral reals and parsed integers
        // have the same representation.
        if page.obj_ref != Some(expected.page)
            || page.dict.to_string() != expected.dictionary.to_string()
        {
            return Err(fail());
        }
        for stream in &expected.streams {
            if !stream.matches(copy)? {
                return Err(fail());
            }
        }
        if !expected.regions.is_empty() {
            for glyph in atelier_render::extract_text(copy, page)?.glyphs {
                let mut bounds = Rect {
                    x0: f64::INFINITY,
                    y0: f64::INFINITY,
                    x1: f64::NEG_INFINITY,
                    y1: f64::NEG_INFINITY,
                };
                for point in glyph.quad {
                    bounds.x0 = bounds.x0.min(point.x);
                    bounds.y0 = bounds.y0.min(point.y);
                    bounds.x1 = bounds.x1.max(point.x);
                    bounds.y1 = bounds.y1.max(point.y);
                }
                if expected
                    .regions
                    .iter()
                    .any(|region| bounds.intersect(region).is_some())
                {
                    return Err(fail());
                }
            }
            if !marks_render_black(copy, page, &expected.regions)? {
                return Err(fail());
            }
        }
    }
    if !copy.warnings().is_empty() {
        return Err(fail());
    }
    Ok(())
}

fn marks_render_black(copy: &Document, page: &Page, regions: &[Rect]) -> Result<bool> {
    let long_edge = page.crop_box.width().max(page.crop_box.height()).max(1.0);
    let scale = 1.0_f64.min(1000.0 / long_edge);
    let options = atelier_render::RenderOptions {
        scale,
        ..Default::default()
    };
    let rendered = atelier_render::render_page(copy, page, &options)?;
    if !rendered.warnings.is_empty() {
        return Ok(false);
    }
    let (matrix, width, height) = atelier_render::page_transform(page, scale);
    for r in regions {
        let a = matrix.apply(atelier_raster::Point::new(r.x0, r.y0));
        let b = matrix.apply(atelier_raster::Point::new(r.x1, r.y1));
        // Avoid antialiased edges. The contents digest and glyph checks also
        // apply to marks that are too small for interior pixels at this scale.
        let x = (a.x.min(b.x).ceil() as usize + 1)..(a.x.max(b.x).floor() as usize).min(width);
        let y = (a.y.min(b.y).ceil() as usize + 1)..(a.y.max(b.y).floor() as usize).min(height);
        for y in y {
            for x in x.clone() {
                if rendered.pixmap.pixel(x, y) != [0, 0, 0, 255] {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

pub(super) fn vector_copy(source: &Document, marks: &Marks) -> Result<Arc<[u8]>> {
    source.signature_write_policy().check(true)?;
    if source.is_encrypted() {
        return Err(Error::Unsupported(
            "Redaction requires an unprotected PDF copy".into(),
        ));
    }
    let pages = source.pages()?;
    if marks.values().all(Vec::is_empty) || marks.keys().any(|&page| page >= pages.len()) {
        return Err(Error::Structure(
            "Mark at least one valid region to redact".into(),
        ));
    }
    if pages.len() > 512 || marks.values().any(|regions| regions.len() > 128) {
        return Err(Error::Unsupported(
            "Vector redaction supports up to 512 pages and 128 regions per page".into(),
        ));
    }
    let seed = empty_pdf()?;
    let mut update = seed.update();
    let mut expected = Vec::with_capacity(pages.len());
    let mut bytes = 0;
    for (index, page) in pages.iter().enumerate() {
        let regions: Vec<_> = marks
            .get(&index)
            .into_iter()
            .flatten()
            .map(|&mark| region(page, mark))
            .collect::<Result<_>>()?;
        let clean = atelier_render::redact_page_content(source, page, &regions, &mut update)
            .map_err(|error| Error::Unsupported(format!("Page {}: {error}", index + 1)))?;
        let stream = update.add(Object::Stream(Stream {
            dict: Dict::new(),
            data: clean.content.into(),
        }));
        let streams = clean
            .asset_streams
            .into_iter()
            .chain([stream])
            .map(|reference| {
                let Object::Stream(stream) = update.get(reference)? else {
                    return Err(Error::Structure(
                        "Missing generated redaction stream".into(),
                    ));
                };
                Ok(ExpectedStream::new(reference, &stream))
            })
            .collect::<Result<Vec<_>>>()?;
        bytes += streams.iter().map(|stream| stream.length).sum::<usize>();
        if bytes > 128 * 1024 * 1024 {
            return Err(Error::Unsupported(
                "The redacted copy exceeds the vector export limit".into(),
            ));
        }
        let mut dictionary = Dict::new();
        dictionary.insert("Type", name("Page"));
        dictionary.insert("Parent", Object::Reference(ObjRef::new(2, 0)));
        let crop = page.crop_box;
        dictionary.insert(
            "MediaBox",
            Object::Array(
                [crop.x0, crop.y0, crop.x1, crop.y1]
                    .map(Object::Real)
                    .to_vec(),
            ),
        );
        dictionary.insert("Rotate", Object::Integer(i64::from(page.rotate)));
        dictionary.insert("Resources", Object::Dict(clean.resources));
        if let Some(group) = clean.group {
            dictionary.insert("Group", Object::Dict(group));
        }
        if let Some(unit) = clean.user_unit {
            dictionary.insert("UserUnit", Object::Real(unit));
        }
        dictionary.insert("Contents", Object::Reference(stream));
        let page = update.add(Object::Dict(dictionary.clone()));
        expected.push(ExpectedPage {
            page,
            dictionary,
            streams,
            regions,
        });
    }
    if !source.warnings().is_empty() {
        return Err(Error::Unsupported(
            "The source PDF has warnings that prevent verified vector redaction".into(),
        ));
    }
    set_page_tree(
        &mut update,
        expected
            .iter()
            .map(|page| Object::Reference(page.page))
            .collect(),
    );
    let bytes: Arc<[u8]> = update.to_compact_bytes(&HashSet::new())?.into();
    drop(update);
    let copy = Document::open(bytes.clone())?;
    verify(&copy, &expected)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(content: &[u8]) -> Document {
        let input = Document::open(super::super::super::quality_tests::fixture(1)).unwrap();
        let mut update = input.update();
        update.set(
            ObjRef::new(5, 0),
            Object::Stream(Stream {
                dict: Dict::new(),
                data: Arc::from(content),
            }),
        );
        let mut root = input.catalog().unwrap();
        root.insert(
            "Lang",
            Object::String(atelier_syntax::PdfString::from_text(
                "hidden-catalog-marker",
            )),
        );
        update.set(ObjRef::new(1, 0), Object::Dict(root));
        Document::open(update.to_bytes().unwrap()).unwrap()
    }

    fn with_resource(
        input: &Document,
        category: &str,
        key: &str,
        install: impl FnOnce(&mut atelier_syntax::Update<'_>) -> Object,
    ) -> Document {
        let page = &input.pages().unwrap()[0];
        let mut update = input.update();
        let mut resources = page.resources.clone();
        let mut entries = input
            .get_key(&resources, category.as_bytes())
            .ok()
            .and_then(|object| object.as_dict().cloned())
            .unwrap_or_default();
        entries.insert(key, install(&mut update));
        resources.insert(category, Object::Dict(entries));
        let mut dictionary = page.dict.clone();
        dictionary.insert("Resources", Object::Dict(resources));
        update.set(page.obj_ref.unwrap(), Object::Dict(dictionary));
        Document::open(update.to_bytes().unwrap()).unwrap()
    }

    fn export_qa_pdf(variable: &str, bytes: &[u8]) {
        if let Some(path) = std::env::var_os(variable) {
            std::fs::write(path, bytes).unwrap();
        }
    }

    #[test]
    fn page_groups_default_color_spaces_and_blending_survive_secure_redaction() {
        let content = b"0.55 g 4 4 190 190 re f
q /OriginalTransparency gs /OriginalPalette cs 1 scn 20 80 120 50 re f Q
0 g BT /Helv 12 Tf 14 100 Td (Keep    PRIVATE    Visible) Tj ET";
        let input = with_resource(&source(content), "ColorSpace", "DefaultGray", |_| {
            let mut params = Dict::new();
            params.insert("Gamma", Object::Integer(2));
            params.insert(
                "WhitePoint",
                Object::Array([0.9505, 1.0, 1.089].map(Object::Real).to_vec()),
            );
            Object::Array(vec![name("CalGray"), Object::Dict(params)])
        });
        let input = with_resource(&input, "ColorSpace", "OriginalPalette", |_| {
            Object::Array(vec![
                name("Indexed"),
                name("DeviceRGB"),
                Object::Integer(2),
                Object::String(atelier_syntax::PdfString {
                    bytes: [
                        b"\x00\x00\x00\x10\xa0\xe0".as_slice(),
                        b"secret-palette-payload",
                    ]
                    .concat(),
                    hex: true,
                }),
            ])
        });
        let input = with_resource(&input, "ExtGState", "OriginalTransparency", |_| {
            let mut dict = Dict::new();
            dict.insert("Type", name("ExtGState"));
            dict.insert("ca", Object::Real(0.43));
            dict.insert("CA", Object::Real(0.62));
            dict.insert(
                "BM",
                Object::Array(vec![name("UnrecognizedBlend"), name("Multiply")]),
            );
            dict.insert("SMask", name("None"));
            Object::Dict(dict)
        });
        let mut update = input.update();
        let mut page = input.pages().unwrap()[0].dict.clone();
        let mut group = Dict::new();
        group.insert("Type", name("Group"));
        group.insert("S", name("Transparency"));
        group.insert("CS", name("DeviceRGB"));
        group.insert("I", Object::Bool(true));
        group.insert("K", Object::Bool(false));
        group.insert("PrivateMetadata", name("secret-group-payload"));
        page.insert("Group", Object::Reference(update.add(Object::Dict(group))));
        page.insert("UserUnit", Object::Real(1.25));
        update.set(
            input.pages().unwrap()[0].obj_ref.unwrap(),
            Object::Dict(page),
        );
        let input = Document::open(update.to_bytes().unwrap()).unwrap();
        let page = &input.pages().unwrap()[0];
        let glyphs = atelier_render::extract_text(&input, page).unwrap().glyphs;
        let start = glyphs.iter().position(|g| g.text == "P").unwrap();
        let mark = egui::Rect::from_min_max(
            egui::pos2(glyphs[start].quad[0].x as f32 / 200.0, 0.4),
            egui::pos2(glyphs[start + 6].quad[1].x as f32 / 200.0, 0.55),
        );
        let bytes = vector_copy(&input, &Marks::from([(0, vec![mark])])).unwrap();
        export_qa_pdf("ATELIER_QA_TRANSPARENCY_REDACTED_PDF", &bytes);
        for marker in [
            "secret-palette-payload",
            "secret-group-payload",
            "OriginalTransparency",
            "OriginalPalette",
            "UnrecognizedBlend",
        ] {
            assert!(
                !bytes.windows(marker.len()).any(|w| w == marker.as_bytes()),
                "{marker}"
            );
        }
        let copy = Document::open(bytes).unwrap();
        let output_page = &copy.pages().unwrap()[0];
        let group = output_page.dict.get(b"Group").unwrap().as_dict().unwrap();
        assert_eq!(group.get(b"CS"), Some(&name("DeviceRGB")));
        assert_eq!(group.get(b"I"), Some(&Object::Bool(true)));
        assert_eq!(group.get(b"K"), Some(&Object::Bool(false)));
        assert_eq!(output_page.dict.get(b"UserUnit"), Some(&Object::Real(1.25)));
        assert!(!output_page.resources.contains_key(b"ColorSpace"));
        let object = copy.get_key(&output_page.resources, b"ExtGState").unwrap();
        let states = object.as_dict().unwrap();
        assert_eq!(states.len(), 1);
        let state = states.get(b"G0").unwrap().as_dict().unwrap();
        assert_eq!(state.get(b"BM"), Some(&name("Multiply")));
        assert_eq!(state.get(b"ca"), Some(&Object::Real(0.43)));
        let selected = atelier_render::extract_text(&copy, output_page)
            .unwrap()
            .plain_text();
        assert!(
            selected.contains("Keep")
                && selected.contains("Visible")
                && !selected.contains("PRIVATE")
        );
        let options = atelier_render::RenderOptions {
            scale: 1.0,
            ..Default::default()
        };
        let before = atelier_render::render_page(&input, page, &options).unwrap();
        let after = atelier_render::render_page(&copy, output_page, &options).unwrap();
        for (x, y) in [(40, 76), (129, 91), (150, 120), (50, 140)] {
            let source_pixel = before.pixmap.pixel(x, y);
            assert_ne!(source_pixel, [0, 0, 0, 255]);
            assert_eq!(source_pixel, after.pixmap.pixel(x, y));
        }
        let x = (mark.center().x * 200.0) as usize;
        assert_eq!(after.pixmap.pixel(x, 100), [0, 0, 0, 255]);
    }

    #[test]
    fn redaction_rejects_soft_masks_it_cannot_rebuild() {
        let input = with_resource(
            &source(b"/Mask gs 20 30 40 50 re f"),
            "ExtGState",
            "Mask",
            |_| {
                let mut state = Dict::new();
                state.insert("SMask", Object::Dict(Dict::new()));
                Object::Dict(state)
            },
        );
        let mark = egui::Rect::from_min_max(egui::pos2(0.6, 0.6), egui::pos2(0.8, 0.8));
        let error = vector_copy(&input, &Marks::from([(0, vec![mark])])).unwrap_err();
        assert!(error.to_string().contains("/SMask"));
    }

    #[test]
    fn embedded_font_redaction_rebuilds_only_retained_glyphs_and_mappings() {
        const FONT_MARKER: &[u8] = b"private-in-original-font-file";
        const CMAP_MARKER: &[u8] = b"private-in-original-unicode-map";
        for encoding in ["WinAnsiEncoding", "Identity-H", "Identity-V"] {
            let composite = encoding != "WinAnsiEncoding";
            let vertical = encoding == "Identity-V";
            // The Identity CID codes use an explicit CIDToGID mapping to the
            // same glyphs as the WinAnsi fixture. Their Unicode is in ToUnicode.
            let show = if composite {
                "<00660069006C006C002000200020004D005100570020002000200068006900670068>"
            } else {
                "(fill   MQW   high)"
            };
            let placement = if vertical {
                "9 Tf 1 Tw 100 190 Td"
            } else {
                "12 Tf 1 Tw 10 100 Td"
            };
            let content = format!("0 0 1 rg BT /Embedded {placement} {show} Tj ET");
            let input = with_resource(&source(content.as_bytes()), "Font", "Embedded", |update| {
                let font = include_bytes!("../../../../atelier-font/tests/data/liberation.ttf");
                let mut program = font.to_vec();
                program.extend_from_slice(FONT_MARKER);
                let program = update.add(Object::Stream(Stream {
                    dict: Dict::new(),
                    data: program.into(),
                }));
                let mut descriptor = Dict::new();
                descriptor.insert("Type", name("FontDescriptor"));
                descriptor.insert("FontName", name("LiberationSans"));
                descriptor.insert("Flags", Object::Integer(32));
                descriptor.insert("FontFile2", Object::Reference(program));
                let mut font = Dict::new();
                font.insert("Type", name("Font"));
                font.insert("BaseFont", name("LiberationSans"));
                font.insert("FontDescriptor", Object::Dict(descriptor));
                if !composite {
                    font.insert("Subtype", name("TrueType"));
                    font.insert("Encoding", name("WinAnsiEncoding"));
                    font.insert("FirstChar", Object::Integer(32));
                    font.insert("LastChar", Object::Integer(126));
                    font.insert("Widths", Object::Array(vec![Object::Integer(600); 95]));
                    return Object::Dict(font);
                }
                font.insert("Subtype", name("CIDFontType2"));
                let mut info = Dict::new();
                info.insert(
                    "Registry",
                    Object::String(atelier_syntax::PdfString::from_text("Adobe")),
                );
                info.insert(
                    "Ordering",
                    Object::String(atelier_syntax::PdfString::from_text("Identity")),
                );
                info.insert("Supplement", Object::Integer(0));
                font.insert("CIDSystemInfo", Object::Dict(info));
                // Glyph IDs from the Liberation fixture's cmap.
                let mut gid_map = vec![0; 256];
                for (code, gid) in [
                    (b' ', 1_u16),
                    (b'M', 15),
                    (b'Q', 16),
                    (b'W', 17),
                    (b'f', 21),
                    (b'g', 22),
                    (b'h', 23),
                    (b'i', 24),
                    (b'l', 27),
                ] {
                    let i = usize::from(code) * 2;
                    gid_map[i..i + 2].copy_from_slice(&gid.to_be_bytes());
                }
                font.insert(
                    "CIDToGIDMap",
                    Object::Reference(update.add(Object::Stream(Stream {
                        dict: Dict::new(),
                        data: gid_map.into(),
                    }))),
                );
                font.insert("DW", Object::Integer(600));
                let cmap = format!(
                    "%{}\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n1 beginbfrange\n<0020> <007E> <0020>\nendbfrange\n",
                    String::from_utf8_lossy(CMAP_MARKER)
                );
                let mut parent = Dict::new();
                parent.insert("Type", name("Font"));
                parent.insert("Subtype", name("Type0"));
                parent.insert("BaseFont", name("LiberationSans"));
                parent.insert("Encoding", name(encoding));
                parent.insert("DescendantFonts", Object::Array(vec![Object::Dict(font)]));
                parent.insert(
                    "ToUnicode",
                    Object::Reference(update.add(Object::Stream(Stream {
                        dict: Dict::new(),
                        data: cmap.into_bytes().into(),
                    }))),
                );
                Object::Dict(parent)
            });
            let page = &input.pages().unwrap()[0];
            let source_text = atelier_render::extract_text(&input, page).unwrap();
            let start = source_text
                .glyphs
                .iter()
                .position(|g| g.text == "M")
                .unwrap();
            let secret = &source_text.glyphs[start..start + 3];
            let mut mark = egui::Rect::NOTHING;
            for p in secret.iter().flat_map(|g| g.quad) {
                mark.extend_with(egui::pos2((p.x / 200.0) as f32, (1.0 - p.y / 200.0) as f32));
            }
            let bytes = vector_copy(&input, &Marks::from([(0, vec![mark])])).unwrap();
            for marker in [FONT_MARKER, CMAP_MARKER, b"LiberationSans"] {
                assert!(!bytes.windows(marker.len()).any(|w| w == marker));
            }
            if vertical {
                export_qa_pdf("ATELIER_QA_VERTICAL_REDACTED_PDF", &bytes);
            } else if composite {
                export_qa_pdf("ATELIER_QA_EMBEDDED_REDACTED_PDF", &bytes);
            }
            let copy = Document::open(bytes).unwrap();
            let page = &copy.pages().unwrap()[0];
            let text = atelier_render::extract_text(&copy, page)
                .unwrap()
                .plain_text();
            assert!(
                text.contains("fill") && text.contains("high") && !text.contains("MQW"),
                "{text}"
            );
            let Object::Dict(fonts) = copy.get_key(&page.resources, b"Font").unwrap() else {
                panic!("missing fonts")
            };
            let font = fonts.get(b"F0_0").unwrap().as_dict().unwrap();
            assert_eq!(font.get(b"Subtype"), Some(&name("Type3")));
            let Object::Stream(cmap) = copy.get_key(font, b"ToUnicode").unwrap() else {
                panic!("missing ToUnicode")
            };
            let cmap = std::str::from_utf8(&cmap.data).unwrap();
            for code in ["<004D>", "<0051>", "<0057>"] {
                assert!(!cmap.contains(code), "{cmap}");
            }
            let procs = font.get(b"CharProcs").unwrap().as_dict().unwrap();
            assert_eq!(procs.len(), "fil hg".chars().count());
            let options = atelier_render::RenderOptions {
                scale: 1.0,
                ..Default::default()
            };
            let rendered = atelier_render::render_page(&copy, page, &options).unwrap();
            let (xs, ys) = if vertical {
                (90..110, 10..48)
            } else {
                (10..50, 90..105)
            };
            assert!(ys.into_iter().any(|y| xs.clone().any(|x| {
                let [r, g, b, _] = rendered.pixmap.pixel(x, y);
                b == 255 && r < 200 && g < 200
            })));
        }
    }

    #[test]
    fn type3_redaction_rebuilds_vector_and_bitmap_glyphs_without_private_procedures() {
        let input = with_resource(
            &source(b"0 0 1 rg BT /Paint 12 Tf 10 100 Td [(A) -2500 (B) -2500 (C)] TJ ET"),
            "Font",
            "Paint",
            |update| {
                let mut procedures = Dict::new();
                for (glyph, drawing) in [
                    ("A", b"1000 0 d0 10 0 600 650 re f".as_slice()),
                    ("B", b"1000 0 d0 % private-glyph-marker\n123 67 321 777 re f"),
                    ("C", b"1000 0 0 0 600 650 d1 q 600 0 0 650 0 0 cm BI /W 8 /H 1 /IM true ID \x00 EI Q"),
                ] {
                    let object = update.add(Object::Stream(Stream { dict: Dict::new(), data: Arc::from(drawing) }));
                    procedures.insert(glyph, Object::Reference(object));
                }
                let mut encoding = Dict::new();
                encoding.insert(
                    "Differences",
                    Object::Array(vec![Object::Integer(65), name("A"), name("B"), name("C")]),
                );
                let mut font = Dict::new();
                font.insert("Type", name("Font"));
                font.insert("Subtype", name("Type3"));
                font.insert(
                    "FontMatrix",
                    Object::Array(
                        [0.001, 0.0, 0.0, 0.001, 0.0, 0.0]
                            .map(Object::Real)
                            .to_vec(),
                    ),
                );
                font.insert(
                    "FontBBox",
                    Object::Array([0, 0, 1000, 1000].map(Object::Integer).to_vec()),
                );
                font.insert("FirstChar", Object::Integer(65));
                font.insert("LastChar", Object::Integer(67));
                font.insert("Widths", Object::Array(vec![Object::Integer(1000); 3]));
                font.insert("Encoding", Object::Dict(encoding));
                font.insert("CharProcs", Object::Dict(procedures));
                Object::Dict(font)
            },
        );
        let mark = egui::Rect::from_min_max(egui::pos2(0.25, 0.44), egui::pos2(0.33, 0.53));
        let bytes = vector_copy(&input, &Marks::from([(0, vec![mark])])).unwrap();
        export_qa_pdf("ATELIER_QA_TYPE3_REDACTED_PDF", &bytes);
        for marker in [
            b"private-glyph-marker".as_slice(),
            b"123 67 321 777",
            b"<0042>",
        ] {
            assert!(!bytes.windows(marker.len()).any(|w| w == marker));
        }
        let copy = Document::open(bytes).unwrap();
        let page = &copy.pages().unwrap()[0];
        let text = atelier_render::extract_text(&copy, page).unwrap();
        assert_eq!(
            text.glyphs
                .iter()
                .map(|g| g.text.as_str())
                .collect::<String>(),
            "AC"
        );
        let options = atelier_render::RenderOptions {
            scale: 1.0,
            ..Default::default()
        };
        let rendered = atelier_render::render_page(&copy, page, &options).unwrap();
        assert_eq!(rendered.pixmap.pixel(12, 96), [0, 0, 255, 255]);
        assert_eq!(rendered.pixmap.pixel(55, 96), [0, 0, 0, 255]);
        assert_eq!(rendered.pixmap.pixel(96, 96), [0, 0, 255, 255]);
    }

    #[test]
    fn inline_image_redaction_uses_verified_boundaries_and_rewrites_samples() {
        let public = [13, 80, 174];
        let pixels = [public.as_slice(), b" EI Q ", public.as_slice()].concat();
        let mark = egui::Rect::from_min_max(egui::pos2(0.21, 0.39), egui::pos2(0.39, 0.61));
        let marks = Marks::from([(0, vec![mark])]);
        for compressed in [false, true] {
            let mut content = b"q 80 0 0 40 20 80 cm BI /W 4 /H 1 /BPC 8 /CS /RGB ".to_vec();
            if compressed {
                content.extend_from_slice(b"/F /Fl ");
            }
            content.extend_from_slice(b"ID ");
            let data = if compressed {
                atelier_filters::encode_flate(&pixels)
            } else {
                pixels.clone()
            };
            content.extend_from_slice(&data);
            content.extend_from_slice(b" EI Q\nBT /Helv 12 Tf 10 30 Td (Public) Tj ET");
            let bytes = vector_copy(&source(&content), &marks).unwrap();
            if compressed {
                export_qa_pdf("ATELIER_QA_INLINE_REDACTED_PDF", &bytes);
            }
            let copy = Document::open(bytes).unwrap();
            let page = &copy.pages().unwrap()[0];
            let Object::Dict(objects) = copy.get_key(&page.resources, b"XObject").unwrap() else {
                panic!("missing rebuilt image")
            };
            let Object::Stream(stream) = copy.get_key(&objects, b"I0").unwrap() else {
                panic!("missing rebuilt image stream")
            };
            assert_eq!(
                copy.decode_stream(&stream).unwrap().data,
                [public, [0; 3], [0; 3], public].concat()
            );
            assert_eq!(
                atelier_render::extract_text(&copy, page)
                    .unwrap()
                    .plain_text(),
                "Public"
            );
        }
        let ambiguous =
            source(b"q 80 0 0 40 20 80 cm BI /W 4 /H 1 /BPC 8 /CS /RGB /F /Fl ID bad EI Q");
        assert!(vector_copy(&ambiguous, &marks).is_err());
    }

    #[test]
    fn partially_marked_images_are_resampled_without_retaining_removed_pixels() {
        let content = b"q 80 0 0 40 20 80 cm /Scan Do Q\nq 40 0 0 40 140 80 cm /Stamp Do Q";
        let retained = [13, 80, 174];
        let private = [223, 39, 17];
        let stamp = [91, 243, 39];
        let install_image = |update: &mut atelier_syntax::Update<'_>, pixels: Vec<u8>| {
            let mut image = Dict::new();
            image.insert("Type", name("XObject"));
            image.insert("Subtype", name("Image"));
            image.insert("Width", Object::Integer(pixels.len() as i64 / 3));
            image.insert("Height", Object::Integer(1));
            image.insert("BitsPerComponent", Object::Integer(8));
            image.insert("ColorSpace", name("DeviceRGB"));
            image.insert(
                "Metadata",
                Object::String(atelier_syntax::PdfString::from_text(
                    "private-image-metadata",
                )),
            );
            Object::Reference(update.add(Object::Stream(Stream {
                dict: image,
                data: pixels.into(),
            })))
        };
        let input = with_resource(&source(content), "XObject", "Scan", |update| {
            install_image(update, [retained, private, private, retained].concat())
        });
        let input = with_resource(&input, "XObject", "Stamp", |update| {
            install_image(update, stamp.to_vec())
        });
        let mark = egui::Rect::from_min_max(egui::pos2(0.21, 0.39), egui::pos2(0.39, 0.61));
        let bytes = vector_copy(&input, &Marks::from([(0, vec![mark])])).unwrap();
        export_qa_pdf("ATELIER_QA_IMAGE_REDACTED_PDF", &bytes);
        let marker = b"private-image-metadata";
        assert!(!bytes.windows(marker.len()).any(|w| w == marker));
        let copy = Document::open(bytes).unwrap();
        let page = &copy.pages().unwrap()[0];
        let objects = copy.get_key(&page.resources, b"XObject").unwrap();
        let objects = objects.as_dict().unwrap();
        let Object::Stream(scan) = copy.get_key(objects, b"I0").unwrap() else {
            panic!("missing scan")
        };
        let pixels = copy.decode_stream(&scan).unwrap();
        assert_eq!(&*pixels.data, [retained, [0; 3], [0; 3], retained].concat());
        let Object::Stream(mask) = copy.get_key(&scan.dict, b"SMask").unwrap() else {
            panic!("missing mask")
        };
        assert_eq!(&*copy.decode_stream(&mask).unwrap().data, &[255, 0, 0, 255]);
        let options = atelier_render::RenderOptions {
            scale: 1.0,
            ..Default::default()
        };
        let rendered = atelier_render::render_page(&copy, page, &options).unwrap();
        assert_eq!(rendered.pixmap.pixel(30, 100), [13, 80, 174, 255]);
        assert_eq!(rendered.pixmap.pixel(60, 100), [0, 0, 0, 255]);
        assert_eq!(rendered.pixmap.pixel(90, 100), [13, 80, 174, 255]);
        assert_eq!(rendered.pixmap.pixel(160, 100), [91, 243, 39, 255]);
    }

    #[test]
    fn vector_redaction_removes_characters_and_leaves_selectable_text_and_vectors() {
        let input = source(b"0.8 0.1 0.2 rg 5 10 190 145 re f\n0 0 1 rg 12 18 20 20 re f\n0 1 0 rg 71.125 96.25 5.5 4.75 re f\n0 g BT /Helv 12 Tf 14 100 Td (Keep    PRIVATE    Visible) Tj ET");
        let page = &input.pages().unwrap()[0];
        let text = atelier_render::extract_text(&input, page).unwrap();
        let start = text.glyphs.iter().position(|g| g.text == "P").unwrap();
        let secret = &text.glyphs[start..start + 7];
        let x0 = secret.first().unwrap().quad[0].x;
        let x1 = secret.last().unwrap().quad[1].x;
        let marks = Marks::from([(
            0,
            vec![egui::Rect::from_min_max(
                egui::pos2(x0 as f32 / 200.0, 0.40),
                egui::pos2(x1 as f32 / 200.0, 0.54),
            )],
        )]);
        let bytes = vector_copy(&input, &marks).unwrap();
        if let Some(path) = std::env::var_os("ATELIER_QA_VECTOR_REDACTED_PDF") {
            std::fs::write(path, &bytes).unwrap();
        }
        assert!(
            !bytes
                .windows(b"hidden-catalog-marker".len())
                .any(|part| part == b"hidden-catalog-marker")
        );
        let copy = Document::open(bytes).unwrap();
        let page = &copy.pages().unwrap()[0];
        let output = atelier_render::extract_text(&copy, page)
            .unwrap()
            .plain_text();
        assert!(
            output.contains("Keep") && output.contains("Visible") && !output.contains("PRIVATE"),
            "{output}"
        );
        let content = page.contents(&copy).unwrap();
        assert!(!String::from_utf8_lossy(&content).contains("71.125 96.25 5.5 4.75 re"));
        let operands = atelier_syntax::ContentParser::new(&content)
            .filter(|op| op.operator == b"Tj")
            .flat_map(|op| op.operands)
            .filter_map(|operand| operand.as_string().map(|s| s.to_text()))
            .collect::<Vec<_>>();
        assert!(!operands.join("").contains("PRIVATE"));
        assert!(!page.resources.contains_key(b"XObject"));
        let options = atelier_render::RenderOptions {
            scale: 1.0,
            ..Default::default()
        };
        let painted = atelier_render::render_page(&copy, page, &options).unwrap();
        assert_eq!(painted.pixmap.pixel(20, 170), [0, 0, 255, 255]);
        assert_eq!(painted.pixmap.pixel(20, 140), [204, 26, 51, 255]);
        assert_eq!(
            painted.pixmap.pixel(((x0 + x1) / 2.0) as usize, 100),
            [0, 0, 0, 255]
        );
    }

    #[test]
    fn vector_redaction_refuses_unknown_operators_and_crossing_curves() {
        let mark = egui::Rect::from_min_max(egui::pos2(0.2, 0.3), egui::pos2(0.8, 0.7));
        let marks = Marks::from([(0, vec![mark])]);
        for content in [b"/Im Do".as_slice(), b"10 100 m 40 100 80 120 160 100 c S"] {
            assert!(vector_copy(&source(content), &marks).is_err());
        }
    }

    #[test]
    fn vector_redaction_uses_the_displayed_coordinates_of_a_rotated_crop() {
        let input = source(b"BT /Helv 12 Tf 30 30 Td (Public) Tj 25 70 Td (Secret) Tj ET");
        let mut update = input.update();
        let page = &input.pages().unwrap()[0];
        let mut dict = page.dict.clone();
        dict.insert("Rotate", Object::Integer(90));
        dict.insert(
            "CropBox",
            Object::Array([20, 20, 180, 180].map(Object::Integer).to_vec()),
        );
        update.set(page.obj_ref.unwrap(), Object::Dict(dict));
        let rotated = Document::open(update.to_bytes().unwrap()).unwrap();
        let mark = egui::Rect::from_min_max(egui::pos2(0.45, 0.20), egui::pos2(0.56, 0.46));
        let bytes = vector_copy(&rotated, &Marks::from([(0, vec![mark])])).unwrap();
        let copy = Document::open(bytes).unwrap();
        let page = &copy.pages().unwrap()[0];
        assert_eq!(page.rotate, 90);
        let text = atelier_render::extract_text(&copy, page)
            .unwrap()
            .plain_text();
        assert!(
            text.contains("Public") && !text.contains("Secret"),
            "{text}"
        );
        assert_eq!(page.crop_box.width(), 160.0);
    }

    #[test]
    fn a_fully_marked_image_is_removed_without_copying_its_stream() {
        let input = source(
            b"q 10 0 0 10 50 90 cm /PrivateImage Do Q\nBT /Helv 12 Tf 10 30 Td (Public) Tj ET",
        );
        let mut update = input.update();
        let mut image = Dict::new();
        image.insert("Type", name("XObject"));
        image.insert("Subtype", name("Image"));
        image.insert("Width", Object::Integer(2));
        image.insert("Height", Object::Integer(2));
        image.insert("BitsPerComponent", Object::Integer(8));
        image.insert("ColorSpace", name("DeviceRGB"));
        let pixels = b"PrivateBytes";
        let object = update.add(Object::Stream(Stream {
            dict: image,
            data: Arc::from(pixels.as_slice()),
        }));
        let page = &input.pages().unwrap()[0];
        let mut dictionary = page.dict.clone();
        let mut resources = page.resources.clone();
        let mut objects = Dict::new();
        objects.insert("PrivateImage", Object::Reference(object));
        resources.insert("XObject", Object::Dict(objects));
        dictionary.insert("Resources", Object::Dict(resources));
        update.set(page.obj_ref.unwrap(), Object::Dict(dictionary));
        let input = Document::open(update.to_bytes().unwrap()).unwrap();
        let mark = egui::Rect::from_min_max(egui::pos2(0.2, 0.3), egui::pos2(0.8, 0.7));
        let bytes = vector_copy(&input, &Marks::from([(0, vec![mark])])).unwrap();
        assert!(!bytes.windows(pixels.len()).any(|window| window == pixels));
        let copy = Document::open(bytes).unwrap();
        let page = &copy.pages().unwrap()[0];
        assert!(!page.resources.contains_key(b"XObject"));
        assert_eq!(
            atelier_render::extract_text(&copy, page)
                .unwrap()
                .plain_text(),
            "Public"
        );
    }
}
