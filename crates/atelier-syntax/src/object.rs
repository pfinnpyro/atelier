//! The PDF object model (ISO 32000-2, section 7.3).

use std::fmt;

/// A name object such as `/Type`, stored as raw bytes after `#xx` unescaping.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Name(pub Vec<u8>);

impl Name {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The name as text, replacing invalid UTF-8.
    pub fn as_str_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl PartialEq<[u8]> for Name {
    fn eq(&self, other: &[u8]) -> bool {
        self.0 == other
    }
}

impl PartialEq<&[u8]> for Name {
    fn eq(&self, other: &&[u8]) -> bool {
        self.0 == *other
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for Name {
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.0 == other[..]
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("/")?;
        for &b in &self.0 {
            if b.is_ascii_graphic() && !b"#()<>[]{}/%".contains(&b) {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "#{b:02X}")?;
            }
        }
        Ok(())
    }
}

/// An indirect reference `num gen R`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ObjRef {
    pub num: u32,
    pub generation: u16,
}

impl ObjRef {
    pub const fn new(num: u32, generation: u16) -> Self {
        Self { num, generation }
    }
}

impl fmt::Display for ObjRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} R", self.num, self.generation)
    }
}

/// A string object. `hex` records whether it was written as `<...>` so a
/// rewrite can keep the original form.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct PdfString {
    pub bytes: Vec<u8>,
    pub hex: bool,
}

impl PdfString {
    pub fn literal(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
            hex: false,
        }
    }
}

impl fmt::Debug for PdfString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for PdfString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let printable = self
            .bytes
            .iter()
            .all(|&b| b.is_ascii_graphic() || b == b' ' || b == b'\n' || b == b'\r' || b == b'\t');
        if self.hex || !printable {
            f.write_str("<")?;
            for b in &self.bytes {
                write!(f, "{b:02X}")?;
            }
            return f.write_str(">");
        }
        f.write_str("(")?;
        for &b in &self.bytes {
            match b {
                b'(' | b')' | b'\\' => write!(f, "\\{}", b as char)?,
                b'\n' => f.write_str("\\n")?,
                b'\r' => f.write_str("\\r")?,
                b'\t' => f.write_str("\\t")?,
                _ => write!(f, "{}", b as char)?,
            }
        }
        f.write_str(")")
    }
}

/// A dictionary. Keys keep their file order, which matters when we write the file
/// back out. Dictionaries are small, so lookups are a linear scan.
#[derive(Clone, PartialEq, Default)]
pub struct Dict(pub Vec<(Name, Object)>);

impl Dict {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn get(&self, key: &[u8]) -> Option<&Object> {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k.0 == key)
            .map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    /// Sets a key, replacing an existing value.
    pub fn insert(&mut self, key: impl Into<Name>, value: Object) {
        let key = key.into();
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            self.0.push((key, value));
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Object)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The `/Type` entry, if it is a direct name.
    pub fn type_name(&self) -> Option<&Name> {
        self.get(b"Type").and_then(Object::as_name)
    }

    pub fn is_type(&self, name: &[u8]) -> bool {
        self.type_name().is_some_and(|n| n.0 == name)
    }
}

impl fmt::Debug for Dict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Dict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<<")?;
        for (k, v) in &self.0 {
            write!(f, " {k} {v}")?;
        }
        f.write_str(" >>")
    }
}

/// A stream: its dictionary plus the raw, still encoded bytes.
#[derive(Clone, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    pub data: std::sync::Arc<[u8]>,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} stream[{} bytes]", self.dict, self.data.len())
    }
}

#[derive(Clone, PartialEq, Default)]
pub enum Object {
    #[default]
    Null,
    Bool(bool),
    Integer(i64),
    Real(f64),
    String(PdfString),
    Name(Name),
    Array(Vec<Object>),
    Dict(Dict),
    Stream(Stream),
    Reference(ObjRef),
}

impl Object {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Integer(_) => "integer",
            Self::Real(_) => "real",
            Self::String(_) => "string",
            Self::Name(_) => "name",
            Self::Array(_) => "array",
            Self::Dict(_) => "dictionary",
            Self::Stream(_) => "stream",
            Self::Reference(_) => "reference",
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(i) => Some(*i),
            // Some producers write integers as reals, e.g. `/Length 120.0`.
            Self::Real(r) if r.fract() == 0.0 && r.abs() < 9.0e15 => Some(*r as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(i) => Some(*i as f64),
            Self::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&Name> {
        match self {
            Self::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&PdfString> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Self::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The dictionary of a dictionary or of a stream.
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Self::Dict(d) => Some(d),
            Self::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    pub fn as_stream(&self) -> Option<&Stream> {
        match self {
            Self::Stream(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_reference(&self) -> Option<ObjRef> {
        match self {
            Self::Reference(r) => Some(*r),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stream(s) => fmt::Debug::fmt(s, f),
            other => fmt::Display::fmt(other, f),
        }
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Integer(i) => write!(f, "{i}"),
            Self::Real(r) => write_real(f, *r),
            Self::String(s) => write!(f, "{s}"),
            Self::Name(n) => write!(f, "{n}"),
            Self::Array(items) => {
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
            Self::Dict(d) => write!(f, "{d}"),
            Self::Stream(s) => write!(f, "{} stream[{} bytes]", s.dict, s.data.len()),
            Self::Reference(r) => write!(f, "{r}"),
        }
    }
}

/// Writes a real without exponent notation, which PDF does not allow.
fn write_real(f: &mut fmt::Formatter<'_>, r: f64) -> fmt::Result {
    if !r.is_finite() {
        return f.write_str("0");
    }
    let s = format!("{r:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    f.write_str(if s == "-0" || s.is_empty() { "0" } else { s })
}

impl From<i64> for Object {
    fn from(v: i64) -> Self {
        Self::Integer(v)
    }
}

impl From<f64> for Object {
    fn from(v: f64) -> Self {
        Self::Real(v)
    }
}

impl From<bool> for Object {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<Name> for Object {
    fn from(v: Name) -> Self {
        Self::Name(v)
    }
}

impl From<Dict> for Object {
    fn from(v: Dict) -> Self {
        Self::Dict(v)
    }
}

impl From<ObjRef> for Object {
    fn from(v: ObjRef) -> Self {
        Self::Reference(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_round_trips_common_forms() {
        let mut d = Dict::new();
        d.insert("Type", Name::from("Page").into());
        d.insert("Parent", ObjRef::new(3, 0).into());
        d.insert(
            "MediaBox",
            Object::Array(vec![
                0i64.into(),
                0i64.into(),
                612.5f64.into(),
                792i64.into(),
            ]),
        );
        d.insert(
            "Title",
            Object::String(PdfString::literal(b"a (b)".to_vec())),
        );
        assert_eq!(
            d.to_string(),
            "<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612.5 792] /Title (a \\(b\\)) >>"
        );
    }

    #[test]
    fn names_escape_delimiters() {
        assert_eq!(Name::new(b"A B#".to_vec()).to_string(), "/A#20B#23");
    }

    #[test]
    fn later_duplicate_key_wins() {
        let d = Dict(vec![
            (Name::from("K"), Object::Integer(1)),
            (Name::from("K"), Object::Integer(2)),
        ]);
        assert_eq!(d.get(b"K"), Some(&Object::Integer(2)));
    }
}
