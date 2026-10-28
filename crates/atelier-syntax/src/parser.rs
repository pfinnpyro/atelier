//! Builds objects from tokens (ISO 32000-2, 7.3) and reads indirect objects.

use std::sync::Arc;

use crate::lexer::is_whitespace;
use crate::{Dict, Error, Lexer, Name, ObjRef, Object, PdfString, Result, Stream, Token};

/// Objects nested deeper than this are rejected; hostile files use deep nesting
/// to overflow the stack.
const MAX_DEPTH: usize = 256;

pub struct Parser<'a> {
    lexer: Lexer<'a>,
}

impl<'a> Parser<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            lexer: Lexer::new(data),
        }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self {
            lexer: Lexer::at(data, pos),
        }
    }

    pub fn pos(&self) -> usize {
        self.lexer.pos()
    }

    pub fn lexer(&mut self) -> &mut Lexer<'a> {
        &mut self.lexer
    }

    /// Parses one direct object. `a b R` becomes a reference.
    pub fn parse_object(&mut self) -> Result<Object> {
        let pos = self.lexer.pos();
        match self.lexer.next_token()? {
            Some(tok) => self.object_from(tok, 0),
            None => Err(Error::syntax(pos, "expected an object, found end of data")),
        }
    }

    /// Builds an object from a token already read from this parser's lexer.
    pub fn object_from(&mut self, tok: Token<'a>, depth: usize) -> Result<Object> {
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        Ok(match tok {
            Token::Integer(a) => {
                if let Some(r) = self.try_reference(a) {
                    Object::Reference(r)
                } else {
                    Object::Integer(a)
                }
            }
            Token::Real(r) => Object::Real(r),
            Token::Name(n) => Object::Name(Name(n)),
            Token::String { bytes, hex } => Object::String(PdfString { bytes, hex }),
            Token::ArrayStart => self.array(depth)?,
            Token::DictStart => Object::Dict(self.dict(depth)?),
            Token::Keyword(b"true") => Object::Bool(true),
            Token::Keyword(b"false") => Object::Bool(false),
            Token::Keyword(b"null") => Object::Null,
            Token::Keyword(k) => {
                return Err(Error::syntax(
                    self.lexer.pos() - k.len(),
                    format!("unexpected keyword `{}`", String::from_utf8_lossy(k)),
                ));
            }
            Token::ArrayEnd | Token::DictEnd => {
                return Err(Error::syntax(
                    self.lexer.pos(),
                    "unexpected closing delimiter",
                ));
            }
        })
    }

    /// After an integer, looks ahead for `gen R`. Restores the position if absent.
    fn try_reference(&mut self, num: i64) -> Option<ObjRef> {
        let save = self.lexer.pos();
        let found = (|| {
            let num = u32::try_from(num).ok()?;
            let Ok(Some(Token::Integer(generation))) = self.lexer.next_token() else {
                return None;
            };
            let generation = u16::try_from(generation).ok()?;
            match self.lexer.next_token() {
                Ok(Some(Token::Keyword(b"R"))) => Some(ObjRef::new(num, generation)),
                _ => None,
            }
        })();
        if found.is_none() {
            self.lexer.set_pos(save);
        }
        found
    }

    fn array(&mut self, depth: usize) -> Result<Object> {
        let start = self.lexer.pos();
        let mut items = Vec::new();
        loop {
            let before = self.lexer.pos();
            match self.lexer.next_token()? {
                None => return Err(Error::syntax(start, "unterminated array")),
                Some(Token::ArrayEnd) => break,
                Some(Token::Keyword(b"endobj" | b"stream" | b"endstream")) => {
                    // Missing `]`: stop here and let the caller see the keyword.
                    self.lexer.set_pos(before);
                    break;
                }
                Some(Token::Keyword(k)) if !matches!(k, b"true" | b"false" | b"null") => {
                    items.push(Object::Null);
                }
                Some(Token::DictEnd) => {}
                Some(tok) => items.push(self.object_from(tok, depth + 1)?),
            }
        }
        Ok(Object::Array(items))
    }

    fn dict(&mut self, depth: usize) -> Result<Dict> {
        let start = self.lexer.pos();
        let mut dict = Dict::new();
        loop {
            let before = self.lexer.pos();
            match self.lexer.next_token()? {
                None => return Err(Error::syntax(start, "unterminated dictionary")),
                Some(Token::DictEnd) => break,
                Some(Token::Keyword(b"endobj" | b"stream" | b"endstream")) => {
                    self.lexer.set_pos(before);
                    break;
                }
                Some(Token::Name(key)) => {
                    let before_value = self.lexer.pos();
                    let value = match self.lexer.next_token()? {
                        None => return Err(Error::syntax(start, "unterminated dictionary")),
                        // A key with no value counts as absent.
                        Some(Token::DictEnd) => break,
                        Some(Token::Keyword(b"endobj" | b"stream" | b"endstream")) => {
                            self.lexer.set_pos(before_value);
                            break;
                        }
                        Some(Token::Keyword(k)) if !matches!(k, b"true" | b"false" | b"null") => {
                            Object::Null
                        }
                        Some(tok) => self.object_from(tok, depth + 1)?,
                    };
                    // Per the spec a null value is the same as an absent key.
                    if !value.is_null() {
                        dict.insert(Name(key), value);
                    }
                }
                // Junk where a key should be: skip it.
                Some(_) => {}
            }
        }
        Ok(dict)
    }

    /// Reads `num gen obj ... endobj` at the current position. `length` resolves
    /// an indirect `/Length` for streams.
    pub fn parse_indirect(
        &mut self,
        length: &dyn Fn(ObjRef) -> Option<i64>,
    ) -> Result<(ObjRef, Object)> {
        let start = self.lexer.pos();
        let header = (
            self.lexer.next_token()?,
            self.lexer.next_token()?,
            self.lexer.next_token()?,
        );
        let r = match header {
            (Some(Token::Integer(n)), Some(Token::Integer(g)), Some(Token::Keyword(b"obj"))) => {
                match (u32::try_from(n), u16::try_from(g)) {
                    (Ok(n), Ok(g)) => ObjRef::new(n, g),
                    _ => return Err(Error::syntax(start, "object number out of range")),
                }
            }
            _ => return Err(Error::syntax(start, "expected `num gen obj`")),
        };

        let before_body = self.lexer.pos();
        let object = match self.lexer.next_token()? {
            // `1 0 obj endobj` is a null object.
            Some(Token::Keyword(b"endobj")) | None => {
                self.lexer.set_pos(before_body);
                Object::Null
            }
            Some(tok) => self.object_from(tok, 0)?,
        };

        let object = match object {
            Object::Dict(dict) => {
                let save = self.lexer.pos();
                match self.lexer.next_token()? {
                    Some(Token::Keyword(b"stream")) => {
                        Object::Stream(self.stream_body(dict, length)?)
                    }
                    _ => {
                        self.lexer.set_pos(save);
                        Object::Dict(dict)
                    }
                }
            }
            other => other,
        };

        let save = self.lexer.pos();
        if !matches!(self.lexer.next_token(), Ok(Some(Token::Keyword(b"endobj")))) {
            // Missing `endobj` is common and harmless.
            self.lexer.set_pos(save);
        }
        Ok((r, object))
    }

    /// Reads stream data after the `stream` keyword.
    fn stream_body(
        &mut self,
        dict: Dict,
        length: &dyn Fn(ObjRef) -> Option<i64>,
    ) -> Result<Stream> {
        let data = self.lexer.data();
        let mut pos = self.lexer.pos();
        // The keyword is followed by CRLF or LF; tolerate spaces and a lone CR.
        while data.get(pos) == Some(&b' ') {
            pos += 1;
        }
        match (data.get(pos), data.get(pos + 1)) {
            (Some(b'\r'), Some(b'\n')) => pos += 2,
            (Some(b'\n' | b'\r'), _) => pos += 1,
            _ => {}
        }
        let begin = pos;

        let declared = match dict.get(b"Length") {
            Some(Object::Reference(r)) => length(*r),
            Some(other) => other.as_i64(),
            None => None,
        };
        let declared = declared
            .and_then(|l| usize::try_from(l).ok())
            .filter(|&l| begin.checked_add(l).is_some_and(|end| end <= data.len()))
            .filter(|&l| followed_by_endstream(data, begin + l));

        let end = match declared {
            Some(len) => begin + len,
            None => {
                // Wrong or missing /Length: find `endstream` and trim the EOL before it.
                let mut end = find(data, b"endstream", begin).unwrap_or(data.len());
                if end > begin && data[end - 1] == b'\n' {
                    end -= 1;
                }
                if end > begin && data[end - 1] == b'\r' {
                    end -= 1;
                }
                end
            }
        };

        let after = find(data, b"endstream", end).map_or(data.len(), |p| p + b"endstream".len());
        self.lexer.set_pos(after);
        Ok(Stream {
            dict,
            data: Arc::from(&data[begin..end]),
        })
    }
}

