//! JPXDecode (ISO 32000-2, 7.4.9): JPEG 2000 Part 1 (ITU-T T.800), either
//! a JP2 file or a bare codestream.
//!
//! Supported: any number of tiles, components and quality layers; every
//! progression order and progression order changes; precincts; all
//! code-block style switches (bypass, reset, termination, vertically
//! causal context, segmentation symbols); SOP and EPH markers; packed
//! packet headers (PPM, PPT); reversible 5/3 and irreversible 9/7
//! wavelets; no, derived and expounded quantization; region of interest
//! (max-shift); RCT and ICT; component subsampling; and from JP2 the colour
//! specification, palettes, component mapping and channel definitions
//! (including opacity). Part 15 high-throughput code-blocks are refused.

mod codestream;
mod dwt;
mod mq;
mod t1;
mod tile;

use crate::{FilterError, Result};
use codestream::{Params, Size};
use tile::{Samples, Tile};

/// Refuse images with more samples than this, over all components.
const MAX_SAMPLES: u64 = 1 << 30;

/// What the image's colour channels are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpxColor {
    Gray,
    Rgb,
    Cmyk,
    /// No colour specification: go by the channel count.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct DecodedJpx {
    pub width: usize,
    pub height: usize,
    /// Colour channels per pixel in `data`.
    pub components: usize,
    /// 8 bits per channel, interleaved, rows top to bottom.
    pub data: Vec<u8>,
    /// An opacity channel, when the JP2 header defines one.
    pub alpha: Option<Vec<u8>>,
    pub color: JpxColor,
    /// The ICC profile of the colour specification, if it has one.
    pub icc: Option<Vec<u8>>,
    pub warnings: Vec<String>,
}

/// One decoded component at its own resolution: raw sample values, level
/// shifted to be unsigned.
struct Plane {
    width: usize,
    height: usize,
    /// Its origin on the component grid.
    x0: u32,
    y0: u32,
    dx: u32,
    dy: u32,
    precision: u32,
    data: Vec<u16>,
}

/// JP2 header boxes that shape the output.
#[derive(Default)]
struct Header {
    /// Enumerated colour space, or the ICC profile.
    enumerated: Option<u32>,
    icc: Option<Vec<u8>>,
    /// Palette columns: (bit depth, signed, entries).
    palette: Option<Vec<(u32, bool, Vec<u32>)>>,
    /// (component, palette column or None for direct use).
    mapping: Option<Vec<(usize, Option<usize>)>>,
    /// (channel, type, association).
    channels: Option<Vec<(usize, u16, u16)>>,
}

fn be(d: &[u8], at: usize, n: usize) -> Option<u64> {
    let b = d.get(at..at + n)?;
    Some(b.iter().fold(0u64, |v, &x| v << 8 | u64::from(x)))
}

/// The boxes in `d` as (type, contents).
fn boxes(d: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 8 <= d.len() {
        let len = be(d, pos, 4).unwrap_or(0);
        let kind: [u8; 4] = d[pos + 4..pos + 8].try_into().unwrap_or_default();
        let (start, end) = match len {
            0 => (pos + 8, d.len()),
            1 => {
                let Some(xl) = be(d, pos + 8, 8) else { break };
                (pos + 16, pos.saturating_add(xl as usize))
            }
            n => (pos + 8, pos.saturating_add(n as usize)),
        };
        if end < start {
            break;
        }
        // A box that claims more than is there keeps what is.
        let end = end.min(d.len());
        out.push((kind, &d[start..end]));
        pos = end;
    }
    out
}

