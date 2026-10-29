//! The viewer's settings, kept in a small text file of `key = value`
//! lines in the user's settings folder.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Files open at Fit page (true) or Fit width.
    pub fit_page: bool,
    /// Turn sideways and upside-down pages upright when a file opens.
    pub orient_on_open: bool,
    /// Read the text of scans that have none.
    pub ocr: bool,
    /// The sidebar of page previews and the outline is shown.
    pub sidebar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fit_page: true,
            orient_on_open: true,
            ocr: true,
            sidebar: true,
        }
    }
}

impl Settings {
    /// The settings file: `%APPDATA%\Atelier\settings.txt` on Windows,
    /// `$XDG_CONFIG_HOME/atelier/settings.txt` (or `~/.config/...`)
    /// elsewhere.
    pub fn path() -> Option<PathBuf> {
        if cfg!(windows) {
            std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Atelier"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|d| d.join("atelier"))
        }
        .map(|d| d.join("settings.txt"))
    }

    /// The saved settings, or the defaults for anything missing.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let on = matches!(value.trim(), "true" | "yes" | "1" | "on");
            match key.trim() {
                "zoom" => s.fit_page = value.trim() != "fit-width",
                "orient-on-open" => s.orient_on_open = on,
                "ocr" => s.ocr = on,
                "sidebar" => s.sidebar = on,
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        format!(
            "zoom = {}\norient-on-open = {}\nocr = {}\nsidebar = {}\n",
            if self.fit_page {
                "fit-page"
            } else {
                "fit-width"
            },
            self.orient_on_open,
            self.ocr,
            self.sidebar
        )
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let s = Settings {
            fit_page: false,
            orient_on_open: false,
            ocr: true,
            sidebar: false,
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(
            Settings::parse("nonsense\nocr = no"),
            Settings {
                ocr: false,
                ..Settings::default()
            }
        );
    }
}
