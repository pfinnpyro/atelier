//! PDF fonts (ISO 32000-2, 9.5 to 9.10): which glyph a character code
//! selects, how wide it is, and what text it stands for.

mod cmap;
mod system;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use atelier_font::{
    Encoding, FontProgram, Std14, glyph_name_to_unicode, standard_font, strip_subset,
};
use atelier_raster::{Path, Transform};
use atelier_syntax::{Dict, Document, Object, Stream};

pub use cmap::CMap;
use system::{Family, Want};

/// Font descriptor flags (9.8.2).
const FIXED_PITCH: i64 = 1;
const SERIF: i64 = 1 << 1;
const SYMBOLIC: i64 = 1 << 2;
const ITALIC: i64 = 1 << 6;
const FORCE_BOLD: i64 = 1 << 18;

/// One character code of a shown string, resolved.
#[derive(Debug, Clone)]
pub struct Glyph {
    pub code: u32,
    /// Glyph index in the font program, if the font has one for this code.
    pub gid: Option<u32>,
    /// Horizontal advance in text space units (the /Widths value / 1000).
    pub width: f64,
    /// For vertical writing: the vertical advance and the position vector,
    /// in text space units.
    pub vertical: Option<(f64, f64, f64)>,
    pub unicode: Option<String>,
    /// A single-byte code 32, which word spacing applies to.
    pub is_space: bool,
}

#[derive(Debug)]
enum Kind {
    Simple {
        gids: Box<[Option<u32>; 256]>,
    },
    Cid {
        cmap: CMap,
        widths: HashMap<u32, f64>,
        default_width: f64,
        /// Vertical metrics: default (vy, w1y) and per CID (w1y, vx, vy).
        dw2: (f64, f64),
        w2: HashMap<u32, (f64, f64, f64)>,
        cid_to_gid: Option<Vec<u16>>,
        /// The program is CID-keyed CFF: look glyphs up by CID.
        cid_keyed: bool,
    },
    Type3 {
        matrix: Transform,
        procs: Dict,
        resources: Option<Dict>,
    },
}

#[derive(Debug)]
pub struct PdfFont {
    pub name: String,
    kind: Kind,
    program: Option<Arc<FontProgram>>,
    /// The program is a system font standing in for a missing one.
    pub substituted: bool,
    /// Simple and Type 3 fonts: glyph names and widths by code.
    names: Box<[Option<String>; 256]>,
    widths: Box<[Option<f64>; 256]>,
    missing_width: f64,
    to_unicode: Option<CMap>,
    outlines: RefCell<HashMap<u32, Option<Rc<Path>>>>,
}

fn name_of(doc: &Document, dict: &Dict, key: &[u8]) -> Option<String> {
    doc.get_key(dict, key)
        .ok()
        .and_then(|o| o.as_name().map(|n| n.as_str_lossy().into_owned()))
}

fn number(doc: &Document, dict: &Dict, key: &[u8]) -> Option<f64> {
    doc.get_key(dict, key).ok().and_then(|o| o.as_f64())
}

fn stream_of(doc: &Document, dict: &Dict, key: &[u8]) -> Option<Stream> {
    match doc.get_key(dict, key).ok()? {
        Object::Stream(s) => Some(s),
        _ => None,
    }
}

fn load_to_unicode(doc: &Document, dict: &Dict) -> Option<CMap> {
    let s = stream_of(doc, dict, b"ToUnicode")?;
    let data = doc.decode_stream(&s).ok()?.data;
    let c = CMap::parse(&data);
    c.has_unicode().then_some(c)
}

/// Reads an embedded font program from a font descriptor.
fn load_program(
    doc: &Document,
    descriptor: Option<&Dict>,
    warn: &mut dyn FnMut(String),
) -> Option<Arc<FontProgram>> {
    let desc = descriptor?;
    for key in [&b"FontFile"[..], b"FontFile2", b"FontFile3"] {
        let Some(s) = stream_of(doc, desc, key) else {
            continue;
        };
        let data = match doc.decode_stream(&s) {
            Ok(d) => d.data,
            Err(e) => {
                warn(format!("embedded font: {e}"));
                return None;
            }
        };
        return match FontProgram::parse(&data) {
            Ok(p) => Some(Arc::new(p)),
            Err(e) => {
                warn(format!("embedded font: {e}"));
                None
            }
        };
    }
    None
}

