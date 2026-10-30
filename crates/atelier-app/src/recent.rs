//! Files opened lately, newest first, kept one path per line in
//! `recent.txt` beside the settings file.

use std::path::{Path, PathBuf};

use crate::settings::Settings;

/// How many files the list keeps.
const KEEP: usize = 12;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Recent {
    pub files: Vec<PathBuf>,
}

impl Recent {
    fn path() -> Option<PathBuf> {
        Settings::path().map(|p| p.with_file_name("recent.txt"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    pub fn parse(text: &str) -> Self {
        let mut r = Self::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()).rev() {
            r.push(Path::new(line));
        }
        r
    }

    pub fn to_text(&self) -> String {
        self.files
            .iter()
            .map(|p| format!("{}\n", p.display()))
            .collect()
    }

    /// Puts a file at the top of the list.
    fn push(&mut self, path: &Path) {
        self.files.retain(|p| !same_file(p, path));
        self.files.insert(0, path.to_path_buf());
        self.files.truncate(KEEP);
    }

    /// Puts a file at the top and saves the list.
    pub fn add(&mut self, path: &Path) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.push(&path);
        self.save();
    }

    /// Drops a file, such as one that is no longer there.
    pub fn remove(&mut self, path: &Path) {
        self.files.retain(|p| !same_file(p, path));
        self.save();
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.save();
    }

    fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // The list is a convenience; failing to keep it is not worth a
        // message.
        let _ = std::fs::write(path, self.to_text());
    }
}

/// Whether two paths name the same file. Windows paths ignore case.
pub fn same_file(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_repeats() {
        let mut r = Recent::parse("/a.pdf\n\n/b.pdf\n");
        assert_eq!(
            r.files,
            vec![PathBuf::from("/a.pdf"), PathBuf::from("/b.pdf")]
        );
        r.push(Path::new("/b.pdf"));
        assert_eq!(
            r.files,
            vec![PathBuf::from("/b.pdf"), PathBuf::from("/a.pdf")]
        );
        for i in 0..20 {
            r.push(&PathBuf::from(format!("/{i}.pdf")));
        }
        assert_eq!(r.files.len(), KEEP);
        assert_eq!(r.files[0], PathBuf::from("/19.pdf"));
        assert_eq!(Recent::parse(&r.to_text()), r);
    }
}
