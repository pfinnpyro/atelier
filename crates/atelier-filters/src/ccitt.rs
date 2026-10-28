//! CCITTFaxDecode (ISO 32000-2, 7.4.6): ITU-T T.4 (Group 3, one- and
//! two-dimensional) and T.6 (Group 4) fax coding.

use std::sync::OnceLock;

use crate::{FilterError, Result};

/// The /DecodeParms of a CCITTFaxDecode filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcittParams {
    /// < 0: Group 4. 0: Group 3, one-dimensional. > 0: Group 3, mixed.
    pub k: i64,
    pub encoded_byte_align: bool,
    pub columns: usize,
    /// 0 when unknown: decode until the data or an end-of-block code ends.
    pub rows: usize,
    pub end_of_block: bool,
    pub black_is_1: bool,
}

impl Default for CcittParams {
    fn default() -> Self {
        Self {
            k: 0,
            encoded_byte_align: false,
            columns: 1728,
            rows: 0,
            end_of_block: true,
            black_is_1: false,
        }
    }
}

const WHITE_TERMINATING: [&str; 64] = [
    "00110101", "000111", "0111", "1000", "1011", "1100", "1110", "1111", "10011", "10100",
    "00111", "01000", "001000", "000011", "110100", "110101", "101010", "101011", "0100111",
    "0001100", "0001000", "0010111", "0000011", "0000100", "0101000", "0101011", "0010011",
    "0100100", "0011000", "00000010", "00000011", "00011010", "00011011", "00010010", "00010011",
    "00010100", "00010101", "00010110", "00010111", "00101000", "00101001", "00101010", "00101011",
    "00101100", "00101101", "00000100", "00000101", "00001010", "00001011", "01010010", "01010011",
    "01010100", "01010101", "00100100", "00100101", "01011000", "01011001", "01011010", "01011011",
    "01001010", "01001011", "00110010", "00110011", "00110100",
];

/// Make-up codes for 64, 128, ... 1728.
const WHITE_MAKEUP: [&str; 27] = [
    "11011",
    "10010",
    "010111",
    "0110111",
    "00110110",
    "00110111",
    "01100100",
    "01100101",
    "01101000",
    "01100111",
    "011001100",
    "011001101",
    "011010010",
    "011010011",
    "011010100",
    "011010101",
    "011010110",
    "011010111",
    "011011000",
    "011011001",
    "011011010",
    "011011011",
    "010011000",
    "010011001",
    "010011010",
    "011000",
    "010011011",
];

const BLACK_TERMINATING: [&str; 64] = [
    "0000110111",
    "010",
    "11",
    "10",
    "011",
    "0011",
    "0010",
    "00011",
    "000101",
    "000100",
    "0000100",
    "0000101",
    "0000111",
    "00000100",
    "00000111",
    "000011000",
    "0000010111",
    "0000011000",
    "0000001000",
    "00001100111",
    "00001101000",
    "00001101100",
    "00000110111",
    "00000101000",
    "00000010111",
    "00000011000",
    "000011001010",
    "000011001011",
    "000011001100",
    "000011001101",
    "000001101000",
    "000001101001",
    "000001101010",
    "000001101011",
    "000011010010",
    "000011010011",
    "000011010100",
    "000011010101",
    "000011010110",
    "000011010111",
    "000001101100",
    "000001101101",
    "000011011010",
    "000011011011",
    "000001010100",
    "000001010101",
    "000001010110",
    "000001010111",
    "000001100100",
    "000001100101",
    "000001010010",
    "000001010011",
    "000000100100",
    "000000110111",
    "000000111000",
    "000000100111",
    "000000101000",
    "000001011000",
    "000001011001",
    "000000101011",
    "000000101100",
    "000001011010",
    "000001100110",
    "000001100111",
];

const BLACK_MAKEUP: [&str; 27] = [
    "0000001111",
    "000011001000",
    "000011001001",
    "000001011011",
    "000000110011",
    "000000110100",
    "000000110101",
    "0000001101100",
    "0000001101101",
    "0000001001010",
    "0000001001011",
    "0000001001100",
    "0000001001101",
    "0000001110010",
    "0000001110011",
    "0000001110100",
    "0000001110101",
    "0000001110110",
    "0000001110111",
    "0000001010010",
    "0000001010011",
    "0000001010100",
    "0000001010101",
    "0000001011010",
    "0000001011011",
    "0000001100100",
    "0000001100101",
];

