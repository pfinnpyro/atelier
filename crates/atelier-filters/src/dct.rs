//! DCTDecode: a baseline and progressive JPEG decoder (ITU T.81), written from
//! scratch. Huffman coding only; arithmetic coding, lossless and 12-bit JPEGs
//! are rare in PDFs and are reported as unsupported.

use crate::{FilterError, Result};

const FILTER: &str = "DCTDecode";

/// Zigzag position to natural (row-major) position. Extra entries keep a
/// corrupt run from indexing past the block.
const ZIGZAG: [usize; 80] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63,
    63, 63, 63, 63, 63, 63,
];

/// A decoded JPEG: 8-bit samples, components interleaved, rows top down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedJpeg {
    pub width: usize,
    pub height: usize,
    /// 1 (gray), 3 (RGB after colour conversion) or 4 (CMYK).
    pub components: usize,
    pub data: Vec<u8>,
}

/// Decodes a JPEG. `color_transform` is PDF's /ColorTransform parameter:
/// `None` lets the file decide (Adobe marker, else YCbCr for 3 components).
pub fn decode_dct(data: &[u8], color_transform: Option<bool>) -> Result<DecodedJpeg> {
    let mut decoder = Decoder::default();
    decoder
        .parse(data)
        .map_err(|reason| FilterError::new(FILTER, reason, Vec::new()))?;
    decoder
        .output(color_transform)
        .map_err(|reason| FilterError::new(FILTER, reason, Vec::new()))
}

type R<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Default)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    blocks_w: usize,
    blocks_h: usize,
    coefs: Vec<i16>,
    dc_pred: i32,
}

#[derive(Clone, Copy)]
struct QuantTable([u16; 64]);

impl Default for QuantTable {
    fn default() -> Self {
        Self([1; 64])
    }
}

#[derive(Default)]
struct Decoder {
    width: usize,
    height: usize,
    progressive: bool,
    components: Vec<Component>,
    qt: [QuantTable; 4],
    dc_tables: [Option<Huffman>; 4],
    ac_tables: [Option<Huffman>; 4],
    restart_interval: usize,
    adobe_transform: Option<u8>,
    jfif: bool,
    eobrun: u32,
    hmax: usize,
    vmax: usize,
    mcus_x: usize,
    mcus_y: usize,
    frame_seen: bool,
}

impl Decoder {
    fn parse(&mut self, data: &[u8]) -> R<()> {
        let mut pos = 0;
        // Some PDFs have junk before SOI.
        while pos + 1 < data.len() && !(data[pos] == 0xFF && data[pos + 1] == 0xD8) {
            pos += 1;
        }
        if pos + 1 >= data.len() {
            return Err("no start-of-image marker");
        }
        pos += 2;
        loop {
            // Find the next marker, skipping fill bytes.
            while pos < data.len() && data[pos] != 0xFF {
                pos += 1;
            }
            while pos < data.len() && data[pos] == 0xFF {
                pos += 1;
            }
            let Some(&marker) = data.get(pos) else {
                break; // missing EOI: keep what we have
            };
            pos += 1;
            match marker {
                0xD9 => break,
                0xD0..=0xD7 | 0x01 => continue,
                _ => {}
            }
            let len = usize::from(u16::from_be_bytes([
                *data.get(pos).ok_or("truncated marker")?,
                *data.get(pos + 1).ok_or("truncated marker")?,
            ]));
            if len < 2 {
                return Err("bad marker length");
            }
            let seg = data.get(pos + 2..pos + len).ok_or("truncated segment")?;
            pos += len;
            match marker {
                0xC0 | 0xC1 => self.frame(seg, false)?,
                0xC2 => self.frame(seg, true)?,
                0xC3 | 0xC5..=0xC7 | 0xCB | 0xCD..=0xCF => {
                    return Err("lossless or hierarchical JPEG");
                }
                0xC9 | 0xCA => return Err("arithmetic-coded JPEG"),
                0xC4 => self.huffman_tables(seg)?,
                0xDB => self.quant_tables(seg)?,
                0xDD => {
                    self.restart_interval = usize::from(u16::from_be_bytes([
                        *seg.first().ok_or("bad DRI")?,
                        *seg.get(1).ok_or("bad DRI")?,
                    ]));
                }
                0xDA => {
                    if !self.frame_seen {
                        return Err("scan before frame header");
                    }
                    pos = self.scan(seg, data, pos)?;
                }
                0xEE if seg.len() >= 12 && seg.starts_with(b"Adobe") => {
                    self.adobe_transform = Some(seg[11]);
                }
                0xE0 if seg.starts_with(b"JFIF\0") => self.jfif = true,
                _ => {}
            }
        }
        if !self.frame_seen {
            return Err("no frame header");
        }
        Ok(())
    }

