//! Decoding one tile: its geometry (annex B), packets (tier 2), code-blocks
//! (tier 1), dequantization (annex E) and the inverse wavelet transform.

use super::codestream::{CompStyle, Size, TileParams, style};
use super::dwt::{Level, Rect, Sample, inverse_level};
use super::t1::{self, Block, Segment};

/// Refuse tiles that would need more precincts or code-blocks than this.
const MAX_UNITS: u64 = 1 << 22;

fn ceil_div(a: u64, b: u64) -> u64 {
    a.div_ceil(b)
}

fn ceil_shift(a: u32, n: u32) -> u32 {
    if n >= 32 {
        u32::from(a > 0)
    } else {
        ((u64::from(a) + (1u64 << n) - 1) >> n) as u32
    }
}

/// A tag tree (B.10.2) over a grid of code-blocks.
struct TagTree {
    /// (width, offset) of each level, leaves first.
    levels: Vec<(usize, usize)>,
    value: Vec<u32>,
    low: Vec<u32>,
}

impl TagTree {
    fn new(w: usize, h: usize) -> Self {
        let mut levels = Vec::new();
        let (mut lw, mut lh, mut total) = (w.max(1), h.max(1), 0);
        loop {
            levels.push((lw, total));
            total += lw * lh;
            if lw == 1 && lh == 1 {
                break;
            }
            lw = lw.div_ceil(2);
            lh = lh.div_ceil(2);
        }
        Self {
            levels,
            value: vec![u32::MAX; total],
            low: vec![0; total],
        }
    }

    /// Whether the leaf's value is below `threshold`, reading as few bits
    /// as needed to tell.
    fn decode(&mut self, bits: &mut Bits, x: usize, y: usize, threshold: u32) -> bool {
        let mut low = 0;
        let mut leaf = 0;
        for k in (0..self.levels.len()).rev() {
            let (w, offset) = self.levels[k];
            let i = offset + (y >> k) * w + (x >> k);
            if low > self.low[i] {
                self.low[i] = low;
            } else {
                low = self.low[i];
            }
            while low < threshold && low < self.value[i] {
                if bits.bit() == 1 {
                    self.value[i] = low;
                } else {
                    low += 1;
                }
            }
            self.low[i] = low;
            leaf = i;
        }
        self.value[leaf] < threshold
    }

    /// The leaf's value in full.
    fn value(&mut self, bits: &mut Bits, x: usize, y: usize) -> u32 {
        let mut t = 1;
        while !self.decode(bits, x, y, t) {
            t += 1;
            if t > 64 || bits.overrun() {
                break;
            }
        }
        self.value[self.levels[0].1 + y * self.levels[0].0 + x].min(64)
    }
}

/// Packet header bits (B.10.1): after a 0xFF byte, the next carries 7.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u32,
    ct: u32,
}

impl Bits<'_> {
    fn bit(&mut self) -> u32 {
        if self.ct == 0 {
            let stuffed = self.buf == 0xff;
            self.buf = u32::from(self.d.get(self.pos).copied().unwrap_or(0));
            self.pos += 1;
            self.ct = if stuffed { 7 } else { 8 };
        }
        self.ct -= 1;
        (self.buf >> self.ct) & 1
    }

    fn bits(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |v, _| (v << 1) | self.bit())
    }

    fn overrun(&self) -> bool {
        self.pos > self.d.len()
    }

    fn align(&mut self) {
        // A 0xFF byte is always followed by one with a stuffed bit, which
        // still belongs to the header.
        if self.buf == 0xff {
            self.pos += 1;
        }
        self.ct = 0;
        self.buf = 0;
    }
}

/// A code-block a packet includes: band, block, and (segment, bytes) for
/// each segment its data adds to.
type Inclusion = (usize, usize, Vec<(usize, usize)>);

struct CodeBlock {
    rect: Rect,
    included: bool,
    zero_planes: u32,
    lblock: u32,
    data: Vec<u8>,
    /// (bytes, passes) of each terminated segment.
    segments: Vec<(usize, u32)>,
}

