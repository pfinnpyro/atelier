//! Type 1 fonts (PFA, PFB, or PDF FontFile with clear and eexec parts) and
//! their Type 1 charstrings.

use std::collections::HashMap;

use atelier_raster::{Path, PathBuilder, Point, Transform};

use crate::encoding::Encoding;
use crate::{FontError, Result};

const MAX_SUBR_DEPTH: usize = 10;
const MAX_OPS: usize = 100_000;

#[derive(Debug)]
pub struct Type1 {
    font_matrix: [f64; 6],
    /// Built-in encoding: code to glyph name. `None` means StandardEncoding.
    encoding: Option<Vec<Option<String>>>,
    subrs: Vec<Vec<u8>>,
    /// Decrypted charstrings, in file order.
    glyphs: Vec<(String, Vec<u8>)>,
    names: HashMap<String, u32>,
}

impl Type1 {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let (clear, encrypted) = split(data).ok_or(FontError("not a Type 1 font"))?;
        let private = decrypt(&encrypted, 55665, 4);
        let len_iv = find_int(&private, b"/lenIV").unwrap_or(4);
        let decrypt_cs = |cs: &[u8]| -> Vec<u8> {
            if len_iv < 0 {
                cs.to_vec()
            } else {
                decrypt(cs, 4330, len_iv as usize)
            }
        };
        let subrs = read_subrs(&private)
            .into_iter()
            .map(|s| decrypt_cs(&s))
            .collect();
        let glyphs: Vec<(String, Vec<u8>)> = read_charstrings(&private)
            .into_iter()
            .map(|(n, cs)| (n, decrypt_cs(&cs)))
            .collect();
        if glyphs.is_empty() {
            return Err(FontError("Type 1 font has no charstrings"));
        }
        let mut names = HashMap::new();
        for (i, (n, _)) in glyphs.iter().enumerate() {
            names.entry(n.clone()).or_insert(i as u32);
        }
        Ok(Self {
            font_matrix: read_matrix(&clear).unwrap_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]),
            encoding: read_encoding(&clear),
            subrs,
            glyphs,
            names,
        })
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    pub fn glyph_by_name(&self, name: &str) -> Option<u32> {
        self.names.get(name).copied()
    }

    pub fn glyph_name(&self, gid: u32) -> Option<&str> {
        self.glyphs.get(gid as usize).map(|(n, _)| n.as_str())
    }

    pub fn builtin_encoding(&self, code: u8) -> Option<&str> {
        match &self.encoding {
            Some(e) => e.get(usize::from(code))?.as_deref(),
            None => Encoding::Standard.glyph_name(code),
        }
    }

    pub fn advance(&self, gid: u32) -> Option<f64> {
        let (_, w) = self.run(gid)?;
        Some(w * self.font_matrix[0])
    }

    pub fn outline(&self, gid: u32) -> Option<Path> {
        let (p, _) = self.run(gid)?;
        Some(p.transform(&Transform::from_array(self.font_matrix)))
    }

    fn run(&self, gid: u32) -> Option<(Path, f64)> {
        let cs = &self.glyphs.get(gid as usize)?.1;
        let mut st = T1State::new(self, Point::default());
        st.exec(cs, 0);
        st.finish_subpath();
        Some((st.builder.finish(), st.width))
    }
}

/// Splits a font into its clear-text part and its encrypted part (binary).
fn split(data: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if data.first() == Some(&0x80) {
        // PFB: segments of ASCII (1) and binary (2) data.
        let (mut clear, mut bin) = (Vec::new(), Vec::new());
        let mut pos = 0;
        while pos + 6 <= data.len() && data[pos] == 0x80 {
            let kind = data[pos + 1];
            let len = u32::from_le_bytes([data[pos + 2], data[pos + 3], data[pos + 4], data[pos + 5]])
                as usize;
            let start = pos + 6;
            let end = start.saturating_add(len).min(data.len());
            match kind {
                1 if bin.is_empty() => clear.extend_from_slice(&data[start..end]),
                2 => bin.extend_from_slice(&data[start..end]),
                _ => {}
            }
            if kind == 3 {
                break;
            }
            pos = end;
        }
        return (!bin.is_empty()).then_some((clear, bin));
    }
    let at = find(data, b"eexec")?;
    let clear = data[..at].to_vec();
    let mut rest = &data[at + 5..];
    while let [b' ' | b'\t' | b'\r' | b'\n', tail @ ..] = rest {
        rest = tail;
    }
    let is_hex = rest.len() >= 4 && rest[..4].iter().all(u8::is_ascii_hexdigit);
    let bin = if is_hex { unhex(rest) } else { rest.to_vec() };
    Some((clear, bin))
}

fn unhex(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut hi: Option<u8> = None;
    for &c in s {
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => break,
        };
        match hi.take() {
            Some(h) => out.push(h << 4 | v),
            None => hi = Some(v),
        }
    }
    out
}