    fn frame(&mut self, seg: &[u8], progressive: bool) -> R<()> {
        if self.frame_seen {
            return Err("more than one frame");
        }
        if seg.len() < 6 {
            return Err("short frame header");
        }
        if seg[0] != 8 {
            return Err("only 8-bit JPEGs are supported");
        }
        self.height = usize::from(u16::from_be_bytes([seg[1], seg[2]]));
        self.width = usize::from(u16::from_be_bytes([seg[3], seg[4]]));
        let n = usize::from(seg[5]);
        if self.width == 0 || self.height == 0 {
            return Err("zero image size (DNL is not supported)");
        }
        if !matches!(n, 1 | 3 | 4) || seg.len() < 6 + 3 * n {
            return Err("unsupported component count");
        }
        self.progressive = progressive;
        for i in 0..n {
            let c = &seg[6 + 3 * i..9 + 3 * i];
            let (h, v) = (usize::from(c[1] >> 4), usize::from(c[1] & 15));
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) || c[2] > 3 {
                return Err("bad sampling factors");
            }
            self.components.push(Component {
                id: c[0],
                h,
                v,
                tq: usize::from(c[2]),
                ..Component::default()
            });
        }
        self.hmax = self.components.iter().map(|c| c.h).max().unwrap_or(1);
        self.vmax = self.components.iter().map(|c| c.v).max().unwrap_or(1);
        self.mcus_x = self.width.div_ceil(8 * self.hmax);
        self.mcus_y = self.height.div_ceil(8 * self.vmax);
        let total: usize = self
            .components
            .iter()
            .map(|c| self.mcus_x * c.h * self.mcus_y * c.v)
            .sum();
        if total > 4_000_000 {
            return Err("image too large");
        }
        for c in &mut self.components {
            c.blocks_w = self.mcus_x * c.h;
            c.blocks_h = self.mcus_y * c.v;
            c.coefs = vec![0; c.blocks_w * c.blocks_h * 64];
        }
        self.frame_seen = true;
        Ok(())
    }

    fn quant_tables(&mut self, mut seg: &[u8]) -> R<()> {
        while !seg.is_empty() {
            let (pq, tq) = (seg[0] >> 4, usize::from(seg[0] & 15));
            if tq > 3 {
                return Err("bad quantization table id");
            }
            let size = if pq == 0 { 64 } else { 128 };
            let body = seg.get(1..1 + size).ok_or("short quantization table")?;
            for k in 0..64 {
                let v = if pq == 0 {
                    u16::from(body[k])
                } else {
                    u16::from_be_bytes([body[2 * k], body[2 * k + 1]])
                };
                self.qt[tq].0[ZIGZAG[k]] = v;
            }
            seg = &seg[1 + size..];
        }
        Ok(())
    }

    fn huffman_tables(&mut self, mut seg: &[u8]) -> R<()> {
        while !seg.is_empty() {
            let (class, id) = (seg[0] >> 4, usize::from(seg[0] & 15));
            if class > 1 || id > 3 {
                return Err("bad Huffman table id");
            }
            let counts: [u8; 16] = seg
                .get(1..17)
                .ok_or("short Huffman table")?
                .try_into()
                .map_err(|_| "short Huffman table")?;
            let n: usize = counts.iter().map(|&c| usize::from(c)).sum();
            let symbols = seg.get(17..17 + n).ok_or("short Huffman table")?;
            let table = Huffman::new(&counts, symbols)?;
            if class == 0 {
                self.dc_tables[id] = Some(table);
            } else {
                self.ac_tables[id] = Some(table);
            }
            seg = &seg[17 + n..];
        }
        Ok(())
    }

    /// Decodes one scan. Returns the position after its entropy-coded data.
    fn scan(&mut self, seg: &[u8], data: &[u8], start: usize) -> R<usize> {
        let ns = usize::from(*seg.first().ok_or("short scan header")?);
        if ns == 0 || ns > 4 || seg.len() < 1 + 2 * ns + 3 {
            return Err("bad scan header");
        }
        let mut comps = Vec::with_capacity(ns);
        for i in 0..ns {
            let id = seg[1 + 2 * i];
            let t = seg[2 + 2 * i];
            let idx = self
                .components
                .iter()
                .position(|c| c.id == id)
                .ok_or("scan names an unknown component")?;
            comps.push((idx, usize::from(t >> 4) & 3, usize::from(t & 15) & 3));
        }
        let ss = usize::from(seg[1 + 2 * ns]);
        let se = usize::from(seg[2 + 2 * ns]);
        let ah = u32::from(seg[3 + 2 * ns] >> 4);
        let al = u32::from(seg[3 + 2 * ns] & 15);
        if !self.progressive {
            if ss != 0 || se != 63 {
                // Some encoders write junk here for baseline; use the full range.
            }
        } else if ss > se || se > 63 || (ss == 0 && se != 0) || (ss > 0 && ns != 1) || al > 13 {
            return Err("bad progressive scan parameters");
        }
        let (ss, se) = if self.progressive { (ss, se) } else { (0, 63) };

        let mut reader = BitReader {
            data,
            pos: start,
            buf: 0,
            bits: 0,
            marker: false,
        };
        for c in &mut self.components {
            c.dc_pred = 0;
        }
        self.eobrun = 0;

        let single = ns == 1;
        let (units_x, units_y) = if single {
            let c = &self.components[comps[0].0];
            let cw = (self.width * c.h).div_ceil(self.hmax);
            let ch = (self.height * c.v).div_ceil(self.vmax);
            (cw.div_ceil(8), ch.div_ceil(8))
        } else {
            (self.mcus_x, self.mcus_y)
        };
        let total = units_x * units_y;
        let mut since_restart = 0;
        for unit in 0..total {
            if self.restart_interval > 0 && since_restart == self.restart_interval {
                reader.restart();
                for c in &mut self.components {
                    c.dc_pred = 0;
                }
                self.eobrun = 0;
                since_restart = 0;
            }
            since_restart += 1;
            let (ux, uy) = (unit % units_x, unit / units_x);
            for &(ci, td, ta) in &comps {
                let (h, v) = if single {
                    (1, 1)
                } else {
                    (self.components[ci].h, self.components[ci].v)
                };
                for by in 0..v {
                    for bx in 0..h {
                        let (col, row) = (ux * h + bx, uy * v + by);
                        let c = &self.components[ci];
                        if col >= c.blocks_w || row >= c.blocks_h {
                            continue;
                        }
                        let offset = (row * c.blocks_w + col) * 64;
                        let r = self.block(&mut reader, ci, offset, td, ta, ss, se, ah, al);
                        if r.is_err() {
                            // Corrupt data: keep what decoded so far.
                            return Ok(next_marker(data, reader.pos));
                        }
                    }
                }
            }
        }
        Ok(next_marker(data, reader.pos))
    }

    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        r: &mut BitReader,
        ci: usize,
        offset: usize,
        td: usize,
        ta: usize,
        ss: usize,
        se: usize,
        ah: u32,
        al: u32,
    ) -> R<()> {
        let comp = &mut self.components[ci];
        let block = &mut comp.coefs[offset..offset + 64];
        if ss == 0 {
            if ah == 0 {
                let dc = self.dc_tables[td].as_ref().ok_or("missing DC table")?;
                let t = dc.decode(r)?;
                let diff = r.receive_extend(t)?;
                comp.dc_pred = comp.dc_pred.wrapping_add(diff);
                block[0] = (comp.dc_pred.wrapping_shl(al)) as i16;
            } else if r.bit()? == 1 {
                block[0] |= 1 << al;
            }
            if !self.progressive {
                let ac = self.ac_tables[ta].as_ref().ok_or("missing AC table")?;
                let mut k = 1;
                while k < 64 {
                    let rs = ac.decode(r)?;
                    let (run, size) = (usize::from(rs >> 4), rs & 15);
                    if size == 0 {
                        if run == 15 {
                            k += 16;
                            continue;
                        }
                        break;
                    }
                    k += run;
                    if k > 63 {
                        break;
                    }
                    block[ZIGZAG[k]] = r.receive_extend(size)? as i16;
                    k += 1;
                }
            }
            return Ok(());
        }

        let ac = self.ac_tables[ta].as_ref().ok_or("missing AC table")?;
        if ah == 0 {
            // AC first pass.
            if self.eobrun > 0 {
                self.eobrun -= 1;
                return Ok(());
            }
            let mut k = ss;
            while k <= se {
                let rs = ac.decode(r)?;
                let (run, size) = (u32::from(rs >> 4), rs & 15);
                if size == 0 {
                    if run < 15 {
                        self.eobrun = (1 << run) - 1;
                        if run > 0 {
                            self.eobrun += r.bits(run)?;
                        }
                        break;
                    }
                    k += 16;
                    continue;
                }
                k += run as usize;
                if k > 63 {
                    break;
                }
                block[ZIGZAG[k]] = (r.receive_extend(size)?.wrapping_shl(al)) as i16;
                k += 1;
            }
            return Ok(());
        }

        // AC refinement, following the structure of libjpeg's decode_mcu_AC_refine.
        let p1: i16 = 1 << al;
        let m1: i16 = -1i16 << al;
        let mut k = ss;
        if self.eobrun == 0 {
            while k <= se {
                let rs = ac.decode(r)?;
                let mut run = i32::from(rs >> 4);
                let size = rs & 15;
                let mut value: i16 = 0;
                if size != 0 {
                    value = if r.bit()? == 1 { p1 } else { m1 };
                } else if run != 15 {
                    self.eobrun = 1 << run;
                    if run > 0 {
                        self.eobrun += r.bits(run as u32)?;
                    }
                    break;
                }
                while k <= se {
                    let z = ZIGZAG[k];
                    if block[z] != 0 {
                        if r.bit()? == 1 && block[z] & p1 == 0 {
                            block[z] += if block[z] >= 0 { p1 } else { m1 };
                        }
                    } else {
                        run -= 1;
                        if run < 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                if value != 0 && k <= se {
                    block[ZIGZAG[k]] = value;
                }
                k += 1;
            }
        }
        if self.eobrun > 0 {
            while k <= se {
                let z = ZIGZAG[k];
                if block[z] != 0 && r.bit()? == 1 && block[z] & p1 == 0 {
                    block[z] += if block[z] >= 0 { p1 } else { m1 };
                }
                k += 1;
            }
            self.eobrun -= 1;
        }
        Ok(())
    }

    fn output(&self, color_transform: Option<bool>) -> R<DecodedJpeg> {
        let n = self.components.len();
        // Each component's full sample plane (padded to whole blocks).
        let planes: Vec<Vec<u8>> = self
            .components
            .iter()
            .map(|c| {
                let pw = c.blocks_w * 8;
                let mut plane = vec![0u8; pw * c.blocks_h * 8];
                let q = &self.qt[c.tq].0;
                let mut tmp = [0f32; 64];
                for by in 0..c.blocks_h {
                    for bx in 0..c.blocks_w {
                        let coefs = &c.coefs[(by * c.blocks_w + bx) * 64..][..64];
                        idct(coefs, q, &mut tmp);
                        for y in 0..8 {
                            let row = (by * 8 + y) * pw + bx * 8;
                            for x in 0..8 {
                                plane[row + x] =
                                    (tmp[y * 8 + x] + 128.0).round().clamp(0.0, 255.0) as u8;
                            }
                        }
                    }
                }
                plane
            })
            .collect();

        let (w, h) = (self.width, self.height);
        let mut out = vec![0u8; w * h * n];
        for (ci, c) in self.components.iter().enumerate() {
            let pw = c.blocks_w * 8;
            let ph = c.blocks_h * 8;
            let plane = &planes[ci];
            if c.h == self.hmax && c.v == self.vmax {
                for y in 0..h {
                    let src = &plane[y * pw..y * pw + w];
                    for (x, &v) in src.iter().enumerate() {
                        out[(y * w + x) * n + ci] = v;
                    }
                }
                continue;
            }
            // Centred linear upsampling, close to libjpeg's "fancy" mode.
            // Weights are in 1/256ths.
            let taps = |len: usize,
                        factor: usize,
                        max: usize,
                        plane_len: usize|
             -> Vec<(usize, usize, u32)> {
                (0..len)
                    .map(|i| {
                        let f = ((i as f32 + 0.5) * factor as f32 / max as f32 - 0.5).max(0.0);
                        let i0 = (f as usize).min(plane_len - 1);
                        let i1 = (i0 + 1).min(plane_len - 1);
                        (i0, i1, ((f - i0 as f32) * 256.0) as u32)
                    })
                    .collect()
            };
            let xs = taps(w, c.h, self.hmax, pw);
            let ys = taps(h, c.v, self.vmax, ph);
            for (y, &(y0, y1, wy)) in ys.iter().enumerate() {
                let r0 = &plane[y0 * pw..y0 * pw + pw];
                let r1 = &plane[y1 * pw..y1 * pw + pw];
                for (x, &(x0, x1, wx)) in xs.iter().enumerate() {
                    let top = u32::from(r0[x0]) * (256 - wx) + u32::from(r0[x1]) * wx;
                    let bot = u32::from(r1[x0]) * (256 - wx) + u32::from(r1[x1]) * wx;
                    out[(y * w + x) * n + ci] = ((top * (256 - wy) + bot * wy + 32768) >> 16) as u8;
                }
            }
        }

        let transform = match color_transform {
            Some(t) => t && n >= 3,
            None => match (n, self.adobe_transform) {
                (3, Some(t)) => t != 0,
                (3, None) => {
                    // Without Adobe or JFIF markers, ids 'R' 'G' 'B' mean RGB.
                    let ids: Vec<u8> = self.components.iter().map(|c| c.id).collect();
                    self.jfif || ids != b"RGB"
                }
                (4, Some(t)) => t == 2,
                _ => false,
            },
        };
        if transform {
            // Fixed-point YCbCr to RGB (16 fractional bits).
            let clamp = |v: i32| ((v + 32768) >> 16).clamp(0, 255) as u8;
            for px in out.chunks_exact_mut(n) {
                let y = i32::from(px[0]) << 16;
                let cb = i32::from(px[1]) - 128;
                let cr = i32::from(px[2]) - 128;
                let r = clamp(y + 91_881 * cr);
                let g = clamp(y - 22_554 * cb - 46_802 * cr);
                let b = clamp(y + 116_130 * cb);
                if n == 4 {
                    // YCCK: the YCC part becomes inverted RGB, i.e. CMY.
                    px[0] = 255 - r;
                    px[1] = 255 - g;
                    px[2] = 255 - b;
                } else {
                    px[0] = r;
                    px[1] = g;
                    px[2] = b;
                }
            }
        }
        Ok(DecodedJpeg {
            width: w,
            height: h,
            components: n,
            data: out,
        })
    }
}

/// Dequantizes and runs the 8x8 inverse DCT: the floating-point AAN algorithm
/// (as in libjpeg's jidctflt), with all-zero AC rows and blocks short-cut.
fn idct(coefs: &[i16], q: &[u16; 64], out: &mut [f32; 64]) {
    if coefs[1..].iter().all(|&c| c == 0) {
        out.fill(f32::from(coefs[0]) * f32::from(q[0]) / 8.0);
        return;
    }
    let scale = aan_scales();
    let mut ws = [0f32; 64];
    // Columns.
    for x in 0..8 {
        let f = |y: usize| f32::from(coefs[y * 8 + x]) * f32::from(q[y * 8 + x]) * scale[y * 8 + x];
        if (1..8).all(|y| coefs[y * 8 + x] == 0) {
            let dc = f(0);
            for y in 0..8 {
                ws[y * 8 + x] = dc;
            }
            continue;
        }
        let v = idct_1d([f(0), f(1), f(2), f(3), f(4), f(5), f(6), f(7)]);
        for y in 0..8 {
            ws[y * 8 + x] = v[y];
        }
    }
    // Rows.
    for y in 0..8 {
        let r = &ws[y * 8..y * 8 + 8];
        let v = idct_1d([r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7]]);
        for x in 0..8 {
            out[y * 8 + x] = v[x] / 8.0;
        }
    }
}