/// Make-up codes for 1792, 1856, ... 2560, shared by both colours.
const EXTENDED_MAKEUP: [&str; 13] = [
    "00000001000",
    "00000001100",
    "00000001101",
    "000000010010",
    "000000010011",
    "000000010100",
    "000000010101",
    "000000010110",
    "000000010111",
    "000000011100",
    "000000011101",
    "000000011110",
    "000000011111",
];

/// Longest code in bits; tables are indexed by this many bits.
const LOOKUP_BITS: u32 = 13;

/// A lookup table: for each 13-bit window, the code length and run.
/// Length 0 means no code matches.
type Table = Vec<(u8, u16)>;

fn build(terminating: &[&str], makeup: &[&str]) -> Table {
    let mut table = vec![(0u8, 0u16); 1 << LOOKUP_BITS];
    let codes = terminating
        .iter()
        .enumerate()
        .map(|(i, c)| (*c, i as u16))
        .chain(
            makeup
                .iter()
                .enumerate()
                .map(|(i, c)| (*c, 64 * (i as u16 + 1))),
        )
        .chain(
            EXTENDED_MAKEUP
                .iter()
                .enumerate()
                .map(|(i, c)| (*c, 1792 + 64 * i as u16)),
        );
    for (code, run) in codes {
        let len = code.len() as u32;
        let value = u32::from_str_radix(code, 2).unwrap_or(0);
        let shift = LOOKUP_BITS - len;
        for low in 0..(1u32 << shift) {
            table[((value << shift) | low) as usize] = (len as u8, run);
        }
    }
    table
}

fn tables() -> &'static (Table, Table) {
    static TABLES: OnceLock<(Table, Table)> = OnceLock::new();
    TABLES.get_or_init(|| {
        (
            build(&WHITE_TERMINATING, &WHITE_MAKEUP),
            build(&BLACK_TERMINATING, &BLACK_MAKEUP),
        )
    })
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn remaining(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }

    /// The next `n` bits (n <= 24), zero-padded past the end.
    fn peek(&self, n: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..n as usize {
            let p = self.pos + i;
            let bit = self.data.get(p / 8).map_or(0, |b| (b >> (7 - p % 8)) & 1);
            v = (v << 1) | u32::from(bit);
        }
        v
    }

    fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    /// Consumes an end-of-line code (000000000001) with any fill zeros
    /// before it. Returns whether one was there.
    fn eol(&mut self) -> bool {
        let start = self.pos;
        while self.remaining() >= 12 && self.peek(12) == 0 {
            self.skip(1);
        }
        if self.remaining() >= 12 && self.peek(12) == 1 {
            self.skip(12);
            true
        } else {
            self.pos = start;
            false
        }
    }

    /// Whether an end-of-line code (after optional fill) comes next.
    fn at_eol(&mut self) -> bool {
        let start = self.pos;
        let found = self.eol();
        self.pos = start;
        found
    }

    /// A run length in the given colour: make-up codes then a terminating
    /// code.
    fn run(&mut self, black: bool) -> Option<usize> {
        let (white_table, black_table) = tables();
        let table = if black { black_table } else { white_table };
        let mut total = 0usize;
        loop {
            if self.remaining() == 0 {
                return None;
            }
            let (len, run) = table[self.peek(LOOKUP_BITS) as usize];
            if len == 0 || len as usize > self.remaining() {
                return None;
            }
            self.skip(u32::from(len));
            total += usize::from(run);
            if run < 64 {
                return Some(total);
            }
        }
    }
}

enum Mode {
    Pass,
    Horizontal,
    Vertical(i64),
}