/// Picks a system font for a font that is not embedded.
fn want_for(base: &str, flags: i64, std: Option<&Std14>, cjk: bool) -> Want {
    let lower = base.to_ascii_lowercase();
    let bold = std.is_some_and(Std14::is_bold)
        || flags & FORCE_BOLD != 0
        || lower.contains("bold")
        || lower.contains("black")
        || lower.contains("heavy");
    let italic = std.is_some_and(Std14::is_italic)
        || flags & ITALIC != 0
        || lower.contains("italic")
        || lower.contains("oblique");
    let family = if cjk {
        Family::Cjk
    } else if let Some(s) = std {
        match s.name {
            "Symbol" => Family::Symbol,
            "ZapfDingbats" => Family::Dingbats,
            _ if s.is_fixed_pitch() => Family::Mono,
            _ if s.is_serif() => Family::Serif,
            _ => Family::Sans,
        }
    } else if flags & FIXED_PITCH != 0 || lower.contains("mono") || lower.contains("courier") {
        Family::Mono
    } else if flags & SERIF != 0
        || lower.contains("times")
        || lower.contains("serif") && !lower.contains("sans")
        || lower.contains("roman")
        || lower.contains("georgia")
        || lower.contains("garamond")
        || lower.contains("cambria")
    {
        Family::Serif
    } else {
        Family::Sans
    };
    Want {
        family,
        bold,
        italic,
    }
}

impl PdfFont {
    /// Loads a font dictionary. Never fails outright: a font that cannot be
    /// read falls back to a system font so text still shows.
    pub fn load(doc: &Document, dict: &Dict, warn: &mut dyn FnMut(String)) -> PdfFont {
        let subtype = name_of(doc, dict, b"Subtype").unwrap_or_default();
        match subtype.as_str() {
            "Type0" => Self::load_type0(doc, dict, warn),
            "Type3" => Self::load_type3(doc, dict, warn),
            _ => Self::load_simple(doc, dict, &subtype, warn),
        }
    }

    fn empty(name: String, kind: Kind) -> Self {
        Self {
            name,
            kind,
            program: None,
            substituted: false,
            names: Box::new(std::array::from_fn(|_| None)),
            widths: Box::new([None; 256]),
            missing_width: 0.0,
            to_unicode: None,
            outlines: RefCell::new(HashMap::new()),
        }
    }

