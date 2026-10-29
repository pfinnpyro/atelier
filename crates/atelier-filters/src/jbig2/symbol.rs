//! Symbol dictionaries (6.5) and text regions (6.4).

use super::arith::{ArithDecoder, IntContexts, contexts};
use super::huffman::{BitReader, Table, Value};
use super::region::{
    Bitmap, Combine, Generic, Refinement, decode_generic, decode_generic_mmr, decode_refinement,
};

/// Symbols or instances beyond this are refused (hostile files).
const MAX_SYMBOLS: usize = 1 << 20;

/// Where the numbers of a text region or symbol dictionary come from.
pub enum Coder<'a, 'b> {
    Arith {
        d: &'b mut ArithDecoder<'a>,
        cx: &'b mut IntContexts,
    },
    Huffman {
        r: &'b mut BitReader<'a>,
    },
}

fn need(v: Option<i64>) -> Result<i64, String> {
    v.ok_or_else(|| "unexpected out-of-band value".to_string())
}

fn huff(t: &Table, r: &mut BitReader) -> Result<Option<i64>, String> {
    match t.decode(r) {
        Some(Value::Int(v)) => Ok(Some(v)),
        Some(Value::Oob) => Ok(None),
        None => Err("bad Huffman code".into()),
    }
}

/// Huffman tables a text region uses.
pub struct TextTables {
    pub fs: Table,
    pub ds: Table,
    pub dt: Table,
    pub rdw: Table,
    pub rdh: Table,
    pub rdx: Table,
    pub rdy: Table,
    pub rsize: Table,
    pub ids: Table,
}

pub struct Text<'s> {
    pub width: usize,
    pub height: usize,
    pub refine: bool,
    pub strips: i64,
    pub ref_corner: u8,
    pub transposed: bool,
    pub combine: Combine,
    pub default_pixel: u8,
    pub ds_offset: i64,
    pub r_template: u8,
    pub r_at: [(i64, i64); 2],
    pub instances: usize,
    pub symbols: &'s [&'s Bitmap],
    pub id_len: u32,
    pub tables: Option<&'s TextTables>,
}

