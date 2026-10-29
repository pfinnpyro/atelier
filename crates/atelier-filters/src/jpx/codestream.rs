//! Codestream syntax (T.800 annex A): markers, the main header, and the
//! tile-parts that carry each tile's packets.

/// One component's place on the reference grid (SIZ). Signed and unsigned
/// samples are both shown shifted to be unsigned, so the sign is not kept.
#[derive(Debug, Clone, Copy)]
pub struct Component {
    pub precision: u32,
    pub dx: u32,
    pub dy: u32,
}

/// The image and tile geometry (SIZ).
#[derive(Debug, Clone)]
pub struct Size {
    pub x1: u32,
    pub y1: u32,
    pub x0: u32,
    pub y0: u32,
    pub tile_w: u32,
    pub tile_h: u32,
    pub tile_x0: u32,
    pub tile_y0: u32,
    pub components: Vec<Component>,
}

impl Size {
    pub fn tiles_across(&self) -> u32 {
        (self.x1 - self.tile_x0).div_ceil(self.tile_w)
    }

    pub fn tiles_down(&self) -> u32 {
        (self.y1 - self.tile_y0).div_ceil(self.tile_h)
    }
}

/// Code-block coding style bits (table A.19).
pub mod style {
    pub const BYPASS: u8 = 0x01;
    pub const RESET: u8 = 0x02;
    pub const TERMALL: u8 = 0x04;
    pub const VSC: u8 = 0x08;
    pub const SEGSYM: u8 = 0x20;
    /// Part 15 high-throughput blocks, which this decoder does not handle.
    pub const HT: u8 = 0x40;
}

/// How one component is coded (COD's SPcod or COC's SPcoc).
#[derive(Debug, Clone)]
pub struct CompStyle {
    pub levels: u32,
    pub cb_w: u32,
    pub cb_h: u32,
    pub cb_style: u8,
    pub reversible: bool,
    /// Precinct size exponents per resolution, lowest first.
    pub precincts: Vec<(u32, u32)>,
}

impl CompStyle {
    pub fn precinct(&self, r: usize) -> (u32, u32) {
        self.precincts.get(r).copied().unwrap_or((15, 15))
    }
}

/// The tile-wide part of COD. (SOP and EPH markers are recognised
/// wherever they appear, so their flags are not kept.)
#[derive(Debug, Clone, Copy)]
pub struct Order {
    pub progression: u8,
    pub layers: u32,
    pub mct: bool,
}

/// Quantization (QCD / QCC): style, guard bits, and (exponent, mantissa)
/// per subband in the order LL, then HL LH HH for each level.
#[derive(Debug, Clone)]
pub struct Quant {
    pub style: u8,
    pub guard: u32,
    pub steps: Vec<(u32, u32)>,
}

impl Quant {
    /// The exponent and mantissa for subband `index` of a component with
    /// `levels` decomposition levels (0 = LL; then three per level).
    pub fn step(&self, index: usize, levels: u32) -> (u32, u32) {
        if self.style == 1 {
            // Scalar derived: everything comes from the LL value.
            let (e0, m0) = self.steps.first().copied().unwrap_or((0, 0));
            let n = if index == 0 {
                levels
            } else {
                levels + 1 - (index as u32 - 1) / 3 - 1
            };
            (e0.saturating_sub(levels).saturating_add(n), m0)
        } else {
            self.steps
                .get(index)
                .or(self.steps.last())
                .copied()
                .unwrap_or((0, 0))
        }
    }
}

/// A progression order change (POC).
#[derive(Debug, Clone, Copy)]
pub struct Poc {
    pub res_start: u32,
    pub comp_start: u32,
    pub layer_end: u32,
    pub res_end: u32,
    pub comp_end: u32,
    pub progression: u8,
}

/// Coding parameters from one header (main or tile), each possibly unset.
#[derive(Debug, Clone, Default)]
pub struct Params {
    pub order: Option<Order>,
    pub cod: Option<CompStyle>,
    pub coc: Vec<Option<CompStyle>>,
    pub qcd: Option<Quant>,
    pub qcc: Vec<Option<Quant>>,
    pub roi: Vec<Option<u32>>,
    pub poc: Vec<Poc>,
}

/// The parameters a tile decodes with, after tile headers override the
/// main header.
#[derive(Debug, Clone)]
pub struct TileParams {
    pub order: Order,
    pub styles: Vec<CompStyle>,
    pub quants: Vec<Quant>,
    pub roi: Vec<u32>,
    pub poc: Vec<Poc>,
}

