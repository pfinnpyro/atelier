//! Content stream parsing (ISO 32000-2, 7.8.2): operands followed by an
//! operator, plus inline images (8.9.7).

use crate::lexer::{is_regular, is_whitespace};
use crate::parser::find;
use crate::{Dict, Name, Object, Parser, Token};

/// One operator with its operands. For `BI ... ID ... EI` the operator is `BI`
/// and `inline_image` holds the dictionary and the raw (still encoded) data.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation<'a> {
    pub operator: &'a [u8],
    pub operands: Vec<Object>,
    pub inline_image: Option<(Dict, Vec<u8>)>,
}

/// Iterates over the operations of a decoded content stream. Malformed input
/// is skipped rather than reported, the way viewers treat damaged content.
pub struct ContentParser<'a> {
    parser: Parser<'a>,
}

impl<'a> ContentParser<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            parser: Parser::new(data),
        }
    }

    fn inline_image(&mut self) -> Option<(Dict, Vec<u8>)> {
        let mut dict = Dict::new();
        loop {
            match self.parser.lexer().next_token().ok()? {
                None => return None,
                Some(Token::Keyword(b"ID")) => break,
                Some(Token::Name(key)) => {
                    let tok = self.parser.lexer().next_token().ok()??;
                    if let Ok(value) = self.parser.object_from(tok, 0) {
                        dict.insert(Name(key), value);
                    }
                }
                Some(_) => {}
            }
        }
        let data = self.parser.lexer().data();
        // A single whitespace byte separates ID from the data.
        let start = (self.parser.pos() + 1).min(data.len());
        let end = match uncompressed_length(&dict) {
            Some(len) if start + len <= data.len() && ends_with_ei(data, start + len) => {
                start + len
            }
            _ => find_ei(data, start)?,
        };
        let image = data[start..end].to_vec();
        let after_ei = skip_ei(data, end);
        self.parser.lexer().set_pos(after_ei);
        Some((dict, image))
    }
}

impl<'a> Iterator for ContentParser<'a> {
    type Item = Operation<'a>;

    fn next(&mut self) -> Option<Operation<'a>> {
        let mut operands = Vec::new();
        loop {
            let tok = match self.parser.lexer().next_token() {
                Ok(Some(t)) => t,
                Ok(None) => return None,
                Err(_) => {
                    // Unterminated string or similar: skip one byte and go on.
                    let pos = self.parser.pos();
                    self.parser.lexer().set_pos(pos + 1);
                    continue;
                }
            };
            match tok {
                Token::Keyword(b"true") => operands.push(Object::Bool(true)),
                Token::Keyword(b"false") => operands.push(Object::Bool(false)),
                Token::Keyword(b"null") => operands.push(Object::Null),
                Token::Keyword(b"BI") => {
                    let image = self.inline_image();
                    return Some(Operation {
                        operator: b"BI",
                        operands,
                        inline_image: image,
                    });
                }
                Token::Keyword(op) => {
                    return Some(Operation {
                        operator: op,
                        operands,
                        inline_image: None,
                    });
                }
                Token::ArrayEnd | Token::DictEnd => {}
                tok => {
                    if let Ok(obj) = self.parser.object_from(tok, 0) {
                        // Guard against unbounded operand lists in garbage data.
                        if operands.len() < 4096 {
                            operands.push(obj);
                        }
                    }
                }
            }
        }
    }
}

