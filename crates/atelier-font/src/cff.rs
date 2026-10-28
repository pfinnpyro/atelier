//! CFF (Compact Font Format) fonts with Type 2 charstrings, both name-keyed
//! and CID-keyed, bare (PDF FontFile3) or inside an OpenType font.

use std::collections::HashMap;

use atelier_raster::{Path, PathBuilder, Point};

use crate::data::cff_std::{EXPERT_CHARSET, EXPERT_SUBSET_CHARSET, STANDARD_STRINGS};
use crate::encoding::Encoding;
use crate::reader::Reader;
use crate::truetype::TrueType;
use crate::{FontError, Result};

const MAX_SUBR_DEPTH: usize = 10;
const MAX_STACK: usize = 513;
/// Charstring operators run per glyph before giving up on it.
const MAX_OPS: usize = 100_000;

/// A CFF INDEX: a list of byte strings.
#[derive(Debug, Clone, Default)]
struct Index {
    /// Absolute offsets of each item's start, plus the end of the last item.
    offsets: Vec<usize>,
}

impl Index {
    /// Reads the INDEX at `pos`; returns it and the position after it.
    fn read(data: &[u8], pos: usize) -> Option<(Index, usize)> {
        let mut r = Reader::at(data, pos);
        let count = usize::from(r.u16()?);
        if count == 0 {
            return Some((Index::default(), pos + 2));
        }
        let size = r.u8()?;
        let base = pos + 3 + (count + 1) * usize::from(size) - 1;
        let mut offsets = Vec::with_capacity(count + 1);
        for _ in 0..=count {
            offsets.push(base + r.uint(size)? as usize);
        }
        let end = *offsets.last()?;
        Some((Index { offsets }, end.min(data.len())))
    }

    fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    fn get<'a>(&self, data: &'a [u8], i: usize) -> Option<&'a [u8]> {
        let (s, e) = (*self.offsets.get(i)?, *self.offsets.get(i + 1)?);
        data.get(s..e)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Operand {
    Int(i32),
    Real(f64),
}

impl Operand {
    fn f(&self) -> f64 {
        match *self {
            Self::Int(i) => f64::from(i),
            Self::Real(r) => r,
        }
    }
    fn i(&self) -> i64 {
        match *self {
            Self::Int(i) => i64::from(i),
            Self::Real(r) => r as i64,
        }
    }
}

/// Parses a DICT into operator (escaped ones as 1200 + op) -> operands.
fn parse_dict(data: &[u8]) -> HashMap<u16, Vec<Operand>> {
    let mut out = HashMap::new();
    let mut ops = Vec::new();
    let mut r = Reader::new(data);
    while let Some(b) = r.u8() {
        match b {
            0..=21 => {
                let key = if b == 12 {
                    1200 + u16::from(r.u8().unwrap_or(0))
                } else {
                    u16::from(b)
                };
                out.insert(key, std::mem::take(&mut ops));
            }
            28 => ops.push(Operand::Int(i32::from(r.i16().unwrap_or(0)))),
            29 => ops.push(Operand::Int(r.u32().unwrap_or(0) as i32)),
            30 => ops.push(Operand::Real(read_real(&mut r))),
            32..=246 => ops.push(Operand::Int(i32::from(b) - 139)),
            247..=250 => {
                let b1 = i32::from(r.u8().unwrap_or(0));
                ops.push(Operand::Int((i32::from(b) - 247) * 256 + b1 + 108));
            }
            251..=254 => {
                let b1 = i32::from(r.u8().unwrap_or(0));
                ops.push(Operand::Int(-(i32::from(b) - 251) * 256 - b1 - 108));
            }
            _ => {}
        }
    }
    out
}

fn read_real(r: &mut Reader) -> f64 {
    let mut s = String::new();
    'outer: while let Some(b) = r.u8() {
        for nib in [b >> 4, b & 15] {
            match nib {
                0..=9 => s.push(char::from(b'0' + nib)),
                0xA => s.push('.'),
                0xB => s.push('E'),
                0xC => s.push_str("E-"),
                0xE => s.push('-'),
                0xF => break 'outer,
                _ => {}
            }
        }
    }
    s.parse().unwrap_or(0.0)
}

