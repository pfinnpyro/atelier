//! TrueType and OpenType (sfnt) fonts: glyf outlines, cmap, hmtx and post.

use std::collections::HashMap;

use atelier_raster::{Path, PathBuilder, Point};

use crate::data::mac_glyphs::MAC_GLYPH_NAMES;
use crate::encoding::glyph_name_to_unicode;
use crate::reader::Reader;
use crate::{FontError, Result};

/// Composite glyphs nested deeper than this are cut off.
const MAX_COMPONENT_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy)]
struct Table {
    offset: usize,
    len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CmapId {
    platform: u16,
    encoding: u16,
}

#[derive(Debug)]
pub struct TrueType {
    data: Vec<u8>,
    tables: HashMap<[u8; 4], Table>,
    units_per_em: f64,
    long_loca: bool,
    glyph_count: usize,
    h_metrics: usize,
    cmaps: Vec<(CmapId, usize)>,
    names: HashMap<String, u32>,
}

impl TrueType {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        if data.starts_with(b"ttcf") {
            // A collection: use its first font.
            r.skip(12);
            let base = r.u32().ok_or(FontError("truncated font collection"))? as usize;
            r = Reader::at(data, base);
        }
        let _version = r.u32().ok_or(FontError("not an sfnt font"))?;
        let count = r.u16().ok_or(FontError("not an sfnt font"))?;
        r.skip(6);
        let mut tables = HashMap::new();
        for _ in 0..count {
            let (Some(tag), Some(_sum), Some(offset), Some(len)) =
                (r.bytes(4), r.u32(), r.u32(), r.u32())
            else {
                break;
            };
            let (offset, len) = (offset as usize, len as usize);
            if offset >= data.len() {
                continue;
            }
            // Clamp tables that run past the end of the file.
            let len = len.min(data.len() - offset);
            tables.insert([tag[0], tag[1], tag[2], tag[3]], Table { offset, len });
        }
        if tables.is_empty() {
            return Err(FontError("sfnt font has no tables"));
        }
        let mut font = Self {
            data: data.to_vec(),
            tables,
            units_per_em: 1000.0,
            long_loca: false,
            glyph_count: 0,
            h_metrics: 0,
            cmaps: Vec::new(),
            names: HashMap::new(),
        };
        if let Some(head) = font.table(b"head") {
            let upem = Reader::at(head, 18).u16().unwrap_or(1000);
            let long_loca = Reader::at(head, 50).i16().unwrap_or(0) == 1;
            if (16..=16384).contains(&upem) {
                font.units_per_em = f64::from(upem);
            }
            font.long_loca = long_loca;
        }
        let loca_glyphs = font.table(b"loca").map(|l| {
            let per = if font.long_loca { 4 } else { 2 };
            (l.len() / per).saturating_sub(1)
        });
        let maxp_glyphs = font
            .table(b"maxp")
            .and_then(|m| Reader::at(m, 4).u16())
            .map(usize::from);
        font.glyph_count = match (maxp_glyphs, loca_glyphs) {
            (Some(m), Some(l)) => m.min(l),
            (Some(m), None) => m,
            (None, Some(l)) => l,
            (None, None) => 0,
        };
        font.h_metrics = font
            .table(b"hhea")
            .and_then(|h| Reader::at(h, 34).u16())
            .map_or(0, usize::from);
        font.read_cmaps();
        font.read_post();
        Ok(font)
    }

    fn table(&self, tag: &[u8; 4]) -> Option<&[u8]> {
        let t = self.tables.get(tag)?;
        self.data.get(t.offset..t.offset + t.len)
    }

    /// The CFF table of an OpenType font with PostScript outlines.
    pub fn cff_table(&self) -> Option<&[u8]> {
        self.table(b"CFF ")
    }

    pub fn units_per_em(&self) -> f64 {
        self.units_per_em
    }

    pub fn glyph_count(&self) -> usize {
        self.glyph_count
    }

    fn read_cmaps(&mut self) {
        let Some(cmap) = self.table(b"cmap") else {
            return;
        };
        let mut r = Reader::at(cmap, 2);
        let n = r.u16().unwrap_or(0);
        let mut out = Vec::new();
        for _ in 0..n {
            let (Some(platform), Some(encoding), Some(offset)) = (r.u16(), r.u16(), r.u32()) else {
                break;
            };
            if (offset as usize) < cmap.len() {
                out.push((CmapId { platform, encoding }, offset as usize));
            }
        }
        self.cmaps = out;
    }

