//! Single-byte encodings and glyph names.

use crate::data::{encodings as enc, glyphlist::GLYPH_LIST};

/// A built-in single-byte encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Standard,
    WinAnsi,
    MacRoman,
    MacExpert,
    PdfDoc,
    Symbol,
    ZapfDingbats,
}

impl Encoding {
    /// The encoding a PDF /Encoding or /BaseEncoding name refers to.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"StandardEncoding" => Self::Standard,
            b"WinAnsiEncoding" => Self::WinAnsi,
            b"MacRomanEncoding" => Self::MacRoman,
            b"MacExpertEncoding" => Self::MacExpert,
            b"PDFDocEncoding" => Self::PdfDoc,
            _ => return None,
        })
    }

    fn table(self) -> &'static [Option<&'static str>; 256] {
        match self {
            Self::Standard => &enc::STANDARD,
            Self::WinAnsi => &enc::WIN_ANSI,
            Self::MacRoman => &enc::MAC_ROMAN,
            Self::MacExpert => &enc::MAC_EXPERT,
            Self::PdfDoc => &enc::PDF_DOC,
            Self::Symbol => &enc::SYMBOL,
            Self::ZapfDingbats => &enc::ZAPF_DINGBATS,
        }
    }

    pub fn glyph_name(self, code: u8) -> Option<&'static str> {
        self.table()[code as usize]
    }

    /// The code of a glyph name in this encoding, if it has one.
    pub fn code_of(self, name: &str) -> Option<u8> {
        self.table()
            .iter()
            .position(|n| *n == Some(name))
            .map(|i| i as u8)
    }
}

/// The Unicode text a glyph name stands for: the Adobe Glyph List, then the
/// `uniXXXX` and `uXXXX` conventions, ligatures joined with `_`, and suffixes
/// after a `.` ignored.
pub fn glyph_name_to_unicode(name: &str) -> Option<String> {
    let base = name.split('.').next().unwrap_or(name);
    if base.is_empty() {
        return None;
    }
    if base.contains('_') {
        let parts: Option<String> = base.split('_').map(single_name).collect();
        return parts;
    }
    single_name(base)
}

fn single_name(name: &str) -> Option<String> {
    if let Ok(i) = GLYPH_LIST.binary_search_by(|(n, _)| (*n).cmp(name)) {
        return Some(GLYPH_LIST[i].1.to_string());
    }
    if let Some(hex) = name.strip_prefix("uni") {
        if hex.len() >= 4 && hex.len() % 4 == 0 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let units: Vec<u16> = (0..hex.len() / 4)
                .filter_map(|i| u16::from_str_radix(&hex[i * 4..i * 4 + 4], 16).ok())
                .collect();
            return String::from_utf16(&units).ok();
        }
    }
    if let Some(hex) = name.strip_prefix('u') {
        if (4..=6).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .map(String::from);
        }
    }
    None
}

/// Glyph names that may stand for `c`, best first.
pub fn unicode_to_glyph_name_candidates(c: char) -> Vec<String> {
    let mut s = [0u8; 4];
    let text: &str = c.encode_utf8(&mut s);
    let mut out: Vec<String> = GLYPH_LIST
        .iter()
        .filter(|(_, u)| *u == text)
        .map(|(n, _)| n.to_string())
        .collect();
    let cp = c as u32;
    if cp <= 0xFFFF {
        out.push(format!("uni{cp:04X}"));
    }
    out.push(format!("u{cp:04X}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_unicode() {
        assert_eq!(glyph_name_to_unicode("A").as_deref(), Some("A"));
        assert_eq!(glyph_name_to_unicode("eacute").as_deref(), Some("é"));
        assert_eq!(glyph_name_to_unicode("uni20AC").as_deref(), Some("€"));
        assert_eq!(glyph_name_to_unicode("u1F600").as_deref(), Some("😀"));
        assert_eq!(glyph_name_to_unicode("f_i").as_deref(), Some("fi"));
        assert_eq!(glyph_name_to_unicode("a.sc").as_deref(), Some("a"));
        assert_eq!(glyph_name_to_unicode("nonsense"), None);
    }

    #[test]
    fn encodings() {
        assert_eq!(Encoding::WinAnsi.glyph_name(0x80), Some("Euro"));
        assert_eq!(Encoding::Standard.glyph_name(0x27), Some("quoteright"));
        assert_eq!(Encoding::MacRoman.glyph_name(0x8E), Some("eacute"));
        assert_eq!(Encoding::Symbol.glyph_name(0x61), Some("alpha"));
        assert_eq!(Encoding::WinAnsi.code_of("A"), Some(65));
    }
}
