//! Default appearance strings (/DA): the font, size and colour that a
//! field's text is drawn with (ISO 32000-2, 12.7.4.3).

use atelier_syntax::{ContentParser, Object};

#[derive(Debug, Clone, PartialEq)]
pub struct DefaultAppearance {
    /// The font's name in the resource dictionary.
    pub font: Option<Vec<u8>>,
    /// 0 means size the text to fit.
    pub size: f64,
    /// Fill colour operator and operands, such as `0 g` or `1 0 0 rg`.
    pub color: String,
}

impl Default for DefaultAppearance {
    fn default() -> Self {
        Self {
            font: None,
            size: 0.0,
            color: "0 g".to_string(),
        }
    }
}

impl DefaultAppearance {
    pub fn parse(da: &str) -> Self {
        let mut out = Self::default();
        let bytes = da.as_bytes();
        for op in ContentParser::new(bytes) {
            let nums: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
            match op.operator {
                b"Tf" => {
                    if let [Object::Name(n), size] = op.operands.as_slice() {
                        out.font = Some(n.0.clone());
                        out.size = size.as_f64().unwrap_or(0.0).abs();
                    }
                }
                b"g" if nums.len() == 1 => out.color = format!("{} g", num(nums[0])),
                b"rg" if nums.len() == 3 => {
                    out.color = format!("{} {} {} rg", num(nums[0]), num(nums[1]), num(nums[2]));
                }
                b"k" if nums.len() == 4 => {
                    out.color = format!(
                        "{} {} {} {} k",
                        num(nums[0]),
                        num(nums[1]),
                        num(nums[2]),
                        num(nums[3])
                    );
                }
                _ => {}
            }
        }
        out
    }
}

/// A number for a content stream: short, without exponent notation.
pub(crate) fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_font_size_and_colour() {
        let da = DefaultAppearance::parse("/Helv 12 Tf 0 0 1 rg");
        assert_eq!(da.font.as_deref(), Some(&b"Helv"[..]));
        assert_eq!(da.size, 12.0);
        assert_eq!(da.color, "0 0 1 rg");
        let da = DefaultAppearance::parse("0.5 g /F1 0 Tf");
        assert_eq!(da.size, 0.0);
        assert_eq!(da.color, "0.5 g");
        assert_eq!(DefaultAppearance::parse("garbage ( Tf").font, None);
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(0.25), "0.25");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(12.3456), "12.346");
    }
}
