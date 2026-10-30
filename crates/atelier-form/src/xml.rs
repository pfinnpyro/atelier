//! Just enough XML to read an XFA form: elements, attributes and text.
//! It is forgiving about damage and never fetches anything: no DTDs, no
//! external entities, only the five built-in entities and character
//! references.

/// One piece of an XML document.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    /// An opening tag. `name` is the local name, without any prefix.
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        empty: bool,
    },
    End {
        name: String,
    },
    /// Character data, with entities expanded; CDATA sections included.
    Text(String),
}

/// Reads tokens from XML bytes. Invalid UTF-8 is replaced, not refused.
pub(crate) struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(src: &'a [u8]) -> Self {
        Self { src, pos: 0 }
    }

    fn starts(&self, s: &[u8]) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    /// Skips past `end`, or to the end of the input.
    fn skip_past(&mut self, end: &[u8]) -> &'a [u8] {
        let from = self.pos;
        match find(&self.src[from..], end) {
            Some(i) => {
                self.pos = from + i + end.len();
                &self.src[from..from + i]
            }
            None => {
                self.pos = self.src.len();
                &self.src[from..]
            }
        }
    }

    fn tag(&mut self) -> Option<Token> {
        // At '<' of an ordinary tag.
        self.pos += 1;
        let end = self.starts(b"/");
        if end {
            self.pos += 1;
        }
        let name = self.name();
        if end {
            self.skip_past(b">");
            return Some(Token::End { name: local(&name) });
        }
        let mut attrs = Vec::new();
        loop {
            self.skip_space();
            if self.pos >= self.src.len() {
                break;
            }
            if self.starts(b"/>") {
                self.pos += 2;
                return Some(Token::Start {
                    name: local(&name),
                    attrs,
                    empty: true,
                });
            }
            if self.starts(b">") {
                self.pos += 1;
                break;
            }
            let key = self.name();
            if key.is_empty() {
                // Junk inside the tag: step over it.
                self.pos += 1;
                continue;
            }
            self.skip_space();
            let mut value = String::new();
            if self.starts(b"=") {
                self.pos += 1;
                self.skip_space();
                if let Some(&q) = self
                    .src
                    .get(self.pos)
                    .filter(|q| **q == b'"' || **q == b'\'')
                {
                    self.pos += 1;
                    let raw = self.skip_past(&[q]);
                    value = unescape(raw);
                } else {
                    value = self.name();
                }
            }
            attrs.push((key, value));
        }
        Some(Token::Start {
            name: local(&name),
            attrs,
            empty: false,
        })
    }

    fn name(&mut self) -> String {
        let from = self.pos;
        while let Some(&b) = self.src.get(self.pos) {
            if b.is_ascii_whitespace() || matches!(b, b'>' | b'/' | b'=' | b'<' | b'"' | b'\'') {
                break;
            }
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.src[from..self.pos]).into_owned()
    }

    fn skip_space(&mut self) {
        while self.src.get(self.pos).is_some_and(u8::is_ascii_whitespace) {
            self.pos += 1;
        }
    }
}

impl Iterator for Reader<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        loop {
            if self.pos >= self.src.len() {
                return None;
            }
            if self.starts(b"<!--") {
                self.pos += 4;
                self.skip_past(b"-->");
            } else if self.starts(b"<![CDATA[") {
                self.pos += 9;
                let raw = self.skip_past(b"]]>");
                return Some(Token::Text(String::from_utf8_lossy(raw).into_owned()));
            } else if self.starts(b"<?") {
                self.skip_past(b"?>");
            } else if self.starts(b"<!") {
                // A DOCTYPE or similar: its contents are never used.
                self.skip_past(b">");
            } else if self.starts(b"<") {
                return self.tag();
            } else {
                let from = self.pos;
                let to = find(&self.src[from..], b"<").map_or(self.src.len(), |i| from + i);
                self.pos = to;
                return Some(Token::Text(unescape(&self.src[from..to])));
            }
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The name without its namespace prefix.
fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// Expands the built-in entities and character references.
fn unescape(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    if !text.contains('&') {
        return text.into_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = &text[..];
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let window = &rest.as_bytes()[..rest.len().min(12)];
        let Some(end) = window.iter().position(|&b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let ch = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The value of an attribute.
pub(crate) fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| local(k) == key)
        .map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tags_text_and_entities() {
        let xml = br#"<?xml version="1.0"?><!-- c --><a:x y='1' z="&lt;&#65;&#x42;"><b/>t&amp;u<![CDATA[<raw>]]></a:x>"#;
        let tokens: Vec<Token> = Reader::new(xml).collect();
        assert_eq!(
            tokens,
            vec![
                Token::Start {
                    name: "x".into(),
                    attrs: vec![("y".into(), "1".into()), ("z".into(), "<AB".into())],
                    empty: false,
                },
                Token::Start {
                    name: "b".into(),
                    attrs: vec![],
                    empty: true,
                },
                Token::Text("t&u".into()),
                Token::Text("<raw>".into()),
                Token::End { name: "x".into() },
            ]
        );
    }

    #[test]
    fn damage_does_not_stop_it() {
        let tokens: Vec<Token> = Reader::new(b"<a b=\"unterminated><c").collect();
        assert!(!tokens.is_empty());
        let tokens: Vec<Token> = Reader::new(b"&bogus; & <").collect();
        assert!(!tokens.is_empty());
    }
}
