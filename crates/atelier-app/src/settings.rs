//! The viewer's settings, kept in a small text file of `key = value`
//! lines in the user's settings folder.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonDisplay {
    IconOnly,
    #[default]
    IconAndText,
    TextOnly,
}
impl ButtonDisplay {
    pub fn key(self) -> &'static str {
        match self {
            Self::IconOnly => "icon",
            Self::IconAndText => "both",
            Self::TextOnly => "text",
        }
    }
    fn parse(value: &str) -> Self {
        match value {
            "icon" => Self::IconOnly,
            "text" => Self::TextOnly,
            _ => Self::IconAndText,
        }
    }
}
pub const TOOLBAR_ACTIONS: [&str; 6] = [
    "Open",
    "Save",
    "Fit width",
    "Fit page",
    "Side by side",
    "Organize",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub review_author: String,
    pub certify_ink: bool,
    pub theme: String,
    pub toolbar_display: [ButtonDisplay; 6],
    /// Files open at Fit page (true) or Fit width.
    pub fit_page: bool,
    /// Turn sideways and upside-down pages upright when a file opens.
    pub orient_on_open: bool,
    /// Read the text of scans that have none.
    pub ocr: bool,
    /// Save the text OCR reads into the file, as invisible text.
    pub ocr_save: bool,
    /// The sidebar of page previews and the outline is shown.
    pub sidebar: bool,
    /// Look for a new version when Atelier starts.
    pub check_updates: bool,
    /// Offer preview releases as well as stable ones.
    pub preview: bool,
    /// The mark ticked check boxes show, for check boxes Atelier adds.
    pub check_mark: atelier_form::CheckMark,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            review_author: "Reviewer".into(),
            certify_ink: false,
            theme: "system".into(),
            toolbar_display: [ButtonDisplay::IconAndText; 6],
            fit_page: true,
            orient_on_open: true,
            ocr: true,
            ocr_save: true,
            sidebar: true,
            check_updates: true,
            // Someone running a preview build has chosen previews.
            preview: crate::update::current().is_preview(),
            check_mark: atelier_form::CheckMark::default(),
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

    /// Where the OCR models are kept: `%LOCALAPPDATA%\Atelier\ocr-models`
    /// on Windows, `~/.local/share/atelier/ocr-models` elsewhere.
    pub fn ocr_models_dir() -> Option<PathBuf> {
        if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Atelier"))
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
                .map(|d| d.join("atelier"))
        }
        .map(|d| d.join("ocr-models"))
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
            if let Some(index) = key
                .trim()
                .strip_prefix("toolbar-")
                .and_then(|key| key.parse::<usize>().ok())
                .filter(|&index| index < 6)
            {
                s.toolbar_display[index] = ButtonDisplay::parse(value.trim());
                continue;
            }
            match key.trim() {
                "theme" => {
                    s.theme = match value.trim() {
                        "light" => "light",
                        "dark" => "dark",
                        _ => "system",
                    }
                    .into()
                }
                "certify-ink" => s.certify_ink = on,
                "review-author" => s.review_author = value.trim().to_string(),
                "zoom" => s.fit_page = value.trim() != "fit-width",
                "orient-on-open" => s.orient_on_open = on,
                "ocr" => s.ocr = on,
                "ocr-save" => s.ocr_save = on,
                "sidebar" => s.sidebar = on,
                "check-updates" => s.check_updates = on,
                "channel" => s.preview = value.trim() == "preview",
                "check-mark" => {
                    s.check_mark = atelier_form::CheckMark::parse(value).unwrap_or_default();
                }
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let mut text = format!(
            "theme = {}\nreview-author = {}\nzoom = {}\norient-on-open = {}\nocr = {}\nocr-save = {}\nsidebar = {}\ncheck-updates = {}\n\
             channel = {}\n",
            self.theme,
            self.review_author.replace(['\n', '\r'], " "),
            if self.fit_page {
                "fit-page"
            } else {
                "fit-width"
            },
            self.orient_on_open,
            self.ocr,
            self.ocr_save,
            self.sidebar,
            self.check_updates,
            if self.preview { "preview" } else { "stable" }
        );
        text.push_str(&format!("certify-ink = {}\n", self.certify_ink));
        text.push_str(&format!("check-mark = {}\n", self.check_mark.key()));
        for (index, display) in self.toolbar_display.iter().enumerate() {
            text.push_str(&format!("toolbar-{index} = {}\n", display.key()));
        }
        text
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
            review_author: "Ada Lovelace".into(),
            certify_ink: true,
            theme: "light".into(),
            toolbar_display: [
                ButtonDisplay::IconOnly,
                ButtonDisplay::TextOnly,
                ButtonDisplay::IconAndText,
                ButtonDisplay::IconOnly,
                ButtonDisplay::TextOnly,
                ButtonDisplay::IconAndText,
            ],
            fit_page: false,
            orient_on_open: false,
            ocr: true,
            ocr_save: false,
            sidebar: false,
            check_updates: false,
            preview: true,
            check_mark: atelier_form::CheckMark::Cross,
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
