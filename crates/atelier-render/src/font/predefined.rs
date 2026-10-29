//! Adobe's predefined CJK CMaps (ISO 32000-2, 9.7.5.2), compiled into
//! `data/cmaps.bin` by tools/gen_cmaps.py from Adobe's cmap-resources
//! (BSD-3-Clause; see data/ADOBE-CMAPS-LICENSE.txt), and the CID to Unicode
//! tables of their character collections, inverted from the UTF-32 CMaps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::cmap::CMap;

static PACKED: &[u8] = include_bytes!("data/cmaps.bin");

struct Entry {
    parent: String,
    ordering: Option<String>,
    vertical: bool,
    codespace: Vec<(usize, Vec<u8>, Vec<u8>)>,
    ranges: Vec<(u32, u32, u32)>,
}

struct Table {
    /// Collection (Japan1, GB1, ...) to its UTF-32 CMap.
    utf32: HashMap<String, String>,
    cmaps: HashMap<String, Entry>,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn uint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    fn int(&mut self) -> Option<i64> {
        let v = self.uint()?;
        Some(if v & 1 == 0 {
            (v >> 1) as i64
        } else {
            -((v >> 1) as i64) - 1
        })
    }

    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let out = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(out)
    }

    fn name(&mut self) -> Option<String> {
        let n = self.uint()? as usize;
        Some(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
}

fn parse(data: &[u8]) -> Option<Table> {
    let mut r = Reader { data, pos: 0 };
    if r.bytes(4)? != b"ACM1" {
        return None;
    }
    let mut orderings = Vec::new();
    let mut utf32 = HashMap::new();
    for _ in 0..r.uint()? {
        let o = r.name()?;
        utf32.insert(o.clone(), r.name()?);
        orderings.push(o);
    }
    let mut cmaps = HashMap::new();
    for _ in 0..r.uint()? {
        let name = r.name()?;
        let parent = r.name()?;
        let ordering = (r.uint()? as usize)
            .checked_sub(1)
            .and_then(|i| orderings.get(i).cloned());
        let vertical = r.byte()? == 1;
        let mut codespace = Vec::new();
        for _ in 0..r.uint()? {
            let len = usize::from(r.byte()?);
            let lo = r.bytes(len)?.to_vec();
            let hi = r.bytes(len)?.to_vec();
            codespace.push((len, lo, hi));
        }
        let n = r.uint()? as usize;
        let mut ranges = Vec::with_capacity(n.min(1 << 16));
        let (mut prev_hi, mut prev_end) = (-1i64, 0i64);
        for _ in 0..n {
            let lo = prev_hi + r.int()?;
            let hi = lo + r.uint()? as i64;
            let cid = prev_end + r.int()?;
            let (Ok(l), Ok(h), Ok(c)) = (u32::try_from(lo), u32::try_from(hi), u32::try_from(cid))
            else {
                return None;
            };
            ranges.push((l, h, c));
            prev_hi = hi;
            prev_end = cid + (hi - lo);
        }
        cmaps.insert(
            name,
            Entry {
                parent,
                ordering,
                vertical,
                codespace,
                ranges,
            },
        );
    }
    Some(Table { utf32, cmaps })
}

fn table() -> Option<&'static Table> {
    static TABLE: OnceLock<Option<Table>> = OnceLock::new();
    TABLE
        .get_or_init(|| parse(&atelier_filters::decode_flate(PACKED).ok()?))
        .as_ref()
}

/// The predefined CMap called `name`, with its parents.
pub fn load(name: &[u8]) -> Option<CMap> {
    load_depth(std::str::from_utf8(name).ok()?, 0)
}

fn load_depth(name: &str, depth: usize) -> Option<CMap> {
    if depth > 4 {
        return None;
    }
    let t = table()?;
    let Some(e) = t.cmaps.get(name) else {
        // UTF-16 horizontal CMaps are their UTF-32 twins re-encoded.
        let utf32 = name.strip_suffix("-UTF16-H")?;
        return load_depth(&format!("{utf32}-UTF32-H"), depth + 1).map(|c| to_utf16(&c));
    };
    let parent = match e.parent.as_str() {
        "" => None,
        "Identity-H" | "Identity-V" => CMap::predefined(e.parent.as_bytes()),
        p => Some(load_depth(p, depth + 1)?),
    };
    Some(CMap::from_parts(
        e.codespace.clone(),
        &e.ranges,
        e.vertical,
        parent,
    ))
}

/// Re-encodes a UTF-32 CMap's codes as UTF-16.
fn to_utf16(c: &CMap) -> CMap {
    let mut ranges = Vec::new();
    for &(lo, hi, cid) in c.ranges() {
        let mut code = lo;
        while code <= hi {
            if code < 0x10000 {
                let end = hi.min(0xFFFF);
                ranges.push((code, end, cid + (code - lo)));
                code = end + 1;
            } else {
                // Supplementary planes: a surrogate pair, contiguous within
                // each run of 1024 code points.
                let end = hi.min(code | 0x3FF);
                let u = code - 0x10000;
                let pair = (0xD800 + (u >> 10)) << 16 | (0xDC00 + (u & 0x3FF));
                ranges.push((pair, pair + (end - code), cid + (code - lo)));
                code = end + 1;
            }
            if code == 0 {
                break;
            }
        }
    }
    let codespace = vec![
        (2, vec![0x00, 0x00], vec![0xD7, 0xFF]),
        (
            4,
            vec![0xD8, 0x00, 0xDC, 0x00],
            vec![0xDB, 0xFF, 0xDF, 0xFF],
        ),
        (2, vec![0xE0, 0x00], vec![0xFF, 0xFF]),
    ];
    CMap::from_parts(codespace, &ranges, false, None)
}

