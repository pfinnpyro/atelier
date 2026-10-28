//! Substitute fonts from the operating system, for fonts a file does not
//! embed. Nothing is bundled: Windows, macOS and Linux all ship metric
//! compatible fonts for the standard 14 (Arial, Times New Roman and Courier
//! New, or Liberation on Linux).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use atelier_font::FontProgram;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Sans,
    Serif,
    Mono,
    Symbol,
    Dingbats,
    Cjk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Want {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
}

fn dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(extra) = std::env::var("ATELIER_FONT_DIR") {
        out.push(PathBuf::from(extra));
    }
    if cfg!(windows) {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        out.push(Path::new(&windir).join("Fonts"));
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            out.push(Path::new(&local).join("Microsoft\\Windows\\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        for d in [
            "/System/Library/Fonts/Supplemental",
            "/System/Library/Fonts",
            "/Library/Fonts",
        ] {
            out.push(PathBuf::from(d));
        }
    } else {
        for d in [
            "/usr/share/fonts/truetype/liberation",
            "/usr/share/fonts/truetype/liberation2",
            "/usr/share/fonts/liberation",
            "/usr/share/fonts/liberation-sans",
            "/usr/share/fonts/liberation-serif",
            "/usr/share/fonts/liberation-mono",
            "/usr/share/fonts/TTF",
            "/usr/share/fonts/truetype/dejavu",
            "/usr/share/fonts/dejavu",
            "/usr/share/fonts/truetype/freefont",
            "/usr/share/fonts/truetype/wqy",
            "/usr/share/fonts/wqy-zenhei",
            "/usr/share/fonts/opentype/noto",
            "/usr/share/fonts/noto-cjk",
            "/usr/share/fonts/opentype/ipafont-gothic",
            "/usr/share/fonts/truetype",
        ] {
            out.push(PathBuf::from(d));
        }
    }
    out
}

/// File names to try, best first, for each style: regular, bold, italic,
/// bold italic.
fn candidates(w: Want) -> Vec<&'static str> {
    let style = usize::from(w.bold) + 2 * usize::from(w.italic);
    let pick = |v: [&'static str; 4]| v[style];
    match w.family {
        Family::Sans => vec![
            pick(["arial.ttf", "arialbd.ttf", "ariali.ttf", "arialbi.ttf"]),
            pick([
                "Arial.ttf",
                "Arial Bold.ttf",
                "Arial Italic.ttf",
                "Arial Bold Italic.ttf",
            ]),
            pick([
                "LiberationSans-Regular.ttf",
                "LiberationSans-Bold.ttf",
                "LiberationSans-Italic.ttf",
                "LiberationSans-BoldItalic.ttf",
            ]),
            pick([
                "DejaVuSans.ttf",
                "DejaVuSans-Bold.ttf",
                "DejaVuSans-Oblique.ttf",
                "DejaVuSans-BoldOblique.ttf",
            ]),
            pick([
                "FreeSans.ttf",
                "FreeSansBold.ttf",
                "FreeSansOblique.ttf",
                "FreeSansBoldOblique.ttf",
            ]),
            "arial.ttf",
            "Arial.ttf",
            "LiberationSans-Regular.ttf",
            "DejaVuSans.ttf",
        ],
        Family::Serif => vec![
            pick(["times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf"]),
            pick([
                "Times New Roman.ttf",
                "Times New Roman Bold.ttf",
                "Times New Roman Italic.ttf",
                "Times New Roman Bold Italic.ttf",
            ]),
            pick([
                "LiberationSerif-Regular.ttf",
                "LiberationSerif-Bold.ttf",
                "LiberationSerif-Italic.ttf",
                "LiberationSerif-BoldItalic.ttf",
            ]),
            pick([
                "DejaVuSerif.ttf",
                "DejaVuSerif-Bold.ttf",
                "DejaVuSerif-Italic.ttf",
                "DejaVuSerif-BoldItalic.ttf",
            ]),
            pick([
                "FreeSerif.ttf",
                "FreeSerifBold.ttf",
                "FreeSerifItalic.ttf",
                "FreeSerifBoldItalic.ttf",
            ]),
            "times.ttf",
            "LiberationSerif-Regular.ttf",
            "DejaVuSerif.ttf",
        ],
        Family::Mono => vec![
            pick(["cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf"]),
            pick([
                "Courier New.ttf",
                "Courier New Bold.ttf",
                "Courier New Italic.ttf",
                "Courier New Bold Italic.ttf",
            ]),
            pick([
                "LiberationMono-Regular.ttf",
                "LiberationMono-Bold.ttf",
                "LiberationMono-Italic.ttf",
                "LiberationMono-BoldItalic.ttf",
            ]),
            pick([
                "DejaVuSansMono.ttf",
                "DejaVuSansMono-Bold.ttf",
                "DejaVuSansMono-Oblique.ttf",
                "DejaVuSansMono-BoldOblique.ttf",
            ]),
            pick([
                "FreeMono.ttf",
                "FreeMonoBold.ttf",
                "FreeMonoOblique.ttf",
                "FreeMonoBoldOblique.ttf",
            ]),
            "cour.ttf",
            "LiberationMono-Regular.ttf",
            "DejaVuSansMono.ttf",
        ],
        Family::Symbol => vec![
            "symbol.ttf",
            "Symbol.ttf",
            "DejaVuSans.ttf",
            "FreeSerif.ttf",
        ],
        Family::Dingbats => vec!["wingding.ttf", "DejaVuSans.ttf", "FreeSerif.ttf"],
        Family::Cjk => vec![
            "msgothic.ttc",
            "YuGothR.ttc",
            "msyh.ttc",
            "simsun.ttc",
            "Hiragino Sans GB.ttc",
            "NotoSansCJK-Regular.ttc",
            "wqy-zenhei.ttc",
            "fonts-japanese-gothic.ttf",
            "ipag.ttf",
            "DejaVuSans.ttf",
        ],
    }
}

type Cache = Mutex<HashMap<Want, Option<Arc<FontProgram>>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A system font for the wanted style, loaded once per process.
pub fn substitute(want: Want) -> Option<Arc<FontProgram>> {
    if let Some(hit) = cache().lock().ok()?.get(&want) {
        return hit.clone();
    }
    let dirs = dirs();
    let found = candidates(want).into_iter().find_map(|name| {
        dirs.iter().find_map(|d| {
            let data = std::fs::read(d.join(name)).ok()?;
            FontProgram::parse(&data).ok().map(Arc::new)
        })
    });
    if let Ok(mut c) = cache().lock() {
        c.insert(want, found.clone());
    }
    found
}