/// Decodes a text region (6.4.5).
pub fn decode_text(t: &Text, coder: &mut Coder) -> Result<Bitmap, String> {
    let mut out = Bitmap::new(t.width, t.height, t.default_pixel);
    let log_strips = t.strips.max(1).trailing_zeros();
    let tables = t.tables;
    let tab = || tables.ok_or_else(|| "missing Huffman tables".to_string());
    macro_rules! num {
        ($ia:ident, $table:ident) => {
            match coder {
                Coder::Arith { d, cx } => d.int(&mut cx.$ia),
                Coder::Huffman { r } => huff(&tab()?.$table, r)?,
            }
        };
    }
    // Refinement statistics for Huffman-coded regions, whose refinement
    // data is arithmetic coded piece by piece.
    let mut huffman_gr = contexts(1 << 13);
    let mut strip_t = -need(num!(iadt, dt))? * t.strips;
    let mut first_s = 0i64;
    let mut placed = 0usize;
    while placed < t.instances {
        strip_t += need(num!(iadt, dt))? * t.strips;
        first_s += need(num!(iafs, fs))?;
        let mut cur_s = first_s;
        loop {
            let cur_t = if t.strips == 1 {
                0
            } else {
                match coder {
                    Coder::Arith { d, cx } => need(d.int(&mut cx.iait))?,
                    Coder::Huffman { r } => i64::from(r.bits(log_strips).ok_or("data ends early")?),
                }
            };
            let ti = strip_t + cur_t;
            let id = match coder {
                Coder::Arith { d, cx } => d.id(&mut cx.iaid, t.id_len),
                Coder::Huffman { r } => match huff(&tab()?.ids, r)? {
                    Some(v) => usize::try_from(v).unwrap_or(usize::MAX),
                    None => return Err("bad symbol ID".into()),
                },
            };
            let symbol = *t
                .symbols
                .get(id)
                .ok_or_else(|| format!("symbol {id} does not exist"))?;
            let refine = t.refine
                && match coder {
                    Coder::Arith { d, cx } => need(d.int(&mut cx.iari))? != 0,
                    Coder::Huffman { r } => r.bit().ok_or("data ends early")? != 0,
                };
            let refined;
            let bitmap = if refine {
                let (rdw, rdh, rdx, rdy) = (
                    need(num!(iardw, rdw))?,
                    need(num!(iardh, rdh))?,
                    need(num!(iardx, rdx))?,
                    need(num!(iardy, rdy))?,
                );
                let width = usize::try_from(symbol.width as i64 + rdw).map_err(|_| "bad width")?;
                let height =
                    usize::try_from(symbol.height as i64 + rdh).map_err(|_| "bad height")?;
                if width.saturating_mul(height) > 1 << 24 {
                    return Err("refined symbol too large".into());
                }
                let params = Refinement {
                    width,
                    height,
                    template: t.r_template,
                    reference: symbol,
                    dx: (rdw >> 1) + rdx,
                    dy: (rdh >> 1) + rdy,
                    tpgron: false,
                    at: &t.r_at,
                };
                refined = match coder {
                    Coder::Arith { d, cx } => decode_refinement(&params, d, &mut cx.gr),
                    Coder::Huffman { r } => {
                        let size = need(huff(&tab()?.rsize, r)?)?;
                        let size = usize::try_from(size).map_err(|_| "bad refinement size")?;
                        r.align();
                        let rest = r.rest();
                        let mut d = ArithDecoder::new(&rest[..size.min(rest.len())]);
                        let b = decode_refinement(&params, &mut d, &mut huffman_gr);
                        r.skip_bytes(size);
                        b
                    }
                };
                &refined
            } else {
                symbol
            };
            let (w, h) = (bitmap.width as i64, bitmap.height as i64);
            // Move to the symbol's far edge when its reference corner is
            // on the near side (6.4.5 step 3c-vi to x).
            if !t.transposed && t.ref_corner > 1 {
                cur_s += w - 1;
            } else if t.transposed && t.ref_corner & 1 == 0 {
                cur_s += h - 1;
            }
            let (s_pos, t_pos) = (cur_s, ti);
            let (x, y) = if t.transposed {
                (t_pos, s_pos)
            } else {
                (s_pos, t_pos)
            };
            let x = if t.ref_corner & 2 != 0 { x - w + 1 } else { x };
            let y = if t.ref_corner & 1 == 0 { y - h + 1 } else { y };
            out.compose(bitmap, x, y, t.combine);
            if !t.transposed && t.ref_corner <= 1 {
                cur_s += w - 1;
            } else if t.transposed && t.ref_corner & 1 == 1 {
                cur_s += h - 1;
            }
            placed += 1;
            // The strip ends with an out-of-band value, even after the last
            // instance.
            let Some(ds) = num!(iads, ds) else { break };
            cur_s += ds + t.ds_offset;
            if placed >= t.instances {
                break;
            }
        }
    }
    Ok(out)
}

/// Huffman tables a symbol dictionary uses.
pub struct DictTables {
    pub dh: Table,
    pub dw: Table,
    pub bmsize: Table,
    pub agginst: Table,
}

pub struct Dict<'s> {
    pub huffman: bool,
    pub refagg: bool,
    pub template: u8,
    pub at: &'s [(i64, i64)],
    pub r_template: u8,
    pub r_at: [(i64, i64); 2],
    pub exported: usize,
    pub new: usize,
    pub input: &'s [&'s Bitmap],
    pub tables: Option<&'s DictTables>,
}

fn id_len(n: usize) -> u32 {
    let mut len = 0;
    while (1usize << len) < n {
        len += 1;
    }
    len
}

