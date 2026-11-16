//! Portable, reviewed labels. TSV columns: split, form, field, class, label, optional section.
//! Forms stay together, and each form belongs to exactly one split.
use crate::Class;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Field {
    pub id: String,
    pub class: Class,
    pub label: String,
    pub section: String,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub split: String,
    pub id: String,
    pub fields: Vec<Field>,
}

impl Form {
    pub fn labels(&self) -> Vec<(String, Class)> {
        self.fields
            .iter()
            .map(|f| (crate::contextual_label(&f.label, &f.section), f.class))
            .collect()
    }
}

/// Reject malformed rows, unknown classes/splits, duplicate fields and
/// split leakage. Labels cannot contain literal tabs or newlines.
pub fn parse(tsv: &str) -> Result<Vec<Form>, String> {
    let mut forms: Vec<Form> = Vec::new();
    let mut indexes = HashMap::new();
    let mut fields = HashSet::new();
    for (line, row) in tsv.lines().enumerate() {
        if row.starts_with('#') || row.is_empty() {
            continue;
        }
        let fail = || format!("invalid corpus row {}", line + 1);
        let cols: Vec<_> = row.split('\t').collect();
        if !(5..=6).contains(&cols.len()) {
            return Err(fail());
        }
        let (split, id, field, class, label) = (cols[0], cols[1], cols[2], cols[3], cols[4]);
        let section = cols.get(5).copied().unwrap_or("");
        if !["train", "validation", "test"].contains(&split)
            || id.is_empty()
            || field.is_empty()
            || label.trim().is_empty()
        {
            return Err(fail());
        }
        let class = Class::ALL
            .into_iter()
            .find(|c| format!("{c:?}") == class)
            .ok_or_else(fail)?;
        if !fields.insert((id.to_string(), field.to_string())) {
            return Err(fail());
        }
        let at = *indexes.entry(id.to_string()).or_insert_with(|| {
            forms.push(Form {
                split: split.to_string(),
                id: id.to_string(),
                fields: Vec::new(),
            });
            forms.len() - 1
        });
        if forms[at].split != split {
            return Err(format!("form {id} leaks across splits"));
        }
        forms[at].fields.push(Field {
            id: field.to_string(),
            class,
            label: label.to_string(),
            section: section.to_string(),
        });
    }
    if forms.is_empty() {
        return Err("empty corpus".into());
    }
    Ok(forms)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_leakage_and_bad_labels() {
        let row = "train\tpublic-w9\tname\tFullName\tName\n";
        assert_eq!(parse(row).unwrap()[0].fields[0].class, Class::FullName);
        for bad in [
            format!("{row}test\tpublic-w9\taddress\tAddressWhole\tAddress\n"),
            format!("{row}{row}"),
            row.replace("FullName", "Invented"),
            row.replace("train", "eval"),
            String::new(),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }
}
