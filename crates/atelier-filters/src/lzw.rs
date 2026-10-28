//! LZWDecode (ISO 32000-2, 7.4.4.2): 9 to 12 bit codes, MSB first.

use crate::{FilterError, Result};

const CLEAR: u16 = 256;
const EOD: u16 = 257;
const MAX_CODES: usize = 4096;

#[derive(Clone, Copy)]
struct Entry {
    prefix: u16,
    suffix: u8,
    first: u8,
    len: u16,
}

/// Decodes LZW data. `early_change` is the /EarlyChange parameter (default 1).
pub fn decode_lzw(data: &[u8], early_change: bool) -> Result<Vec<u8>> {
    let early = usize::from(early_change);
    let mut table: Vec<Entry> = Vec::with_capacity(MAX_CODES);
    reset(&mut table);
    let mut out = Vec::with_capacity(data.len() * 3);
    let mut reader = MsbReader { data, bit: 0 };
    let mut code_len = 9;
    let mut prev: Option<u16> = None;

    while let Some(code) = reader.read(code_len) {
        if code == CLEAR {
            reset(&mut table);
            code_len = 9;
            prev = None;
            continue;
        }
        if code == EOD {
            break;
        }
        let code_idx = usize::from(code);
        match prev {
            None => {
                if code_idx >= table.len() {
                    return Err(FilterError::new(
                        "LZWDecode",
                        "first code is not a literal",
                        out,
                    ));
                }
                emit(&table, code, &mut out);
            }
            Some(p) => {
                let first = if code_idx < table.len() {
                    emit(&table, code, &mut out);
                    table[code_idx].first
                } else if code_idx == table.len() {
                    // The KwKwK case: code refers to the entry being built.
                    let first = table[usize::from(p)].first;
                    emit(&table, p, &mut out);
                    out.push(first);
                    first
                } else {
                    return Err(FilterError::new("LZWDecode", "code beyond table", out));
                };
                if table.len() < MAX_CODES {
                    let parent = table[usize::from(p)];
                    table.push(Entry {
                        prefix: p,
                        suffix: first,
                        first: parent.first,
                        len: parent.len + 1,
                    });
                }
            }
        }
        prev = Some(code);
        code_len = match table.len() + early {
            n if n >= 2048 => 12,
            n if n >= 1024 => 11,
            n if n >= 512 => 10,
            _ => 9,
        };
    }
    Ok(out)
}

fn reset(table: &mut Vec<Entry>) {
    table.clear();
    for b in 0..=255u8 {
        table.push(Entry {
            prefix: 0,
            suffix: b,
            first: b,
            len: 1,
        });
    }
    // CLEAR and EOD occupy 256 and 257 but are never emitted.
    let filler = Entry {
        prefix: 0,
        suffix: 0,
        first: 0,
        len: 0,
    };
    table.push(filler);
    table.push(filler);
}

fn emit(table: &[Entry], code: u16, out: &mut Vec<u8>) {
    let len = usize::from(table[usize::from(code)].len);
    let start = out.len();
    out.resize(start + len, 0);
    let mut c = code;
    for slot in out[start..].iter_mut().rev() {
        let e = table[usize::from(c)];
        *slot = e.suffix;
        c = e.prefix;
    }
}

struct MsbReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl MsbReader<'_> {
    fn read(&mut self, n: u32) -> Option<u16> {
        let mut v: u16 = 0;
        for _ in 0..n {
            let byte = *self.data.get(self.bit / 8)?;
            let b = (byte >> (7 - self.bit % 8)) & 1;
            v = v << 1 | u16::from(b);
            self.bit += 1;
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(n: usize) -> u32 {
        match n {
            n if n >= 2048 => 12,
            n if n >= 1024 => 11,
            n if n >= 512 => 10,
            _ => 9,
        }
    }

    /// Minimal encoder used only to test the decoder. It mirrors the decoder's
    /// table, which lags the encoder's by one entry, and clears before it fills.
    fn encode(data: &[u8], early_change: bool) -> Vec<u8> {
        use std::collections::HashMap;
        let early = usize::from(early_change);
        let mut dict: HashMap<Vec<u8>, u16> = HashMap::new();
        let mut next = 258usize;
        let mut bits: Vec<bool> = Vec::new();
        let push = |bits: &mut Vec<bool>, code: u16, n: u32| {
            for i in (0..n).rev() {
                bits.push(code >> i & 1 == 1);
            }
        };
        let code_of = |dict: &HashMap<Vec<u8>, u16>, w: &[u8]| {
            if w.len() == 1 {
                u16::from(w[0])
            } else {
                dict[w]
            }
        };
        push(&mut bits, CLEAR, 9);
        let mut code_len = 9;
        let mut w: Vec<u8> = Vec::new();
        for &c in data {
            let mut wc = w.clone();
            wc.push(c);
            if wc.len() == 1 || dict.contains_key(&wc) {
                w = wc;
                continue;
            }
            push(&mut bits, code_of(&dict, &w), code_len);
            dict.insert(wc, next as u16);
            next += 1;
            code_len = width(next - 1 + early);
            if next >= 4000 {
                push(&mut bits, CLEAR, code_len);
                dict.clear();
                next = 258;
                code_len = 9;
            }
            w = vec![c];
        }
        if !w.is_empty() {
            push(&mut bits, code_of(&dict, &w), code_len);
            next += 1;
            code_len = width(next - 1 + early);
        }
        push(&mut bits, EOD, code_len);
        bits.chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |b, (i, &on)| b | (u8::from(on) << (7 - i)))
            })
            .collect()
    }

    #[test]
    fn spec_example() {
        // ISO 32000-2, 7.4.4.2: "-----A---B" encodes to these bytes.
        let encoded = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(decode_lzw(&encoded, true).unwrap(), b"-----A---B");
    }

    #[test]
    fn round_trips_through_code_width_changes() {
        let mut data = Vec::new();
        let mut state = 3u32;
        for _ in 0..40_000 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            data.push(b'a' + (state >> 28) as u8);
        }
        for early in [true, false] {
            assert_eq!(decode_lzw(&encode(&data, early), early).unwrap(), data);
        }
    }
}