fn mode(bits: &mut Bits) -> Option<Mode> {
    let codes: [(u32, u32, Mode); 9] = [
        (1, 0b1, Mode::Vertical(0)),
        (3, 0b011, Mode::Vertical(1)),
        (3, 0b010, Mode::Vertical(-1)),
        (3, 0b001, Mode::Horizontal),
        (4, 0b0001, Mode::Pass),
        (6, 0b000011, Mode::Vertical(2)),
        (6, 0b000010, Mode::Vertical(-2)),
        (7, 0b0000011, Mode::Vertical(3)),
        (7, 0b0000010, Mode::Vertical(-3)),
    ];
    for (len, code, m) in codes {
        if bits.remaining() >= len as usize && bits.peek(len) == code {
            bits.skip(len);
            return Some(m);
        }
    }
    None
}

/// Decodes one two-dimensional row against the reference row's changing
/// elements. Returns the new row's changing elements.
fn row_2d(bits: &mut Bits, reference: &[usize], columns: usize) -> Option<Vec<usize>> {
    let mut changes = Vec::new();
    let mut a0: Option<usize> = None;
    let mut black = false;
    let mut ri = 0usize;
    loop {
        let start = a0.unwrap_or(0);
        if a0.is_some() && start >= columns {
            break;
        }
        // b1: the first changing element on the reference row right of a0
        // whose colour is opposite to a0's. Changes alternate, starting with
        // white to black at even indices.
        while ri > 0 && reference[ri - 1] > start {
            ri -= 1;
        }
        while ri < reference.len()
            && (a0.is_some_and(|a| reference[ri] <= a) || (ri % 2 == 1) != black)
        {
            ri += 1;
        }
        let b1 = reference.get(ri).copied().unwrap_or(columns);
        let b2 = reference.get(ri + 1).copied().unwrap_or(columns);
        match mode(bits)? {
            Mode::Pass => {
                a0 = Some(b2);
            }
            Mode::Horizontal => {
                let r1 = bits.run(black)?;
                let r2 = bits.run(!black)?;
                let a1 = (start + r1).min(columns);
                let a2 = (a1 + r2).min(columns);
                changes.push(a1);
                changes.push(a2);
                a0 = Some(a2);
            }
            Mode::Vertical(d) => {
                let a1 = (b1 as i64 + d).clamp(start as i64, columns as i64) as usize;
                changes.push(a1);
                a0 = Some(a1);
                black = !black;
            }
        }
    }
    Some(changes)
}

/// Decodes one one-dimensional row.
fn row_1d(bits: &mut Bits, columns: usize) -> Option<Vec<usize>> {
    let mut changes = Vec::new();
    let mut pos = 0;
    let mut black = false;
    while pos < columns {
        pos = (pos + bits.run(black)?).min(columns);
        changes.push(pos);
        black = !black;
    }
    Some(changes)
}

/// Decodes CCITT fax data to packed 1-bit rows, MSB first, each row padded
/// to a whole byte. Pixels that are black come out as 0 unless
/// `black_is_1`, as PDF expects for 1-bit DeviceGray.
pub fn decode_ccitt(data: &[u8], params: &CcittParams) -> Result<Vec<u8>> {
    let columns = params.columns.clamp(1, 1 << 20);
    let row_bytes = columns.div_ceil(8);
    let mut out = Vec::new();
    let mut bits = Bits { data, pos: 0 };
    // An imaginary all-white row above the first.
    let mut reference: Vec<usize> = vec![columns, columns];
    let mut rows = 0usize;
    loop {
        if params.rows > 0 && rows >= params.rows {
            break;
        }
        if bits.remaining() < 8 && params.rows == 0 {
            break;
        }
        if params.encoded_byte_align && params.k < 0 {
            bits.align();
        }
        let two_d = if params.k < 0 {
            // Group 4: end-of-facsimile-block is two EOLs.
            if params.end_of_block && bits.peek(24) == 0x001001 {
                break;
            }
            true
        } else {
            if params.encoded_byte_align && !bits.at_eol() {
                bits.align();
            }
            // EOLs before the row. Two or more in a row (return to control)
            // end the data; in 2-D mode each carries a tag bit.
            let mut eols = 0;
            while bits.eol() {
                eols += 1;
                if params.k > 0 {
                    let save = bits.pos;
                    bits.skip(1);
                    if !bits.at_eol() {
                        bits.pos = save;
                        break;
                    }
                }
            }
            if eols >= 2 {
                break;
            }
            if params.k > 0 {
                let tag = bits.peek(1);
                bits.skip(1);
                tag == 0
            } else {
                false
            }
        };
        let decoded = if two_d {
            row_2d(&mut bits, &reference, columns)
        } else {
            row_1d(&mut bits, columns)
        };
        let Some(changes) = decoded else {
            if rows == 0 || (params.rows > 0 && rows < params.rows) {
                if params.rows > 0 {
                    // Damaged: fill the rest with white.
                    let white = if params.black_is_1 { 0 } else { 0xff };
                    out.resize(params.rows * row_bytes, white);
                }
                return Err(FilterError::new(
                    "CCITTFaxDecode",
                    format!("bad code in row {}", rows + 1),
                    out,
                ));
            }
            break;
        };
        emit(&changes, columns, params.black_is_1, &mut out);
        reference = changes;
        reference.push(columns);
        reference.push(columns);
        rows += 1;
    }
    Ok(out)
}

