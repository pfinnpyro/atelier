//! PDF CMaps (ISO 32000-2, 9.7.5) and ToUnicode CMaps (9.10.3).

use std::collections::HashMap;

use atelier_syntax::{ContentParser, Object};

#[derive(Debug, Clone, Default)]
pub struct CMap {
    /// Code space ranges: byte length, low and high bytes.
    codespace: Vec<(usize, Vec<u8>, Vec<u8>)>,
    /// Code ranges to CIDs: (low, high, first CID).
    cid_ranges: Vec<(u32, u32, u32)>,
    /// Single codes to Unicode text.
    unicode: HashMap<u32, String>,
    /// Code ranges to Unicode: (low, high, text of the low code).
    unicode_ranges: Vec<(u32, u32, Vec<u16>)>,
    pub vertical: bool,
    /// Identity-H or Identity-V: two-byte codes that are their own CIDs.
    identity: bool,
}

/// CMaps a `usecmap` may pull in, by name. Only the identity CMaps are
/// built in for now.
fn predefined(name: &[u8]) -> Option<CMap> {
    match name {
        b"Identity-H" | b"Identity-V" => Some(CMap::identity(name == b"Identity-V")),
        _ => None,
    }
}

impl CMap {
    pub fn identity(vertical: bool) -> Self {
        Self {
            codespace: vec![(2, vec![0, 0], vec![0xFF, 0xFF])],
            identity: true,
            vertical,
            ..Self::default()
        }
    }

    /// A predefined CMap by name. Returns `None` for ones we do not ship yet.
    pub fn predefined(name: &[u8]) -> Option<Self> {
        predefined(name)
    }

