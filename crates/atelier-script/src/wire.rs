//! The helper process's input and output: each string as its length in
//! bytes, a colon, and the bytes; lists as their length and their items.

use crate::{Commit, CommitOutcome, FieldValue};

const KINDS: [&str; 7] = [
    "text",
    "checkbox",
    "radiobutton",
    "combobox",
    "listbox",
    "button",
    "signature",
];

struct Writer(Vec<u8>);

impl Writer {
    fn str(&mut self, s: &str) {
        self.0.extend_from_slice(format!("{}:", s.len()).as_bytes());
        self.0.extend_from_slice(s.as_bytes());
    }
    fn num(&mut self, n: usize) {
        self.str(&n.to_string());
    }
    fn list<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) {
        self.num(items.len());
        for i in items {
            each(self, i);
        }
    }
    fn opt(&mut self, s: &Option<String>) {
        match s {
            Some(s) => {
                self.num(1);
                self.str(s);
            }
            None => self.num(0),
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn str(&mut self) -> Option<String> {
        let colon = self.0.iter().take(12).position(|&b| b == b':')?;
        let len: usize = std::str::from_utf8(&self.0[..colon]).ok()?.parse().ok()?;
        let rest = &self.0[colon + 1..];
        if rest.len() < len {
            return None;
        }
        let s = String::from_utf8(rest[..len].to_vec()).ok()?;
        self.0 = &rest[len..];
        Some(s)
    }
    fn num(&mut self) -> Option<usize> {
        self.str()?.parse().ok()
    }
    fn list<T>(&mut self, mut each: impl FnMut(&mut Self) -> Option<T>) -> Option<Vec<T>> {
        let n = self.num()?;
        // Each item takes at least two bytes; a bigger count is a lie.
        if n > self.0.len() {
            return None;
        }
        (0..n).map(|_| each(self)).collect()
    }
    fn opt(&mut self) -> Option<Option<String>> {
        match self.num()? {
            0 => Some(None),
            1 => Some(Some(self.str()?)),
            _ => None,
        }
    }
}

pub fn encode_commit(c: &Commit) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.list(&c.document_scripts, |w, s| w.str(s));
    w.list(&c.fields, |w, f| {
        w.str(&f.name);
        w.str(&f.value);
        w.str(f.kind);
        w.num(usize::from(f.hidden));
    });
    w.str(&c.target);
    w.str(&c.value);
    w.opt(&c.keystroke);
    w.opt(&c.validate);
    for list in [&c.calculations, &c.calculated_validate] {
        w.list(list, |w, (t, s)| {
            w.str(t);
            w.str(s);
        });
    }
    w.0
}

pub fn decode_commit(bytes: &[u8]) -> Option<Commit> {
    let mut r = Reader(bytes);
    let document_scripts = r.list(Reader::str)?;
    let fields = r.list(|r| {
        let name = r.str()?;
        let value = r.str()?;
        let kind = r.str()?;
        let kind = KINDS
            .iter()
            .find(|k| **k == kind)
            .copied()
            .unwrap_or("text");
        let hidden = r.num()? == 1;
        Some(FieldValue {
            name,
            value,
            kind,
            hidden,
        })
    })?;
    let target = r.str()?;
    let value = r.str()?;
    let keystroke = r.opt()?;
    let validate = r.opt()?;
    let calculations = r.list(|r| Some((r.str()?, r.str()?)))?;
    let calculated_validate = r.list(|r| Some((r.str()?, r.str()?)))?;
    Some(Commit {
        document_scripts,
        fields,
        target,
        value,
        keystroke,
        validate,
        calculations,
        calculated_validate,
    })
}

pub fn encode_outcome(o: &CommitOutcome) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.num(usize::from(o.accepted));
    w.str(&o.value);
    w.list(&o.changes, |w, (n, v)| {
        w.str(n);
        w.str(v);
    });
    w.list(&o.hidden, |w, (n, h)| {
        w.str(n);
        w.num(usize::from(*h));
    });
    w.list(&o.alerts, |w, s| w.str(s));
    w.list(&o.errors, |w, s| w.str(s));
    w.0
}

pub fn decode_outcome(bytes: &[u8]) -> Option<CommitOutcome> {
    let mut r = Reader(bytes);
    let accepted = r.num()? == 1;
    let value = r.str()?;
    let changes = r.list(|r| Some((r.str()?, r.str()?)))?;
    let hidden = r.list(|r| Some((r.str()?, r.num()? == 1)))?;
    let alerts = r.list(Reader::str)?;
    let errors = r.list(Reader::str)?;
    Some(CommitOutcome {
        accepted,
        value,
        changes,
        hidden,
        alerts,
        errors,
    })
}