#[inline]
fn idct_1d(i: [f32; 8]) -> [f32; 8] {
    // Even part.
    let tmp10 = i[0] + i[4];
    let tmp11 = i[0] - i[4];
    let tmp13 = i[2] + i[6];
    let tmp12 = (i[2] - i[6]) * std::f32::consts::SQRT_2 - tmp13;
    let tmp0 = tmp10 + tmp13;
    let tmp3 = tmp10 - tmp13;
    let tmp1 = tmp11 + tmp12;
    let tmp2 = tmp11 - tmp12;
    // Odd part.
    let z13 = i[5] + i[3];
    let z10 = i[5] - i[3];
    let z11 = i[1] + i[7];
    let z12 = i[1] - i[7];
    let tmp7 = z11 + z13;
    let tmp11 = (z11 - z13) * std::f32::consts::SQRT_2;
    let z5 = (z10 + z12) * 1.847_759;
    let tmp10 = 1.082_392_2 * z12 - z5;
    let tmp12 = -2.613_126 * z10 + z5;
    let tmp6 = tmp12 - tmp7;
    let tmp5 = tmp11 - tmp6;
    let tmp4 = tmp10 + tmp5;
    [
        tmp0 + tmp7,
        tmp1 + tmp6,
        tmp2 + tmp5,
        tmp3 - tmp4,
        tmp3 + tmp4,
        tmp2 - tmp5,
        tmp1 - tmp6,
        tmp0 - tmp7,
    ]
}