struct Band {
    orient: u8,
    rect: Rect,
    cb_w: u32,
    cb_h: u32,
    /// First code-block column and row of the grid, and its size.
    grid_x0: u32,
    grid_y0: u32,
    grid_w: usize,
    blocks: Vec<CodeBlock>,
    /// Magnitude bit planes (Mb) and step size.
    planes: u32,
    delta: f32,
}

/// One band's share of a precinct: a window on the band's code-blocks.
struct PrecinctBand {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    inclusion: TagTree,
    zero_planes: TagTree,
}

struct Resolution {
    rect: Rect,
    ppx: u32,
    ppy: u32,
    pw: u32,
    ph: u32,
    bands: Vec<Band>,
    precincts: Vec<Vec<PrecinctBand>>,
    /// Layers decoded so far for each precinct.
    layers_done: Vec<u32>,
}

pub struct TileComponent {
    pub rect: Rect,
    style: CompStyle,
    res: Vec<Resolution>,
}

/// Decoded samples of one tile-component, before level shifting.
pub enum Samples {
    Int(Vec<i32>),
    Float(Vec<f32>),
}

pub struct Tile {
    /// On the reference grid.
    pub rect: Rect,
    pub params: TileParams,
    pub comps: Vec<TileComponent>,
}

impl Tile {
    pub fn new(size: &Size, index: u32, params: TileParams) -> Result<Tile, String> {
        let (p, q) = (index % size.tiles_across(), index / size.tiles_across());
        let tx = |p: u32| u64::from(size.tile_x0) + u64::from(p) * u64::from(size.tile_w);
        let ty = |q: u32| u64::from(size.tile_y0) + u64::from(q) * u64::from(size.tile_h);
        let rect = Rect {
            x0: tx(p).max(u64::from(size.x0)) as u32,
            y0: ty(q).max(u64::from(size.y0)) as u32,
            x1: tx(p + 1).min(u64::from(size.x1)) as u32,
            y1: ty(q + 1).min(u64::from(size.y1)) as u32,
        };
        let mut units = 0u64;
        let mut comps = Vec::with_capacity(size.components.len());
        for (c, info) in size.components.iter().enumerate() {
            let style = params.styles[c].clone();
            let quant = &params.quants[c];
            let tc = Rect {
                x0: ceil_div(u64::from(rect.x0), u64::from(info.dx)) as u32,
                y0: ceil_div(u64::from(rect.y0), u64::from(info.dy)) as u32,
                x1: ceil_div(u64::from(rect.x1), u64::from(info.dx)) as u32,
                y1: ceil_div(u64::from(rect.y1), u64::from(info.dy)) as u32,
            };
            let levels = style.levels;
            let mut res = Vec::with_capacity(levels as usize + 1);
            for r in 0..=levels {
                let n = levels - r;
                let rr = Rect {
                    x0: ceil_shift(tc.x0, n),
                    y0: ceil_shift(tc.y0, n),
                    x1: ceil_shift(tc.x1, n),
                    y1: ceil_shift(tc.y1, n),
                };
                let (ppx, ppy) = style.precinct(r as usize);
                let count = |a0: u32, a1: u32, e: u32| -> u32 {
                    if a1 <= a0 {
                        0
                    } else {
                        (ceil_div(u64::from(a1), 1 << e) - (u64::from(a0) >> e)) as u32
                    }
                };
                let (pw, ph) = (count(rr.x0, rr.x1, ppx), count(rr.y0, rr.y1, ppy));
                // Code-blocks never cross precinct boundaries.
                let sub = u32::from(r > 0);
                let cb_w = style.cb_w.min(ppx.saturating_sub(sub));
                let cb_h = style.cb_h.min(ppy.saturating_sub(sub));
                let kinds: &[(u8, u32, u32)] = if r == 0 {
                    &[(t1::LL, 0, 0)]
                } else {
                    &[(t1::HL, 1, 0), (t1::LH, 0, 1), (t1::HH, 1, 1)]
                };
                let mut bands = Vec::new();
                for (k, &(orient, xob, yob)) in kinds.iter().enumerate() {
                    let rect = if r == 0 {
                        rr
                    } else {
                        let half = |a: u32, ob: u32| if ob == 1 { a / 2 } else { a.div_ceil(2) };
                        Rect {
                            x0: half(rr.x0, xob),
                            y0: half(rr.y0, yob),
                            x1: half(rr.x1, xob),
                            y1: half(rr.y1, yob),
                        }
                    };
                    let index = if r == 0 {
                        0
                    } else {
                        1 + 3 * (r as usize - 1) + k
                    };
                    let (exp, mant) = quant.step(index, levels);
                    let gain = match orient {
                        t1::LL => 0,
                        t1::HH => 2,
                        _ => 1,
                    };
                    let range = info.precision as i32 + gain - exp as i32;
                    let delta = 2f32.powi(range) * (1.0 + mant as f32 / 2048.0);
                    let grid_x0 = rect.x0 >> cb_w;
                    let grid_y0 = rect.y0 >> cb_h;
                    let (gw, gh) = if rect.width() == 0 || rect.height() == 0 {
                        (0, 0)
                    } else {
                        (
                            (ceil_shift(rect.x1, cb_w) - grid_x0) as usize,
                            (ceil_shift(rect.y1, cb_h) - grid_y0) as usize,
                        )
                    };
                    units += (gw * gh) as u64;
                    if units > MAX_UNITS {
                        return Err("too many code-blocks".into());
                    }
                    let mut blocks = Vec::with_capacity(gw * gh);
                    for by in 0..gh as u32 {
                        for bx in 0..gw as u32 {
                            let x0 = (grid_x0 + bx) << cb_w;
                            let y0 = (grid_y0 + by) << cb_h;
                            blocks.push(CodeBlock {
                                rect: Rect {
                                    x0: x0.max(rect.x0),
                                    y0: y0.max(rect.y0),
                                    x1: (x0 + (1 << cb_w)).min(rect.x1),
                                    y1: (y0 + (1 << cb_h)).min(rect.y1),
                                },
                                included: false,
                                zero_planes: 0,
                                lblock: 3,
                                data: Vec::new(),
                                segments: Vec::new(),
                            });
                        }
                    }
                    bands.push(Band {
                        orient,
                        rect,
                        cb_w,
                        cb_h,
                        grid_x0,
                        grid_y0,
                        grid_w: gw,
                        blocks,
                        planes: (quant.guard + exp).saturating_sub(1),
                        delta,
                    });
                }
                units += u64::from(pw) * u64::from(ph);
                if units > MAX_UNITS {
                    return Err("too many precincts".into());
                }
                let mut precincts = Vec::with_capacity((pw * ph) as usize);
                let (px0, py0) = (rr.x0 >> ppx, rr.y0 >> ppy);
                for j in 0..ph {
                    for i in 0..pw {
                        let mut parts = Vec::with_capacity(bands.len());
                        for b in &bands {
                            // The precinct in band coordinates.
                            let (ex, ey) = (ppx - sub.min(ppx), ppy - sub.min(ppy));
                            let x0 = u64::from(px0 + i) << ex;
                            let y0 = u64::from(py0 + j) << ey;
                            let x1 = (x0 + (1 << ex)).min(u64::from(b.rect.x1));
                            let y1 = (y0 + (1 << ey)).min(u64::from(b.rect.y1));
                            let x0 = x0.max(u64::from(b.rect.x0));
                            let y0 = y0.max(u64::from(b.rect.y0));
                            let (cx0, cy0, w, h) = if x1 <= x0 || y1 <= y0 {
                                (0, 0, 0, 0)
                            } else {
                                let cx0 = (x0 >> b.cb_w) as u32;
                                let cy0 = (y0 >> b.cb_h) as u32;
                                let cx1 = ceil_div(x1, 1 << b.cb_w) as u32;
                                let cy1 = ceil_div(y1, 1 << b.cb_h) as u32;
                                (
                                    (cx0 - b.grid_x0) as usize,
                                    (cy0 - b.grid_y0) as usize,
                                    (cx1 - cx0) as usize,
                                    (cy1 - cy0) as usize,
                                )
                            };
                            parts.push(PrecinctBand {
                                x0: cx0,
                                y0: cy0,
                                w,
                                h,
                                inclusion: TagTree::new(w, h),
                                zero_planes: TagTree::new(w, h),
                            });
                        }
                        precincts.push(parts);
                    }
                }
                res.push(Resolution {
                    rect: rr,
                    ppx,
                    ppy,
                    pw,
                    ph,
                    bands,
                    layers_done: vec![0; precincts.len()],
                    precincts,
                });
            }
            comps.push(TileComponent {
                rect: tc,
                style,
                res,
            });
        }
        Ok(Tile {
            rect,
            params,
            comps,
        })
    }