#[derive(Debug, Clone, Default)]
struct Private {
    subrs: Index,
    default_width: f64,
    nominal_width: f64,
}

fn read_private(data: &[u8], top: &HashMap<u16, Vec<Operand>>) -> Private {
    let Some([size, offset]) = top.get(&18).and_then(|v| v.get(..2)) else {
        return Private::default();
    };
    let (size, offset) = (size.i().max(0) as usize, offset.i().max(0) as usize);
    let Some(pdata) = data.get(offset..offset.saturating_add(size).min(data.len())) else {
        return Private::default();
    };
    let dict = parse_dict(pdata);
    let num = |k: u16| dict.get(&k).and_then(|v| v.first()).map_or(0.0, Operand::f);
    let subrs = dict
        .get(&19)
        .and_then(|v| v.first())
        .and_then(|o| Index::read(data, offset + o.i().max(0) as usize))
        .map(|(i, _)| i)
        .unwrap_or_default();
    Private {
        subrs,
        default_width: num(20),
        nominal_width: num(21),
    }
}

fn matrix(dict: &HashMap<u16, Vec<Operand>>) -> Option<[f64; 6]> {
    let v = dict.get(&1207)?;
    (v.len() == 6).then(|| [v[0].f(), v[1].f(), v[2].f(), v[3].f(), v[4].f(), v[5].f()])
}

#[derive(Debug)]
pub struct Cff {
    data: Vec<u8>,
    strings: Index,
    global_subrs: Index,
    charstrings: Index,
    private: Private,
    font_matrix: [f64; 6],
    /// For CID fonts: the font dict of each glyph, and each font dict's
    /// private data and matrix.
    fd_select: Vec<u8>,
    fds: Vec<(Private, Option<[f64; 6]>)>,
    is_cid: bool,
    /// Glyph index to SID (name-keyed) or CID (CID-keyed).
    charset: Vec<u16>,
    names: HashMap<String, u32>,
    cids: HashMap<u16, u32>,
    /// Built-in encoding: code to glyph, when the font has a custom one.
    encoding: Option<Box<[u32; 256]>>,
    sfnt: Option<Box<TrueType>>,
}