/// AAN prescale factors: s[k] = cos(k pi / 16) * sqrt(2) for k > 0, 1 for k = 0,
/// combined for row and column.
fn aan_scales() -> &'static [f32; 64] {
    static TABLE: std::sync::OnceLock<[f32; 64]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let s = |k: usize| {
            if k == 0 {
                1.0
            } else {
                (k as f64 * std::f64::consts::PI / 16.0).cos() * std::f64::consts::SQRT_2
            }
        };
        let mut t = [0f32; 64];
        for y in 0..8 {
            for x in 0..8 {
                t[y * 8 + x] = (s(y) * s(x)) as f32;
            }
        }
        t
    })
}

/// The position of the next marker that is not a restart marker.
fn next_marker(data: &[u8], mut pos: usize) -> usize {
    while pos + 1 < data.len() {
        if data[pos] == 0xFF && data[pos + 1] != 0 && !(0xD0..=0xD7).contains(&data[pos + 1]) {
            return pos;
        }
        pos += 1;
    }
    data.len()
}

struct Huffman {
    /// Indexed by the next 9 bits: (length, symbol), length 0 if longer.
    fast: Box<[(u8, u8); 512]>,
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    symbols: Vec<u8>,
}

impl Huffman {
    fn new(counts: &[u8; 16], symbols: &[u8]) -> R<Self> {
        let mut fast = Box::new([(0u8, 0u8); 512]);
        let mut maxcode = [-1i32; 18];
        let mut valptr = [0i32; 17];
        let mut mincode = [0i32; 17];
        let mut code = 0i32;
        let mut k = 0usize;
        for len in 1..=16usize {
            let n = usize::from(counts[len - 1]);
            valptr[len] = k as i32;
            mincode[len] = code;
            for _ in 0..n {
                if len <= 9 {
                    let shift = 9 - len;
                    let base = (code as usize) << shift;
                    for fill in 0..(1usize << shift) {
                        if let Some(slot) = fast.get_mut(base + fill) {
                            *slot = (len as u8, symbols[k]);
                        }
                    }
                }
                code += 1;
                k += 1;
            }
            if n > 0 {
                maxcode[len] = code - 1;
            }
            if code > (1 << len) {
                return Err("bad Huffman table");
            }
            code <<= 1;
        }
        maxcode[17] = i32::MAX;
        Ok(Self {
            fast,
            maxcode,
            valptr,
            mincode,
            symbols: symbols.to_vec(),
        })
    }