impl Params {
    fn new(n: usize) -> Self {
        Self {
            coc: vec![None; n],
            qcc: vec![None; n],
            roi: vec![None; n],
            ..Self::default()
        }
    }

    /// Tile-part COC beats tile-part COD beats main COC beats main COD (and
    /// the same for quantization).
    pub fn resolve(main: &Params, tile: &Params) -> Result<TileParams, String> {
        let n = main.coc.len();
        let order = tile
            .order
            .or(main.order)
            .ok_or("no COD marker in the main header")?;
        let pick_style = |c: usize| {
            tile.coc[c]
                .clone()
                .or_else(|| tile.cod.clone())
                .or_else(|| main.coc[c].clone())
                .or_else(|| main.cod.clone())
        };
        let pick_quant = |c: usize| {
            tile.qcc[c]
                .clone()
                .or_else(|| tile.qcd.clone())
                .or_else(|| main.qcc[c].clone())
                .or_else(|| main.qcd.clone())
        };
        let mut styles = Vec::with_capacity(n);
        let mut quants = Vec::with_capacity(n);
        for c in 0..n {
            styles.push(pick_style(c).ok_or("no coding style")?);
            quants.push(pick_quant(c).ok_or("no QCD marker")?);
        }
        Ok(TileParams {
            order,
            styles,
            quants,
            roi: (0..n)
                .map(|c| tile.roi[c].or(main.roi[c]).unwrap_or(0))
                .collect(),
            poc: if tile.poc.is_empty() {
                main.poc.clone()
            } else {
                tile.poc.clone()
            },
        })
    }
}

/// One tile's coded data: its own header parameters, the packet data of
/// all its tile-parts, and packed packet headers when PPM or PPT is used.
#[derive(Debug, Default)]
pub struct TileData {
    pub params: Option<Params>,
    pub data: Vec<u8>,
    pub headers: Option<Vec<u8>>,
}

pub struct Codestream {
    pub size: Size,
    pub main: Params,
    pub tiles: Vec<TileData>,
    pub warnings: Vec<String>,
}

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.d.get(self.pos).ok_or("marker segment too short")?;
        self.pos += 1;
        Ok(v)
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from(self.u8()?) << 8 | u16::from(self.u8()?))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from(self.u16()?) << 16 | u32::from(self.u16()?))
    }

    fn left(&self) -> usize {
        self.d.len().saturating_sub(self.pos)
    }
}

fn parse_size(seg: &[u8]) -> Result<Size, String> {
    let mut r = Reader { d: seg, pos: 0 };
    let _caps = r.u16()?;
    let (x1, y1, x0, y0) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
    let (tile_w, tile_h, tile_x0, tile_y0) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
    let n = r.u16()? as usize;
    if n == 0 || n > 16384 {
        return Err(format!("{n} components"));
    }
    let mut components = Vec::with_capacity(n);
    for _ in 0..n {
        let s = r.u8()?;
        let (dx, dy) = (u32::from(r.u8()?), u32::from(r.u8()?));
        if dx == 0 || dy == 0 {
            return Err("component subsampling of 0".into());
        }
        components.push(Component {
            precision: u32::from(s & 0x7f) + 1,
            dx,
            dy,
        });
    }
    if x0 >= x1 || y0 >= y1 || tile_w == 0 || tile_h == 0 {
        return Err("empty image".into());
    }
    if tile_x0 > x0
        || tile_y0 > y0
        || tile_x0.saturating_add(tile_w) <= x0
        || tile_y0.saturating_add(tile_h) <= y0
    {
        return Err("tiles do not cover the image".into());
    }
    if components.iter().any(|c| c.precision > 38) {
        return Err("component precision over 38 bits".into());
    }
    let size = Size {
        x1,
        y1,
        x0,
        y0,
        tile_w,
        tile_h,
        tile_x0,
        tile_y0,
        components,
    };
    if u64::from(size.tiles_across()) * u64::from(size.tiles_down()) > 65535 {
        return Err("more than 65535 tiles".into());
    }
    Ok(size)
}