/// Appends one row of packed pixels from its changing elements.
fn emit(changes: &[usize], columns: usize, black_is_1: bool, out: &mut Vec<u8>) {
    let start = out.len();
    let (white_bit, black_bit) = if black_is_1 { (0u8, 1u8) } else { (1, 0) };
    out.resize(
        start + columns.div_ceil(8),
        if white_bit == 1 { 0xff } else { 0 },
    );
    let row = &mut out[start..];
    let mut pos = 0;
    for (i, &c) in changes.iter().enumerate() {
        let c = c.min(columns);
        if i % 2 == 0 {
            pos = c;
        } else {
            for x in pos..c {
                let mask = 0x80 >> (x % 8);
                if black_bit == 1 {
                    row[x / 8] |= mask;
                } else {
                    row[x / 8] &= !mask;
                }
            }
        }
    }
    // A row that ends black.
    if changes.len() % 2 == 1 {
        for x in pos..columns {
            let mask = 0x80 >> (x % 8);
            if black_bit == 1 {
                row[x / 8] |= mask;
            } else {
                row[x / 8] &= !mask;
            }
        }
    }
    // Padding bits beyond the last column stay as they are.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_prefix_free() {
        for (term, makeup) in [
            (&WHITE_TERMINATING[..], &WHITE_MAKEUP[..]),
            (&BLACK_TERMINATING[..], &BLACK_MAKEUP[..]),
        ] {
            let all: Vec<&str> = term
                .iter()
                .chain(makeup)
                .chain(&EXTENDED_MAKEUP)
                .copied()
                .collect();
            for (i, a) in all.iter().enumerate() {
                for (j, b) in all.iter().enumerate() {
                    assert!(i == j || !b.starts_with(a), "{a} is a prefix of {b}");
                }
            }
        }
    }

    #[test]
    fn one_dimensional_row() {
        // 8 columns: 2 white, 3 black, 3 white: 0111 10 1000, then EOL.
        let data = [0b0111_1010, 0b0000_0000];
        let params = CcittParams {
            k: 0,
            columns: 8,
            rows: 1,
            ..CcittParams::default()
        };
        assert_eq!(decode_ccitt(&data, &params).unwrap(), vec![0b1100_0111]);
    }

    #[test]
    fn group_4_rows() {
        // Row 1: horizontal mode, 2 white then 3 black, then V0 for the
        // final change back to white at 5... encoded as H(2 white, 3 black)
        // V0? The reference row is all white, so: H 0111 10, then the
        // remaining white run ends at the edge: V0 against b1 = 8.
        // Row 2: V0 V0 V0 copies row 1.
        let bits = "001 0111 10 1 1 1 1";
        let mut data = Vec::new();
        let clean: String = bits.chars().filter(|c| *c != ' ').collect();
        for chunk in clean.as_bytes().chunks(8) {
            let mut b = 0u8;
            for (i, c) in chunk.iter().enumerate() {
                if *c == b'1' {
                    b |= 0x80 >> i;
                }
            }
            data.push(b);
        }
        let params = CcittParams {
            k: -1,
            columns: 8,
            rows: 2,
            ..CcittParams::default()
        };
        assert_eq!(
            decode_ccitt(&data, &params).unwrap(),
            vec![0b1100_0111, 0b1100_0111]
        );
    }
}