fn followed_by_endstream(data: &[u8], mut pos: usize) -> bool {
    while data.get(pos).is_some_and(|&b| is_whitespace(b)) {
        pos += 1;
    }
    data[pos..].starts_with(b"endstream")
}

/// Finds `needle` in `haystack` at or after `from`.
pub(crate) fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() || needle.is_empty() {
        return None;
    }
    let first = needle[0];
    let mut i = from;
    while i + needle.len() <= haystack.len() {
        i += haystack[i..].iter().position(|&b| b == first)?;
        if haystack[i..].starts_with(needle) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Finds the last `needle` in `haystack`.
pub(crate) fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Object {
        Parser::new(src.as_bytes()).parse_object().unwrap()
    }

    #[test]
    fn references_and_plain_integers() {
        assert_eq!(parse("[1 0 R 2 3 4 R 5]").to_string(), "[1 0 R 2 3 4 R 5]");
        assert_eq!(parse("[1 2]").to_string(), "[1 2]");
    }

    #[test]
    fn nested_dicts() {
        let obj = parse("<< /A << /B [1 (x) /N] >> /C true /D null >>");
        assert_eq!(obj.to_string(), "<< /A << /B [1 (x) /N] >> /C true >>");
    }

    #[test]
    fn missing_value_is_tolerated() {
        assert_eq!(parse("<< /A /B /C >>").to_string(), "<< /A /B >>");
    }

    #[test]
    fn deep_nesting_is_rejected() {
        let src = "[".repeat(10_000);
        assert_eq!(
            Parser::new(src.as_bytes()).parse_object(),
            Err(Error::TooDeep)
        );
    }

    #[test]
    fn indirect_stream_with_direct_length() {
        let src = b"7 0 obj\n<< /Length 5 >>\nstream\r\nhello\nendstream\nendobj";
        let (r, obj) = Parser::new(src).parse_indirect(&|_| None).unwrap();
        assert_eq!(r, ObjRef::new(7, 0));
        assert_eq!(&*obj.as_stream().unwrap().data, b"hello");
    }

    #[test]
    fn wrong_length_falls_back_to_endstream() {
        let src = b"7 0 obj << /Length 99 >> stream\nhello world\nendstream endobj";
        let (_, obj) = Parser::new(src).parse_indirect(&|_| None).unwrap();
        assert_eq!(&*obj.as_stream().unwrap().data, b"hello world");
    }

    #[test]
    fn indirect_length_uses_resolver() {
        let src = b"7 0 obj << /Length 8 0 R >> stream\nabc\nendstream endobj";
        let (_, obj) = Parser::new(src)
            .parse_indirect(&|r| (r == ObjRef::new(8, 0)).then_some(3))
            .unwrap();
        assert_eq!(&*obj.as_stream().unwrap().data, b"abc");
    }

    #[test]
    fn missing_endobj_is_fine() {
        let (_, obj) = Parser::new(b"3 0 obj 42 4 0 obj")
            .parse_indirect(&|_| None)
            .unwrap();
        assert_eq!(obj, Object::Integer(42));
    }
}
