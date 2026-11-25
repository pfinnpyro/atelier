use atelier_raster::{Path, Point, Verb};

use super::*;

pub(super) enum Kind {
    Standard(StandardFont),
    Embedded(Embedded),
    Type3(Embedded),
}

pub(super) fn resource_name(id: usize, subset: Option<usize>) -> String {
    match subset {
        Some(subset) => format!("F{id}_{subset}"),
        None => format!("F{id}"),
    }
}

/// Returns the code width for an embedded font the renderer can interpret
/// without guessing its encoding or substituting another font.
pub(super) fn embedded(doc: &Document, font: &Dict) -> Result<Option<bool>> {
    let subtype = font.get(b"Subtype").and_then(Object::as_name);
    let kind = subtype.map(Name::as_bytes).unwrap_or_default();
    let (descendant, two_byte) = match kind {
        b"TrueType" | b"Type1" | b"MMType1" => (font.clone(), false),
        b"Type0" => {
            let encoding = doc.get_key(font, b"Encoding")?;
            if !matches!(
                encoding.as_name().map(Name::as_bytes),
                Some(b"Identity-H" | b"Identity-V")
            ) {
                return Err(unsupported("Unsupported composite font encoding"));
            }
            let children = doc.get_key(font, b"DescendantFonts")?;
            let Some([child]) = children.as_array() else {
                return Err(unsupported("Invalid composite font"));
            };
            let descendant = doc.resolve_dict(child)?;
            let kind = descendant.get(b"Subtype").and_then(Object::as_name);
            if !matches!(
                kind.map(Name::as_bytes),
                Some(b"CIDFontType0" | b"CIDFontType2")
            ) {
                return Err(unsupported("Unsupported composite font descendant"));
            }
            (descendant, true)
        }
        _ => return Ok(None),
    };
    let Some(descriptor) = descendant.get(b"FontDescriptor") else {
        return Ok(None);
    };
    let descriptor = doc.resolve_dict(descriptor)?;
    let found = [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"]
        .into_iter()
        .any(|key| descriptor.contains_key(key));
    Ok(found.then_some(two_byte))
}

const CODES_PER_FONT: usize = 255;

pub(super) struct Embedded {
    pub two_byte: bool,
    codes: HashMap<u32, usize>,
    glyphs: Vec<RetainedGlyph>,
}

struct RetainedGlyph {
    unicode: String,
    width: f64,
    drawing: Drawing,
}

pub(super) enum Drawing {
    Outline(Arc<Path>),
    Procedure(Arc<super::type3::Procedure>),
}

impl Embedded {
    pub fn new(two_byte: bool) -> Self {
        Self {
            two_byte,
            codes: HashMap::new(),
            glyphs: Vec::new(),
        }
    }

    pub fn retain(
        &mut self,
        source: u32,
        unicode: &str,
        width: f64,
        drawing: Drawing,
    ) -> Result<(usize, u8)> {
        let next = self.glyphs.len();
        let index = match self.codes.entry(source) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                if next >= 4096 || unicode.encode_utf16().count() > 8 || width.abs() > 100.0 {
                    return Err(unsupported(
                        "Unsupported embedded-font glyph metrics or mapping",
                    ));
                }
                entry.insert(next);
                self.glyphs.push(RetainedGlyph {
                    unicode: unicode.to_owned(),
                    width: width * 1000.0,
                    drawing,
                });
                next
            }
        };
        Ok((index / CODES_PER_FONT, (index % CODES_PER_FONT + 1) as u8))
    }
}

pub(super) fn write(
    id: usize,
    kind: Kind,
    fonts: &mut Dict,
    assets: &mut Assets<'_, '_>,
) -> Result<()> {
    match kind {
        Kind::Standard(font) => {
            if font.used {
                fonts.insert(
                    Name::from(resource_name(id, None).as_str()),
                    Object::Dict(font.dictionary),
                );
            }
        }
        Kind::Embedded(font) | Kind::Type3(font) => {
            for (subset, glyphs) in font.glyphs.chunks(CODES_PER_FONT).enumerate() {
                let dict = type3(glyphs, assets)?;
                fonts.insert(
                    Name::from(resource_name(id, Some(subset)).as_str()),
                    Object::Dict(dict),
                );
            }
        }
    }
    Ok(())
}

