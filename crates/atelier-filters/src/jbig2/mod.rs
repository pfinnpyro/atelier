//! JBIG2Decode (ISO 32000-2, 7.4.7; ITU-T T.88): the embedded stream
//! organisation PDF uses, with optional global segments.
//!
//! Supported: generic regions (arithmetic and MMR, typical prediction),
//! generic refinement, symbol dictionaries (arithmetic and Huffman,
//! refinement/aggregate with arithmetic coding), text regions, pattern
//! dictionaries, halftone regions, custom Huffman tables, and striped
//! pages of unknown height.

mod arith;
mod halftone;
mod huffman;
mod region;
mod symbol;

use std::collections::HashMap;

use crate::{FilterError, Result};
use arith::{ArithDecoder, contexts};
use halftone::{Halftone, PatternDict, decode_halftone, decode_patterns};
use huffman::{BitReader, Table};
use region::{
    Bitmap, Combine, Generic, Refinement, decode_generic, decode_generic_mmr, decode_refinement,
};
use symbol::{Coder, Dict, DictTables, Text, TextTables, decode_dict, decode_text};

/// Pages and regions larger than this many pixels are refused: that is a
/// 600 dpi page of about 18 by 24 inches.
const MAX_PIXELS: usize = 1 << 27;

struct Segment<'a> {
    number: u32,
    kind: u8,
    referred: Vec<u32>,
    data: &'a [u8],
}

/// What earlier segments left for later ones to use.
enum Stored {
    Symbols(Vec<Bitmap>),
    Patterns(Vec<Bitmap>),
    Table(Table),
    Region(Bitmap),
}

#[derive(Clone, Copy)]
struct RegionInfo {
    width: usize,
    height: usize,
    x: i64,
    y: i64,
    combine: Combine,
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(i..i + 2)?.try_into().ok()?))
}

/// Splits the embedded organisation into segments: each header followed by
/// its data.
fn segments(data: &[u8]) -> std::result::Result<Vec<Segment<'_>>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    let bad = || "segment header runs past the data".to_string();
    while pos + 11 <= data.len() {
        let number = u32_at(data, pos).ok_or_else(bad)?;
        let flags = data[pos + 4];
        let kind = flags & 0x3f;
        let long_page = flags & 0x40 != 0;
        pos += 5;
        let first = *data.get(pos).ok_or_else(bad)?;
        let mut count = usize::from(first >> 5);
        if count == 7 {
            count = (u32_at(data, pos).ok_or_else(bad)? & 0x1fff_ffff) as usize;
            pos += 4 + (count + 8) / 8;
        } else {
            pos += 1;
        }
        if count > 1 << 16 {
            return Err("too many referred segments".into());
        }
        let ref_size = if number <= 256 {
            1
        } else if number <= 65536 {
            2
        } else {
            4
        };
        let mut referred = Vec::with_capacity(count);
        for _ in 0..count {
            let r = match ref_size {
                1 => u32::from(*data.get(pos).ok_or_else(bad)?),
                2 => u32::from(u16_at(data, pos).ok_or_else(bad)?),
                _ => u32_at(data, pos).ok_or_else(bad)?,
            };
            referred.push(r);
            pos += ref_size;
        }
        pos += if long_page { 4 } else { 1 };
        let length = u32_at(data, pos).ok_or_else(bad)?;
        pos += 4;
        let length = if length == u32::MAX {
            // Only an immediate generic region may leave its length unknown.
            unknown_length(data, pos, kind).ok_or("segment of unknown length has no end")?
        } else {
            length as usize
        };
        let end = pos.saturating_add(length).min(data.len());
        out.push(Segment {
            number,
            kind,
            referred,
            data: &data[pos..end],
        });
        pos = end;
        if kind == 51 {
            break;
        }
    }
    Ok(out)
}

/// Finds the end of an immediate generic region of unknown length (7.2.7):
/// its data ends with an end marker followed by a four-byte row count.
fn unknown_length(data: &[u8], start: usize, kind: u8) -> Option<usize> {
    if kind != 38 && kind != 39 {
        return None;
    }
    let flags = *data.get(start + 17)?;
    let mmr = flags & 1 != 0;
    let marker: [u8; 2] = if mmr { [0x00, 0x00] } else { [0xff, 0xac] };
    let body = start + 18;
    let mut i = body;
    while i + 6 <= data.len() {
        if data[i..i + 2] == marker {
            return Some(i + 6 - start);
        }
        i += 1;
    }
    None
}

