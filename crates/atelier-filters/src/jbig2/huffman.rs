//! Huffman tables (T.88 annex B): the 15 standard tables, tables from
//! table segments, and the bit reader they decode from.

/// Reads bits MSB first.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(u32::from(bit))
    }

    pub fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }

    pub fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    /// The byte offset of the next whole byte.
    pub fn byte_pos(&self) -> usize {
        self.pos.div_ceil(8)
    }

    pub fn skip_bytes(&mut self, n: usize) {
        self.pos = (self.byte_pos() + n) * 8;
    }

    pub fn rest(&self) -> &'a [u8] {
        self.data.get(self.byte_pos()..).unwrap_or(&[])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Normal,
    Lower,
    Upper,
    Oob,
}

#[derive(Debug, Clone, Copy)]
struct Line {
    low: i64,
    prefix_len: u32,
    range_len: u32,
    kind: Kind,
}

/// A decoded value: a number or out-of-band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Oob,
}

#[derive(Debug, Clone)]
pub struct Table {
    /// (prefix length, prefix code, line).
    codes: Vec<(u32, u32, Line)>,
}

impl Table {
    fn from_lines(lines: Vec<Line>) -> Self {
        // Canonical code assignment (B.3).
        let max = lines.iter().map(|l| l.prefix_len).max().unwrap_or(0);
        let mut count = vec![0u32; max as usize + 2];
        for l in &lines {
            count[l.prefix_len as usize] += 1;
        }
        count[0] = 0;
        let mut codes = Vec::new();
        let mut first = 0u32;
        for len in 1..=max {
            first = (first + count[len as usize - 1]) << 1;
            for (code, l) in (first..).zip(lines.iter().filter(|l| l.prefix_len == len)) {
                codes.push((len, code, *l));
            }
        }
        codes.sort_by_key(|c| c.0);
        Self { codes }
    }

    /// Decodes one value.
    pub fn decode(&self, r: &mut BitReader) -> Option<Value> {
        let mut code = 0u32;
        let mut len = 0u32;
        let mut i = 0;
        while i < self.codes.len() {
            let (l, c, line) = self.codes[i];
            while len < l {
                code = (code << 1) | r.bit()?;
                len += 1;
            }
            if c == code {
                return Some(match line.kind {
                    Kind::Oob => Value::Oob,
                    Kind::Lower => Value::Int(line.low - i64::from(r.bits(32)?)),
                    Kind::Normal | Kind::Upper => {
                        Value::Int(line.low + i64::from(r.bits(line.range_len)?))
                    }
                });
            }
            i += 1;
        }
        None
    }

    /// A table whose line `i` decodes to value `i` with code length
    /// `lengths[i]` (lines with length 0 have no code).
    pub fn from_lengths(lengths: &[u32]) -> Self {
        Self::from_lines(
            lengths
                .iter()
                .enumerate()
                .filter(|(_, l)| **l > 0)
                .map(|(i, &l)| Line {
                    low: i as i64,
                    prefix_len: l,
                    range_len: 0,
                    kind: Kind::Normal,
                })
                .collect(),
        )
    }

    /// A table from a table segment (7.4.13).
    pub fn from_segment(data: &[u8]) -> Option<Self> {
        let flags = *data.first()?;
        let oob = flags & 1 != 0;
        let ps = u32::from((flags >> 1) & 7) + 1;
        let rs = u32::from((flags >> 4) & 7) + 1;
        let low = i64::from(i32::from_be_bytes(data.get(1..5)?.try_into().ok()?));
        let high = i64::from(i32::from_be_bytes(data.get(5..9)?.try_into().ok()?));
        let mut r = BitReader::new(&data[9..]);
        let mut lines = Vec::new();
        let mut cur = low;
        while cur < high {
            let prefix_len = r.bits(ps)?;
            let range_len = r.bits(rs)?;
            lines.push(Line {
                low: cur,
                prefix_len,
                range_len,
                kind: Kind::Normal,
            });
            cur += 1i64 << range_len.min(32);
            if lines.len() > 1 << 16 {
                return None;
            }
        }
        lines.push(Line {
            low: low - 1,
            prefix_len: r.bits(ps)?,
            range_len: 32,
            kind: Kind::Lower,
        });
        lines.push(Line {
            low: high,
            prefix_len: r.bits(ps)?,
            range_len: 32,
            kind: Kind::Upper,
        });
        if oob {
            lines.push(Line {
                low: 0,
                prefix_len: r.bits(ps)?,
                range_len: 0,
                kind: Kind::Oob,
            });
        }
        Some(Self::from_lines(lines))
    }