    fn read_post(&mut self) {
        let Some(post) = self.table(b"post") else {
            return;
        };
        let mut r = Reader::new(post);
        let version = r.u32().unwrap_or(0);
        let mut names = HashMap::new();
        match version {
            0x0001_0000 => {
                for (i, n) in MAC_GLYPH_NAMES.iter().enumerate().take(self.glyph_count) {
                    names.insert(n.to_string(), i as u32);
                }
            }
            0x0002_0000 => {
                let mut r = Reader::at(post, 32);
                let count = usize::from(r.u16().unwrap_or(0));
                let indices: Vec<u16> = (0..count).map_while(|_| r.u16()).collect();
                let mut pascal = Vec::new();
                while let Some(len) = r.u8() {
                    match r.bytes(usize::from(len)) {
                        Some(s) => pascal.push(String::from_utf8_lossy(s).into_owned()),
                        None => break,
                    }
                }
                for (gid, &idx) in indices.iter().enumerate() {
                    let idx = usize::from(idx);
                    let name = if idx < 258 {
                        Some(MAC_GLYPH_NAMES[idx].to_string())
                    } else {
                        pascal.get(idx - 258).cloned()
                    };
                    if let Some(name) = name {
                        names.entry(name).or_insert(gid as u32);
                    }
                }
            }
            _ => {}
        }
        self.names = names;
    }

    fn cmap_lookup(&self, id: CmapId, code: u32) -> Option<u32> {
        let cmap = self.table(b"cmap")?;
        let &(_, offset) = self.cmaps.iter().find(|(i, _)| *i == id)?;
        let gid = lookup_subtable(cmap, offset, code)?;
        (gid != 0 && (gid as usize) < self.glyph_count.max(1)).then_some(gid)
    }

    fn has_cmap(&self, platform: u16, encoding: u16) -> bool {
        self.cmaps
            .iter()
            .any(|(i, _)| i.platform == platform && i.encoding == encoding)
    }

    /// Whether the font has a Microsoft Unicode cmap.
    pub fn has_unicode_cmap(&self) -> bool {
        self.has_cmap(3, 1)
            || self.has_cmap(3, 10)
            || self.cmaps.iter().any(|(i, _)| i.platform == 0)
    }

    /// Whether the font has a (3,0) symbol cmap.
    pub fn has_symbol_cmap(&self) -> bool {
        self.has_cmap(3, 0)
    }

    pub fn glyph_by_unicode(&self, c: char) -> Option<u32> {
        let code = c as u32;
        let ids = [
            CmapId {
                platform: 3,
                encoding: 10,
            },
            CmapId {
                platform: 3,
                encoding: 1,
            },
            CmapId {
                platform: 0,
                encoding: 4,
            },
            CmapId {
                platform: 0,
                encoding: 3,
            },
            CmapId {
                platform: 0,
                encoding: 6,
            },
            CmapId {
                platform: 0,
                encoding: 1,
            },
            CmapId {
                platform: 0,
                encoding: 0,
            },
        ];
        ids.iter().find_map(|&id| self.cmap_lookup(id, code))
    }

    /// The glyph for a single-byte code through the (3,0) symbol cmap, which
    /// may map codes at 0xF000, 0xF100 or 0xF200 + code, or the (1,0) Mac cmap.
    pub fn glyph_by_symbol_code(&self, code: u8) -> Option<u32> {
        let c = u32::from(code);
        let sym = CmapId {
            platform: 3,
            encoding: 0,
        };
        [c, 0xF000 + c, 0xF100 + c, 0xF200 + c]
            .iter()
            .find_map(|&v| self.cmap_lookup(sym, v))
            .or_else(|| {
                self.cmap_lookup(
                    CmapId {
                        platform: 1,
                        encoding: 0,
                    },
                    c,
                )
            })
    }

    /// The glyph for a Mac Roman code through the (1,0) cmap.
    pub fn glyph_by_mac_code(&self, code: u8) -> Option<u32> {
        self.cmap_lookup(
            CmapId {
                platform: 1,
                encoding: 0,
            },
            u32::from(code),
        )
    }