fn parse_header(d: &[u8], h: &mut Header) {
    for (kind, body) in boxes(d) {
        match &kind {
            b"colr" if h.enumerated.is_none() && h.icc.is_none() => match body.first() {
                Some(1) => h.enumerated = be(body, 3, 4).map(|v| v as u32),
                Some(2 | 3) => h.icc = body.get(3..).map(<[u8]>::to_vec),
                _ => {}
            },
            b"pclr" => {
                let (Some(n), Some(&cols)) = (be(body, 0, 2), body.get(2)) else {
                    continue;
                };
                let (n, cols) = (n as usize, usize::from(cols));
                let Some(depths) = body.get(3..3 + cols) else {
                    continue;
                };
                let mut columns: Vec<(u32, bool, Vec<u32>)> = depths
                    .iter()
                    .map(|&b| {
                        (
                            u32::from(b & 0x7f) + 1,
                            b & 0x80 != 0,
                            Vec::with_capacity(n),
                        )
                    })
                    .collect();
                let mut at = 3 + cols;
                'entries: for _ in 0..n {
                    for col in columns.iter_mut() {
                        let bytes = col.0.div_ceil(8) as usize;
                        let Some(v) = be(body, at, bytes) else {
                            break 'entries;
                        };
                        col.2.push(v as u32);
                        at += bytes;
                    }
                }
                h.palette = Some(columns);
            }
            b"cmap" => {
                h.mapping = Some(
                    body.chunks_exact(4)
                        .map(|c| {
                            let comp = usize::from(u16::from_be_bytes([c[0], c[1]]));
                            (comp, (c[2] == 1).then_some(usize::from(c[3])))
                        })
                        .collect(),
                );
            }
            b"cdef" => {
                h.channels = Some(
                    body.get(2..)
                        .unwrap_or(&[])
                        .chunks_exact(6)
                        .map(|c| {
                            (
                                usize::from(u16::from_be_bytes([c[0], c[1]])),
                                u16::from_be_bytes([c[2], c[3]]),
                                u16::from_be_bytes([c[4], c[5]]),
                            )
                        })
                        .collect(),
                );
            }
            _ => {}
        }
    }
}

/// Decodes a JPXDecode stream.
pub fn decode_jpx(data: &[u8]) -> Result<DecodedJpx> {
    let fail = |reason: String| FilterError::new("JPXDecode", reason, Vec::new());
    let mut header = Header::default();
    let stream = if data.starts_with(&[0xff, 0x4f]) {
        data
    } else {
        let mut stream = None;
        for (kind, body) in boxes(data) {
            match &kind {
                b"jp2h" => parse_header(body, &mut header),
                b"jp2c" if stream.is_none() => stream = Some(body),
                _ => {}
            }
        }
        stream.ok_or_else(|| fail("no codestream".into()))?
    };
    let (planes, size, mut warnings) = decode_codestream(stream).map_err(fail)?;
    assemble(planes, &size, header, &mut warnings)
        .map(|mut d| {
            d.warnings = warnings;
            d
        })
        .map_err(fail)
}

fn decode_codestream(d: &[u8]) -> std::result::Result<(Vec<Plane>, Size, Vec<String>), String> {
    let cs = codestream::parse(d)?;
    let size = cs.size.clone();
    let mut warnings = cs.warnings;
    let mut total = 0u64;
    let mut planes: Vec<Plane> = Vec::new();
    for c in &size.components {
        let x0 = size.x0.div_ceil(c.dx);
        let y0 = size.y0.div_ceil(c.dy);
        let w = u64::from(size.x1.div_ceil(c.dx) - x0);
        let h = u64::from(size.y1.div_ceil(c.dy) - y0);
        total += w * h;
        if total > MAX_SAMPLES {
            return Err("image too large".into());
        }
        planes.push(Plane {
            width: w as usize,
            height: h as usize,
            x0,
            y0,
            dx: c.dx,
            dy: c.dy,
            precision: c.precision.min(16),
            data: Vec::new(),
        });
    }
    for p in &mut planes {
        p.data = vec![0; p.width * p.height];
    }
    for (index, td) in cs.tiles.iter().enumerate() {
        if td.params.is_none() {
            warnings.push(format!("tile {index} is missing"));
            continue;
        }
        let empty = Params::default();
        let tile_params = match &td.params {
            Some(p) => p,
            None => &empty,
        };
        let params = Params::resolve(&cs.main, tile_params)?;
        if params
            .styles
            .iter()
            .any(|s| s.cb_style & codestream::style::HT != 0)
        {
            return Err("high-throughput (Part 15) code-blocks are not supported".into());
        }
        let roi = params.roi.clone();
        let mct = params.order.mct;
        let mut tile = Tile::new(&size, index as u32, params)?;
        if let Some(w) = tile.read_packets(&size, &td.data, td.headers.as_deref()) {
            warnings.push(format!("tile {index}: {w}"));
        }
        let mut samples = tile.reconstruct(&roi);
        if mct && samples.len() >= 3 {
            inverse_mct(&mut samples, &mut warnings);
        }
        for ((s, tc), (plane, info)) in samples
            .iter()
            .zip(&tile.comps)
            .zip(planes.iter_mut().zip(&size.components))
        {
            store(s, tc.rect, plane, info.precision);
        }
    }
    Ok((planes, size, warnings))
}