    fn load_simple(
        doc: &Document,
        dict: &Dict,
        subtype: &str,
        warn: &mut dyn FnMut(String),
    ) -> PdfFont {
        let base = name_of(doc, dict, b"BaseFont").unwrap_or_default();
        let descriptor = doc
            .get_key(dict, b"FontDescriptor")
            .ok()
            .and_then(|o| o.as_dict().cloned());
        let flags = descriptor
            .as_ref()
            .and_then(|d| number(doc, d, b"Flags"))
            .map_or(0, |f| f as i64);
        let std = standard_font(&base);
        let mut font = Self::empty(
            strip_subset(&base).to_string(),
            Kind::Simple {
                gids: Box::new([None; 256]),
            },
        );
        font.to_unicode = load_to_unicode(doc, dict);
        font.missing_width = descriptor
            .as_ref()
            .and_then(|d| number(doc, d, b"MissingWidth"))
            .unwrap_or(0.0)
            / 1000.0;

        let embedded = load_program(doc, descriptor.as_ref(), warn);
        font.program = embedded.clone();
        if font.program.is_none() {
            font.program = system::substitute(want_for(&base, flags, std, false));
            font.substituted = true;
            if font.program.is_none() {
                warn(format!("no font available to stand in for {}", font.name));
            }
        }
        let is_truetype = matches!(font.program.as_deref(), Some(FontProgram::TrueType(_)));
        let symbolic = flags & SYMBOLIC != 0 || std.is_some_and(Std14::is_symbolic);

        // The encoding: a base, then /Differences.
        let enc_obj = doc.get_key(dict, b"Encoding").ok();
        let (base_name, differences) = match &enc_obj {
            Some(Object::Name(n)) => (Some(n.as_bytes().to_vec()), None),
            Some(Object::Dict(d)) => (
                d.get(b"BaseEncoding")
                    .and_then(Object::as_name)
                    .map(|n| n.as_bytes().to_vec()),
                doc.get_key(d, b"Differences").ok(),
            ),
            _ => (None, None),
        };
        // An encoding dictionary without a base means StandardEncoding for a
        // nonsymbolic font (9.6.5.1).
        let base_encoding = base_name.as_deref().and_then(Encoding::from_name).or(
            if matches!(enc_obj, Some(Object::Dict(_))) && !symbolic {
                Some(Encoding::Standard)
            } else {
                None
            },
        );
        let has_encoding = base_encoding.is_some() || differences.is_some();
        for code in 0..=255u8 {
            let name: Option<String> = match base_encoding {
                Some(e) => e.glyph_name(code).map(str::to_string),
                None => match (std.map(|s| s.name), embedded.as_deref()) {
                    // Embedded Type 1 and CFF fonts use their own encoding.
                    (_, Some(p @ (FontProgram::Type1(_) | FontProgram::Cff(_)))) => p
                        .builtin_encoding(code)
                        .or_else(|| {
                            if symbolic {
                                None
                            } else {
                                Encoding::Standard.glyph_name(code)
                            }
                        })
                        .map(str::to_string),
                    (Some("Symbol"), _) => Encoding::Symbol.glyph_name(code).map(str::to_string),
                    (Some("ZapfDingbats"), _) => {
                        Encoding::ZapfDingbats.glyph_name(code).map(str::to_string)
                    }
                    (_, Some(FontProgram::TrueType(_))) if symbolic => None,
                    _ if subtype == "TrueType" => {
                        Encoding::WinAnsi.glyph_name(code).map(str::to_string)
                    }
                    _ => Encoding::Standard.glyph_name(code).map(str::to_string),
                },
            };
            font.names[usize::from(code)] = name;
        }
        if let Some(Object::Array(diffs)) = differences {
            let mut code: i64 = 0;
            for d in diffs {
                match d {
                    Object::Integer(i) => code = i,
                    Object::Real(r) => code = r as i64,
                    Object::Name(n) => {
                        if let Ok(c) = usize::try_from(code) {
                            if c < 256 {
                                font.names[c] = Some(n.as_str_lossy().into_owned());
                            }
                        }
                        code += 1;
                    }
                    _ => {}
                }
            }
        }

        // Code to glyph.
        let mut gids = Box::new([None; 256]);
        if let Some(p) = font.program.as_deref() {
            for code in 0..=255u8 {
                let name = font.names[usize::from(code)].as_deref();
                gids[usize::from(code)] = match p {
                    FontProgram::TrueType(tt) => {
                        let by_name = || {
                            let n = name?;
                            tt.glyph_by_name(n).or_else(|| {
                                // A name in a Mac Roman-encoded subset.
                                let mac = Encoding::MacRoman.code_of(n)?;
                                tt.glyph_by_mac_code(mac)
                            })
                        };
                        let by_code = || tt.glyph_by_symbol_code(code);
                        if (symbolic && !has_encoding)
                            || tt.has_symbol_cmap() && !tt.has_unicode_cmap()
                        {
                            by_code().or_else(by_name)
                        } else {
                            by_name().or_else(by_code)
                        }
                        .or_else(|| {
                            // Embedded subsets without a usable cmap often
                            // number their glyphs by code.
                            (!font.substituted
                                && is_truetype
                                && !tt.has_unicode_cmap()
                                && !tt.has_symbol_cmap())
                            .then_some(u32::from(code))
                            .filter(|&g| (g as usize) < tt.glyph_count())
                        })
                    }
                    _ => name
                        .and_then(|n| p.glyph_by_name(n))
                        .or_else(|| {
                            let text = glyph_name_to_unicode(name?)?;
                            let mut chars = text.chars();
                            let c = chars.next()?;
                            chars.next().is_none().then(|| p.glyph_by_unicode(c))?
                        })
                        .or_else(|| {
                            if font.substituted {
                                None
                            } else {
                                p.glyph_by_code(code)
                            }
                        }),
                };
            }
        }
        font.kind = Kind::Simple { gids };

        // Widths: /Widths, else the standard font's metrics, else the program.
        let first = number(doc, dict, b"FirstChar").unwrap_or(0.0) as i64;
        if let Ok(Object::Array(w)) = doc.get_key(dict, b"Widths") {
            for (i, v) in w.iter().enumerate() {
                let code = first + i as i64;
                if (0..256).contains(&code) {
                    let v = doc.resolve(v).ok().and_then(|o| o.as_f64());
                    font.widths[code as usize] = v.map(|v| v / 1000.0);
                }
            }
        } else if let Some(s) = std {
            for code in 0..256 {
                // A name the metrics lack (often `.notdef` for the space)
                // falls back to the standard encoding's glyph for the code.
                let w = font.names[code]
                    .as_deref()
                    .and_then(|n| s.width(n))
                    .or_else(|| s.width(Encoding::Standard.glyph_name(code as u8)?));
                font.widths[code] = w.map(|w| f64::from(w) / 1000.0);
            }
        }
        if !dict.contains_key(b"Widths") && std.is_none() {
            if let (Kind::Simple { gids }, Some(p)) = (&font.kind, font.program.as_deref()) {
                for code in 0..256 {
                    if font.widths[code].is_none() {
                        font.widths[code] = gids[code].and_then(|g| p.advance(g));
                    }
                }
            }
        }
        font
    }