/// The byte length of an unfiltered inline image, when it can be computed.
fn uncompressed_length(dict: &Dict) -> Option<usize> {
    let get = |short: &[u8], long: &[u8]| dict.get(short).or_else(|| dict.get(long));
    if get(b"F", b"Filter").is_some_and(|f| !matches!(f, Object::Array(a) if a.is_empty())) {
        return None;
    }
    let w = usize::try_from(get(b"W", b"Width")?.as_i64()?).ok()?;
    let h = usize::try_from(get(b"H", b"Height")?.as_i64()?).ok()?;
    let mask = get(b"IM", b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false);
    let (bpc, comps) = if mask {
        (1, 1)
    } else {
        let bpc = usize::try_from(get(b"BPC", b"BitsPerComponent")?.as_i64()?).ok()?;
        let comps = match get(b"CS", b"ColorSpace")? {
            Object::Name(n) => match n.as_bytes() {
                b"G" | b"DeviceGray" | b"CalGray" => 1,
                b"RGB" | b"DeviceRGB" | b"CalRGB" => 3,
                b"CMYK" | b"DeviceCMYK" => 4,
                _ => return None,
            },
            Object::Array(a)
                if a.first()
                    .and_then(Object::as_name)
                    .is_some_and(|n| *n == b"I" || *n == b"Indexed") =>
            {
                1
            }
            _ => return None,
        };
        (bpc, comps)
    };
    (w * comps * bpc).div_ceil(8).checked_mul(h)
}

fn ends_with_ei(data: &[u8], mut pos: usize) -> bool {
    while data.get(pos).is_some_and(|&b| is_whitespace(b)) {
        pos += 1;
    }
    data[pos..].starts_with(b"EI") && data.get(pos + 2).is_none_or(|&b| !is_regular(b))
}

/// Finds the end of inline image data: `EI` with whitespace before it and a
/// delimiter or end of data after it.
fn find_ei(data: &[u8], start: usize) -> Option<usize> {
    let mut from = start;
    while let Some(pos) = find(data, b"EI", from) {
        let before_ok = pos > start && is_whitespace(data[pos - 1]);
        let after_ok = data.get(pos + 2).is_none_or(|&b| !is_regular(b));
        if before_ok && after_ok {
            return Some(pos - 1);
        }
        from = pos + 2;
    }
    None
}

fn skip_ei(data: &[u8], mut pos: usize) -> usize {
    while data.get(pos).is_some_and(|&b| is_whitespace(b)) {
        pos += 1;
    }
    if data[pos..].starts_with(b"EI") {
        pos + 2
    } else {
        pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(src: &[u8]) -> Vec<(String, usize)> {
        ContentParser::new(src)
            .map(|op| {
                (
                    String::from_utf8_lossy(op.operator).into_owned(),
                    op.operands.len(),
                )
            })
            .collect()
    }

    #[test]
    fn operators_and_operands() {
        assert_eq!(
            ops(b"q 1 0 0 1 72 72 cm 0 0 m 10 10 l S /GS1 gs [(a) 5 (b)] TJ Q"),
            vec![
                ("q".into(), 0),
                ("cm".into(), 6),
                ("m".into(), 2),
                ("l".into(), 2),
                ("S".into(), 0),
                ("gs".into(), 1),
                ("TJ".into(), 1),
                ("Q".into(), 0),
            ]
        );
    }

    #[test]
    fn inline_image_with_known_length_may_contain_ei() {
        let src = b"q BI /W 2 /H 1 /BPC 8 /CS /G ID \x45\x49 EI Q";
        let all: Vec<_> = ContentParser::new(src).collect();
        assert_eq!(all.len(), 3);
        let (dict, data) = all[1].inline_image.clone().unwrap();
        assert_eq!(dict.get(b"W"), Some(&Object::Integer(2)));
        assert_eq!(data, b"EI");
        assert_eq!(all[2].operator, b"Q");
    }

    #[test]
    fn filtered_inline_image_is_found_by_searching() {
        let src = b"BI /W 4 /H 4 /BPC 8 /CS /RGB /F /AHx ID 00ff00 ff0000> EI 0 g";
        let all: Vec<_> = ContentParser::new(src).collect();
        let (_, data) = all[0].inline_image.clone().unwrap();
        assert_eq!(data, b"00ff00 ff0000>");
        assert_eq!(all[1].operator, b"g");
    }

    #[test]
    fn garbage_does_not_stop_parsing() {
        assert_eq!(ops(b"1 2 ] >> (unterminated"), vec![]);
        assert_eq!(ops(b"} 0 g"), vec![("}".into(), 0), ("g".into(), 1)]);
    }
}