fn decrypt(data: &[u8], mut r: u16, skip: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for &c in data {
        out.push(c ^ (r >> 8) as u8);
        r = (u16::from(c).wrapping_add(r)).wrapping_mul(52845).wrapping_add(22719);
    }
    out.drain(..skip.min(out.len()));
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0 | 0x0C)
}

fn is_delim(b: u8) -> bool {
    is_space(b) || matches!(b, b'/' | b'[' | b']' | b'{' | b'}' | b'(' | b')' | b'<' | b'>')
}

/// A minimal PostScript token reader over the font's text.
struct Tok<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Tok<'a> {
    fn skip_space(&mut self) {
        while self.p < self.d.len() {
            if is_space(self.d[self.p]) {
                self.p += 1;
            } else if self.d[self.p] == b'%' {
                while self.p < self.d.len() && !matches!(self.d[self.p], b'\r' | b'\n') {
                    self.p += 1;
                }
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> Option<&'a [u8]> {
        self.skip_space();
        let start = self.p;
        let first = *self.d.get(self.p)?;
        self.p += 1;
        if matches!(first, b'[' | b']' | b'{' | b'}') {
            return Some(&self.d[start..self.p]);
        }
        while self.p < self.d.len() && !is_delim(self.d[self.p]) {
            self.p += 1;
        }
        Some(&self.d[start..self.p])
    }

    fn int(&mut self) -> Option<i64> {
        let t = self.next()?;
        std::str::from_utf8(t).ok()?.parse().ok()
    }

    /// Reads `len` binary bytes after an RD token and its single space.
    fn binary(&mut self, len: usize) -> Option<&'a [u8]> {
        self.next()?; // RD or -|
        let start = self.p + 1;
        let end = start.checked_add(len)?;
        let s = self.d.get(start..end)?;
        self.p = end;
        Some(s)
    }
}

fn find_int(d: &[u8], key: &[u8]) -> Option<i64> {
    let at = find(d, key)?;
    let mut t = Tok {
        d,
        p: at + key.len(),
    };
    t.int()
}

fn read_matrix(clear: &[u8]) -> Option<[f64; 6]> {
    let at = find(clear, b"/FontMatrix")?;
    let mut t = Tok {
        d: clear,
        p: at + 11,
    };
    let open = t.next()?;
    if open != b"[" && open != b"{" {
        return None;
    }
    let mut v = [0.0f64; 6];
    for x in &mut v {
        *x = std::str::from_utf8(t.next()?).ok()?.parse().ok()?;
    }
    (v.iter().all(|x| x.is_finite()) && v[0] != 0.0).then_some(v)
}

fn read_encoding(clear: &[u8]) -> Option<Vec<Option<String>>> {
    let at = find(clear, b"/Encoding")?;
    let mut t = Tok {
        d: clear,
        p: at + 9,
    };
    let first = t.next()?;
    if first == b"StandardEncoding" {
        return None;
    }
    let mut enc = vec![None; 256];
    // Scan `dup code /name put` until `readonly def` or `def`.
    loop {
        let tok = t.next()?;
        match tok {
            b"def" | b"readonly" => break,
            b"dup" => {
                let Some(code) = t.int() else { continue };
                let Some(name) = t.next() else { break };
                if let (Ok(code), Some(name)) = (usize::try_from(code), name.strip_prefix(b"/")) {
                    if code < 256 {
                        enc[code] = Some(String::from_utf8_lossy(name).into_owned());
                    }
                }
            }
            _ => {}
        }
    }
    Some(enc)
}

fn read_subrs(private: &[u8]) -> Vec<Vec<u8>> {
    let Some(at) = find(private, b"/Subrs") else {
        return Vec::new();
    };
    let mut t = Tok {
        d: private,
        p: at + 6,
    };
    let count = t.int().unwrap_or(0).clamp(0, 65536) as usize;
    let mut subrs = vec![Vec::new(); count];
    let _ = t.next(); // array
    loop {
        match t.next() {
            Some(b"dup") => {
                let (Some(i), Some(len)) = (t.int(), t.int()) else {
                    break;
                };
                let Some(bytes) = t.binary(len.max(0) as usize) else {
                    break;
                };
                if let Some(slot) = usize::try_from(i).ok().and_then(|i| subrs.get_mut(i)) {
                    *slot = bytes.to_vec();
                }
            }
            Some(b"NP" | b"|" | b"noaccess" | b"put" | b"readonly") => {}
            _ => break,
        }
    }
    subrs
}