fn region_info(d: &[u8]) -> Option<RegionInfo> {
    Some(RegionInfo {
        width: u32_at(d, 0)? as usize,
        height: u32_at(d, 4)? as usize,
        x: i64::from(u32_at(d, 8)?),
        y: i64::from(u32_at(d, 12)?),
        combine: Combine::from_bits(*d.get(16)? & 7),
    })
}

fn at_pixels(d: &[u8], n: usize) -> Option<Vec<(i64, i64)>> {
    (0..n)
        .map(|i| {
            Some((
                i64::from(*d.get(i * 2)? as i8),
                i64::from(*d.get(i * 2 + 1)? as i8),
            ))
        })
        .collect()
}

struct Decoder {
    stored: HashMap<u32, Stored>,
    page: Option<Bitmap>,
    /// The page height is unknown until the end of the last stripe.
    growing: bool,
    page_default: u8,
    warnings: Vec<String>,
}

impl Decoder {
    fn symbols_of(&self, referred: &[u32]) -> Vec<&Bitmap> {
        referred
            .iter()
            .filter_map(|r| match self.stored.get(r) {
                Some(Stored::Symbols(s)) => Some(s.iter()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn tables_of(&self, referred: &[u32]) -> Vec<&Table> {
        referred
            .iter()
            .filter_map(|r| match self.stored.get(r) {
                Some(Stored::Table(t)) => Some(t),
                _ => None,
            })
            .collect()
    }

    /// Whether a region is a sane size: within the pixel limit and not far
    /// larger than its page (damaged headers claim gigapixel regions).
    fn fits(&self, info: &RegionInfo) -> bool {
        let area = info.width.saturating_mul(info.height);
        let page_area = self.page.as_ref().map_or(MAX_PIXELS, |p| {
            if self.growing {
                MAX_PIXELS
            } else {
                p.width.saturating_mul(p.height)
            }
        });
        area <= MAX_PIXELS && area <= page_area.saturating_mul(4).saturating_add(1 << 20)
    }

    fn place(&mut self, bitmap: &Bitmap, info: RegionInfo) {
        let Some(page) = &mut self.page else { return };
        let bottom = (info.y as usize).saturating_add(bitmap.height);
        if self.growing && bottom > page.height && bottom.saturating_mul(page.width) <= MAX_PIXELS {
            page.data.resize(bottom * page.width, self.page_default);
            page.height = bottom;
        }
        page.compose(bitmap, info.x, info.y, info.combine);
    }

    fn segment(&mut self, seg: &Segment) -> std::result::Result<(), String> {
        let d = seg.data;
        let short = || format!("segment {} is too short", seg.number);
        match seg.kind {
            // Symbol dictionary.
            0 => {
                let flags = u16_at(d, 0).ok_or_else(short)?;
                let huffman = flags & 1 != 0;
                let refagg = flags & 2 != 0;
                let template = ((flags >> 10) & 3) as u8;
                let r_template = ((flags >> 12) & 1) as u8;
                let mut pos = 2;
                let at = if huffman {
                    Vec::new()
                } else {
                    let n = if template == 0 { 4 } else { 1 };
                    let at = at_pixels(&d[pos..], n).ok_or_else(short)?;
                    pos += n * 2;
                    at
                };
                let mut r_at = [(-1, -1), (-1, -1)];
                if refagg && r_template == 0 {
                    let a = at_pixels(&d[pos..], 2).ok_or_else(short)?;
                    r_at = [a[0], a[1]];
                    pos += 4;
                }
                let exported = u32_at(d, pos).ok_or_else(short)? as usize;
                let new = u32_at(d, pos + 4).ok_or_else(short)? as usize;
                pos += 8;
                if flags & 0x100 != 0 {
                    self.warnings.push(
                        "JBIG2 symbol dictionaries that reuse contexts are approximated".into(),
                    );
                }
                let input = self.symbols_of(&seg.referred);
                let tables = if huffman {
                    let custom = self.tables_of(&seg.referred);
                    let mut custom = custom.into_iter();
                    let mut pick =
                        |sel: u16, a: usize, b: usize| -> std::result::Result<Table, String> {
                            match sel {
                                0 => Ok(Table::standard(a)),
                                1 if b > 0 => Ok(Table::standard(b)),
                                _ => custom
                                    .next()
                                    .cloned()
                                    .ok_or_else(|| "missing custom table".into()),
                            }
                        };
                    Some(DictTables {
                        dh: pick((flags >> 2) & 3, 4, 5)?,
                        dw: pick((flags >> 4) & 3, 2, 3)?,
                        bmsize: pick((flags >> 6) & 1, 1, 0)?,
                        agginst: pick((flags >> 7) & 1, 1, 0)?,
                    })
                } else {
                    None
                };
                let symbols = decode_dict(
                    &Dict {
                        huffman,
                        refagg,
                        template,
                        at: &at,
                        r_template,
                        r_at,
                        exported,
                        new,
                        input: &input,
                        tables: tables.as_ref(),
                    },
                    &d[pos..],
                )?;
                self.stored.insert(seg.number, Stored::Symbols(symbols));
            }
            // Text regions: intermediate, immediate, immediate lossless.
            4 | 6 | 7 => {
                let info = region_info(d).ok_or_else(short)?;
                let flags = u16_at(d, 17).ok_or_else(short)?;
                let huffman = flags & 1 != 0;
                let refine = flags & 2 != 0;
                let log_strips = (flags >> 2) & 3;
                let ref_corner = ((flags >> 4) & 3) as u8;
                let transposed = flags & 0x40 != 0;
                let combine = Combine::from_bits(((flags >> 7) & 3) as u8);
                let default_pixel = ((flags >> 9) & 1) as u8;
                let ds_offset = {
                    let v = i64::from((flags >> 10) & 0x1f);
                    if v >= 16 { v - 32 } else { v }
                };
                let r_template = ((flags >> 15) & 1) as u8;
                let mut pos = 19;
                let mut huff_flags = 0u16;
                if huffman {
                    huff_flags = u16_at(d, pos).ok_or_else(short)?;
                    pos += 2;
                }
                let mut r_at = [(-1, -1), (-1, -1)];
                if refine && r_template == 0 {
                    let a = at_pixels(&d[pos..], 2).ok_or_else(short)?;
                    r_at = [a[0], a[1]];
                    pos += 4;
                }
                let instances = u32_at(d, pos).ok_or_else(short)? as usize;
                pos += 4;
                if !self.fits(&info) {
                    return Err("text region too large".into());
                }
                let symbols = self.symbols_of(&seg.referred);
                let id_len = symbol::symbol_id_len(symbols.len());
                let base = Text {
                    width: info.width,
                    height: info.height,
                    refine,
                    strips: 1 << log_strips,
                    ref_corner,
                    transposed,
                    combine,
                    default_pixel,
                    ds_offset,
                    r_template,
                    r_at,
                    instances,
                    symbols: &symbols,
                    id_len,
                    tables: None,
                };
                let bitmap = if huffman {
                    let mut r = BitReader::new(&d[pos..]);
                    let ids = symbol::read_symbol_codes(&mut r, symbols.len())
                        .ok_or("bad symbol ID code table")?;
                    let custom = self.tables_of(&seg.referred);
                    let mut custom = custom.into_iter();
                    let mut pick =
                        |sel: u16, options: &[usize]| -> std::result::Result<Table, String> {
                            match options.get(sel as usize) {
                                Some(&n) => Ok(Table::standard(n)),
                                None => custom
                                    .next()
                                    .cloned()
                                    .ok_or_else(|| "missing custom table".into()),
                            }
                        };
                    let tables = TextTables {
                        fs: pick(huff_flags & 3, &[6, 7])?,
                        ds: pick((huff_flags >> 2) & 3, &[8, 9, 10])?,
                        dt: pick((huff_flags >> 4) & 3, &[11, 12, 13])?,
                        rdw: pick((huff_flags >> 6) & 3, &[14, 15])?,
                        rdh: pick((huff_flags >> 8) & 3, &[14, 15])?,
                        rdx: pick((huff_flags >> 10) & 3, &[14, 15])?,
                        rdy: pick((huff_flags >> 12) & 3, &[14, 15])?,
                        rsize: pick((huff_flags >> 14) & 1, &[1])?,
                        ids,
                    };
                    let text = Text {
                        tables: Some(&tables),
                        ..base
                    };
                    decode_text(&text, &mut Coder::Huffman { r: &mut r })?
                } else {
                    let mut dec = ArithDecoder::new(&d[pos..]);
                    let mut cx = arith::IntContexts::new(id_len);
                    decode_text(
                        &base,
                        &mut Coder::Arith {
                            d: &mut dec,
                            cx: &mut cx,
                        },
                    )?
                };
                self.region_done(seg, bitmap, info);
            }
            // Pattern dictionary.
            16 => {
                let flags = *d.first().ok_or_else(short)?;
                let p = PatternDict {
                    mmr: flags & 1 != 0,
                    template: (flags >> 1) & 3,
                    width: usize::from(*d.get(1).ok_or_else(short)?),
                    height: usize::from(*d.get(2).ok_or_else(short)?),
                    gray_max: u32_at(d, 3).ok_or_else(short)? as usize,
                };
                let patterns = decode_patterns(&p, &d[7..])?;
                self.stored.insert(seg.number, Stored::Patterns(patterns));
            }
            // Halftone regions.
            20 | 22 | 23 => {
                let info = region_info(d).ok_or_else(short)?;
                let flags = *d.get(17).ok_or_else(short)?;
                let i32_at = |i: usize| u32_at(d, i).map(|v| i64::from(v as i32));
                let patterns = seg
                    .referred
                    .iter()
                    .find_map(|r| match self.stored.get(r) {
                        Some(Stored::Patterns(p)) => Some(p),
                        _ => None,
                    })
                    .ok_or("halftone region without patterns")?;
                if !self.fits(&info) {
                    return Err("halftone region too large".into());
                }
                let h = Halftone {
                    width: info.width,
                    height: info.height,
                    mmr: flags & 1 != 0,
                    template: (flags >> 1) & 3,
                    enable_skip: flags & 8 != 0,
                    combine: Combine::from_bits((flags >> 4) & 7),
                    default_pixel: (flags >> 7) & 1,
                    grid_width: u32_at(d, 18).ok_or_else(short)? as usize,
                    grid_height: u32_at(d, 22).ok_or_else(short)? as usize,
                    grid_x: i32_at(26).ok_or_else(short)?,
                    grid_y: i32_at(30).ok_or_else(short)?,
                    step_x: i64::from(u16_at(d, 34).ok_or_else(short)?),
                    step_y: i64::from(u16_at(d, 36).ok_or_else(short)?),
                    patterns,
                };
                let bitmap = decode_halftone(&h, &d[38..])?;
                self.region_done(seg, bitmap, info);
            }
            // Generic regions.
            36 | 38 | 39 => {
                let info = region_info(d).ok_or_else(short)?;
                let flags = *d.get(17).ok_or_else(short)?;
                let mmr = flags & 1 != 0;
                let template = (flags >> 1) & 3;
                let tpgdon = flags & 8 != 0;
                if !self.fits(&info) {
                    return Err("generic region too large".into());
                }
                let mut height = info.height;
                // With an unknown length, the row count follows the data.
                if mmr || d.len() >= 4 {
                    if let Some(rows) = d.len().checked_sub(4).and_then(|i| u32_at(d, i)) {
                        if self.growing && rows as usize <= height && seg_has_unknown_length(d, mmr)
                        {
                            height = rows as usize;
                        }
                    }
                }
                let bitmap = if mmr {
                    decode_generic_mmr(info.width, height, &d[18..]).0
                } else {
                    let n = if template == 0 { 4 } else { 1 };
                    let at = at_pixels(&d[18..], n).ok_or_else(short)?;
                    let mut dec = ArithDecoder::new(&d[18 + n * 2..]);
                    let mut cx = contexts(1 << 16);
                    decode_generic(
                        &Generic {
                            width: info.width,
                            height,
                            template,
                            tpgdon,
                            at: &at,
                            skip: None,
                        },
                        &mut dec,
                        &mut cx,
                    )
                };
                self.region_done(seg, bitmap, RegionInfo { height, ..info });
            }
            // Generic refinement regions.
            40 | 42 | 43 => {
                let info = region_info(d).ok_or_else(short)?;
                let flags = *d.get(17).ok_or_else(short)?;
                let template = flags & 1;
                let tpgron = flags & 2 != 0;
                let (at, pos) = if template == 0 {
                    (at_pixels(&d[18..], 2).ok_or_else(short)?, 22)
                } else {
                    (Vec::new(), 18)
                };
                if !self.fits(&info) {
                    return Err("refinement region too large".into());
                }
                // The reference is a region an earlier intermediate segment
                // left, or else what the page holds under this one.
                let reference = seg
                    .referred
                    .iter()
                    .find_map(|r| match self.stored.get(r) {
                        Some(Stored::Region(b)) => Some(b.clone()),
                        _ => None,
                    })
                    .or_else(|| {
                        self.page
                            .as_ref()
                            .map(|p| p.crop(info.x, info.y, info.width, info.height))
                    })
                    .ok_or("refinement region without a reference")?;
                let mut dec = ArithDecoder::new(&d[pos..]);
                let mut cx = contexts(1 << 13);
                let bitmap = decode_refinement(
                    &Refinement {
                        width: info.width,
                        height: info.height,
                        template,
                        reference: &reference,
                        dx: 0,
                        dy: 0,
                        tpgron,
                        at: &at,
                    },
                    &mut dec,
                    &mut cx,
                );
                // Refining the page replaces what was there.
                let info = if seg.referred.is_empty() {
                    RegionInfo {
                        combine: Combine::Replace,
                        ..info
                    }
                } else {
                    info
                };
                self.region_done(seg, bitmap, info);
            }
            // Page information.
            48 => {
                let width = u32_at(d, 0).ok_or_else(short)? as usize;
                let height = u32_at(d, 4).ok_or_else(short)?;
                let flags = *d.get(16).ok_or_else(short)?;
                self.page_default = (flags >> 2) & 1;
                self.growing = height == u32::MAX;
                let height = if self.growing { 0 } else { height as usize };
                if width.saturating_mul(height) > MAX_PIXELS {
                    return Err("page too large".into());
                }
                self.page = Some(Bitmap::new(width, height, self.page_default));
            }
            // End of stripe: the page is at least this tall.
            50 => {
                let end = u32_at(d, 0).ok_or_else(short)? as usize + 1;
                if let Some(page) = &mut self.page {
                    if self.growing
                        && end > page.height
                        && end.saturating_mul(page.width) <= MAX_PIXELS
                    {
                        page.data.resize(end * page.width, self.page_default);
                        page.height = end;
                    }
                }
            }
            // Custom Huffman table.
            53 => {
                let table = Table::from_segment(d).ok_or("bad Huffman table segment")?;
                self.stored.insert(seg.number, Stored::Table(table));
            }
            // End of page, end of file, profiles, extensions and others.
            _ => {}
        }
        Ok(())
    }

    /// Places an immediate region on the page or keeps an intermediate one.
    fn region_done(&mut self, seg: &Segment, bitmap: Bitmap, info: RegionInfo) {
        if matches!(seg.kind, 4 | 20 | 36 | 40) {
            self.stored.insert(seg.number, Stored::Region(bitmap));
        } else {
            self.place(&bitmap, info);
        }
    }
}

/// Whether an immediate generic region ends with the end marker and row
/// count of the unknown-length form.
fn seg_has_unknown_length(d: &[u8], mmr: bool) -> bool {
    let n = d.len();
    n >= 6 && d[n - 6..n - 4] == if mmr { [0x00, 0x00] } else { [0xff, 0xac] }
}

/// Decodes a JBIG2 image. `globals` is the JBIG2Globals stream, if any.
/// Returns packed 1-bit rows in which 0 is black, as PDF expects, and
/// warnings about anything approximated.
pub fn decode_jbig2(data: &[u8], globals: Option<&[u8]>) -> Result<(Vec<u8>, Vec<String>)> {
    let fail = |reason: String, partial: Vec<u8>| FilterError::new("JBIG2Decode", reason, partial);
    let mut dec = Decoder {
        stored: HashMap::new(),
        page: None,
        growing: false,
        page_default: 0,
        warnings: Vec::new(),
    };
    let mut error = None;
    let global_segments = match globals {
        Some(g) => segments(g).map_err(|e| fail(e, Vec::new()))?,
        None => Vec::new(),
    };
    let page_segments = segments(data).map_err(|e| fail(e, Vec::new()))?;
    for seg in global_segments.iter().chain(&page_segments) {
        if let Err(e) = dec.segment(seg) {
            error = Some(format!("segment {}: {e}", seg.number));
            break;
        }
    }
    let Some(page) = dec.page else {
        return Err(fail(
            error.unwrap_or_else(|| "no page information".into()),
            Vec::new(),
        ));
    };
    let packed = pack(&page);
    match error {
        Some(e) => Err(fail(e, packed)),
        None => Ok((packed, dec.warnings)),
    }
}

/// Packs a page into rows of bits, 0 = black.
fn pack(page: &Bitmap) -> Vec<u8> {
    let row_bytes = page.width.div_ceil(8);
    let mut out = vec![0xffu8; row_bytes * page.height];
    for y in 0..page.height {
        let row = &page.data[y * page.width..(y + 1) * page.width];
        for (x, &v) in row.iter().enumerate() {
            if v != 0 {
                out[y * row_bytes + x / 8] &= !(0x80 >> (x % 8));
            }
        }
    }
    out
}