    fn load_type3(doc: &Document, dict: &Dict, _warn: &mut dyn FnMut(String)) -> PdfFont {
        let matrix = doc
            .get_key(dict, b"FontMatrix")
            .ok()
            .and_then(|m| {
                let v: Vec<f64> = m.as_array()?.iter().filter_map(Object::as_f64).collect();
                (v.len() == 6).then(|| Transform::from_array([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .filter(Transform::is_finite)
            .unwrap_or(Transform::new(0.001, 0.0, 0.0, 0.001, 0.0, 0.0));
        let procs = doc
            .get_key(dict, b"CharProcs")
            .ok()
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        let resources = doc
            .get_key(dict, b"Resources")
            .ok()
            .and_then(|o| o.as_dict().cloned());
        let name = name_of(doc, dict, b"Name").unwrap_or_else(|| "Type3".into());
        let mut font = Self::empty(
            name,
            Kind::Type3 {
                matrix,
                procs,
                resources,
            },
        );
        font.to_unicode = load_to_unicode(doc, dict);
        if let Ok(enc) = doc.get_key(dict, b"Encoding") {
            if let Some(Ok(Object::Array(diffs))) =
                enc.as_dict().map(|d| doc.get_key(d, b"Differences"))
            {
                let mut code: i64 = 0;
                for d in diffs {
                    match d {
                        Object::Integer(i) => code = i,
                        Object::Name(n) => {
                            if (0..256).contains(&code) {
                                font.names[code as usize] = Some(n.as_str_lossy().into_owned());
                            }
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        // Type 3 widths are in glyph space; map them through the font matrix.
        let first = number(doc, dict, b"FirstChar").unwrap_or(0.0) as i64;
        if let Ok(Object::Array(w)) = doc.get_key(dict, b"Widths") {
            for (i, v) in w.iter().enumerate() {
                let code = first + i as i64;
                if (0..256).contains(&code) {
                    if let Some(v) = doc.resolve(v).ok().and_then(|o| o.as_f64()) {
                        font.widths[code as usize] = Some(v * matrix.a);
                    }
                }
            }
        }
        font
    }

    fn load_type0(doc: &Document, dict: &Dict, warn: &mut dyn FnMut(String)) -> PdfFont {
        let base = name_of(doc, dict, b"BaseFont").unwrap_or_default();
        let cmap = match doc.get_key(dict, b"Encoding") {
            Ok(Object::Name(n)) => CMap::predefined(n.as_bytes()).unwrap_or_else(|| {
                warn(format!(
                    "the {} CMap is not built in yet; reading codes as two-byte CIDs",
                    n.as_str_lossy()
                ));
                CMap::identity(n.as_bytes().ends_with(b"-V"))
            }),
            Ok(Object::Stream(s)) => match doc.decode_stream(&s) {
                Ok(d) => {
                    let mut c = CMap::parse(&d.data);
                    if let Some(Object::Name(parent)) = s.dict.get(b"UseCMap") {
                        if let Some(p) = CMap::predefined(parent.as_bytes()) {
                            c = c.with_parent(p);
                        }
                    }
                    c
                }
                Err(_) => CMap::identity(false),
            },
            _ => CMap::identity(false),
        };
        let desc_font = doc
            .get_key(dict, b"DescendantFonts")
            .ok()
            .and_then(|o| o.as_array().and_then(|a| a.first().cloned()))
            .and_then(|o| doc.resolve(&o).ok())
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        let descriptor = doc
            .get_key(&desc_font, b"FontDescriptor")
            .ok()
            .and_then(|o| o.as_dict().cloned());
        let flags = descriptor
            .as_ref()
            .and_then(|d| number(doc, d, b"Flags"))
            .map_or(0, |f| f as i64);
        let default_width = number(doc, &desc_font, b"DW").unwrap_or(1000.0) / 1000.0;
        let mut widths = HashMap::new();
        if let Ok(Object::Array(w)) = doc.get_key(&desc_font, b"W") {
            read_w(doc, &w, 1, |cid, v| {
                widths.insert(cid, v[0] / 1000.0);
            });
        }
        let dw2 = doc
            .get_key(&desc_font, b"DW2")
            .ok()
            .and_then(|o| {
                let v: Vec<f64> = o.as_array()?.iter().filter_map(Object::as_f64).collect();
                (v.len() == 2).then(|| (v[0] / 1000.0, v[1] / 1000.0))
            })
            .unwrap_or((0.88, -1.0));
        let mut w2 = HashMap::new();
        if let Ok(Object::Array(w)) = doc.get_key(&desc_font, b"W2") {
            read_w(doc, &w, 3, |cid, v| {
                w2.insert(cid, (v[0] / 1000.0, v[1] / 1000.0, v[2] / 1000.0));
            });
        }
        let cid_to_gid = match doc.get_key(&desc_font, b"CIDToGIDMap") {
            Ok(Object::Stream(s)) => doc.decode_stream(&s).ok().map(|d| {
                d.data
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect()
            }),
            _ => None,
        };
        let ordering = doc
            .get_key(&desc_font, b"CIDSystemInfo")
            .ok()
            .and_then(|o| o.as_dict().cloned())
            .and_then(|d| doc.get_key(&d, b"Ordering").ok())
            .and_then(|o| o.as_string().map(|s| s.bytes.clone()))
            .unwrap_or_default();
        let cjk = matches!(&ordering[..], b"Japan1" | b"GB1" | b"CNS1" | b"Korea1");

        let mut program = load_program(doc, descriptor.as_ref(), warn);
        let mut substituted = false;
        if program.is_none() {
            program = system::substitute(want_for(&base, flags, None, cjk));
            substituted = true;
            if program.is_none() {
                warn(format!(
                    "no font available to stand in for {}",
                    strip_subset(&base)
                ));
            }
        }
        let cid_keyed = matches!(program.as_deref(), Some(FontProgram::Cff(c)) if c.is_cid());
        let mut font = Self::empty(
            strip_subset(&base).to_string(),
            Kind::Cid {
                cmap,
                widths,
                default_width,
                dw2,
                w2,
                cid_to_gid,
                cid_keyed,
            },
        );
        font.program = program;
        font.substituted = substituted;
        font.to_unicode = load_to_unicode(doc, dict);
        if substituted && font.to_unicode.is_none() {
            warn(format!(
                "{} is not embedded and has no Unicode map; its text may show the wrong glyphs",
                font.name
            ));
        }
        font
    }

    pub fn is_type3(&self) -> bool {
        matches!(self.kind, Kind::Type3 { .. })
    }

    pub fn is_vertical(&self) -> bool {
        matches!(&self.kind, Kind::Cid { cmap, .. } if cmap.vertical)
    }

    /// For Type 3 fonts: the font matrix, the glyph procedure for a code, and
    /// the font's own resources.
    pub fn type3(&self, code: u32) -> Option<(Transform, Option<&Object>, Option<&Dict>)> {
        let Kind::Type3 {
            matrix,
            procs,
            resources,
        } = &self.kind
        else {
            return None;
        };
        let name = self.names.get(code as usize)?.as_deref();
        Some((
            *matrix,
            name.and_then(|n| procs.get(n.as_bytes())),
            resources.as_ref(),
        ))
    }

    /// Splits a shown string into glyphs.
    pub fn decode(&self, bytes: &[u8]) -> Vec<Glyph> {
        let mut out = Vec::with_capacity(bytes.len());
        match &self.kind {
            Kind::Simple { .. } | Kind::Type3 { .. } => {
                for &b in bytes {
                    out.push(self.simple_glyph(b));
                }
            }
            Kind::Cid {
                cmap,
                widths,
                default_width,
                dw2,
                w2,
                cid_to_gid,
                cid_keyed,
            } => {
                let mut pos = 0;
                while pos < bytes.len() {
                    let (code, len) = cmap.next_code(bytes, pos);
                    pos += len;
                    let cid = cmap.cid(code);
                    let width = widths.get(&cid).copied().unwrap_or(*default_width);
                    let vertical = cmap.vertical.then(|| {
                        let (w1, vx, vy) =
                            w2.get(&cid).copied().unwrap_or((dw2.1, width / 2.0, dw2.0));
                        (w1, vx, vy)
                    });
                    let unicode = self.to_unicode.as_ref().and_then(|t| t.unicode(code));
                    let gid =
                        self.cid_gid(cid, *cid_keyed, cid_to_gid.as_deref(), unicode.as_deref());
                    out.push(Glyph {
                        code,
                        gid,
                        width,
                        vertical,
                        unicode,
                        is_space: len == 1 && code == 32,
                    });
                }
            }
        }
        out
    }

    fn cid_gid(
        &self,
        cid: u32,
        cid_keyed: bool,
        map: Option<&[u16]>,
        unicode: Option<&str>,
    ) -> Option<u32> {
        let p = self.program.as_deref()?;
        if self.substituted {
            // A system font: the CID means nothing to it, so go by Unicode.
            let c = unicode?.chars().next()?;
            return p.glyph_by_unicode(c);
        }
        if cid_keyed {
            return p.glyph_by_cid(cid);
        }
        match map {
            Some(m) => m.get(cid as usize).map(|&g| u32::from(g)),
            None => Some(cid),
        }
    }

    fn simple_glyph(&self, code: u8) -> Glyph {
        let c = usize::from(code);
        let gid = match &self.kind {
            Kind::Simple { gids } => gids[c],
            _ => None,
        };
        let width = self.widths[c].unwrap_or(self.missing_width);
        let unicode = self
            .to_unicode
            .as_ref()
            .and_then(|t| t.unicode(u32::from(code)))
            .or_else(|| self.names[c].as_deref().and_then(glyph_name_to_unicode))
            // Code 32 is a space even when the encoding names it oddly.
            .or_else(|| (code == 32).then(|| " ".to_string()));
        Glyph {
            code: u32::from(code),
            gid,
            width,
            vertical: None,
            unicode,
            is_space: code == 32,
        }
    }

    /// The outline of a glyph in text space (1.0 = font size), cached.
    pub fn outline(&self, gid: u32) -> Option<Rc<Path>> {
        if let Some(hit) = self.outlines.borrow().get(&gid) {
            return hit.clone();
        }
        let path = self
            .program
            .as_deref()
            .and_then(|p| p.outline(gid))
            .map(Rc::new);
        self.outlines.borrow_mut().insert(gid, path.clone());
        path
    }
}

/// Reads a CID width array: `c [w ...]` or `c_first c_last w ...`, where
/// each entry has `n` numbers.
fn read_w(doc: &Document, w: &[Object], n: usize, mut put: impl FnMut(u32, &[f64])) {
    let num = |o: &Object| doc.resolve(o).ok().and_then(|o| o.as_f64());
    let mut i = 0;
    let mut budget = 1_000_000usize;
    while i < w.len() {
        let Some(first) = num(&w[i]) else {
            i += 1;
            continue;
        };
        let first = first.max(0.0) as u32;
        match w.get(i + 1).map(|o| doc.resolve(o).unwrap_or(Object::Null)) {
            Some(Object::Array(list)) => {
                let vals: Vec<f64> = list.iter().filter_map(num).collect();
                for (k, chunk) in vals.chunks_exact(n).enumerate() {
                    put(first + k as u32, chunk);
                }
                i += 2;
            }
            Some(o) => {
                let Some(last) = o.as_f64() else {
                    i += 1;
                    continue;
                };
                let vals: Vec<f64> = w[i + 2..].iter().take(n).filter_map(num).collect();
                if vals.len() == n {
                    for cid in first..=(last.max(0.0) as u32) {
                        if budget == 0 {
                            return;
                        }
                        budget -= 1;
                        put(cid, &vals);
                    }
                }
                i += 2 + n;
            }
            None => break,
        }
    }
}