fn read_charstrings(private: &[u8]) -> Vec<(String, Vec<u8>)> {
    let Some(at) = find(private, b"/CharStrings") else {
        return Vec::new();
    };
    let mut t = Tok {
        d: private,
        p: at + 12,
    };
    let mut out = Vec::new();
    loop {
        let Some(tok) = t.next() else { break };
        if tok == b"end" {
            break;
        }
        let Some(name) = tok.strip_prefix(b"/") else {
            continue;
        };
        // Skip the dict header `/CharStrings n dict dup begin`.
        let save = t.p;
        let Some(len) = t.int() else {
            t.p = save;
            continue;
        };
        let Some(bytes) = t.binary(len.max(0) as usize) else {
            break;
        };
        out.push((String::from_utf8_lossy(name).into_owned(), bytes.to_vec()));
    }
    out
}

struct T1State<'a> {
    font: &'a Type1,
    builder: PathBuilder,
    stack: Vec<f64>,
    ps_stack: Vec<f64>,
    pos: Point,
    origin: Point,
    width: f64,
    open: bool,
    flex: Option<Vec<Point>>,
    ops: usize,
    done: bool,
    in_seac: bool,
}

impl<'a> T1State<'a> {
    fn new(font: &'a Type1, origin: Point) -> Self {
        Self {
            font,
            builder: PathBuilder::new(),
            stack: Vec::new(),
            ps_stack: Vec::new(),
            pos: origin,
            origin,
            width: 0.0,
            open: false,
            flex: None,
            ops: 0,
            done: false,
            in_seac: false,
        }
    }

    fn finish_subpath(&mut self) {
        if self.open {
            self.builder.close();
            self.open = false;
        }
    }

    fn move_to(&mut self, dx: f64, dy: f64) {
        self.pos = Point::new(self.pos.x + dx, self.pos.y + dy);
        if let Some(f) = &mut self.flex {
            f.push(self.pos);
            return;
        }
        self.finish_subpath();
        self.builder.move_to(self.pos);
        self.open = true;
    }

    fn line_to(&mut self, dx: f64, dy: f64) {
        if !self.open {
            self.builder.move_to(self.pos);
            self.open = true;
        }
        self.pos = Point::new(self.pos.x + dx, self.pos.y + dy);
        self.builder.line_to(self.pos);
    }

    fn curve_to(&mut self, d: [f64; 6]) {
        if !self.open {
            self.builder.move_to(self.pos);
            self.open = true;
        }
        let c1 = Point::new(self.pos.x + d[0], self.pos.y + d[1]);
        let c2 = Point::new(c1.x + d[2], c1.y + d[3]);
        let p = Point::new(c2.x + d[4], c2.y + d[5]);
        self.builder.cubic_to(c1, c2, p);
        self.pos = p;
    }

    fn abs_curve(&mut self, c1: Point, c2: Point, p: Point) {
        if !self.open {
            self.builder.move_to(self.pos);
            self.open = true;
        }
        self.builder.cubic_to(c1, c2, p);
        self.pos = p;
    }

    fn exec(&mut self, cs: &[u8], depth: usize) {
        if depth > MAX_SUBR_DEPTH {
            return;
        }
        let mut i = 0;
        while i < cs.len() && !self.done {
            self.ops += 1;
            if self.ops > MAX_OPS || self.stack.len() > 256 {
                self.done = true;
                return;
            }
            let b = cs[i];
            i += 1;
            let mut byte = || {
                let v = cs.get(i).copied().unwrap_or(0);
                i += 1;
                v
            };
            match b {
                32..=246 => self.stack.push(f64::from(i32::from(b) - 139)),
                247..=250 => {
                    let b1 = i32::from(byte());
                    self.stack.push(f64::from((i32::from(b) - 247) * 256 + b1 + 108));
                }
                251..=254 => {
                    let b1 = i32::from(byte());
                    self.stack.push(f64::from(-(i32::from(b) - 251) * 256 - b1 - 108));
                }
                255 => {
                    let v = i32::from_be_bytes([byte(), byte(), byte(), byte()]);
                    self.stack.push(f64::from(v));
                }
                12 => {
                    let op = byte();
                    self.escape(op, depth);
                }
                _ => self.op(b, depth),
            }
        }
    }

    fn a(&self, i: usize) -> f64 {
        self.stack.get(i).copied().unwrap_or(0.0)
    }