/// Undoes the multiple component transform on the first three components.
fn inverse_mct(s: &mut [Samples], warnings: &mut Vec<String>) {
    let (a, rest) = s.split_at_mut(1);
    let (b, c) = rest.split_at_mut(1);
    match (&mut a[0], &mut b[0], &mut c[0]) {
        (Samples::Int(y0), Samples::Int(y1), Samples::Int(y2))
            if y0.len() == y1.len() && y1.len() == y2.len() =>
        {
            for i in 0..y0.len() {
                let g = y0[i] - ((y2[i] + y1[i]) >> 2);
                let r = y2[i] + g;
                let b = y1[i] + g;
                (y0[i], y1[i], y2[i]) = (r, g, b);
            }
        }
        (Samples::Float(y), Samples::Float(cb), Samples::Float(cr))
            if y.len() == cb.len() && cb.len() == cr.len() =>
        {
            for i in 0..y.len() {
                let (l, u, v) = (y[i], cb[i], cr[i]);
                y[i] = l + 1.402 * v;
                cb[i] = l - 0.344_136 * u - 0.714_136 * v;
                cr[i] = l + 1.772 * u;
            }
        }
        _ => warnings.push("component transform on mismatched components skipped".into()),
    }
}

/// Level shifts a tile-component's samples and writes them into its plane.
fn store(s: &Samples, rect: dwt::Rect, plane: &mut Plane, precision: u32) {
    let w = rect.width();
    let shift = 1i64 << (precision - 1);
    let max = (1i64 << precision) - 1;
    // Planes keep at most 16 bits.
    let drop = precision.saturating_sub(16);
    let value = |i: usize| -> u16 {
        let v = match s {
            Samples::Int(v) => i64::from(v[i]),
            Samples::Float(v) => v[i].round() as i64,
        };
        ((v + shift).clamp(0, max) >> drop) as u16
    };
    for y in 0..rect.height() {
        let py = (rect.y0 + y as u32).checked_sub(plane.y0);
        let Some(py) = py.map(|v| v as usize).filter(|&v| v < plane.height) else {
            continue;
        };
        for x in 0..w {
            let Some(px) = (rect.x0 + x as u32).checked_sub(plane.x0) else {
                continue;
            };
            let px = px as usize;
            if px < plane.width && y * w + x < sample_len(s) {
                plane.data[py * plane.width + px] = value(y * w + x);
            }
        }
    }
}

fn sample_len(s: &Samples) -> usize {
    match s {
        Samples::Int(v) => v.len(),
        Samples::Float(v) => v.len(),
    }
}

/// Scales a `bits`-deep value to 8 bits.
fn to8(v: u32, bits: u32) -> u8 {
    match bits {
        8 => v.min(255) as u8,
        0 => 0,
        b if b > 8 => ((v + (1 << (b - 9))) >> (b - 8)).min(255) as u8,
        b => (v * 255 / ((1 << b) - 1)).min(255) as u8,
    }
}

/// A channel: a plane used directly or through a palette column.
struct Channel<'a> {
    plane: &'a Plane,
    palette: Option<&'a (u32, bool, Vec<u32>)>,
}

impl Channel<'_> {
    fn sample8(&self, i: usize) -> u8 {
        let v = u32::from(self.plane.data[i]);
        match self.palette {
            None => to8(v, self.plane.precision),
            Some((bits, signed, entries)) => {
                let e = entries
                    .get(v as usize)
                    .or(entries.last())
                    .copied()
                    .unwrap_or(0);
                let e = if *signed { e ^ (1 << (bits - 1)) } else { e };
                to8(e, (*bits).min(32))
            }
        }
    }
}