impl Cff {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let err = FontError("damaged CFF font");
        let hdr_size = usize::from(*data.get(2).ok_or(err.clone())?);
        let (_names, p) = Index::read(data, hdr_size).ok_or(err.clone())?;
        let (tops, p) = Index::read(data, p).ok_or(err.clone())?;
        let (strings, p) = Index::read(data, p).ok_or(err.clone())?;
        let (global_subrs, _) = Index::read(data, p).ok_or(err.clone())?;
        let top = parse_dict(tops.get(data, 0).ok_or(err.clone())?);
        let cs_off = top
            .get(&17)
            .and_then(|v| v.first())
            .ok_or(FontError("CFF font has no charstrings"))?
            .i()
            .max(0) as usize;
        let (charstrings, _) = Index::read(data, cs_off).ok_or(err.clone())?;
        let n = charstrings.len();
        if n == 0 {
            return Err(FontError("CFF font has no glyphs"));
        }
        let is_cid = top.contains_key(&1230);
        let mut fds = Vec::new();
        let mut fd_select = Vec::new();
        if is_cid {
            if let Some(off) = top.get(&1236).and_then(|v| v.first()) {
                if let Some((fd_index, _)) = Index::read(data, off.i().max(0) as usize) {
                    for i in 0..fd_index.len() {
                        let d = fd_index.get(data, i).map(parse_dict).unwrap_or_default();
                        fds.push((read_private(data, &d), matrix(&d)));
                    }
                }
            }
            if let Some(off) = top.get(&1237).and_then(|v| v.first()) {
                fd_select = read_fd_select(data, off.i().max(0) as usize, n);
            }
        }
        let charset_off = top.get(&15).and_then(|v| v.first()).map_or(0, Operand::i);
        let charset = read_charset(data, charset_off, n, is_cid);
        let mut font = Self {
            data: data.to_vec(),
            strings,
            global_subrs,
            charstrings,
            private: read_private(data, &top),
            font_matrix: matrix(&top).unwrap_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]),
            fd_select,
            fds,
            is_cid,
            charset,
            names: HashMap::new(),
            cids: HashMap::new(),
            encoding: None,
            sfnt: None,
        };
        if is_cid {
            for (gid, &cid) in font.charset.iter().enumerate() {
                font.cids.entry(cid).or_insert(gid as u32);
            }
        } else {
            for (gid, &sid) in font.charset.iter().enumerate() {
                if let Some(name) = font.sid_name(sid) {
                    font.names.entry(name.to_string()).or_insert(gid as u32);
                }
            }
            let enc_off = top.get(&16).and_then(|v| v.first()).map_or(0, Operand::i);
            if enc_off > 1 {
                font.encoding = font.read_encoding(enc_off as usize);
            }
        }
        Ok(font)
    }

    /// Keeps the OpenType wrapper for its cmap and metrics.
    pub(crate) fn attach_sfnt(&mut self, tt: TrueType) {
        self.sfnt = Some(Box::new(tt));
    }

    pub fn glyph_count(&self) -> usize {
        self.charstrings.len()
    }

    pub fn is_cid(&self) -> bool {
        self.is_cid
    }

    fn sid_name(&self, sid: u16) -> Option<&str> {
        let sid = usize::from(sid);
        if sid < STANDARD_STRINGS.len() {
            return Some(STANDARD_STRINGS[sid]);
        }
        let s = self.strings.get(&self.data, sid - STANDARD_STRINGS.len())?;
        std::str::from_utf8(s).ok()
    }

    fn read_encoding(&self, off: usize) -> Option<Box<[u32; 256]>> {
        let mut map = Box::new([0u32; 256]);
        let mut r = Reader::at(&self.data, off);
        let format = r.u8()?;
        match format & 0x7F {
            0 => {
                let n = r.u8()?;
                for gid in 1..=u32::from(n) {
                    map[usize::from(r.u8()?)] = gid;
                }
            }
            1 => {
                let ranges = r.u8()?;
                let mut gid = 1u32;
                for _ in 0..ranges {
                    let (first, left) = (r.u8()?, r.u8()?);
                    for c in u32::from(first)..=u32::from(first) + u32::from(left) {
                        if c < 256 {
                            map[c as usize] = gid;
                        }
                        gid += 1;
                    }
                }
            }
            _ => return None,
        }
        if format & 0x80 != 0 {
            let sups = r.u8()?;
            for _ in 0..sups {
                let (code, sid) = (r.u8()?, r.u16()?);
                if let Some(gid) = self.charset.iter().position(|&s| s == sid) {
                    map[usize::from(code)] = gid as u32;
                }
            }
        }
        Some(map)
    }

    pub fn glyph_by_name(&self, name: &str) -> Option<u32> {
        if let Some(&g) = self.names.get(name) {
            return Some(g);
        }
        let tt = self.sfnt.as_ref()?;
        tt.glyph_by_name(name)
    }

    pub fn glyph_by_cid(&self, cid: u32) -> Option<u32> {
        if !self.is_cid {
            return Some(cid);
        }
        u16::try_from(cid)
            .ok()
            .and_then(|c| self.cids.get(&c).copied())
    }

    pub fn glyph_by_unicode(&self, c: char) -> Option<u32> {
        if let Some(g) = self.sfnt.as_ref().and_then(|t| t.glyph_by_unicode(c)) {
            return Some(g);
        }
        crate::encoding::unicode_to_glyph_name_candidates(c)
            .into_iter()
            .find_map(|n| self.names.get(&n).copied())
    }

    pub fn builtin_encoding(&self, code: u8) -> Option<&str> {
        if self.is_cid {
            return None;
        }
        match &self.encoding {
            Some(map) => {
                let gid = map[usize::from(code)];
                if gid == 0 {
                    return None;
                }
                self.sid_name(*self.charset.get(gid as usize)?)
            }
            None => Encoding::Standard.glyph_name(code),
        }
    }

    pub fn glyph_by_code(&self, code: u8) -> Option<u32> {
        if let Some(map) = &self.encoding {
            let g = map[usize::from(code)];
            return (g != 0).then_some(g);
        }
        self.builtin_encoding(code)
            .and_then(|n| self.glyph_by_name(n))
    }

    fn private_for(&self, gid: u32) -> (&Private, [f64; 6]) {
        if self.is_cid {
            let fd = self.fd_select.get(gid as usize).copied().unwrap_or(0);
            if let Some((p, m)) = self.fds.get(usize::from(fd)) {
                return (p, m.unwrap_or(self.font_matrix));
            }
        }
        (&self.private, self.font_matrix)
    }

    pub fn advance(&self, gid: u32) -> Option<f64> {
        if let Some(a) = self.sfnt.as_ref().and_then(|t| t.advance(gid)) {
            return Some(a);
        }
        let (_, m) = self.private_for(gid);
        let (_, width) = self.run(gid)?;
        Some(width * m[0])
    }

    pub fn outline(&self, gid: u32) -> Option<Path> {
        let (_, m) = self.private_for(gid);
        let (path, _) = self.run(gid)?;
        let t = atelier_raster::Transform::from_array(m);
        Some(path.transform(&t))
    }

    /// Runs a glyph's charstring; returns its outline in glyph units and
    /// its advance width.
    fn run(&self, gid: u32) -> Option<(Path, f64)> {
        let cs = self.charstrings.get(&self.data, gid as usize)?;
        let (private, _) = self.private_for(gid);
        let mut st = T2State {
            font: self,
            private,
            builder: PathBuilder::new(),
            stack: Vec::with_capacity(48),
            pos: Point::default(),
            open: false,
            stems: 0,
            width: None,
            ops: 0,
            done: false,
            seac_depth: 0,
        };
        st.exec(cs, 0);
        if st.open {
            st.builder.close();
        }
        let width = st.width.unwrap_or(private.default_width);
        Some((st.builder.finish(), width))
    }
}