    fn op(&mut self, b: u8, depth: usize) {
        match b {
            1 | 3 => {}
            4 => self.move_to(0.0, self.a(0)),
            5 => self.line_to(self.a(0), self.a(1)),
            6 => self.line_to(self.a(0), 0.0),
            7 => self.line_to(0.0, self.a(0)),
            8 => self.curve_to([self.a(0), self.a(1), self.a(2), self.a(3), self.a(4), self.a(5)]),
            9 => {
                if self.open {
                    self.builder.close();
                    self.open = false;
                }
            }
            10 => {
                let Some(n) = self.stack.pop() else { return };
                let font = self.font;
                if let Some(sub) = usize::try_from(n as i64).ok().and_then(|n| font.subrs.get(n)) {
                    self.exec(sub, depth + 1);
                }
                return;
            }
            11 => return,
            13 => {
                // hsbw: side bearing and width.
                self.pos = Point::new(self.origin.x + self.a(0), self.origin.y);
                if !self.in_seac {
                    self.width = self.a(1);
                }
            }
            14 => {
                self.finish_subpath();
                self.done = true;
            }
            21 => self.move_to(self.a(0), self.a(1)),
            22 => self.move_to(self.a(0), 0.0),
            30 => self.curve_to([0.0, self.a(0), self.a(1), self.a(2), self.a(3), 0.0]),
            31 => self.curve_to([self.a(0), 0.0, self.a(1), self.a(2), 0.0, self.a(3)]),
            _ => {}
        }
        self.stack.clear();
    }

    fn escape(&mut self, op: u8, _depth: usize) {
        match op {
            6 => {
                // seac: asb adx ady bchar achar
                let (asb, adx, ady) = (self.a(0), self.a(1), self.a(2));
                let (bchar, achar) = (self.a(3) as i64, self.a(4) as i64);
                self.stack.clear();
                self.seac(asb, adx, ady, bchar, achar);
                self.done = true;
                return;
            }
            7 => {
                // sbw
                self.pos = Point::new(self.origin.x + self.a(0), self.origin.y + self.a(1));
                if !self.in_seac {
                    self.width = self.a(2);
                }
            }
            12 => {
                let n = self.stack.len();
                if n >= 2 {
                    let d = self.stack[n - 1];
                    let v = if d == 0.0 { 0.0 } else { self.stack[n - 2] / d };
                    self.stack.truncate(n - 2);
                    self.stack.push(v);
                }
                return;
            }
            16 => {
                // callothersubr: args... n othersubr#
                let Some(num) = self.stack.pop() else { return };
                let n = self.stack.pop().unwrap_or(0.0).max(0.0) as usize;
                let n = n.min(self.stack.len());
                let args: Vec<f64> = self.stack.split_off(self.stack.len() - n);
                match num as i64 {
                    1 => self.flex = Some(Vec::new()),
                    2 => {}
                    0 => {
                        let pts = self.flex.take().unwrap_or_default();
                        if pts.len() >= 7 {
                            self.pos = pts[0];
                            self.abs_curve(pts[1], pts[2], pts[3]);
                            self.abs_curve(pts[4], pts[5], pts[6]);
                        }
                        // The font pops the end point and calls setcurrentpoint.
                        let end = self.pos;
                        self.ps_stack = vec![end.y, end.x];
                    }
                    3 => self.ps_stack = vec![3.0],
                    _ => self.ps_stack = args.into_iter().rev().collect(),
                }
                return;
            }
            17 => {
                // pop
                let v = self.ps_stack.pop().unwrap_or(0.0);
                self.stack.push(v);
                return;
            }
            33 => {
                // setcurrentpoint
                self.pos = Point::new(self.a(0), self.a(1));
            }
            _ => {}
        }
        self.stack.clear();
    }

    fn seac(&mut self, asb: f64, adx: f64, ady: f64, bchar: i64, achar: i64) {
        if self.in_seac {
            return;
        }
        let font = self.font;
        let glyph = |code: i64| {
            let code = u8::try_from(code).ok()?;
            let name = Encoding::Standard.glyph_name(code)?;
            let gid = font.glyph_by_name(name)?;
            font.glyphs.get(gid as usize).map(|g| g.1.as_slice())
        };
        let (Some(base), Some(accent)) = (glyph(bchar), glyph(achar)) else {
            return;
        };
        self.finish_subpath();
        // The composite's side bearing; the accent is shifted by
        // adx + sbx - asb, as in the Type 1 specification's seac.
        let sbx = self.pos.x - self.origin.x;
        let accent_origin = Point::new(self.origin.x + adx + sbx - asb, self.origin.y + ady);
        for (cs, origin) in [(base, self.origin), (accent, accent_origin)] {
            let mut sub = T1State::new(font, origin);
            sub.in_seac = true;
            sub.exec(cs, 0);
            sub.finish_subpath();
            self.builder.append(&sub.builder.finish());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eexec_round_trip() {
        // Encrypt then decrypt with the eexec key.
        let plain = b"\0\0\0\0hello";
        let mut r: u16 = 55665;
        let enc: Vec<u8> = plain
            .iter()
            .map(|&p| {
                let c = p ^ (r >> 8) as u8;
                r = (u16::from(c).wrapping_add(r)).wrapping_mul(52845).wrapping_add(22719);
                c
            })
            .collect();
        assert_eq!(decrypt(&enc, 55665, 4), b"hello");
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(Type1::parse(b"%!PS-AdobeFont-1.0 no eexec here").is_err());
    }
}