fn assemble(
    planes: Vec<Plane>,
    size: &Size,
    header: Header,
    warnings: &mut Vec<String>,
) -> std::result::Result<DecodedJpx, String> {
    // The output grid is the finest component's: an image whose components
    // are all subsampled comes out at their size, not the reference grid's.
    let mdx = size.components.iter().map(|c| c.dx).min().unwrap_or(1);
    let mdy = size.components.iter().map(|c| c.dy).min().unwrap_or(1);
    let (gx0, gy0) = (size.x0.div_ceil(mdx), size.y0.div_ceil(mdy));
    let width = (size.x1.div_ceil(mdx) - gx0) as usize;
    let height = (size.y1.div_ceil(mdy) - gy0) as usize;
    // Channels, after the palette.
    let channels: Vec<Channel> = match (&header.palette, &header.mapping) {
        (Some(pal), Some(map)) => map
            .iter()
            .filter_map(|&(c, col)| {
                let plane = planes.get(c)?;
                Some(Channel {
                    plane,
                    palette: col.and_then(|k| pal.get(k)),
                })
            })
            .collect(),
        _ => planes
            .iter()
            .map(|plane| Channel {
                plane,
                palette: None,
            })
            .collect(),
    };
    if channels.is_empty() {
        return Err("no channels".into());
    }
    let icc_space = header.icc.as_ref().and_then(|p| p.get(16..20));
    let (color, ycc) = match (header.enumerated, icc_space) {
        (Some(16 | 20 | 21 | 24), _) => (JpxColor::Rgb, false),
        (Some(18), _) => (JpxColor::Rgb, true),
        (Some(17), _) => (JpxColor::Gray, false),
        (Some(12), _) => (JpxColor::Cmyk, false),
        (_, Some(b"RGB ")) => (JpxColor::Rgb, false),
        (_, Some(b"GRAY")) => (JpxColor::Gray, false),
        (_, Some(b"CMYK")) => (JpxColor::Cmyk, false),
        _ => (JpxColor::Unknown, false),
    };
    // Which channels are colours (in order) and which is opacity.
    let mut order: Vec<usize> = Vec::new();
    let mut alpha_channel = None;
    if let Some(defs) = &header.channels {
        let mut colours: Vec<(u16, usize)> = Vec::new();
        for &(ch, kind, assoc) in defs {
            if ch >= channels.len() {
                continue;
            }
            match kind {
                0 => colours.push((assoc, ch)),
                1 | 2 if alpha_channel.is_none() => alpha_channel = Some(ch),
                _ => {}
            }
        }
        colours.sort_by_key(|&(assoc, ch)| (assoc, ch));
        order = colours.into_iter().map(|c| c.1).collect();
    }
    if order.is_empty() {
        order = (0..channels.len())
            .filter(|&c| Some(c) != alpha_channel)
            .collect();
    }
    let wanted = match color {
        JpxColor::Gray => 1,
        JpxColor::Rgb => 3,
        JpxColor::Cmyk => 4,
        JpxColor::Unknown => order.len(),
    };
    if order.len() < wanted {
        warnings.push(format!(
            "{} colour channels for a space that needs {wanted}",
            order.len()
        ));
    }
    order.truncate(wanted.max(1));
    let color = if order.len() == wanted {
        color
    } else {
        JpxColor::Unknown
    };
    let n = order.len();

    // Each channel's sample index for every output column and row.
    let index_maps = |ch: &Channel| -> (Vec<usize>, Vec<usize>) {
        let p = ch.plane;
        let xs = (0..width)
            .map(|x| {
                let gx = (gx0 + x as u32) * mdx;
                ((gx / p.dx).saturating_sub(p.x0) as usize).min(p.width.saturating_sub(1))
            })
            .collect();
        let ys = (0..height)
            .map(|y| {
                let gy = (gy0 + y as u32) * mdy;
                ((gy / p.dy).saturating_sub(p.y0) as usize).min(p.height.saturating_sub(1))
            })
            .collect();
        (xs, ys)
    };
    let mut out = vec![0u8; width * height * n];
    for (k, &ch) in order.iter().enumerate() {
        let ch = &channels[ch];
        let (xs, ys) = index_maps(ch);
        let pw = ch.plane.width;
        for (y, &sy) in ys.iter().enumerate() {
            let row = &mut out[y * width * n..(y + 1) * width * n];
            for (x, &sx) in xs.iter().enumerate() {
                row[x * n + k] = ch.sample8(sy * pw + sx);
            }
        }
    }
    if ycc && n == 3 {
        for px in out.chunks_exact_mut(3) {
            let (y, cb, cr) = (
                f32::from(px[0]),
                f32::from(px[1]) - 128.0,
                f32::from(px[2]) - 128.0,
            );
            px[0] = (y + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
            px[1] = (y - 0.344_136 * cb - 0.714_136 * cr)
                .round()
                .clamp(0.0, 255.0) as u8;
            px[2] = (y + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
        }
    }
    let alpha = alpha_channel.map(|a| {
        let ch = &channels[a];
        let (xs, ys) = index_maps(ch);
        let mut v = Vec::with_capacity(width * height);
        for &sy in &ys {
            for &sx in &xs {
                v.push(ch.sample8(sy * ch.plane.width + sx));
            }
        }
        v
    });
    Ok(DecodedJpx {
        width,
        height,
        components: n,
        data: out,
        alpha,
        color,
        icc: header.icc,
        warnings: Vec::new(),
    })
}