/// SPcod / SPcoc, after the fields that differ between the two.
fn parse_comp_style(r: &mut Reader, precincts: bool) -> Result<CompStyle, String> {
    let levels = u32::from(r.u8()?);
    let cb_w = u32::from(r.u8()? & 0xf) + 2;
    let cb_h = u32::from(r.u8()? & 0xf) + 2;
    let cb_style = r.u8()?;
    let reversible = r.u8()? == 1;
    if levels > 32 {
        return Err(format!("{levels} decomposition levels"));
    }
    if cb_w > 10 || cb_h > 10 || cb_w + cb_h > 12 {
        return Err("code-block size too large".into());
    }
    let mut sizes = Vec::new();
    if precincts {
        for _ in 0..=levels {
            let v = r.u8()?;
            sizes.push((u32::from(v & 0xf), u32::from(v >> 4)));
        }
    }
    Ok(CompStyle {
        levels,
        cb_w,
        cb_h,
        cb_style,
        reversible,
        precincts: sizes,
    })
}

fn parse_quant(r: &mut Reader) -> Result<Quant, String> {
    let s = r.u8()?;
    let style = s & 0x1f;
    let mut steps = Vec::new();
    match style {
        0 => {
            while r.left() > 0 {
                steps.push((u32::from(r.u8()? >> 3), 0));
            }
        }
        1 | 2 => {
            while r.left() >= 2 {
                let v = r.u16()?;
                steps.push((u32::from(v >> 11), u32::from(v & 0x7ff)));
            }
        }
        _ => return Err(format!("quantization style {style}")),
    }
    if steps.is_empty() {
        return Err("no quantization values".into());
    }
    Ok(Quant {
        style,
        guard: u32::from(s >> 5),
        steps,
    })
}

