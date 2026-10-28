//! Tokenizer for PDF syntax (ISO 32000-2, 7.2 and 7.3).

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Token<'a> {
    Integer(i64),
    Real(f64),
    Name(Vec<u8>),
    String {
        bytes: Vec<u8>,
        hex: bool,
    },
    ArrayStart,
    ArrayEnd,
    DictStart,
    DictEnd,
    /// A bare word: `obj`, `R`, `true`, `stream`, an operator in a content stream...
    Keyword(&'a [u8]),
}

#[inline]
pub(crate) fn is_whitespace(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

#[inline]
pub(crate) fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

#[inline]
pub(crate) fn is_regular(b: u8) -> bool {
    !is_whitespace(b) && !is_delimiter(b)
}

/// A cursor over PDF bytes that yields tokens.
#[derive(Clone)]
pub struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos: pos.min(data.len()),
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos.min(self.data.len());
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Skips whitespace and `%` comments.
    pub fn skip_whitespace(&mut self) {
        while let Some(&b) = self.data.get(self.pos) {
            if is_whitespace(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(&c) = self.data.get(self.pos) {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// Returns the next token, or `None` at the end of the data.
    pub fn next_token(&mut self) -> Result<Option<Token<'a>>> {
        self.skip_whitespace();
        let Some(&b) = self.data.get(self.pos) else {
            return Ok(None);
        };
        let start = self.pos;
        let token = match b {
            b'[' => {
                self.pos += 1;
                Token::ArrayStart
            }
            b']' => {
                self.pos += 1;
                Token::ArrayEnd
            }
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                Token::DictStart
            }
            b'>' if self.data.get(self.pos + 1) == Some(&b'>') => {
                self.pos += 2;
                Token::DictEnd
            }
            b'<' => self.hex_string()?,
            b'(' => self.literal_string()?,
            b'/' => self.name(),
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.number(),
            b')' | b'>' | b'{' | b'}' => {
                // Stray delimiters; PostScript functions use braces, which the
                // function parser handles itself. Return them as one-byte keywords.
                self.pos += 1;
                Token::Keyword(&self.data[start..self.pos])
            }
            _ => {
                while self.data.get(self.pos).is_some_and(|&c| is_regular(c)) {
                    self.pos += 1;
                }
                Token::Keyword(&self.data[start..self.pos])
            }
        };
        Ok(Some(token))
    }

    /// Peeks at the next token without consuming it.
    pub fn peek_token(&mut self) -> Result<Option<Token<'a>>> {
        let pos = self.pos;
        let t = self.next_token();
        self.pos = pos;
        t
    }

    fn name(&mut self) -> Token<'a> {
        self.pos += 1; // '/'
        let mut out = Vec::new();
        while let Some(&b) = self.data.get(self.pos) {
            if !is_regular(b) {
                break;
            }
            if b == b'#' {
                let hex = self.data.get(self.pos + 1..self.pos + 3);
                if let Some(v) = hex
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    out.push(v);
                    self.pos += 3;
                    continue;
                }
            }
            out.push(b);
            self.pos += 1;
        }
        Token::Name(out)
    }

    /// Parses a number leniently: junk like `--5` or `5-` is read the way other
    /// viewers read it rather than rejected.
    fn number(&mut self) -> Token<'a> {
        let start = self.pos;
        while self
            .data
            .get(self.pos)
            .is_some_and(|&c| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.'))
        {
            self.pos += 1;
        }
        let raw = &self.data[start..self.pos];
        let negative = raw
            .iter()
            .take_while(|&&c| c == b'-' || c == b'+')
            .any(|&c| c == b'-');
        let body: Vec<u8> = raw
            .iter()
            .copied()
            .skip_while(|&c| c == b'-' || c == b'+')
            .filter(|&c| c != b'-' && c != b'+')
            .collect();
        let sign = if negative { -1.0 } else { 1.0 };
        if body.contains(&b'.') {
            // Keep only the first decimal point.
            let mut seen = false;
            let cleaned: String = body
                .iter()
                .filter(|&&c| {
                    if c == b'.' {
                        let keep = !seen;
                        seen = true;
                        keep
                    } else {
                        true
                    }
                })
                .map(|&c| c as char)
                .collect();
            let v = cleaned.parse::<f64>().unwrap_or(0.0);
            return Token::Real(sign * v);
        }
        let digits = std::str::from_utf8(&body).unwrap_or("0");
        match digits.parse::<i64>() {
            Ok(v) => Token::Integer(if negative { -v } else { v }),
            Err(_) if digits.is_empty() => Token::Integer(0),
            Err(_) => Token::Real(sign * digits.parse::<f64>().unwrap_or(0.0)),
        }
    }

    fn hex_string(&mut self) -> Result<Token<'a>> {
        let start = self.pos;
        self.pos += 1;
        let mut out = Vec::new();
        let mut high: Option<u8> = None;
        loop {
            let Some(&b) = self.data.get(self.pos) else {
                return Err(Error::syntax(start, "unterminated hex string"));
            };
            self.pos += 1;
            let nibble = match b {
                b'>' => break,
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                // Whitespace and junk are skipped.
                _ => continue,
            };
            match high.take() {
                Some(h) => out.push(h << 4 | nibble),
                None => high = Some(nibble),
            }
        }
        if let Some(h) = high {
            out.push(h << 4);
        }
        Ok(Token::String {
            bytes: out,
            hex: true,
        })
    }

    fn literal_string(&mut self) -> Result<Token<'a>> {
        let start = self.pos;
        self.pos += 1;
        let mut out = Vec::new();
        let mut depth = 1usize;
        loop {
            let Some(&b) = self.data.get(self.pos) else {
                return Err(Error::syntax(start, "unterminated string"));
            };
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(b);
                }
                b'\r' => {
                    // End-of-line in a string is always read as a single LF.
                    if self.data.get(self.pos) == Some(&b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                }
                b'\\' => {
                    let Some(&e) = self.data.get(self.pos) else {
                        continue;
                    };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.data.get(self.pos) {
                                    Some(&d @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(d - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        // Backslash at end of line continues the string.
                        b'\r' => {
                            if self.data.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        // \( \) \\ and unknown escapes: keep the character.
                        other => out.push(other),
                    }
                }
                _ => out.push(b),
            }
        }
        Ok(Token::String {
            bytes: out,
            hex: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(src: &[u8]) -> Vec<Token<'_>> {
        let mut lx = Lexer::new(src);
        let mut out = Vec::new();
        while let Some(t) = lx.next_token().unwrap() {
            out.push(t);
        }
        out
    }

    fn string(bytes: &[u8], hex: bool) -> Token<'static> {
        Token::String {
            bytes: bytes.to_vec(),
            hex,
        }
    }

    #[test]
    fn numbers() {
        assert_eq!(
            tokens(b"123 -17 +4 3.25 -.002 0.0 4. --5 2147483648000"),
            vec![
                Token::Integer(123),
                Token::Integer(-17),
                Token::Integer(4),
                Token::Real(3.25),
                Token::Real(-0.002),
                Token::Real(0.0),
                Token::Real(4.0),
                Token::Integer(-5),
                Token::Integer(2_147_483_648_000),
            ]
        );
    }

    #[test]
    fn names_with_escapes() {
        assert_eq!(
            tokens(b"/Type/A#20B /#2Fslash /"),
            vec![
                Token::Name(b"Type".to_vec()),
                Token::Name(b"A B".to_vec()),
                Token::Name(b"/slash".to_vec()),
                Token::Name(Vec::new()),
            ]
        );
    }

    #[test]
    fn literal_strings() {
        assert_eq!(
            tokens(b"(a (nested) str) (esc\\n\\(\\)\\\\\\101\\7) (line\\\ncont) (cr\r\nlf)"),
            vec![
                string(b"a (nested) str", false),
                string(b"esc\n()\\A\x07", false),
                string(b"linecont", false),
                string(b"cr\nlf", false),
            ]
        );
    }

    #[test]
    fn hex_strings() {
        assert_eq!(
            tokens(b"<48 65 6C6c6F> <414>"),
            vec![string(b"Hello", true), string(&[0x41, 0x40], true)]
        );
    }

    #[test]
    fn structure_and_comments() {
        assert_eq!(
            tokens(b"<< /K [1 2 R] >> % comment\nendobj"),
            vec![
                Token::DictStart,
                Token::Name(b"K".to_vec()),
                Token::ArrayStart,
                Token::Integer(1),
                Token::Integer(2),
                Token::Keyword(b"R"),
                Token::ArrayEnd,
                Token::DictEnd,
                Token::Keyword(b"endobj"),
            ]
        );
    }

    #[test]
    fn unterminated_string_is_an_error() {
        let mut lx = Lexer::new(b"(abc");
        assert!(lx.next_token().is_err());
    }
}
