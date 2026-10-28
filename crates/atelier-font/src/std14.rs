//! The 14 standard fonts every PDF reader must know without embedding.

use crate::data::std14_widths::FONTS;

/// Metrics of a standard font, in 1/1000 em.
#[derive(Debug)]
pub struct Std14 {
    pub name: &'static str,
    pub ascent: i16,
    pub descent: i16,
    /// Glyph widths, sorted by glyph name.
    pub widths: &'static [(&'static str, u16)],
}

impl Std14 {
    pub fn width(&self, glyph: &str) -> Option<u16> {
        self.widths
            .binary_search_by(|(n, _)| (*n).cmp(glyph))
            .ok()
            .map(|i| self.widths[i].1)
    }

    pub fn is_fixed_pitch(&self) -> bool {
        self.name.starts_with("Courier")
    }

    pub fn is_serif(&self) -> bool {
        self.name.starts_with("Times")
    }

    pub fn is_bold(&self) -> bool {
        self.name.contains("Bold")
    }

    pub fn is_italic(&self) -> bool {
        self.name.contains("Italic") || self.name.contains("Oblique")
    }

    /// Symbol and ZapfDingbats use their own encodings and glyph sets.
    pub fn is_symbolic(&self) -> bool {
        matches!(self.name, "Symbol" | "ZapfDingbats")
    }
}

/// The standard font a base font name refers to, including common aliases
/// such as Arial and TimesNewRoman. A subset prefix (`ABCDEF+`) is ignored.
pub fn standard_font(base_font: &str) -> Option<&'static Std14> {
    let name = strip_subset(base_font);
    let canonical = match name {
        "Courier" | "CourierNew" | "CourierNewPSMT" => "Courier",
        "Courier-Bold" | "CourierNew,Bold" | "CourierNew-Bold" | "CourierNewPS-BoldMT" => {
            "Courier-Bold"
        }
        "Courier-Oblique" | "CourierNew,Italic" | "CourierNew-Italic" | "CourierNewPS-ItalicMT" => {
            "Courier-Oblique"
        }
        "Courier-BoldOblique"
        | "CourierNew,BoldItalic"
        | "CourierNew-BoldItalic"
        | "CourierNewPS-BoldItalicMT" => "Courier-BoldOblique",
        "Helvetica" | "Arial" | "ArialMT" => "Helvetica",
        "Helvetica-Bold" | "Arial,Bold" | "Arial-Bold" | "Arial-BoldMT" | "Helvetica,Bold" => {
            "Helvetica-Bold"
        }
        "Helvetica-Oblique" | "Arial,Italic" | "Arial-Italic" | "Arial-ItalicMT"
        | "Helvetica,Italic" | "Helvetica-Italic" => "Helvetica-Oblique",
        "Helvetica-BoldOblique"
        | "Arial,BoldItalic"
        | "Arial-BoldItalic"
        | "Arial-BoldItalicMT"
        | "Helvetica,BoldItalic"
        | "Helvetica-BoldItalic" => "Helvetica-BoldOblique",
        "Times-Roman" | "TimesNewRoman" | "TimesNewRomanPSMT" | "Times" | "TimesNewRomanPS" => {
            "Times-Roman"
        }
        "Times-Bold" | "TimesNewRoman,Bold" | "TimesNewRoman-Bold" | "TimesNewRomanPS-BoldMT" => {
            "Times-Bold"
        }
        "Times-Italic"
        | "TimesNewRoman,Italic"
        | "TimesNewRoman-Italic"
        | "TimesNewRomanPS-ItalicMT" => "Times-Italic",
        "Times-BoldItalic"
        | "TimesNewRoman,BoldItalic"
        | "TimesNewRoman-BoldItalic"
        | "TimesNewRomanPS-BoldItalicMT" => "Times-BoldItalic",
        "Symbol" | "Symbol,Bold" | "Symbol,Italic" | "Symbol,BoldItalic" | "SymbolMT" => "Symbol",
        "ZapfDingbats" | "Dingbats" | "ZapfDingbatsITC" => "ZapfDingbats",
        _ => return None,
    };
    FONTS.iter().find(|f| f.name == canonical)
}

/// Removes a subset tag such as `ABCDEF+` from a font name.
pub fn strip_subset(name: &str) -> &str {
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_and_aliases() {
        let h = standard_font("Helvetica").unwrap();
        assert_eq!(h.width("A"), Some(667));
        assert_eq!(h.width("space"), Some(278));
        assert_eq!(
            standard_font("ABCDEF+Arial,Bold").unwrap().name,
            "Helvetica-Bold"
        );
        assert_eq!(
            standard_font("TimesNewRomanPSMT").unwrap().name,
            "Times-Roman"
        );
        assert!(standard_font("Courier").unwrap().is_fixed_pitch());
        assert!(standard_font("Comic Sans").is_none());
    }
}