fn type3(glyphs: &[RetainedGlyph], assets: &mut Assets<'_, '_>) -> Result<Dict> {
    let mut procs = Dict::new();
    let mut differences = vec![Object::Integer(1)];
    let mut bounds = None;
    for (index, glyph) in glyphs.iter().enumerate() {
        let name = Name::from(format!("g{}", index + 1).as_str());
        let mut content = line(b"d0", &[glyph.width, 0.0]);
        let point = |p: Point| [p.x * 1000.0, p.y * 1000.0];
        let glyph_bounds = match &glyph.drawing {
            Drawing::Outline(outline) => {
                for verb in &outline.verbs {
                    match *verb {
                        Verb::MoveTo(p) => content.push_str(&line(b"m", &point(p))),
                        Verb::LineTo(p) => content.push_str(&line(b"l", &point(p))),
                        Verb::CubicTo(a, b, c) => {
                            let ([ax, ay], [bx, by], [cx, cy]) = (point(a), point(b), point(c));
                            content.push_str(&line(b"c", &[ax, ay, bx, by, cx, cy]));
                        }
                        Verb::Close => content.push_str("h\n"),
                    }
                }
                if !outline.is_empty() {
                    content.push_str("f\n");
                }
                outline.bounds()
            }
            Drawing::Procedure(proc) => {
                content.push_str(&line(
                    b"cm",
                    &proc
                        .matrix
                        .then(&Transform::scale(1000.0, 1000.0))
                        .to_array(),
                ));
                content.push_str(&proc.content);
                proc.bounds
                    .map(|b| (Point::new(b.x0, b.y0), Point::new(b.x1, b.y1)))
            }
        };
        if let Some((min, max)) = glyph_bounds {
            for p in [min, max] {
                let [x, y] = point(p);
                add_point(&mut bounds, Point::new(x, y))?;
            }
        }
        procs.insert(name.clone(), assets.add(Dict::new(), content.into_bytes())?);
        differences.push(Object::Name(name));
    }
    let mut encoding = Dict::new();
    encoding.insert("Differences", Object::Array(differences));
    let bounds = bounds.map_or([0.0; 4], |r| {
        [r.x0.floor(), r.y0.floor(), r.x1.ceil(), r.y1.ceil()]
    });
    let mut dict = Dict::new();
    dict.insert("Type", Object::Name(Name::from("Font")));
    dict.insert("Subtype", Object::Name(Name::from("Type3")));
    dict.insert("FontBBox", real_array(bounds));
    dict.insert("FontMatrix", real_array([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]));
    dict.insert("CharProcs", Object::Dict(procs));
    dict.insert("Encoding", Object::Dict(encoding));
    dict.insert("FirstChar", Object::Integer(1));
    dict.insert("LastChar", Object::Integer(glyphs.len() as i64));
    dict.insert(
        "Widths",
        Object::Array(glyphs.iter().map(|g| Object::Real(g.width)).collect()),
    );
    dict.insert("Resources", Object::Dict(Dict::new()));
    dict.insert("ToUnicode", assets.add(Dict::new(), unicode_map(glyphs))?);
    Ok(dict)
}

fn real_array<const N: usize>(values: [f64; N]) -> Object {
    Object::Array(values.map(Object::Real).to_vec())
}

fn unicode_map(glyphs: &[RetainedGlyph]) -> Vec<u8> {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
        /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
        /CMapName /AtelierUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    let mut code = 1;
    for batch in glyphs.chunks(100) {
        writeln!(cmap, "{} beginbfchar", batch.len()).unwrap();
        for glyph in batch {
            write!(cmap, "<{code:02X}> <").unwrap();
            for unit in glyph.unicode.encode_utf16() {
                write!(cmap, "{unit:04X}").unwrap();
            }
            cmap.push_str(">\n");
            code += 1;
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap.into_bytes()
}