    fn decode(&self, r: &mut BitReader) -> R<u8> {
        r.fill();
        if r.bits >= 9 {
            let peek = (r.buf >> (r.bits - 9)) & 0x1FF;
            let (len, sym) = self.fast[peek as usize];
            if len > 0 {
                r.bits -= u32::from(len);
                return Ok(sym);
            }
        }
        let mut code = 0i32;
        for len in 1..=16 {
            code = code << 1 | r.bit()? as i32;
            if code <= self.maxcode[len] {
                let idx = self.valptr[len] + code - self.mincode[len];
                return self
                    .symbols
                    .get(idx as usize)
                    .copied()
                    .ok_or("bad Huffman code");
            }
        }
        Err("bad Huffman code")
    }
}

/// MSB-first bit reader over entropy-coded data, undoing 0xFF00 stuffing and
/// stopping (feeding zeros) at a marker.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    bits: u32,
    marker: bool,
}

impl BitReader<'_> {
    fn fill(&mut self) {
        while self.bits <= 56 {
            let byte = if self.marker || self.pos >= self.data.len() {
                0
            } else if self.data[self.pos] == 0xFF {
                match self.data.get(self.pos + 1) {
                    Some(0) => {
                        self.pos += 2;
                        0xFF
                    }
                    _ => {
                        self.marker = true;
                        0
                    }
                }
            } else {
                self.pos += 1;
                self.data[self.pos - 1]
            };
            self.buf = self.buf << 8 | u64::from(byte);
            self.bits += 8;
        }
    }

    fn bits(&mut self, n: u32) -> R<u32> {
        if n == 0 {
            return Ok(0);
        }
        if n > 16 {
            return Err("bad bit count");
        }
        self.fill();
        self.bits -= n;
        Ok(((self.buf >> self.bits) & ((1 << n) - 1)) as u32)
    }

    fn bit(&mut self) -> R<u32> {
        self.bits(1)
    }

    /// Reads `size` bits and sign-extends them (T.81 F.2.2.1).
    fn receive_extend(&mut self, size: u8) -> R<i32> {
        if size == 0 {
            return Ok(0);
        }
        if size > 16 {
            return Err("bad coefficient size");
        }
        let v = self.bits(u32::from(size))? as i32;
        Ok(if v < 1 << (size - 1) {
            v - (1 << size) + 1
        } else {
            v
        })
    }

    /// Skips to just after the next RSTn marker and resets the bit buffer.
    fn restart(&mut self) {
        self.buf = 0;
        self.bits = 0;
        self.marker = false;
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF && (0xD0..=0xD7).contains(&self.data[self.pos + 1]) {
                self.pos += 2;
                return;
            }
            if self.data[self.pos] == 0xFF
                && self.data[self.pos + 1] != 0
                && self.data[self.pos + 1] != 0xFF
            {
                return; // some other marker: let decoding run out
            }
            self.pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The textbook 2D inverse DCT, as a reference.
    fn reference(coefs: &[i16; 64]) -> [f32; 64] {
        let mut out = [0f32; 64];
        let c = |u: usize| {
            if u == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.0
            }
        };
        for y in 0..8 {
            for x in 0..8 {
                let mut s = 0.0f64;
                for v in 0..8 {
                    for u in 0..8 {
                        s += c(u)
                            * c(v)
                            * f64::from(coefs[v * 8 + u])
                            * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos()
                            * ((2 * y + 1) as f64 * v as f64 * std::f64::consts::PI / 16.0).cos();
                    }
                }
                out[y * 8 + x] = (s / 4.0) as f32;
            }
        }
        out
    }

    #[test]
    fn fast_idct_matches_reference() {
        let mut state = 12345u32;
        let q = [1u16; 64];
        for round in 0..200 {
            let mut coefs = [0i16; 64];
            for (k, c) in coefs.iter_mut().enumerate() {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                // Sparse, like real blocks, with large low frequencies.
                if k < 10 || state % 4 == 0 || round % 10 == 0 {
                    *c = ((state >> 16) % 512) as i16 - 256;
                }
            }
            let mut fast = [0f32; 64];
            idct(&coefs, &q, &mut fast);
            let slow = reference(&coefs);
            for k in 0..64 {
                assert!(
                    (fast[k] - slow[k]).abs() < 0.05,
                    "round {round} k {k}: {} vs {}",
                    fast[k],
                    slow[k]
                );
            }
        }
    }
}