fn read_fd_select(data: &[u8], off: usize, n: usize) -> Vec<u8> {
    let mut r = Reader::at(data, off);
    let mut out = vec![0u8; n];
    match r.u8() {
        Some(0) => {
            for v in out.iter_mut() {
                *v = r.u8().unwrap_or(0);
            }
        }
        Some(3) => {
            let ranges = usize::from(r.u16().unwrap_or(0));
            let mut prev: Option<(usize, u8)> = None;
            for _ in 0..=ranges {
                let Some(first) = r.u16().map(usize::from) else {
                    break;
                };
                if let Some((start, fd)) = prev {
                    for v in out.iter_mut().take(first.min(n)).skip(start) {
                        *v = fd;
                    }
                }
                match r.u8() {
                    Some(fd) => prev = Some((first, fd)),
                    None => break,
                }
            }
        }
        _ => {}
    }
    out
}

fn read_charset(data: &[u8], off: i64, n: usize, is_cid: bool) -> Vec<u16> {
    let predefined: Option<&[u16]> = match off {
        0 if !is_cid => return (0..n as u16).collect(),
        1 => Some(EXPERT_CHARSET),
        2 => Some(EXPERT_SUBSET_CHARSET),
        0 => return (0..n as u16).collect(),
        _ => None,
    };
    let mut out = vec![0u16];
    if let Some(p) = predefined {
        out.extend(p.iter().take(n - 1));
        out.resize(n, 0);
        return out;
    }
    let mut r = Reader::at(data, off as usize);
    match r.u8() {
        Some(0) => {
            while out.len() < n {
                match r.u16() {
                    Some(s) => out.push(s),
                    None => break,
                }
            }
        }
        Some(f @ (1 | 2)) => {
            while out.len() < n {
                let Some(first) = r.u16() else { break };
                let left = if f == 1 {
                    r.u8().map(u16::from)
                } else {
                    r.u16()
                };
                let Some(left) = left else { break };
                for k in 0..=left {
                    if out.len() >= n {
                        break;
                    }
                    out.push(first.wrapping_add(k));
                }
            }
        }
        _ => {}
    }
    out.resize(n, 0);
    out
}