    pub fn glyph_by_name(&self, name: &str) -> Option<u32> {
        if let Some(&g) = self.names.get(name) {
            return Some(g);
        }
        let text = glyph_name_to_unicode(name)?;
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => self.glyph_by_unicode(c),
            _ => None,
        }
    }

    pub fn advance(&self, gid: u32) -> Option<f64> {
        let hmtx = self.table(b"hmtx")?;
        if self.h_metrics == 0 {
            return None;
        }
        let i = (gid as usize).min(self.h_metrics - 1);
        let adv = Reader::at(hmtx, i * 4).u16()?;
        Some(f64::from(adv) / self.units_per_em)
    }

    fn glyph_data(&self, gid: u32) -> Option<&[u8]> {
        if gid as usize >= self.glyph_count {
            return None;
        }
        let loca = self.table(b"loca")?;
        let glyf = self.table(b"glyf")?;
        let (start, end) = if self.long_loca {
            let mut r = Reader::at(loca, gid as usize * 4);
            (r.u32()? as usize, r.u32()? as usize)
        } else {
            let mut r = Reader::at(loca, gid as usize * 2);
            (usize::from(r.u16()?) * 2, usize::from(r.u16()?) * 2)
        };
        if end <= start {
            return None;
        }
        glyf.get(start..end.min(glyf.len()))
    }

    /// The glyph outline in text space (1.0 = one em).
    pub fn outline(&self, gid: u32) -> Option<Path> {
        let mut b = PathBuilder::new();
        let scale = 1.0 / self.units_per_em;
        self.append_glyph(gid, &[scale, 0.0, 0.0, scale, 0.0, 0.0], &mut b, 0);
        Some(b.finish())
    }

    fn append_glyph(&self, gid: u32, m: &[f64; 6], b: &mut PathBuilder, depth: usize) {
        if depth > MAX_COMPONENT_DEPTH {
            return;
        }
        let Some(g) = self.glyph_data(gid) else {
            return;
        };
        let mut r = Reader::new(g);
        let contours = r.i16().unwrap_or(0);
        r.skip(8);
        if contours >= 0 {
            simple_glyph(&mut r, contours as usize, m, b);
        } else {
            self.composite_glyph(&mut r, m, b, depth);
        }
    }

    fn composite_glyph(&self, r: &mut Reader, m: &[f64; 6], b: &mut PathBuilder, depth: usize) {
        const WORDS: u16 = 0x0001;
        const XY_VALUES: u16 = 0x0002;
        const SCALE: u16 = 0x0008;
        const MORE: u16 = 0x0020;
        const XY_SCALE: u16 = 0x0040;
        const TWO_BY_TWO: u16 = 0x0080;
        loop {
            let (Some(flags), Some(gid)) = (r.u16(), r.u16()) else {
                return;
            };
            let (dx, dy) = if flags & WORDS != 0 {
                (r.i16().map(f64::from), r.i16().map(f64::from))
            } else {
                (r.i8().map(f64::from), r.i8().map(f64::from))
            };
            let (Some(mut dx), Some(mut dy)) = (dx, dy) else {
                return;
            };
            if flags & XY_VALUES == 0 {
                // Point matching is rare; ignore the offset.
                dx = 0.0;
                dy = 0.0;
            }
            let f2 = |r: &mut Reader| r.i16().map(|v| f64::from(v) / 16384.0);
            let (a, bb, c, d) = if flags & SCALE != 0 {
                let s = f2(r).unwrap_or(1.0);
                (s, 0.0, 0.0, s)
            } else if flags & XY_SCALE != 0 {
                (f2(r).unwrap_or(1.0), 0.0, 0.0, f2(r).unwrap_or(1.0))
            } else if flags & TWO_BY_TWO != 0 {
                (
                    f2(r).unwrap_or(1.0),
                    f2(r).unwrap_or(0.0),
                    f2(r).unwrap_or(0.0),
                    f2(r).unwrap_or(1.0),
                )
            } else {
                (1.0, 0.0, 0.0, 1.0)
            };
            // Component matrix, then the parent's.
            let cm = [
                a * m[0] + bb * m[2],
                a * m[1] + bb * m[3],
                c * m[0] + d * m[2],
                c * m[1] + d * m[3],
                dx * m[0] + dy * m[2] + m[4],
                dx * m[1] + dy * m[3] + m[5],
            ];
            self.append_glyph(u32::from(gid), &cm, b, depth + 1);
            if flags & MORE == 0 {
                return;
            }
        }
    }
}