    /// Standard table B.n.
    pub fn standard(n: usize) -> Table {
        // (range low, prefix length, range length); then the lower range
        // line, the upper range line and the OOB prefix length, if any.
        type Spec = (
            &'static [(i64, u32, u32)],
            Option<(i64, u32)>,
            (i64, u32),
            Option<u32>,
        );
        let spec: Spec = match n {
            1 => (
                &[(0, 1, 4), (16, 2, 8), (272, 3, 16)],
                None,
                (65808, 3),
                None,
            ),
            2 => (
                &[(0, 1, 0), (1, 2, 0), (2, 3, 0), (3, 4, 3), (11, 5, 6)],
                None,
                (75, 6),
                Some(6),
            ),
            3 => (
                &[
                    (-256, 8, 8),
                    (0, 1, 0),
                    (1, 2, 0),
                    (2, 3, 0),
                    (3, 4, 3),
                    (11, 5, 6),
                ],
                Some((-257, 8)),
                (75, 7),
                Some(6),
            ),
            4 => (
                &[(1, 1, 0), (2, 2, 0), (3, 3, 0), (4, 4, 3), (12, 5, 6)],
                None,
                (76, 5),
                None,
            ),
            5 => (
                &[
                    (-255, 7, 8),
                    (1, 1, 0),
                    (2, 2, 0),
                    (3, 3, 0),
                    (4, 4, 3),
                    (12, 5, 6),
                ],
                Some((-256, 7)),
                (76, 6),
                None,
            ),
            6 => (
                &[
                    (-2048, 5, 10),
                    (-1024, 4, 9),
                    (-512, 4, 8),
                    (-256, 4, 7),
                    (-128, 5, 6),
                    (-64, 5, 5),
                    (-32, 4, 5),
                    (0, 2, 7),
                    (128, 3, 7),
                    (256, 3, 8),
                    (512, 4, 9),
                    (1024, 4, 10),
                ],
                Some((-2049, 6)),
                (2048, 6),
                None,
            ),
            7 => (
                &[
                    (-1024, 4, 9),
                    (-512, 3, 8),
                    (-256, 4, 7),
                    (-128, 5, 6),
                    (-64, 5, 5),
                    (-32, 4, 5),
                    (0, 4, 5),
                    (32, 5, 5),
                    (64, 5, 6),
                    (128, 4, 7),
                    (256, 3, 8),
                    (512, 3, 9),
                    (1024, 3, 10),
                ],
                Some((-1025, 5)),
                (2048, 5),
                None,
            ),
            8 => (
                &[
                    (-15, 8, 3),
                    (-7, 9, 1),
                    (-5, 8, 1),
                    (-3, 9, 0),
                    (-2, 7, 0),
                    (-1, 4, 0),
                    (0, 2, 1),
                    (2, 5, 0),
                    (3, 6, 0),
                    (4, 3, 4),
                    (20, 6, 1),
                    (22, 4, 4),
                    (38, 4, 5),
                    (70, 5, 6),
                    (134, 5, 7),
                    (262, 6, 7),
                    (390, 7, 8),
                    (646, 6, 10),
                ],
                Some((-16, 9)),
                (1670, 9),
                Some(2),
            ),
            9 => (
                &[
                    (-31, 8, 4),
                    (-15, 9, 2),
                    (-11, 8, 2),
                    (-7, 9, 1),
                    (-5, 7, 1),
                    (-3, 4, 1),
                    (-1, 3, 1),
                    (1, 3, 1),
                    (3, 5, 1),
                    (5, 6, 1),
                    (7, 3, 5),
                    (39, 6, 2),
                    (43, 4, 5),
                    (75, 4, 6),
                    (139, 5, 7),
                    (267, 5, 8),
                    (523, 6, 8),
                    (779, 7, 9),
                    (1291, 6, 11),
                ],
                Some((-32, 9)),
                (3339, 9),
                Some(2),
            ),
            10 => (
                &[
                    (-21, 7, 4),
                    (-5, 8, 0),
                    (-4, 7, 0),
                    (-3, 5, 0),
                    (-2, 2, 2),
                    (2, 5, 0),
                    (3, 6, 0),
                    (4, 7, 0),
                    (5, 8, 0),
                    (6, 2, 6),
                    (70, 5, 5),
                    (102, 6, 5),
                    (134, 6, 6),
                    (198, 6, 7),
                    (326, 6, 8),
                    (582, 6, 9),
                    (1094, 6, 10),
                    (2118, 7, 11),
                ],
                Some((-22, 8)),
                (4166, 8),
                Some(2),
            ),
            11 => (
                &[
                    (1, 1, 0),
                    (2, 2, 1),
                    (4, 4, 0),
                    (5, 4, 1),
                    (7, 5, 1),
                    (9, 5, 2),
                    (13, 6, 2),
                    (17, 7, 2),
                    (21, 7, 3),
                    (29, 7, 4),
                    (45, 7, 5),
                    (77, 7, 6),
                ],
                None,
                (141, 7),
                None,
            ),
            12 => (
                &[
                    (1, 1, 0),
                    (2, 2, 0),
                    (3, 3, 1),
                    (5, 5, 0),
                    (6, 5, 1),
                    (8, 6, 1),
                    (10, 7, 0),
                    (11, 7, 1),
                    (13, 7, 2),
                    (17, 7, 3),
                    (25, 7, 4),
                    (41, 8, 5),
                ],
                None,
                (73, 8),
                None,
            ),
            13 => (
                &[
                    (1, 1, 0),
                    (2, 3, 0),
                    (3, 4, 0),
                    (4, 5, 0),
                    (5, 4, 1),
                    (7, 3, 3),
                    (15, 6, 1),
                    (17, 6, 2),
                    (21, 6, 3),
                    (29, 6, 4),
                    (45, 6, 5),
                    (77, 7, 6),
                ],
                None,
                (141, 7),
                None,
            ),
            14 => (
                &[(-2, 3, 0), (-1, 3, 0), (0, 1, 0), (1, 3, 0), (2, 3, 0)],
                None,
                (0, 0),
                None,
            ),
            _ => (
                &[
                    (-24, 7, 4),
                    (-8, 6, 2),
                    (-4, 5, 1),
                    (-2, 4, 0),
                    (-1, 3, 0),
                    (0, 1, 0),
                    (1, 3, 0),
                    (2, 4, 0),
                    (3, 5, 1),
                    (5, 6, 2),
                    (9, 7, 4),
                ],
                Some((-25, 7)),
                (25, 7),
                None,
            ),
        };
        let (normal, lower, upper, oob) = spec;
        let mut lines: Vec<Line> = normal
            .iter()
            .map(|&(low, prefix_len, range_len)| Line {
                low,
                prefix_len,
                range_len,
                kind: Kind::Normal,
            })
            .collect();
        if let Some((low, prefix_len)) = lower {
            lines.push(Line {
                low,
                prefix_len,
                range_len: 32,
                kind: Kind::Lower,
            });
        }
        // Table B.14 has no upper range line.
        if upper.1 > 0 {
            lines.push(Line {
                low: upper.0,
                prefix_len: upper.1,
                range_len: 32,
                kind: Kind::Upper,
            });
        }
        if let Some(prefix_len) = oob {
            lines.push(Line {
                low: 0,
                prefix_len,
                range_len: 0,
                kind: Kind::Oob,
            });
        }
        Table::from_lines(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(t: &Table, low: i64, kind: Kind) -> (u32, u32) {
        t.codes
            .iter()
            .find(|(_, _, l)| l.low == low && l.kind == kind)
            .map(|(len, code, _)| (*len, *code))
            .unwrap()
    }

    #[test]
    fn canonical_codes_match_the_spec() {
        // Codes as printed in annex B.
        let b3 = Table::standard(3);
        assert_eq!(code_of(&b3, -256, Kind::Normal), (8, 0xfe));
        assert_eq!(code_of(&b3, -257, Kind::Lower), (8, 0xff));
        assert_eq!(code_of(&b3, 75, Kind::Upper), (7, 0x7e));
        assert_eq!(code_of(&b3, 0, Kind::Oob), (6, 0x3e));
        let b8 = Table::standard(8);
        assert_eq!(code_of(&b8, 0, Kind::Oob), (2, 0x1));
        assert_eq!(code_of(&b8, 646, Kind::Normal), (6, 0x3d));
        let b10 = Table::standard(10);
        assert_eq!(code_of(&b10, 2118, Kind::Normal), (7, 0x7d));
        let b15 = Table::standard(15);
        assert_eq!(code_of(&b15, 9, Kind::Normal), (7, 0x7d));
    }

    #[test]
    fn decodes_values() {
        // B.1: "10" then 8 bits 00000101 = 16 + 5; "0" then 4 bits 0011 = 3.
        let data = [0b1000_0001, 0b0100_0110];
        let mut r = BitReader::new(&data);
        let t = Table::standard(1);
        assert_eq!(t.decode(&mut r), Some(Value::Int(21)));
        assert_eq!(t.decode(&mut r), Some(Value::Int(3)));
        // B.2 OOB is 111111.
        let mut r = BitReader::new(&[0b1111_1100]);
        assert_eq!(Table::standard(2).decode(&mut r), Some(Value::Oob));
    }

    #[test]
    fn ranges_are_contiguous() {
        for n in 1..=15 {
            let t = Table::standard(n);
            let mut normal: Vec<Line> = t
                .codes
                .iter()
                .map(|c| c.2)
                .filter(|l| l.kind == Kind::Normal)
                .collect();
            normal.sort_by_key(|l| l.low);
            for w in normal.windows(2) {
                assert_eq!(w[0].low + (1 << w[0].range_len), w[1].low, "B.{n}");
            }
        }
    }
}