    /// Reads every packet in progression order. Returns a warning when the
    /// data ran out early.
    pub fn read_packets(
        &mut self,
        size: &Size,
        data: &[u8],
        headers: Option<&[u8]>,
    ) -> Option<String> {
        let mut body = 0usize;
        let mut hdr = Bits {
            d: headers.unwrap_or(&[]),
            pos: 0,
            buf: 0,
            ct: 0,
        };
        let separate = headers.is_some();
        let order = self.params.order;
        let ncomp = self.comps.len() as u32;
        let max_res = self
            .comps
            .iter()
            .map(|c| c.style.levels + 1)
            .max()
            .unwrap_or(1);
        let mut volumes: Vec<(u32, u32, u32, u32, u32, u8)> = self
            .params
            .poc
            .iter()
            .map(|p| {
                (
                    p.res_start,
                    p.comp_start,
                    p.layer_end.min(order.layers),
                    p.res_end.min(max_res),
                    p.comp_end.min(ncomp),
                    p.progression,
                )
            })
            .collect();
        if volumes.is_empty() {
            volumes.push((0, 0, order.layers, max_res, ncomp, order.progression));
        }
        let mut ended = None;
        for (rs, cs, le, re, ce, prog) in volumes {
            let mut packets = Vec::new();
            self.sequence(size, (rs, cs, le, re, ce, prog), &mut packets);
            for (l, r, c, p) in packets {
                let done = &mut self.comps[c as usize].res[r as usize].layers_done[p];
                if l < *done {
                    continue;
                }
                *done = l + 1;
                let at_end = if separate {
                    hdr.pos >= hdr.d.len()
                } else {
                    body >= data.len()
                };
                if at_end {
                    ended = Some("the tile's data ends early".to_string());
                    break;
                }
                if !separate {
                    hdr = Bits {
                        d: data,
                        pos: body,
                        buf: 0,
                        ct: 0,
                    };
                }
                self.packet(
                    c as usize, r as usize, p, l, data, &mut body, &mut hdr, separate,
                );
            }
            if ended.is_some() {
                break;
            }
        }
        ended
    }