impl Params {
    /// Applies one marker segment found in a main or tile-part header.
    /// Returns false for markers this level does not use.
    fn apply(&mut self, marker: u8, seg: &[u8], ncomp: usize) -> Result<bool, String> {
        let mut r = Reader { d: seg, pos: 0 };
        let comp = |r: &mut Reader| -> Result<usize, String> {
            let c = if ncomp < 257 {
                usize::from(r.u8()?)
            } else {
                usize::from(r.u16()?)
            };
            if c >= ncomp {
                return Err(format!("component {c} does not exist"));
            }
            Ok(c)
        };
        match marker {
            0x52 => {
                let s = r.u8()?;
                let progression = r.u8()?;
                let layers = u32::from(r.u16()?);
                let mct = r.u8()?;
                if layers == 0 {
                    return Err("zero quality layers".into());
                }
                self.order = Some(Order {
                    progression,
                    layers,
                    mct: mct != 0,
                });
                self.cod = Some(parse_comp_style(&mut r, s & 1 != 0)?);
            }
            0x53 => {
                let c = comp(&mut r)?;
                let s = r.u8()?;
                self.coc[c] = Some(parse_comp_style(&mut r, s & 1 != 0)?);
            }
            0x5c => self.qcd = Some(parse_quant(&mut r)?),
            0x5d => {
                let c = comp(&mut r)?;
                self.qcc[c] = Some(parse_quant(&mut r)?);
            }
            0x5e => {
                let c = comp(&mut r)?;
                let _style = r.u8()?;
                self.roi[c] = Some(u32::from(r.u8()?));
            }
            0x5f => {
                let wide = ncomp >= 257;
                let entry = if wide { 9 } else { 7 };
                while r.left() >= entry {
                    let res_start = u32::from(r.u8()?);
                    let comp_start = if wide { r.u16()? } else { u16::from(r.u8()?) };
                    let layer_end = u32::from(r.u16()?);
                    let res_end = u32::from(r.u8()?);
                    let comp_end = if wide {
                        r.u16()?
                    } else {
                        // 0 means 256 with one-byte component indices.
                        match r.u8()? {
                            0 => 256,
                            v => u16::from(v),
                        }
                    };
                    let progression = r.u8()?;
                    self.poc.push(Poc {
                        res_start,
                        comp_start: u32::from(comp_start),
                        layer_end,
                        res_end,
                        comp_end: u32::from(comp_end),
                        progression,
                    });
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// Splits a codestream into its main header and tiles.
pub fn parse(d: &[u8]) -> Result<Codestream, String> {
    if d.get(..2) != Some(&[0xff, 0x4f]) {
        return Err("no SOC marker".into());
    }
    let mut pos = 2;
    let mut size: Option<Size> = None;
    let mut main = Params::default();
    let mut ppm: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut warnings = Vec::new();
    let segment = |pos: usize| -> Result<(u8, &[u8], usize), String> {
        if d.get(pos) != Some(&0xff) {
            return Err(format!("expected a marker at byte {pos}"));
        }
        let marker = *d.get(pos + 1).ok_or("truncated marker")?;
        let len = usize::from(u16::from_be_bytes([
            *d.get(pos + 2).ok_or("truncated marker")?,
            *d.get(pos + 3).ok_or("truncated marker")?,
        ]));
        if len < 2 {
            return Err("marker segment length below 2".into());
        }
        let body = d
            .get(pos + 4..pos + 2 + len)
            .ok_or("marker segment runs past the end")?;
        Ok((marker, body, pos + 2 + len))
    };
    // Main header, up to the first SOT.
    loop {
        if d.get(pos..pos + 2) == Some(&[0xff, 0x90]) {
            break;
        }
        let (marker, body, next) = segment(pos)?;
        pos = next;
        match marker {
            0x51 => {
                let s = parse_size(body)?;
                main = Params::new(s.components.len());
                size = Some(s);
            }
            0x60 => {
                if let Some((&z, rest)) = body.split_first() {
                    ppm.push((z, rest.to_vec()));
                }
            }
            _ => {
                let n = size
                    .as_ref()
                    .ok_or("SIZ is not the first marker")?
                    .components
                    .len();
                main.apply(marker, body, n)?;
            }
        }
    }
    let size = size.ok_or("no SIZ marker")?;
    let ncomp = size.components.len();
    let ntiles = (size.tiles_across() * size.tiles_down()) as usize;
    let mut tiles: Vec<TileData> = (0..ntiles).map(|_| TileData::default()).collect();

    // PPM data is one stream of (length, headers) pairs, one per tile-part.
    ppm.sort_by_key(|p| p.0);
    let ppm_data: Vec<u8> = ppm.into_iter().flat_map(|p| p.1).collect();
    let use_ppm = !ppm_data.is_empty();
    let mut ppm_pos = 0usize;

    let mut ppt: Vec<Vec<(u8, Vec<u8>)>> = vec![Vec::new(); ntiles];
    while pos + 2 <= d.len() {
        if d[pos..pos + 2] == [0xff, 0xd9] {
            break;
        }
        if d[pos..pos + 2] != [0xff, 0x90] {
            warnings.push(format!(
                "unexpected data at byte {pos}; tile-parts end there"
            ));
            break;
        }
        let sot = pos;
        let (_, body, next) = segment(pos)?;
        let mut r = Reader { d: body, pos: 0 };
        let index = usize::from(r.u16()?);
        let psot = r.u32()? as usize;
        let part = r.u8()?;
        pos = next;
        if index >= ntiles {
            return Err(format!("tile {index} does not exist"));
        }
        let mut params = tiles[index]
            .params
            .take()
            .unwrap_or_else(|| Params::new(ncomp));
        // Tile-part header up to SOD.
        loop {
            if d.get(pos..pos + 2) == Some(&[0xff, 0x93]) {
                pos += 2;
                break;
            }
            let (marker, body, next) = segment(pos)?;
            pos = next;
            match marker {
                0x61 => {
                    if let Some((&z, rest)) = body.split_first() {
                        ppt[index].push((z, rest.to_vec()));
                    }
                }
                // Only the first tile-part may change coding parameters.
                _ if part == 0 => {
                    params.apply(marker, body, ncomp)?;
                }
                _ => {}
            }
        }
        tiles[index].params = Some(params);
        let end = if psot == 0 {
            // The last tile-part: runs to EOC.
            if d.ends_with(&[0xff, 0xd9]) {
                d.len() - 2
            } else {
                d.len()
            }
        } else {
            sot + psot
        };
        if end > d.len() {
            warnings.push(format!("tile {index} is truncated"));
        }
        let end = end.min(d.len()).max(pos);
        tiles[index].data.extend_from_slice(&d[pos..end]);
        if use_ppm {
            let n = d_u32(&ppm_data, ppm_pos).unwrap_or(0) as usize;
            let start = (ppm_pos + 4).min(ppm_data.len());
            let stop = start.saturating_add(n).min(ppm_data.len());
            tiles[index]
                .headers
                .get_or_insert_with(Vec::new)
                .extend_from_slice(&ppm_data[start..stop]);
            ppm_pos = stop;
        }
        pos = end;
    }
    for (tile, mut parts) in tiles.iter_mut().zip(ppt) {
        if !parts.is_empty() {
            parts.sort_by_key(|p| p.0);
            tile.headers = Some(parts.into_iter().flat_map(|p| p.1).collect());
        }
    }
    Ok(Codestream {
        size,
        main,
        tiles,
        warnings,
    })
}

fn d_u32(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?))
}