/// Decodes a symbol dictionary (6.5.5) and returns its exported symbols.
pub fn decode_dict(sd: &Dict, data: &[u8]) -> Result<Vec<Bitmap>, String> {
    if sd.new > MAX_SYMBOLS || sd.exported > MAX_SYMBOLS + sd.input.len() {
        return Err("too many symbols".into());
    }
    let total = sd.input.len() + sd.new;
    let code_len = id_len(total);
    let mut d = ArithDecoder::new(data);
    let mut cx = IntContexts::new(code_len);
    let mut r = BitReader::new(data);
    let tables = sd.tables;
    let tab = || tables.ok_or_else(|| "missing Huffman tables".to_string());
    let mut new: Vec<Bitmap> = Vec::with_capacity(sd.new.min(4096));
    let mut height = 0i64;
    while new.len() < sd.new {
        let dh = if sd.huffman {
            huff(&tab()?.dh, &mut r)?
        } else {
            d.int(&mut cx.iadh)
        };
        height += need(dh)?;
        let h = usize::try_from(height).map_err(|_| "negative symbol height")?;
        let mut width = 0i64;
        let mut total_width = 0usize;
        let class_start = new.len();
        // Symbols of one height class.
        loop {
            let dw = if sd.huffman {
                huff(&tab()?.dw, &mut r)?
            } else {
                d.int(&mut cx.iadw)
            };
            let Some(dw) = dw else { break };
            if new.len() >= sd.new {
                return Err("more symbols than declared".into());
            }
            width += dw;
            let w = usize::try_from(width).map_err(|_| "negative symbol width")?;
            if w.saturating_mul(h) > 1 << 24 {
                return Err("symbol too large".into());
            }
            total_width += w;
            if sd.huffman && !sd.refagg {
                // Collected into one bitmap per height class, below.
                new.push(Bitmap::new(w, h, 0));
                continue;
            }
            let bitmap = if sd.refagg {
                let instances = if sd.huffman {
                    huff(&tab()?.agginst, &mut r)?
                } else {
                    d.int(&mut cx.iaai)
                };
                let instances = need(instances)?;
                let mut all: Vec<&Bitmap> = sd.input.to_vec();
                all.extend(new.iter());
                if instances == 1 {
                    let (id, rdx, rdy) = if sd.huffman {
                        let b15 = Table::standard(15);
                        let id = r.bits(code_len).ok_or("data ends early")? as usize;
                        (id, need(huff(&b15, &mut r)?)?, need(huff(&b15, &mut r)?)?)
                    } else {
                        let id = d.id(&mut cx.iaid, code_len);
                        (id, need(d.int(&mut cx.iardx))?, need(d.int(&mut cx.iardy))?)
                    };
                    let reference = *all.get(id).ok_or("bad symbol reference")?;
                    let params = Refinement {
                        width: w,
                        height: h,
                        template: sd.r_template,
                        reference,
                        dx: rdx,
                        dy: rdy,
                        tpgron: false,
                        at: &sd.r_at,
                    };
                    if sd.huffman {
                        let size = need(huff(&Table::standard(1), &mut r)?)?;
                        let size = usize::try_from(size).map_err(|_| "bad refinement size")?;
                        r.align();
                        let rest = r.rest();
                        let mut sub = ArithDecoder::new(&rest[..size.min(rest.len())]);
                        let b = decode_refinement(&params, &mut sub, &mut cx.gr);
                        r.skip_bytes(size);
                        b
                    } else {
                        decode_refinement(&params, &mut d, &mut cx.gr)
                    }
                } else {
                    let instances = usize::try_from(instances).map_err(|_| "bad instance count")?;
                    // Refinement/aggregate coding through a text region
                    // (6.5.8.2.1), with fixed tables when Huffman coded.
                    let huffman_tables = sd.huffman.then(|| TextTables {
                        fs: Table::standard(6),
                        ds: Table::standard(8),
                        dt: Table::standard(11),
                        rdw: Table::standard(15),
                        rdh: Table::standard(15),
                        rdx: Table::standard(15),
                        rdy: Table::standard(15),
                        rsize: Table::standard(1),
                        ids: Table::from_lengths(&vec![code_len.max(1); all.len()]),
                    });
                    let text = Text {
                        width: w,
                        height: h,
                        refine: true,
                        strips: 1,
                        ref_corner: 1,
                        transposed: false,
                        combine: Combine::Or,
                        default_pixel: 0,
                        ds_offset: 0,
                        r_template: sd.r_template,
                        r_at: sd.r_at,
                        instances,
                        symbols: &all,
                        id_len: code_len,
                        tables: huffman_tables.as_ref(),
                    };
                    if sd.huffman {
                        decode_text(&text, &mut Coder::Huffman { r: &mut r })?
                    } else {
                        decode_text(
                            &text,
                            &mut Coder::Arith {
                                d: &mut d,
                                cx: &mut cx,
                            },
                        )?
                    }
                }
            } else {
                decode_generic(
                    &Generic {
                        width: w,
                        height: h,
                        template: sd.template,
                        tpgdon: false,
                        at: sd.at,
                        skip: None,
                    },
                    &mut d,
                    &mut cx.gb,
                )
            };
            new.push(bitmap);
        }
        if sd.huffman && !sd.refagg {
            // The height class's collective bitmap (6.5.9).
            let size = huff(&tab()?.bmsize, &mut r)?.ok_or("bad bitmap size")?;
            let size = usize::try_from(size).map_err(|_| "bad bitmap size")?;
            r.align();
            let collective = if size == 0 {
                let row_bytes = total_width.div_ceil(8);
                let bytes = r.rest();
                let mut b = Bitmap::new(total_width, h, 0);
                for y in 0..h {
                    for x in 0..total_width {
                        let byte = bytes.get(y * row_bytes + x / 8).copied().unwrap_or(0);
                        b.data[y * total_width + x] = (byte >> (7 - x % 8)) & 1;
                    }
                }
                r.skip_bytes(row_bytes * h);
                b
            } else {
                let bytes = r.rest();
                let (b, _) = decode_generic_mmr(total_width, h, &bytes[..size.min(bytes.len())]);
                r.skip_bytes(size);
                b
            };
            let mut x = 0;
            for symbol in &mut new[class_start..] {
                *symbol = collective.crop(x as i64, 0, symbol.width, h);
                x += symbol.width;
            }
        }
    }
    // Which symbols are exported (6.5.10): alternating runs of off and on.
    let mut exported = Vec::with_capacity(sd.exported.min(4096));
    let mut flag = false;
    let mut i = 0usize;
    let all_count = sd.input.len() + new.len();
    let run_table = Table::standard(1);
    while i < all_count {
        let run = if sd.huffman {
            huff(&run_table, &mut r)?
        } else {
            d.int(&mut cx.iaex)
        };
        let run = usize::try_from(need(run)?).map_err(|_| "bad export run")?;
        if run > all_count - i {
            return Err("export runs past the symbols".into());
        }
        if flag {
            for j in i..i + run {
                let symbol = if j < sd.input.len() {
                    sd.input[j].clone()
                } else {
                    new[j - sd.input.len()].clone()
                };
                exported.push(symbol);
            }
        }
        i += run;
        flag = !flag;
    }
    Ok(exported)
}

/// Reads the symbol ID Huffman table at the start of a Huffman-coded text
/// region (7.4.3.1.7).
pub fn read_symbol_codes(r: &mut BitReader, symbols: usize) -> Option<Table> {
    let mut run_lengths = [0u32; 35];
    for l in &mut run_lengths {
        *l = r.bits(4)?;
    }
    let runs = Table::from_lengths(&run_lengths);
    let mut lengths: Vec<u32> = Vec::with_capacity(symbols);
    while lengths.len() < symbols {
        let Value::Int(code) = runs.decode(r)? else {
            return None;
        };
        match code {
            0..=31 => lengths.push(code as u32),
            32 => {
                let prev = *lengths.last()?;
                let n = 3 + r.bits(2)?;
                lengths.extend(std::iter::repeat_n(prev, n as usize));
            }
            33 => lengths.extend(std::iter::repeat_n(0, 3 + r.bits(3)? as usize)),
            _ => lengths.extend(std::iter::repeat_n(0, 11 + r.bits(7)? as usize)),
        }
    }
    lengths.truncate(symbols);
    r.align();
    Some(Table::from_lengths(&lengths))
}

pub fn symbol_id_len(n: usize) -> u32 {
    id_len(n)
}