    /// The (layer, resolution, component, precinct) order of one
    /// progression volume (B.12).
    fn sequence(
        &self,
        size: &Size,
        v: (u32, u32, u32, u32, u32, u8),
        out: &mut Vec<(u32, u32, u32, usize)>,
    ) {
        let (rs, cs, le, re, ce, prog) = v;
        let nres = |c: u32| self.comps[c as usize].res.len() as u32;
        let nprec = |c: u32, r: u32| {
            let res = &self.comps[c as usize].res[r as usize];
            res.precincts.len()
        };
        match prog {
            0 => {
                for l in 0..le {
                    for r in rs..re {
                        for c in cs..ce {
                            if r < nres(c) {
                                for p in 0..nprec(c, r) {
                                    out.push((l, r, c, p));
                                }
                            }
                        }
                    }
                }
            }
            1 => {
                for r in rs..re {
                    for l in 0..le {
                        for c in cs..ce {
                            if r < nres(c) {
                                for p in 0..nprec(c, r) {
                                    out.push((l, r, c, p));
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                // Position-driven orders step over the reference grid.
                let mut step_x = u64::MAX;
                let mut step_y = u64::MAX;
                for c in cs..ce {
                    let info = size.components[c as usize];
                    for r in rs..re.min(nres(c)) {
                        let res = &self.comps[c as usize].res[r as usize];
                        let level = nres(c) - 1 - r;
                        let sx = u64::from(res.ppx + level);
                        let sy = u64::from(res.ppy + level);
                        if sx < 48 {
                            step_x = step_x.min(u64::from(info.dx) << sx);
                        }
                        if sy < 48 {
                            step_y = step_y.min(u64::from(info.dy) << sy);
                        }
                    }
                }
                if step_x == u64::MAX || step_y == u64::MAX {
                    return;
                }
                let t = self.rect;
                let ys: Vec<u64> = steps(u64::from(t.y0), u64::from(t.y1), step_y);
                let xs: Vec<u64> = steps(u64::from(t.x0), u64::from(t.x1), step_x);
                let precinct_at = |c: u32, r: u32, x: u64, y: u64| -> Option<usize> {
                    if r >= nres(c) {
                        return None;
                    }
                    let info = size.components[c as usize];
                    let res = &self.comps[c as usize].res[r as usize];
                    let level = u64::from(nres(c) - 1 - r);
                    let (rpx, rpy) = (u64::from(res.ppx) + level, u64::from(res.ppy) + level);
                    if rpx >= 48 || rpy >= 48 || res.pw == 0 || res.ph == 0 {
                        return None;
                    }
                    let (rx0, ry0) = (u64::from(res.rect.x0), u64::from(res.rect.y0));
                    let ok_y = y % (u64::from(info.dy) << rpy) == 0
                        || (y == u64::from(t.y0) && (ry0 << level) % (1 << rpy) != 0);
                    let ok_x = x % (u64::from(info.dx) << rpx) == 0
                        || (x == u64::from(t.x0) && (rx0 << level) % (1 << rpx) != 0);
                    if !ok_x || !ok_y {
                        return None;
                    }
                    let i =
                        (ceil_div(x, u64::from(info.dx) << level) >> res.ppx) - (rx0 >> res.ppx);
                    let j =
                        (ceil_div(y, u64::from(info.dy) << level) >> res.ppy) - (ry0 >> res.ppy);
                    if i >= u64::from(res.pw) || j >= u64::from(res.ph) {
                        return None;
                    }
                    Some((i + j * u64::from(res.pw)) as usize)
                };
                let mut emit = |c: u32, r: u32, x: u64, y: u64| {
                    if let Some(p) = precinct_at(c, r, x, y) {
                        for l in 0..le {
                            out.push((l, r, c, p));
                        }
                    }
                };
                match prog {
                    2 => {
                        for r in rs..re {
                            for &y in &ys {
                                for &x in &xs {
                                    for c in cs..ce {
                                        emit(c, r, x, y);
                                    }
                                }
                            }
                        }
                    }
                    3 => {
                        for &y in &ys {
                            for &x in &xs {
                                for c in cs..ce {
                                    for r in rs..re {
                                        emit(c, r, x, y);
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        for c in cs..ce {
                            for &y in &ys {
                                for &x in &xs {
                                    for r in rs..re {
                                        emit(c, r, x, y);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Reads one packet (B.9, B.10).
    #[allow(clippy::too_many_arguments)]
    fn packet(
        &mut self,
        c: usize,
        r: usize,
        p: usize,
        layer: u32,
        data: &[u8],
        body: &mut usize,
        hdr: &mut Bits,
        separate: bool,
    ) {
        // An SOP marker segment may precede the packet.
        if data.get(*body..*body + 2) == Some(&[0xff, 0x91]) {
            *body += 6;
            if !separate {
                hdr.pos = *body;
            }
        }
        let cb_style = self.comps[c].style.cb_style;
        let res = &mut self.comps[c].res[r];
        // (band, block, bytes for each segment touched)
        let mut included: Vec<Inclusion> = Vec::new();
        if hdr.bit() == 1 {
            for (b, part) in res.precincts[p].iter_mut().enumerate() {
                let band = &mut res.bands[b];
                for y in 0..part.h {
                    for x in 0..part.w {
                        let k = (part.y0 + y) * band.grid_w + part.x0 + x;
                        let cb = &mut band.blocks[k];
                        let first = !cb.included;
                        let inc = if first {
                            part.inclusion.decode(hdr, x, y, layer + 1)
                        } else {
                            hdr.bit() == 1
                        };
                        if !inc {
                            continue;
                        }
                        if first {
                            cb.included = true;
                            cb.zero_planes = part.zero_planes.value(hdr, x, y);
                        }
                        let passes = read_pass_count(hdr);
                        while hdr.bit() == 1 {
                            cb.lblock += 1;
                            if cb.lblock > 32 {
                                break;
                            }
                        }
                        let mut lengths = Vec::new();
                        let mut left = passes;
                        while left > 0 {
                            let max = max_passes(cb_style, cb.segments.len().saturating_sub(1));
                            let room = match cb.segments.last() {
                                Some(&(_, n)) if n < max => max - n,
                                _ => {
                                    cb.segments.push((0, 0));
                                    max_passes(cb_style, cb.segments.len() - 1)
                                }
                            };
                            let n = left.min(room);
                            let bits = cb.lblock + (31 - n.leading_zeros());
                            let len = hdr.bits(bits.min(32)) as usize;
                            let seg = cb.segments.len() - 1;
                            cb.segments[seg].1 += n;
                            lengths.push((seg, len));
                            left -= n;
                        }
                        included.push((b, k, lengths));
                    }
                }
            }
        }
        hdr.align();
        // An EPH marker may follow the header.
        if hdr.d.get(hdr.pos..hdr.pos + 2) == Some(&[0xff, 0x92]) {
            hdr.pos += 2;
        }
        if !separate {
            *body = hdr.pos;
        }
        for (b, k, segs) in included {
            let cb = &mut res.bands[b].blocks[k];
            for (seg, len) in segs {
                let start = (*body).min(data.len());
                let end = body.saturating_add(len).min(data.len());
                cb.data.extend_from_slice(&data[start..end]);
                cb.segments[seg].0 += end - start;
                *body = body.saturating_add(len);
            }
        }
    }

    /// Decodes every code-block and runs the inverse transform, giving each
    /// component's samples over its tile-component rectangle. Code-blocks,
    /// then components, are spread over the available cores.
    pub fn reconstruct(&self, roi: &[u32]) -> Vec<Samples> {
        let mut jobs = Vec::new();
        let mut area = 0;
        for (c, tc) in self.comps.iter().enumerate() {
            for (r, res) in tc.res.iter().enumerate() {
                for (b, band) in res.bands.iter().enumerate() {
                    for (k, cb) in band.blocks.iter().enumerate() {
                        if cb.segments.iter().any(|s| s.1 > 0) {
                            jobs.push((c, r, b, k));
                            area += cb.rect.width() * cb.rect.height();
                        }
                    }
                }
            }
        }
        let decoded = parallel_map(&jobs, area, Block::default, |block, &(c, r, b, k)| {
            let tc = &self.comps[c];
            let shift = roi.get(c).copied().unwrap_or(0);
            decode_block(&tc.res[r].bands[b], k, block, tc.style.cb_style, shift)
        });
        // Each component's bands, by resolution.
        let mut bands: Vec<Vec<Vec<Vec<i32>>>> = self
            .comps
            .iter()
            .map(|tc| {
                tc.res
                    .iter()
                    .map(|res| {
                        res.bands
                            .iter()
                            .map(|b| vec![0; b.rect.width() * b.rect.height()])
                            .collect()
                    })
                    .collect()
            })
            .collect();
        for (&(c, r, b, k), values) in jobs.iter().zip(decoded) {
            let band = &self.comps[c].res[r].bands[b];
            let rect = band.blocks[k].rect;
            let (w, bw) = (band.rect.width(), rect.width());
            let ox = (rect.x0 - band.rect.x0) as usize;
            let oy = (rect.y0 - band.rect.y0) as usize;
            let out = &mut bands[c][r][b];
            for (y, row) in values.chunks_exact(bw.max(1)).enumerate() {
                out[(oy + y) * w + ox..(oy + y) * w + ox + bw].copy_from_slice(row);
            }
        }
        let comps: Vec<usize> = (0..self.comps.len()).collect();
        let samples: usize = self
            .comps
            .iter()
            .map(|c| c.rect.width() * c.rect.height())
            .sum();
        parallel_map(
            &comps,
            samples,
            || (),
            |_, &c| {
                let tc = &self.comps[c];
                let bands = &bands[c];
                if tc.style.reversible {
                    let b: Vec<Vec<Vec<i32>>> = bands
                        .iter()
                        .map(|r| {
                            r.iter()
                                .map(|b| b.iter().map(|v| v / 2).collect())
                                .collect()
                        })
                        .collect();
                    Samples::Int(rebuild(&tc.res, b))
                } else {
                    let b: Vec<Vec<Vec<f32>>> = bands
                        .iter()
                        .zip(&tc.res)
                        .map(|(r, res)| {
                            r.iter()
                                .zip(&res.bands)
                                .map(|(b, band)| {
                                    let scale = band.delta / 2.0;
                                    b.iter().map(|&v| v as f32 * scale).collect()
                                })
                                .collect()
                        })
                        .collect();
                    Samples::Float(rebuild(&tc.res, b))
                }
            },
        )
    }
}

/// Runs `work` over `items` on up to every core (when `cost`, in samples,
/// makes threads worth starting), each thread with its own `init()` state.
/// Results keep the items' order.
fn parallel_map<T: Sync, S, R: Send>(
    items: &[T],
    cost: usize,
    init: impl Fn() -> S + Sync,
    work: impl Fn(&mut S, &T) -> R + Sync,
) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = cores.min(items.len()).min(cost / (1 << 15)).max(1);
    if threads == 1 {
        let mut state = init();
        return items.iter().map(|t| work(&mut state, t)).collect();
    }
    let next = AtomicUsize::new(0);
    let mut results: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut state = init();
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        out.push((i, work(&mut state, item)));
                    }
                    out
                })
            })
            .collect();
        for h in handles {
            match h.join() {
                Ok(done) => {
                    for (i, r) in done {
                        results[i] = Some(r);
                    }
                }
                Err(e) => std::panic::resume_unwind(e),
            }
        }
    });
    results.into_iter().flatten().collect()
}

fn steps(from: u64, to: u64, step: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut v = from;
    while v < to {
        out.push(v);
        v += step - v % step;
    }
    out
}

/// The number of new coding passes (table B.4).
fn read_pass_count(b: &mut Bits) -> u32 {
    if b.bit() == 0 {
        return 1;
    }
    if b.bit() == 0 {
        return 2;
    }
    let v = b.bits(2);
    if v < 3 {
        return 3 + v;
    }
    let v = b.bits(5);
    if v < 31 {
        return 6 + v;
    }
    37 + b.bits(7)
}

/// The most passes segment `index` of a code-block can hold.
fn max_passes(cb_style: u8, index: usize) -> u32 {
    if cb_style & style::TERMALL != 0 {
        1
    } else if cb_style & style::BYPASS != 0 {
        match index {
            0 => 10,
            i if i % 2 == 1 => 2,
            _ => 1,
        }
    } else {
        u32::MAX
    }
}

/// Decodes code-block `k` of a band into doubled coefficients.
fn decode_block(band: &Band, k: usize, block: &mut Block, cb_style: u8, roi: u32) -> Vec<i32> {
    let cb = &band.blocks[k];
    let mut segments = Vec::with_capacity(cb.segments.len());
    let mut at = 0;
    for &(len, passes) in &cb.segments {
        let end = (at + len).min(cb.data.len());
        segments.push(Segment {
            data: &cb.data[at..end],
            passes,
        });
        at = end;
    }
    let planes = (band.planes + roi).saturating_sub(cb.zero_planes);
    block.decode(
        cb.rect.width(),
        cb.rect.height(),
        band.orient,
        cb_style,
        planes,
        &segments,
    );
    let mut out = block.data.clone();
    if roi > 0 {
        // Max-shift ROI: coefficients at or above 2^roi were scaled up.
        for v in &mut out {
            let m = v.unsigned_abs();
            let m = if roi < 31 && m >= 2u32 << roi {
                m >> roi
            } else {
                m
            };
            *v = if *v < 0 { -(m as i32) } else { m as i32 };
        }
    }
    out
}

/// Runs the inverse transform from the lowest resolution up.
fn rebuild<T: Sample>(res: &[Resolution], mut bands: Vec<Vec<Vec<T>>>) -> Vec<T> {
    let mut ll = std::mem::take(&mut bands[0][0]);
    for r in 1..res.len() {
        let b = &bands[r];
        ll = inverse_level(
            &ll,
            res[r - 1].rect,
            &Level {
                rect: res[r].rect,
                hl: &b[0],
                lh: &b[1],
                hh: &b[2],
            },
        );
    }
    ll
}