fn simple_glyph(r: &mut Reader, contours: usize, m: &[f64; 6], b: &mut PathBuilder) {
    let ends: Vec<usize> = (0..contours)
        .map_while(|_| r.u16().map(usize::from))
        .collect();
    let Some(&last) = ends.last() else {
        return;
    };
    let n = last + 1;
    let Some(ins) = r.u16() else { return };
    r.skip(usize::from(ins));
    let mut flags = Vec::with_capacity(n);
    while flags.len() < n {
        let Some(f) = r.u8() else { return };
        flags.push(f);
        if f & 8 != 0 {
            let repeat = r.u8().unwrap_or(0);
            for _ in 0..repeat {
                if flags.len() < n {
                    flags.push(f);
                }
            }
        }
    }
    let mut coords = |short: u8, same: u8| -> Option<Vec<i32>> {
        let mut v = 0i32;
        let mut out = Vec::with_capacity(n);
        for &f in &flags {
            if f & short != 0 {
                let d = i32::from(r.u8()?);
                v += if f & same != 0 { d } else { -d };
            } else if f & same == 0 {
                v += i32::from(r.i16()?);
            }
            out.push(v);
        }
        Some(out)
    };
    let Some(xs) = coords(2, 16) else { return };
    let Some(ys) = coords(4, 32) else { return };
    let pt = |i: usize| {
        let (x, y) = (f64::from(xs[i]), f64::from(ys[i]));
        Point::new(m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    };
    let mut start = 0;
    for &end in &ends {
        if end < start || end >= n {
            break;
        }
        emit_contour(
            &(start..=end)
                .map(|i| (pt(i), flags[i] & 1 != 0))
                .collect::<Vec<_>>(),
            b,
        );
        start = end + 1;
    }
}

/// Emits one quadratic contour as lines and cubics.
fn emit_contour(pts: &[(Point, bool)], b: &mut PathBuilder) {
    let n = pts.len();
    if n == 0 {
        return;
    }
    // Start at an on-curve point, or else at the midpoint of the first two
    // off-curve points; then visit the rest in order.
    let (start, seq): (Point, Vec<(Point, bool)>) = match pts.iter().position(|p| p.1) {
        Some(i) => (pts[i].0, (1..n).map(|k| pts[(i + k) % n]).collect()),
        None => (
            pts[0].0.lerp(pts[1 % n].0, 0.5),
            (1..=n).map(|k| pts[k % n]).collect(),
        ),
    };
    b.move_to(start);
    let mut cur = start;
    let mut ctrl: Option<Point> = None;
    for (p, on) in seq {
        match (on, ctrl) {
            (true, None) => {
                b.line_to(p);
                cur = p;
            }
            (true, Some(c)) => {
                quad(b, cur, c, p);
                cur = p;
                ctrl = None;
            }
            (false, None) => ctrl = Some(p),
            (false, Some(c)) => {
                let mid = c.lerp(p, 0.5);
                quad(b, cur, c, mid);
                cur = mid;
                ctrl = Some(p);
            }
        }
    }
    match ctrl {
        Some(c) => quad(b, cur, c, start),
        None if cur != start => b.line_to(start),
        None => {}
    }
    b.close();
}

fn quad(b: &mut PathBuilder, p0: Point, c: Point, p: Point) {
    let c1 = p0.lerp(c, 2.0 / 3.0);
    let c2 = p.lerp(c, 2.0 / 3.0);
    b.cubic_to(c1, c2, p);
}

/// Looks a code up in one cmap subtable.
fn lookup_subtable(cmap: &[u8], offset: usize, code: u32) -> Option<u32> {
    let mut r = Reader::at(cmap, offset);
    match r.u16()? {
        0 => {
            r.skip(4);
            if code < 256 {
                r.skip(code as usize);
                return r.u8().map(u32::from);
            }
            None
        }
        4 => {
            r.skip(4);
            let seg_x2 = usize::from(r.u16()?);
            r.skip(6);
            let ends = offset + 14;
            let starts = ends + seg_x2 + 2;
            let deltas = starts + seg_x2;
            let range_offsets = deltas + seg_x2;
            let code = u16::try_from(code).ok()?;
            // Binary search on segment ends.
            let segs = seg_x2 / 2;
            let (mut lo, mut hi) = (0usize, segs);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let end = Reader::at(cmap, ends + mid * 2).u16()?;
                if end < code {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            if lo >= segs {
                return None;
            }
            let i = lo;
            let start = Reader::at(cmap, starts + i * 2).u16()?;
            if code < start {
                return None;
            }
            let delta = Reader::at(cmap, deltas + i * 2).u16()?;
            let ro = usize::from(Reader::at(cmap, range_offsets + i * 2).u16()?);
            let g = if ro == 0 {
                code.wrapping_add(delta)
            } else {
                let at = range_offsets + i * 2 + ro + usize::from(code - start) * 2;
                let g = Reader::at(cmap, at).u16()?;
                if g == 0 { 0 } else { g.wrapping_add(delta) }
            };
            Some(u32::from(g))
        }
        6 => {
            r.skip(4);
            let first = u32::from(r.u16()?);
            let count = u32::from(r.u16()?);
            if code < first || code - first >= count {
                return None;
            }
            r.skip(((code - first) * 2) as usize);
            r.u16().map(u32::from)
        }
        12 => {
            r.skip(10);
            let groups = r.u32()? as usize;
            let base = offset + 16;
            let (mut lo, mut hi) = (0usize, groups);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let mut g = Reader::at(cmap, base + mid * 12);
                let (s, e, gid) = (g.u32()?, g.u32()?, g.u32()?);
                if code < s {
                    hi = mid;
                } else if code > e {
                    lo = mid + 1;
                } else {
                    return Some(gid + (code - s));
                }
            }
            None
        }
        _ => None,
    }
}
