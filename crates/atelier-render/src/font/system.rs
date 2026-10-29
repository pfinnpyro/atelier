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
    Cjk { script: Script, serif: bool },
}

/// Which CJK fonts suit a character collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Script {
    Japanese,
    Simplified,
    Traditional,
    Korean,
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
        Family::Cjk { script, serif } => cjk(script, serif),
    }
}

/// CJK fonts for a script, the style asked for first. Windows, macOS and
/// common Linux packages; any CJK font beats none, so the others follow.
fn cjk(script: Script, serif: bool) -> Vec<&'static str> {
    let (sans, mincho) = cjk_lists(script);
    let (first, second) = if serif {
        (mincho, sans)
    } else {
        (sans, mincho)
    };
    let mut out: Vec<&'static str> = first.iter().chain(second).copied().collect();
    // Then any CJK font at all, and Unicode fonts with some coverage.
    for s in [
        Script::Japanese,
        Script::Simplified,
        Script::Traditional,
        Script::Korean,
    ] {
        let (a, b) = cjk_lists(s);
        for name in a.iter().chain(b) {
            if !out.contains(name) {
                out.push(name);
            }
        }
    }
    out.extend(["Arial Unicode.ttf", "ARIALUNI.TTF", "DejaVuSans.ttf"]);
    out
}

/// Sans and serif CJK fonts for a script, best first.
fn cjk_lists(script: Script) -> (&'static [&'static str], &'static [&'static str]) {
    match script {
        Script::Japanese => (
            &[
                "YuGothM.ttc",
                "YuGothR.ttc",
                "meiryo.ttc",
                "msgothic.ttc",
                "ヒラギノ角ゴシック W3.ttc",
                "NotoSansCJKjp-Regular.otf",
                "NotoSansCJK-Regular.ttc",
                "ipag.ttf",
                "ipagp.ttf",
                "fonts-japanese-gothic.ttf",
            ],
            &[
                "yumin.ttf",
                "msmincho.ttc",
                "ヒラギノ明朝 ProN.ttc",
                "NotoSerifCJKjp-Regular.otf",
                "NotoSerifCJK-Regular.ttc",
                "ipam.ttf",
                "ipamp.ttf",
                "fonts-japanese-mincho.ttf",
            ],
        ),
        Script::Simplified => (
            &[
                "msyh.ttc",
                "simhei.ttf",
                "PingFang.ttc",
                "Hiragino Sans GB.ttc",
                "NotoSansCJKsc-Regular.otf",
                "NotoSansCJK-Regular.ttc",
                "wqy-microhei.ttc",
                "wqy-zenhei.ttc",
            ],
            &[
                "simsun.ttc",
                "Songti.ttc",
                "NotoSerifCJKsc-Regular.otf",
                "NotoSerifCJK-Regular.ttc",
                "uming.ttc",
            ],
        ),
        Script::Traditional => (
            &[
                "msjh.ttc",
                "PingFang.ttc",
                "NotoSansCJKtc-Regular.otf",
                "NotoSansCJK-Regular.ttc",
                "wqy-microhei.ttc",
                "wqy-zenhei.ttc",
            ],
            &[
                "mingliu.ttc",
                "Songti.ttc",
                "NotoSerifCJKtc-Regular.otf",
                "NotoSerifCJK-Regular.ttc",
                "bsmi00lp.ttf",
                "uming.ttc",
            ],
        ),
        Script::Korean => (
            &[
                "malgun.ttf",
                "gulim.ttc",
                "AppleSDGothicNeo.ttc",
                "NotoSansCJKkr-Regular.otf",
                "NotoSansCJK-Regular.ttc",
                "NanumGothic.ttf",
                "UnDotum.ttf",
            ],
            &[
                "batang.ttc",
                "AppleMyungjo.ttf",
                "NotoSerifCJKkr-Regular.otf",
                "NotoSerifCJK-Regular.ttc",
                "NanumMyeongjo.ttf",
                "UnBatang.ttf",
            ],
        ),
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
    // A CJK font must have the script's letters: a Japanese font may have
    // no Hangul, for one.
    let probe = match want.family {
        Family::Cjk { script, .. } => Some(match script {
            Script::Japanese => 'あ',
            Script::Simplified => '们',
            Script::Traditional => '們',
            Script::Korean => '한',
        }),
        _ => None,
    };
    let found = candidates(want).into_iter().find_map(|name| {
        dirs.iter().find_map(|d| {
            let data = std::fs::read(d.join(name)).ok()?;
            let program = FontProgram::parse(&data).ok()?;
            if let Some(c) = probe {
                program.glyph_by_unicode(c).filter(|&g| g != 0)?;
            }
            Some(Arc::new(program))
        })
    });
    if let Ok(mut c) = cache().lock() {
        c.insert(want, found.clone());
    }
    found
}