/// The character collection (Japan1, GB1, CNS1, Korea1, KR) a predefined
/// CMap belongs to.
pub fn ordering(name: &[u8]) -> Option<&'static str> {
    let t = table()?;
    let mut name = std::str::from_utf8(name).ok()?.to_string();
    if let Some(stem) = name.strip_suffix("-UTF16-H") {
        name = format!("{stem}-UTF32-H");
    }
    t.cmaps.get(&name)?.ordering.as_deref()
}

/// CID to Unicode for a character collection, by CID; 0 where a CID has
/// no code point.
pub fn cid_to_unicode(ordering: &str) -> Option<Arc<Vec<u32>>> {
    type Cache = Mutex<HashMap<String, Option<Arc<Vec<u32>>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok()?.get(ordering) {
        return hit.clone();
    }
    let built = build_cid_to_unicode(ordering).map(Arc::new);
    if let Ok(mut c) = cache.lock() {
        c.insert(ordering.to_string(), built.clone());
    }
    built
}

fn build_cid_to_unicode(ordering: &str) -> Option<Vec<u32>> {
    let t = table()?;
    let e = t.cmaps.get(t.utf32.get(ordering)?)?;
    let max = e.ranges.iter().map(|r| r.2 + (r.1 - r.0)).max()?;
    let mut out = vec![0u32; (max as usize + 1).min(1 << 17)];
    // Several code points can share a CID: 日 and the radical ⽇, say.
    // Keep the ordinary character: the lowest rank, then the first.
    for &(lo, hi, cid) in &e.ranges {
        for k in 0..=(hi - lo) {
            let Some(slot) = out.get_mut((cid + k) as usize) else {
                break;
            };
            let u = lo + k;
            if *slot == 0 || rank(u) < rank(*slot) {
                *slot = u;
            }
        }
    }
    // CIDs the UTF-32 CMap leaves out, like Japan1's half-width Roman, come
    // from the collection's other Unicode CMaps (UCS-2 codes are code
    // points too).
    let mut others: Vec<&String> = t
        .cmaps
        .iter()
        .filter(|(name, e)| {
            name.starts_with("Uni")
                && (name.contains("-UCS2-") || name.contains("-UTF32-"))
                && e.ordering.as_deref() == Some(ordering)
        })
        .map(|(name, _)| name)
        .collect();
    others.sort();
    for name in others {
        let Some(cmap) = load(name.as_bytes()) else {
            continue;
        };
        for &(lo, hi, cid) in cmap.ranges() {
            for k in 0..=(hi - lo).min(1 << 17) {
                if let Some(slot) = out.get_mut((cid + k) as usize) {
                    if *slot == 0 {
                        *slot = lo + k;
                    }
                }
            }
        }
    }
    Some(out)
}

/// How unlikely a code point is to be the character text means: radicals,
/// compatibility ideographs and presentation forms after everything else,
/// and other planes after the BMP.
fn rank(u: u32) -> u8 {
    match u {
        0x2E80..=0x2FDF | 0xF900..=0xFAFF | 0x2F800..=0x2FA1F | 0xFE30..=0xFE4F => 2,
        0x10000.. => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_cmaps_load() {
        let t = table().expect("the CMap table parses");
        assert!(t.cmaps.len() > 100);
        for name in t.cmaps.keys() {
            assert!(load(name.as_bytes()).is_some(), "{name}");
        }
    }

    #[test]
    fn shift_jis_and_unicode_cmaps() {
        // "あ" is CID 843 in Adobe-Japan1: 0x82A0 in Shift-JIS, U+3042.
        let sjis = load(b"90ms-RKSJ-H").unwrap();
        assert_eq!(sjis.next_code(b"\x82\xA0", 0), (0x82A0, 2));
        assert_eq!(sjis.cid(0x82A0), 843);
        assert_eq!(sjis.next_code(b"A", 0), (0x41, 1));
        let ucs2 = load(b"UniJIS-UCS2-H").unwrap();
        assert_eq!(ucs2.cid(0x3042), 843);
        let utf16 = load(b"UniJIS-UTF16-H").unwrap();
        assert_eq!(utf16.cid(0x3042), 843);
        // U+20B9F (𠮟) as a surrogate pair.
        let utf32 = load(b"UniJIS-UTF32-H").unwrap();
        assert_eq!(utf16.next_code(b"\xD8\x42\xDF\x9F", 0), (0xD842DF9F, 4));
        assert_eq!(utf16.cid(0xD842DF9F), utf32.cid(0x20B9F));
        assert_ne!(utf32.cid(0x20B9F), 0);
        let vertical = load(b"UniJIS-UTF16-V").unwrap();
        assert!(vertical.vertical);
        assert_eq!(vertical.cid(0x3042), 843);
        assert_eq!(ordering(b"UniJIS-UTF16-V"), Some("Japan1"));
        assert_eq!(ordering(b"GBK-EUC-H"), Some("GB1"));
    }

    #[test]
    fn collections_map_back_to_unicode() {
        let japan = cid_to_unicode("Japan1").unwrap();
        assert_eq!(japan[843], 0x3042);
        // 日, not the Kangxi radical that shares its CID.
        let utj = load(b"UniJIS-UTF32-H").unwrap();
        assert_eq!(japan[utj.cid(0x65E5) as usize], 0x65E5);
        let gb = cid_to_unicode("GB1").unwrap();
        let cid = load(b"UniGB-UCS2-H").unwrap().cid(0x4E2D); // 中
        assert_eq!(gb[cid as usize], 0x4E2D);
        assert!(cid_to_unicode("Identity").is_none());
    }
}