    pub fn parse(data: &[u8]) -> Self {
        let mut cmap = CMap::default();
        for op in ContentParser::new(data) {
            let o = &op.operands;
            match op.operator {
                b"endcodespacerange" => {
                    for pair in o.chunks_exact(2) {
                        if let (Some(lo), Some(hi)) = (pair[0].as_string(), pair[1].as_string()) {
                            if lo.bytes.len() == hi.bytes.len() && (1..=4).contains(&lo.bytes.len())
                            {
                                cmap.codespace.push((
                                    lo.bytes.len(),
                                    lo.bytes.clone(),
                                    hi.bytes.clone(),
                                ));
                            }
                        }
                    }
                }
                b"endcidrange" => {
                    for t in o.chunks_exact(3) {
                        if let (Some(lo), Some(hi), Some(cid)) =
                            (t[0].as_string(), t[1].as_string(), t[2].as_i64())
                        {
                            cmap.cid_ranges
                                .push((be(&lo.bytes), be(&hi.bytes), cid.max(0) as u32));
                        }
                    }
                }
                b"endcidchar" => {
                    for t in o.chunks_exact(2) {
                        if let (Some(c), Some(cid)) = (t[0].as_string(), t[1].as_i64()) {
                            let c = be(&c.bytes);
                            cmap.cid_ranges.push((c, c, cid.max(0) as u32));
                        }
                    }
                }
                b"endbfchar" => {
                    for t in o.chunks_exact(2) {
                        let Some(src) = t[0].as_string() else {
                            continue;
                        };
                        let text = match &t[1] {
                            Object::String(s) => utf16be(&s.bytes),
                            Object::Name(n) => {
                                atelier_font::glyph_name_to_unicode(&n.as_str_lossy())
                            }
                            _ => None,
                        };
                        if let Some(text) = text {
                            cmap.unicode.insert(be(&src.bytes), text);
                        }
                    }
                }
                b"endbfrange" => {
                    for t in o.chunks_exact(3) {
                        let (Some(lo), Some(hi)) = (t[0].as_string(), t[1].as_string()) else {
                            continue;
                        };
                        let (lo, hi) = (be(&lo.bytes), be(&hi.bytes));
                        if hi < lo || hi - lo > 0xFFFF {
                            continue;
                        }
                        match &t[2] {
                            Object::String(s) => {
                                let units: Vec<u16> = s
                                    .bytes
                                    .chunks(2)
                                    .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                                    .collect();
                                if !units.is_empty() {
                                    cmap.unicode_ranges.push((lo, hi, units));
                                }
                            }
                            Object::Array(items) => {
                                for (k, item) in items.iter().enumerate() {
                                    let code = lo + k as u32;
                                    if code > hi {
                                        break;
                                    }
                                    if let Some(text) =
                                        item.as_string().and_then(|s| utf16be(&s.bytes))
                                    {
                                        cmap.unicode.insert(code, text);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                b"usecmap" => {
                    let name = o
                        .first()
                        .and_then(Object::as_name)
                        .map(|n| n.as_bytes().to_vec());
                    if let Some(parent) = name.as_deref().and_then(predefined) {
                        cmap.inherit(parent);
                    }
                }
                b"def" => {
                    if let (Some(Object::Name(n)), Some(v)) = (o.first(), o.get(1)) {
                        if n.as_bytes() == b"WMode" {
                            cmap.vertical = v.as_i64() == Some(1);
                        }
                    }
                }
                _ => {}
            }
        }
        cmap
    }

    /// Adds a parent CMap's mappings under this one's.
    fn inherit(&mut self, parent: CMap) {
        self.codespace.extend(parent.codespace);
        let own = std::mem::replace(&mut self.cid_ranges, parent.cid_ranges);
        self.cid_ranges.extend(own);
        self.identity |= parent.identity;
        self.vertical |= parent.vertical;
    }

    pub fn with_parent(mut self, parent: CMap) -> CMap {
        self.inherit(parent);
        self
    }

    /// Reads the next character code from `data` at `pos`; returns the code
    /// and its length in bytes.
    pub fn next_code(&self, data: &[u8], pos: usize) -> (u32, usize) {
        let rest = &data[pos..];
        for len in 1..=4.min(rest.len()) {
            let bytes = &rest[..len];
            let fits = self.codespace.iter().any(|(l, lo, hi)| {
                *l == len
                    && bytes
                        .iter()
                        .zip(lo.iter().zip(hi))
                        .all(|(b, (lo, hi))| lo <= b && b <= hi)
            });
            if fits {
                return (be(bytes), len);
            }
        }
        // Not in any code space: take the shortest code length.
        let len = self
            .codespace
            .iter()
            .map(|(l, _, _)| *l)
            .min()
            .unwrap_or(1)
            .min(rest.len())
            .max(1);
        (be(&rest[..len]), len)
    }

    pub fn cid(&self, code: u32) -> u32 {
        // Later ranges override earlier ones, as later definitions win.
        for &(lo, hi, cid) in self.cid_ranges.iter().rev() {
            if (lo..=hi).contains(&code) {
                return cid + (code - lo);
            }
        }
        if self.identity { code } else { 0 }
    }

    pub fn unicode(&self, code: u32) -> Option<String> {
        if let Some(s) = self.unicode.get(&code) {
            return Some(s.clone());
        }
        for (lo, hi, base) in self.unicode_ranges.iter().rev() {
            if (*lo..=*hi).contains(&code) {
                let mut units = base.clone();
                if let Some(last) = units.last_mut() {
                    *last = last.wrapping_add((code - lo) as u16);
                }
                return String::from_utf16(&units).ok();
            }
        }
        None
    }

    pub fn has_unicode(&self) -> bool {
        !self.unicode.is_empty() || !self.unicode_ranges.is_empty()
    }
}

fn be(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0, |acc, &b| acc << 8 | u32::from(b))
}

fn utf16be(bytes: &[u8]) -> Option<String> {
    if bytes.len() == 1 {
        return Some(char::from(bytes[0]).to_string());
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cid_and_unicode_maps() {
        let src = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap
            /CMapName /Test def
            2 begincodespacerange <00> <80> <8140> <FFFF> endcodespacerange
            1 begincidrange <8140> <817E> 100 endcidrange
            1 begincidchar <41> 7 endcidchar
            2 beginbfchar <41> <0041> <42> <D83DDE00> endbfchar
            2 beginbfrange <0061> <0063> <0061> <8140> <8141> [<00660069> <00E9>] endbfrange
            endcmap CMapName currentdict /CMap defineresource pop end end";
        let c = CMap::parse(src);
        assert_eq!(c.next_code(b"\x41\x81\x42", 0), (0x41, 1));
        assert_eq!(c.next_code(b"\x41\x81\x42", 1), (0x8142, 2));
        assert_eq!(c.cid(0x8142), 102);
        assert_eq!(c.cid(0x41), 7);
        assert_eq!(c.unicode(0x42).as_deref(), Some("😀"));
        assert_eq!(c.unicode(0x62).as_deref(), Some("b"));
        assert_eq!(c.unicode(0x8140).as_deref(), Some("fi"));
        assert_eq!(c.unicode(0x8141).as_deref(), Some("é"));
        assert_eq!(c.unicode(0x99), None);
    }

    #[test]
    fn usecmap_identity_and_vertical() {
        let c = CMap::parse(b"/Identity-V usecmap");
        assert!(c.vertical);
        assert_eq!(c.next_code(b"\x12\x34", 0), (0x1234, 2));
        assert_eq!(c.cid(0x1234), 0x1234);
        let c = CMap::parse(b"/WMode 1 def");
        assert!(c.vertical);
    }
}
