//! Cross-reference data (ISO 32000-2, 7.5.4 to 7.5.8) and repair of files whose
//! cross-reference data is missing or wrong.

use std::collections::{HashMap, HashSet};

use crate::decode::decode;
use crate::lexer::{is_regular, is_whitespace};
use crate::parser::{find, rfind};
use crate::{Dict, Error, Lexer, ObjRef, Object, Parser, Result, Token};

/// Where an object lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefEntry {
    Free,
    /// A top-level object at a byte offset.
    Offset {
        offset: usize,
        generation: u16,
    },
    /// Object number `index` inside object stream `stream`.
    Compressed {
        stream: u32,
        index: u32,
    },
}

#[derive(Debug, Default)]
pub(crate) struct XrefTable {
    pub entries: HashMap<u32, XrefEntry>,
    pub trailer: Dict,
}

impl XrefTable {
    /// Adds entries that newer sections have not already defined.
    fn merge_older(&mut self, entries: Vec<(u32, XrefEntry)>) {
        for (num, entry) in entries {
            self.entries.entry(num).or_insert(entry);
        }
    }
}

/// Reads the cross-reference chain starting at `startxref`.
/// `shift` is added to every offset (bytes of junk before `%PDF-`).
pub(crate) fn read(data: &[u8], shift: usize) -> Result<XrefTable> {
    let start = find_startxref(data)?;
    let mut table = XrefTable::default();
    let mut visited = HashSet::new();
    let mut next = Some(start);
    let mut first = true;

    while let Some(offset) = next.take() {
        if !visited.insert(offset) {
            break; // /Prev loop
        }
        let (entries, trailer) = read_section(data, offset).or_else(|e| {
            if shift > 0 {
                read_section(data, offset + shift)
            } else {
                Err(e)
            }
        })?;
        table.merge_older(entries);

        // Hybrid files: the table's /XRefStm stream comes before /Prev.
        if let Some(stm) = trailer.get(b"XRefStm").and_then(Object::as_i64) {
            if let Ok((entries, _)) = usize::try_from(stm)
                .map_err(|_| Error::Structure("bad /XRefStm".into()))
                .and_then(|o| read_section(data, o))
            {
                table.merge_older(entries);
            }
        }

        next = trailer
            .get(b"Prev")
            .and_then(Object::as_i64)
            .and_then(|p| usize::try_from(p).ok());
        if first {
            table.trailer = trailer;
            first = false;
        } else {
            // Older trailers fill in keys the newest one lacks.
            for (k, v) in trailer.iter() {
                if !table.trailer.contains_key(k.as_bytes()) && k.as_bytes() != b"Prev" {
                    table.trailer.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if shift > 0 {
        for entry in table.entries.values_mut() {
            if let XrefEntry::Offset { offset, .. } = entry {
                if !looks_like_object(data, *offset) && looks_like_object(data, *offset + shift) {
                    *offset += shift;
                }
            }
        }
    }
    Ok(table)
}

fn find_startxref(data: &[u8]) -> Result<usize> {
    let tail_start = data.len().saturating_sub(4096);
    let pos = rfind(&data[tail_start..], b"startxref")
        .map(|p| p + tail_start)
        .ok_or_else(|| Error::Structure("no startxref".into()))?;
    let mut lx = Lexer::at(data, pos + b"startxref".len());
    match lx.next_token()? {
        Some(Token::Integer(o)) if o >= 0 && (o as usize) < data.len() => Ok(o as usize),
        _ => Err(Error::Structure("startxref offset is invalid".into())),
    }
}

type Section = (Vec<(u32, XrefEntry)>, Dict);

fn read_section(data: &[u8], offset: usize) -> Result<Section> {
    let mut lx = Lexer::at(data, offset);
    match lx.peek_token()? {
        Some(Token::Keyword(b"xref")) => read_table(data, offset),
        Some(Token::Integer(_)) => read_stream(data, offset),
        _ => Err(Error::syntax(offset, "no cross-reference section here")),
    }
}

/// A classic `xref` table followed by `trailer << ... >>`.
fn read_table(data: &[u8], offset: usize) -> Result<Section> {
    let mut parser = Parser::at(data, offset);
    parser.lexer().next_token()?; // xref
    let mut entries = Vec::new();
    loop {
        let lx = parser.lexer();
        let pos = lx.pos();
        match lx.next_token()? {
            Some(Token::Keyword(b"trailer")) => break,
            Some(Token::Integer(start)) => {
                let Some(Token::Integer(count)) = lx.next_token()? else {
                    return Err(Error::syntax(pos, "bad xref subsection header"));
                };
                let mut num = u32::try_from(start).map_err(|_| Error::syntax(pos, "bad start"))?;
                for i in 0..count.max(0) {
                    let row = (lx.next_token()?, lx.next_token()?, lx.next_token()?);
                    let (
                        Some(Token::Integer(off)),
                        Some(Token::Integer(generation)),
                        Some(Token::Keyword(kind)),
                    ) = row
                    else {
                        return Err(Error::syntax(lx.pos(), "bad xref row"));
                    };
                    // A common producer bug: the first subsection says it starts at
                    // 1 but its first row is the free head of the list, object 0.
                    if i == 0 && num == 1 && kind == b"f" && generation == 65535 {
                        num = 0;
                    }
                    let entry = match (kind, usize::try_from(off)) {
                        (b"n", Ok(off)) if off > 0 => XrefEntry::Offset {
                            offset: off,
                            generation: u16::try_from(generation).unwrap_or(0),
                        },
                        _ => XrefEntry::Free,
                    };
                    entries.push((num, entry));
                    num = num.saturating_add(1);
                }
            }
            _ => return Err(Error::syntax(pos, "expected xref subsection or trailer")),
        }
    }
    let trailer = match parser.parse_object()? {
        Object::Dict(d) => d,
        _ => return Err(Error::syntax(parser.pos(), "trailer is not a dictionary")),
    };
    Ok((entries, trailer))
}

/// A cross-reference stream (PDF 1.5+).
fn read_stream(data: &[u8], offset: usize) -> Result<Section> {
    let (_, obj) = Parser::at(data, offset).parse_indirect(&|_| None)?;
    let Object::Stream(stream) = obj else {
        return Err(Error::syntax(offset, "expected a cross-reference stream"));
    };
    let entries = xref_stream_entries(&stream.dict, &stream.data)?;
    Ok((entries, stream.dict))
}

fn xref_stream_entries(dict: &Dict, raw: &[u8]) -> Result<Vec<(u32, XrefEntry)>> {
    let decoded = decode(raw, dict, &|o| o.clone(), &mut |_| {})?.data;
    let widths: Vec<usize> = dict
        .get(b"W")
        .and_then(Object::as_array)
        .ok_or_else(|| Error::Structure("xref stream without /W".into()))?
        .iter()
        .map(|w| {
            w.as_i64()
                .and_then(|w| usize::try_from(w).ok())
                .unwrap_or(0)
        })
        .collect();
    if widths.len() < 3 || widths.iter().any(|&w| w > 8) {
        return Err(Error::Structure("bad /W in xref stream".into()));
    }
    let row_len: usize = widths.iter().sum();
    if row_len == 0 {
        return Err(Error::Structure("empty /W in xref stream".into()));
    }
    let size = dict.get(b"Size").and_then(Object::as_i64).unwrap_or(0);
    let index: Vec<i64> = match dict.get(b"Index").and_then(Object::as_array) {
        Some(items) => items.iter().filter_map(Object::as_i64).collect(),
        None => vec![0, size],
    };

    let field = |row: &[u8], i: usize| -> u64 {
        let start: usize = widths[..i].iter().sum();
        row[start..start + widths[i]]
            .iter()
            .fold(0u64, |acc, &b| acc << 8 | u64::from(b))
    };

    let mut entries = Vec::new();
    let mut rows = decoded.chunks_exact(row_len);
    for pair in index.chunks_exact(2) {
        let (Ok(start), Ok(count)) = (u32::try_from(pair[0]), u32::try_from(pair[1])) else {
            continue;
        };
        for k in 0..count {
            let Some(row) = rows.next() else {
                return Ok(entries);
            };
            let kind = if widths[0] == 0 { 1 } else { field(row, 0) };
            let (a, b) = (field(row, 1), field(row, 2));
            let entry = match kind {
                0 => XrefEntry::Free,
                1 => XrefEntry::Offset {
                    offset: a as usize,
                    generation: u16::try_from(b).unwrap_or(0),
                },
                2 => XrefEntry::Compressed {
                    stream: u32::try_from(a).unwrap_or(0),
                    index: u32::try_from(b).unwrap_or(0),
                },
                _ => continue,
            };
            entries.push((start.saturating_add(k), entry));
        }
    }
    Ok(entries)
}

/// True if `num gen obj` starts at `offset`.
pub(crate) fn looks_like_object(data: &[u8], offset: usize) -> bool {
    let mut lx = Lexer::at(data, offset);
    matches!(
        (lx.next_token(), lx.next_token(), lx.next_token()),
        (
            Ok(Some(Token::Integer(_))),
            Ok(Some(Token::Integer(_))),
            Ok(Some(Token::Keyword(b"obj")))
        )
    )
}

/// Scans the whole file for `num gen obj` headers. Later definitions win, as
/// they would in an incremental update.
pub(crate) fn scan_objects(data: &[u8]) -> HashMap<u32, (usize, u16)> {
    let mut found = HashMap::new();
    let mut from = 0;
    while let Some(pos) = find(data, b"obj", from) {
        from = pos + 3;
        if data.get(pos + 3).is_some_and(|&b| is_regular(b)) {
            continue; // `objects`, `obj1`...
        }
        if let Some((num, generation, start)) = header_before(data, pos) {
            found.insert(num, (start, generation));
        }
    }
    found
}

/// Walks back from `obj` over `num ws gen ws`.
fn header_before(data: &[u8], obj_pos: usize) -> Option<(u32, u16, usize)> {
    let mut i = obj_pos;
    let skip_ws = |i: &mut usize| {
        let before = *i;
        while *i > 0 && is_whitespace(data[*i - 1]) {
            *i -= 1;
        }
        *i < before
    };
    let digits = |i: &mut usize| {
        let end = *i;
        while *i > 0 && data[*i - 1].is_ascii_digit() && end - *i < 10 {
            *i -= 1;
        }
        (*i < end).then(|| {
            std::str::from_utf8(&data[*i..end])
                .ok()?
                .parse::<u64>()
                .ok()
        })?
    };
    if !skip_ws(&mut i) {
        return None;
    }
    let generation = digits(&mut i)?;
    if !skip_ws(&mut i) {
        return None;
    }
    let num = digits(&mut i)?;
    if i > 0 && is_regular(data[i - 1]) {
        return None;
    }
    Some((u32::try_from(num).ok()?, u16::try_from(generation).ok()?, i))
}

/// Rebuilds cross-reference data by scanning the file.
pub(crate) fn repair(data: &[u8]) -> Result<XrefTable> {
    let scanned = scan_objects(data);
    let mut table = XrefTable::default();
    for (&num, &(offset, generation)) in &scanned {
        table
            .entries
            .insert(num, XrefEntry::Offset { offset, generation });
    }

    let mut catalog: Option<(usize, ObjRef)> = None;
    let mut xref_stream_trailer: Option<(usize, Dict)> = None;
    let mut object_streams: Vec<(u32, crate::Stream)> = Vec::new();

    for (&num, &(offset, _)) in &scanned {
        let Ok((r, obj)) = Parser::at(data, offset).parse_indirect(&|_| None) else {
            continue;
        };
        let Some(dict) = obj.as_dict() else { continue };
        if dict.is_type(b"Catalog") && catalog.is_none_or(|(o, _)| offset > o) {
            catalog = Some((offset, r));
        }
        if dict.is_type(b"XRef")
            && dict.contains_key(b"Root")
            && xref_stream_trailer
                .as_ref()
                .is_none_or(|(o, _)| offset > *o)
        {
            xref_stream_trailer = Some((offset, dict.clone()));
        }
        if dict.is_type(b"ObjStm") {
            if let Object::Stream(s) = obj {
                object_streams.push((num, s));
            }
        }
    }

    // Objects inside object streams, unless a top-level copy exists.
    object_streams.sort_by_key(|(num, _)| *num);
    for (stream_num, stream) in object_streams {
        let Ok(decoded) = decode(&stream.data, &stream.dict, &|o| o.clone(), &mut |_| {}) else {
            continue;
        };
        let n = stream.dict.get(b"N").and_then(Object::as_i64).unwrap_or(0);
        let mut lx = Lexer::new(&decoded.data);
        for index in 0..u32::try_from(n).unwrap_or(0) {
            let Ok(Some(Token::Integer(num))) = lx.next_token() else {
                break;
            };
            let Ok(Some(Token::Integer(_))) = lx.next_token() else {
                break;
            };
            if let Ok(num) = u32::try_from(num) {
                table.entries.entry(num).or_insert(XrefEntry::Compressed {
                    stream: stream_num,
                    index,
                });
            }
        }
    }

    // Trailer: the last `trailer` dictionary with a /Root, then an xref stream's
    // dictionary, then a synthesized one pointing at the catalog we found.
    let mut from = 0;
    let mut trailer: Option<Dict> = None;
    while let Some(pos) = find(data, b"trailer", from) {
        from = pos + 7;
        if let Ok(Object::Dict(d)) = Parser::at(data, from).parse_object() {
            if d.contains_key(b"Root") {
                trailer = Some(d);
            }
        }
    }
    let mut trailer = trailer
        .or(xref_stream_trailer.map(|(_, d)| d))
        .unwrap_or_default();
    let root_ok = trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .is_some_and(|r| table.entries.contains_key(&r.num));
    if !root_ok {
        let (_, r) =
            catalog.ok_or_else(|| Error::Structure("no catalog found while repairing".into()))?;
        trailer.insert("Root", Object::Reference(r));
    }
    table.trailer = trailer;
    Ok(table)
}