struct T2State<'a> {
    font: &'a Cff,
    private: &'a Private,
    builder: PathBuilder,
    stack: Vec<f64>,
    pos: Point,
    open: bool,
    stems: usize,
    width: Option<f64>,
    ops: usize,
    done: bool,
    seac_depth: usize,
}

fn bias(n: usize) -> i64 {
    if n < 1240 {
        107
    } else if n < 33900 {
        1131
    } else {
        32768
    }
}

impl T2State<'_> {
    /// Takes the width from the first stack-clearing operator, which has
    /// one more argument than it needs when a width is present.
    fn take_width(&mut self, expected_even: bool, expected: usize) {
        if self.width.is_some() {
            return;
        }
        let extra = if expected_even {
            self.stack.len() % 2 == 1
        } else {
            self.stack.len() > expected
        };
        if extra && !self.stack.is_empty() {
            let w = self.stack.remove(0);
            self.width = Some(self.private.nominal_width + w);
        } else {
            self.width = Some(self.private.default_width);
        }
    }

    fn move_to(&mut self, dx: f64, dy: f64) {
        if self.open {
            self.builder.close();
        }
        self.pos = Point::new(self.pos.x + dx, self.pos.y + dy);
        self.builder.move_to(self.pos);
        self.open = true;
    }

    fn line_to(&mut self, dx: f64, dy: f64) {
        self.pos = Point::new(self.pos.x + dx, self.pos.y + dy);
        self.builder.line_to(self.pos);
    }

    fn curve_to(&mut self, d: [f64; 6]) {
        let c1 = Point::new(self.pos.x + d[0], self.pos.y + d[1]);
        let c2 = Point::new(c1.x + d[2], c1.y + d[3]);
        let p = Point::new(c2.x + d[4], c2.y + d[5]);
        self.builder.cubic_to(c1, c2, p);
        self.pos = p;
    }

    fn exec(&mut self, cs: &[u8], depth: usize) {
        if depth > MAX_SUBR_DEPTH {
            return;
        }
        let mut r = Reader::new(cs);
        while !self.done {
            let Some(b) = r.u8() else { return };
            self.ops += 1;
            if self.ops > MAX_OPS {
                self.done = true;
                return;
            }
            if self.stack.len() > MAX_STACK {
                self.stack.clear();
            }
            match b {
                28 => {
                    let v = r.i16().unwrap_or(0);
                    self.stack.push(f64::from(v));
                }
                32..=246 => self.stack.push(f64::from(i32::from(b) - 139)),
                247..=250 => {
                    let b1 = i32::from(r.u8().unwrap_or(0));
                    self.stack
                        .push(f64::from((i32::from(b) - 247) * 256 + b1 + 108));
                }
                251..=254 => {
                    let b1 = i32::from(r.u8().unwrap_or(0));
                    self.stack
                        .push(f64::from(-(i32::from(b) - 251) * 256 - b1 - 108));
                }
                255 => {
                    let v = r.u32().unwrap_or(0) as i32;
                    self.stack.push(f64::from(v) / 65536.0);
                }
                // hstem, vstem, hstemhm, vstemhm
                1 | 3 | 18 | 23 => {
                    self.take_width(true, 0);
                    self.stems += self.stack.len() / 2;
                    self.stack.clear();
                }
                // hintmask, cntrmask
                19 | 20 => {
                    self.take_width(true, 0);
                    self.stems += self.stack.len() / 2;
                    self.stack.clear();
                    r.skip(self.stems.div_ceil(8));
                }
                21 => {
                    self.take_width(false, 2);
                    let (dx, dy) = (self.arg(0), self.arg(1));
                    self.move_to(dx, dy);
                    self.stack.clear();
                }
                22 => {
                    self.take_width(false, 1);
                    let dx = self.arg(0);
                    self.move_to(dx, 0.0);
                    self.stack.clear();
                }
                4 => {
                    self.take_width(false, 1);
                    let dy = self.arg(0);
                    self.move_to(0.0, dy);
                    self.stack.clear();
                }
                5 => {
                    let s = std::mem::take(&mut self.stack);
                    for p in s.chunks_exact(2) {
                        self.line_to(p[0], p[1]);
                    }
                }
                6 | 7 => {
                    let s = std::mem::take(&mut self.stack);
                    let mut horizontal = b == 6;
                    for &d in &s {
                        if horizontal {
                            self.line_to(d, 0.0);
                        } else {
                            self.line_to(0.0, d);
                        }
                        horizontal = !horizontal;
                    }
                }
                8 => {
                    let s = std::mem::take(&mut self.stack);
                    for c in s.chunks_exact(6) {
                        self.curve_to([c[0], c[1], c[2], c[3], c[4], c[5]]);
                    }
                }
                24 => {
                    // rcurveline
                    let s = std::mem::take(&mut self.stack);
                    let curves = s.len().saturating_sub(2) / 6;
                    for c in s.chunks_exact(6).take(curves) {
                        self.curve_to([c[0], c[1], c[2], c[3], c[4], c[5]]);
                    }
                    if s.len() >= curves * 6 + 2 {
                        self.line_to(s[curves * 6], s[curves * 6 + 1]);
                    }
                }
                25 => {
                    // rlinecurve
                    let s = std::mem::take(&mut self.stack);
                    if s.len() >= 6 {
                        let lines = (s.len() - 6) / 2;
                        for p in s.chunks_exact(2).take(lines) {
                            self.line_to(p[0], p[1]);
                        }
                        let c = &s[lines * 2..];
                        self.curve_to([c[0], c[1], c[2], c[3], c[4], c[5]]);
                    }
                }
                26 => {
                    // vvcurveto
                    let mut s = std::mem::take(&mut self.stack);
                    let mut dx1 = 0.0;
                    if s.len() % 4 == 1 {
                        dx1 = s.remove(0);
                    }
                    for c in s.chunks_exact(4) {
                        self.curve_to([dx1, c[0], c[1], c[2], 0.0, c[3]]);
                        dx1 = 0.0;
                    }
                }
                27 => {
                    // hhcurveto
                    let mut s = std::mem::take(&mut self.stack);
                    let mut dy1 = 0.0;
                    if s.len() % 4 == 1 {
                        dy1 = s.remove(0);
                    }
                    for c in s.chunks_exact(4) {
                        self.curve_to([c[0], dy1, c[1], c[2], c[3], 0.0]);
                        dy1 = 0.0;
                    }
                }
                30 | 31 => {
                    // vhcurveto, hvcurveto
                    let s = std::mem::take(&mut self.stack);
                    let mut horizontal = b == 31;
                    let mut i = 0;
                    while i + 4 <= s.len() {
                        let last = i + 4 == s.len() - 1;
                        let extra = if last { s[i + 4] } else { 0.0 };
                        if horizontal {
                            self.curve_to([s[i], 0.0, s[i + 1], s[i + 2], extra, s[i + 3]]);
                        } else {
                            self.curve_to([0.0, s[i], s[i + 1], s[i + 2], s[i + 3], extra]);
                        }
                        horizontal = !horizontal;
                        i += if last { 5 } else { 4 };
                    }
                }
                10 | 29 => {
                    let Some(n) = self.stack.pop() else { return };
                    let (subrs, data) = if b == 10 {
                        (&self.private.subrs, &self.font.data)
                    } else {
                        (&self.font.global_subrs, &self.font.data)
                    };
                    let i = n as i64 + bias(subrs.len());
                    if let Some(sub) = usize::try_from(i).ok().and_then(|i| subrs.get(data, i)) {
                        self.exec(sub, depth + 1);
                    }
                }
                11 => return,
                14 => {
                    // endchar, possibly with seac-style accent arguments.
                    self.take_width(false, if self.stack.len() >= 4 { 4 } else { 0 });
                    if self.stack.len() >= 4 {
                        let s = std::mem::take(&mut self.stack);
                        self.seac(s[0], s[1], s[2] as u8, s[3] as u8);
                    }
                    if self.open {
                        self.builder.close();
                        self.open = false;
                    }
                    self.done = true;
                    return;
                }
                12 => {
                    let op = r.u8().unwrap_or(0);
                    self.escape(op);
                }
                _ => self.stack.clear(),
            }
        }
    }

    fn arg(&self, i: usize) -> f64 {
        self.stack.get(i).copied().unwrap_or(0.0)
    }

    fn seac(&mut self, adx: f64, ady: f64, bchar: u8, achar: u8) {
        if self.seac_depth > 0 {
            return;
        }
        let font = self.font;
        let glyph = |code: u8| {
            Encoding::Standard
                .glyph_name(code)
                .and_then(|n| font.glyph_by_name(n))
        };
        let (Some(base), Some(accent)) = (glyph(bchar), glyph(achar)) else {
            return;
        };
        for (gid, offset) in [(base, Point::default()), (accent, Point::new(adx, ady))] {
            let Some(cs) = font.charstrings.get(&font.data, gid as usize) else {
                continue;
            };
            let mut sub = T2State {
                font,
                private: self.private,
                builder: PathBuilder::new(),
                stack: Vec::new(),
                pos: offset,
                open: false,
                stems: 0,
                width: Some(0.0),
                ops: self.ops,
                done: false,
                seac_depth: 1,
            };
            sub.exec(cs, 0);
            if sub.open {
                sub.builder.close();
            }
            self.ops = sub.ops;
            self.builder.append(&sub.builder.finish());
        }
    }

    fn escape(&mut self, op: u8) {
        let s = std::mem::take(&mut self.stack);
        let a = |i: usize| s.get(i).copied().unwrap_or(0.0);
        match op {
            35 if s.len() >= 12 => {
                // flex
                self.curve_to([a(0), a(1), a(2), a(3), a(4), a(5)]);
                self.curve_to([a(6), a(7), a(8), a(9), a(10), a(11)]);
            }
            34 if s.len() >= 7 => {
                // hflex
                self.curve_to([a(0), 0.0, a(1), a(2), a(3), 0.0]);
                self.curve_to([a(4), 0.0, a(5), -a(2), a(6), 0.0]);
            }
            36 if s.len() >= 9 => {
                // hflex1: the second curve returns to the starting y.
                self.curve_to([a(0), a(1), a(2), a(3), a(4), 0.0]);
                self.curve_to([a(5), 0.0, a(6), a(7), a(8), -(a(1) + a(3) + a(7))]);
            }
            37 if s.len() >= 11 => {
                // flex1
                let start = self.pos;
                let dx: f64 = a(0) + a(2) + a(4) + a(6) + a(8);
                let dy: f64 = a(1) + a(3) + a(5) + a(7) + a(9);
                self.curve_to([a(0), a(1), a(2), a(3), a(4), a(5)]);
                let mid = self.pos;
                let (last_x, last_y) = if dx.abs() > dy.abs() {
                    (a(10), start.y - (mid.y + a(7) + a(9)))
                } else {
                    (start.x - (mid.x + a(6) + a(8)), a(10))
                };
                self.curve_to([a(6), a(7), a(8), a(9), last_x, last_y]);
            }
            // Arithmetic, kept for the rare fonts that use it.
            9 => self.stack.push(a(0).abs()),
            10 => self.stack.push(a(0) + a(1)),
            11 => self.stack.push(a(0) - a(1)),
            12 => self.stack.push(if a(1) == 0.0 { 0.0 } else { a(0) / a(1) }),
            14 => self.stack.push(-a(0)),
            18 => self.stack.extend(s.iter().take(s.len().saturating_sub(1))),
            24 => self.stack.push(a(0) * a(1)),
            27 => {
                self.stack.extend(&s);
                if let Some(&last) = s.last() {
                    self.stack.push(last);
                }
            }
            28 => {
                self.stack.extend(&s);
                let n = self.stack.len();
                if n >= 2 {
                    self.stack.swap(n - 1, n - 2);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reals() {
        let mut r = Reader::new(&[0x1E, 0xE2, 0xA2, 0x5F]);
        r.skip(1);
        assert!((read_real(&mut r) + 2.25).abs() < 1e-9);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(Cff::parse(&[1, 0, 4, 1, 0, 0, 0, 0]).is_err());
        assert!(Cff::parse(b"").is_err());
    }
}
