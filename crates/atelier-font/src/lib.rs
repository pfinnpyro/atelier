//! Atelier's font readers.
//!
//! Each reader turns a font program into glyph outlines in text space, where
//! 1.0 is one em (the font size). Readers are lenient: a damaged glyph gives
//! an empty or partial outline instead of failing the whole font.

mod cff;
mod data;
mod encoding;
mod reader;
mod std14;
mod truetype;
mod type1;

use atelier_raster::Path;

pub use cff::Cff;
pub use encoding::{Encoding, glyph_name_to_unicode, unicode_to_glyph_name_candidates};
pub use std14::{Std14, standard_font, strip_subset};
pub use truetype::TrueType;
pub use type1::Type1;

/// Why a font program could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontError(pub &'static str);

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for FontError {}

pub type Result<T> = std::result::Result<T, FontError>;

/// Any supported font program.
#[derive(Debug)]
pub enum FontProgram {
    TrueType(TrueType),
    Cff(Cff),
    Type1(Type1),
}

impl FontProgram {
    /// Reads a font program, detecting its format from the data.
    pub fn parse(data: &[u8]) -> Result<Self> {
        match data.get(..4) {
            Some([0, 1, 0, 0]) | Some(b"true") | Some(b"OTTO") | Some(b"ttcf") => {
                let tt = TrueType::parse(data)?;
                // OpenType with CFF outlines carries the outlines in its CFF table.
                if let Some(cff) = tt.cff_table() {
                    let mut c = Cff::parse(cff)?;
                    c.attach_sfnt(tt);
                    return Ok(Self::Cff(c));
                }
                Ok(Self::TrueType(tt))
            }
            Some([1, 0, 4, _]) | Some([1, 0, 3, _]) => Ok(Self::Cff(Cff::parse(data)?)),
            _ if data.starts_with(b"%!") || data.first() == Some(&0x80) => {
                Ok(Self::Type1(Type1::parse(data)?))
            }
            _ => {
                // Unknown: try each reader in turn.
                if let Ok(tt) = TrueType::parse(data) {
                    return Ok(Self::TrueType(tt));
                }
                if let Ok(c) = Cff::parse(data) {
                    return Ok(Self::Cff(c));
                }
                Type1::parse(data).map(Self::Type1)
            }
        }
    }

    pub fn glyph_count(&self) -> usize {
        match self {
            Self::TrueType(f) => f.glyph_count(),
            Self::Cff(f) => f.glyph_count(),
            Self::Type1(f) => f.glyph_count(),
        }
    }

    /// The outline of a glyph, in text space (1.0 = one em).
    pub fn outline(&self, gid: u32) -> Option<Path> {
        match self {
            Self::TrueType(f) => f.outline(gid),
            Self::Cff(f) => f.outline(gid),
            Self::Type1(f) => f.outline(gid),
        }
    }

    /// The glyph's advance width in text space.
    pub fn advance(&self, gid: u32) -> Option<f64> {
        match self {
            Self::TrueType(f) => f.advance(gid),
            Self::Cff(f) => f.advance(gid),
            Self::Type1(f) => f.advance(gid),
        }
    }

    /// The glyph with this PostScript name.
    pub fn glyph_by_name(&self, name: &str) -> Option<u32> {
        match self {
            Self::TrueType(f) => f.glyph_by_name(name),
            Self::Cff(f) => f.glyph_by_name(name),
            Self::Type1(f) => f.glyph_by_name(name),
        }
    }

    /// The glyph name a font's built-in encoding gives a code, for Type 1 and
    /// CFF fonts.
    pub fn builtin_encoding(&self, code: u8) -> Option<&str> {
        match self {
            Self::TrueType(_) => None,
            Self::Cff(f) => f.builtin_encoding(code),
            Self::Type1(f) => f.builtin_encoding(code),
        }
    }

    /// The glyph for a code in a font with a built-in code mapping (CFF
    /// encoding without names, or TrueType's (3,0) symbol cmap).
    pub fn glyph_by_code(&self, code: u8) -> Option<u32> {
        match self {
            Self::TrueType(f) => f.glyph_by_symbol_code(code),
            Self::Cff(f) => f.glyph_by_code(code),
            Self::Type1(f) => f
                .builtin_encoding(code)
                .and_then(|n| f.glyph_by_name(n)),
        }
    }

    /// The glyph for a Unicode character, using the font's own Unicode cmap.
    pub fn glyph_by_unicode(&self, c: char) -> Option<u32> {
        match self {
            Self::TrueType(f) => f.glyph_by_unicode(c),
            Self::Cff(f) => f.glyph_by_unicode(c),
            Self::Type1(f) => unicode_to_glyph_name_candidates(c)
                .into_iter()
                .find_map(|n| f.glyph_by_name(&n)),
        }
    }

    /// For CID-keyed CFF fonts, the glyph for a CID.
    pub fn glyph_by_cid(&self, cid: u32) -> Option<u32> {
        match self {
            Self::Cff(f) => f.glyph_by_cid(cid),
            _ => Some(cid),
        }
    }
}
