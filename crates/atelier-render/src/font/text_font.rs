//! Fonts for text that Atelier writes itself, such as form field values.

use atelier_font::{standard_font, strip_subset};
use atelier_syntax::{Dict, Document};

use super::PdfFont;

/// A font from a resource dictionary, with what laying out new text needs:
/// codes for characters, widths and vertical metrics.
#[derive(Debug)]
pub struct TextFont {
    font: PdfFont,
    ascent: f64,
    descent: f64,
}

impl TextFont {
    pub fn load(doc: &Document, dict: &Dict) -> Self {
        let font = PdfFont::load(doc, dict, &mut |_| {});
        let (mut ascent, mut descent) = (0.0, 0.0);
        let desc = doc
            .get_key(dict, b"FontDescriptor")
            .ok()
            .and_then(|o| o.as_dict().cloned())
            .or_else(|| {
                // Type 0: the descendant font holds the descriptor.
                let kids = doc.get_key(dict, b"DescendantFonts").ok()?;
                let first = doc.resolve(kids.as_array()?.first()?).ok()?;
                let first = first.as_dict()?;
                doc.get_key(first, b"FontDescriptor")
                    .ok()?
                    .as_dict()
                    .cloned()
            });
        if let Some(desc) = desc {
            let num = |k: &[u8]| doc.get_key(&desc, k).ok().and_then(|o| o.as_f64());
            ascent = num(b"Ascent").unwrap_or(0.0) / 1000.0;
            descent = num(b"Descent").unwrap_or(0.0) / 1000.0;
        }
        if ascent <= 0.0 {
            let base = doc
                .get_key(dict, b"BaseFont")
                .ok()
                .and_then(|o| o.as_name().map(|n| n.as_str_lossy().into_owned()))
                .unwrap_or_default();
            match standard_font(strip_subset(&base)) {
                Some(std) if std.ascent > 0 => {
                    ascent = f64::from(std.ascent) / 1000.0;
                    descent = f64::from(std.descent) / 1000.0;
                }
                _ => (ascent, descent) = (0.8, -0.2),
            }
        }
        // Descent is negative; some files give it as a positive number.
        Self {
            font,
            ascent,
            descent: -descent.abs(),
        }
    }

    /// Codes that show `text`, or None when a character is missing.
    pub fn encode(&self, text: &str) -> Option<Vec<u8>> {
        self.font.encode(text)
    }

    /// Width of shown bytes, in text space units (1.0 = font size).
    pub fn width(&self, bytes: &[u8]) -> f64 {
        self.font.width_of(bytes)
    }

    /// Height above the baseline, as a fraction of the font size.
    pub fn ascent(&self) -> f64 {
        self.ascent
    }

    /// Depth below the baseline, negative, as a fraction of the font size.
    pub fn descent(&self) -> f64 {
        self.descent
    }

    pub fn name(&self) -> &str {
        &self.font.name
    }
}
