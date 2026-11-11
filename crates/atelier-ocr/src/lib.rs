//! OCR for scanned pages: finds and reads the words in a page picture with
//! the ocrs engine, works out which way up the page reads from which turn
//! gives real words, and hands the words back as page text so they can be
//! searched, selected and copied like any other text.
//!
//! The models are fetched separately (see [`models`]) and checked against
//! fixed hashes before they are loaded.

pub mod models;
mod page;

use std::path::Path;

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};

pub use page::{PageReading, read_page};

/// A loaded OCR engine. Loading takes a moment, so one is kept and shared.
pub struct Engine {
    ocr: OcrEngine,
}

/// A character found in a picture and its box in that picture's pixels:
/// left, top, right, bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Char {
    pub c: char,
    pub rect: [f32; 4],
}

/// The characters of one line of text, spaces included, in reading order.
pub type Line = Vec<Char>;

/// What reading a picture found: the clockwise quarter turns (0, 90, 180
/// or 270 degrees) that make it upright, and its lines in the upright
/// picture's pixels.
#[derive(Debug, Clone)]
pub struct Reading {
    pub turn: u16,
    pub lines: Vec<Line>,
    /// The upright picture's size.
    pub size: (usize, usize),
}

/// Lines read from each way up when working out which is right.
const SAMPLE_LINES: usize = 40;
/// A turn is taken without trying the others when this share of its
/// letters make up plausible words...
const SURE_SHARE: f64 = 0.6;
/// ...and there are at least this many such letters.
const SURE_LETTERS: usize = 40;
/// Too few letters in plausible words to say anything.
const MIN_LETTERS: usize = 12;
/// The best turn must beat the next best by this much.
const MIN_LEAD: f64 = 1.3;
/// No word is longer than this.
const MAX_WORD: usize = 24;
/// The share of a word of four letters or more that is vowels...
const MIN_VOWELS: f64 = 0.12;
const MAX_VOWELS: f64 = 0.8;
/// ...and the most vowels and consonants it has in a row.
const MAX_VOWEL_RUN: usize = 4;
const MAX_CONSONANT_RUN: usize = 5;

impl Engine {
    /// Loads the models from `dir`, after checking their hashes.
    pub fn load(dir: &Path) -> Result<Engine, String> {
        let load = |i: usize| -> Result<rten::Model, String> {
            let data = models::read_checked(dir, &models::MODELS[i])?;
            rten::Model::load(data).map_err(|e| format!("Could not load the OCR model: {e}"))
        };
        let ocr = OcrEngine::new(OcrEngineParams {
            detection_model: Some(load(0)?),
            recognition_model: Some(load(1)?),
            ..Default::default()
        })
        .map_err(|e| format!("Could not start OCR: {e}"))?;
        Ok(Engine { ocr })
    }

    /// Reads the lines of an upright grey picture, at most `max_lines` of
    /// them (the ones found first, top to bottom).
    pub fn read(
        &self,
        gray: &[u8],
        width: usize,
        height: usize,
        max_lines: Option<usize>,
    ) -> Result<Vec<Line>, String> {
        if width == 0 || height == 0 || gray.len() < width * height {
            return Ok(Vec::new());
        }
        let source =
            ImageSource::from_bytes(&gray[..width * height], (width as u32, height as u32))
                .map_err(|e| e.to_string())?;
        let input = self.ocr.prepare_input(source).map_err(|e| e.to_string())?;
        let words = self.ocr.detect_words(&input).map_err(|e| e.to_string())?;
        let mut lines = self.ocr.find_text_lines(&input, &words);
        if let Some(n) = max_lines {
            lines.truncate(n);
        }
        let read = self
            .ocr
            .recognize_text(&input, &lines)
            .map_err(|e| e.to_string())?;
        Ok(read
            .into_iter()
            .flatten()
            .map(|line| {
                line.chars()
                    .iter()
                    .map(|c| Char {
                        c: c.char,
                        rect: [
                            c.rect.left() as f32,
                            c.rect.top() as f32,
                            c.rect.right() as f32,
                            c.rect.bottom() as f32,
                        ],
                    })
                    .collect()
            })
            .collect())
    }

    /// Reads a grey picture that may be on its side or upside down. The
    /// ways up are read and the one whose text makes the most sense wins.
    /// `hint` is the likely way up: it and its opposite are read first, and
    /// when one of those clearly reads as words the sideways ones are not
    /// tried. None when no way up gives readable text.
    pub fn read_any_way_up(
        &self,
        gray: &[u8],
        width: usize,
        height: usize,
        hint: Option<u16>,
    ) -> Result<Option<Reading>, String> {
        let first = hint.unwrap_or(0) % 360;
        let order = [first, first + 180, first + 90, first + 270].map(|t| t % 360);
        // The likely way up is read in full, so if it is right nothing
        // needs reading again.
        let mut full = None;
        let mut scores: Vec<(u16, Score)> = Vec::new();
        for (i, turn) in order.into_iter().enumerate() {
            let (img, w, h) = turn_gray(gray, width, height, turn);
            let score = if i == 0 && hint.is_some() {
                let lines = self.read(&img, w, h, None)?;
                // Scored on as many lines as the others, to compare fairly.
                let score = Score::of(&lines[..lines.len().min(SAMPLE_LINES)]);
                full = Some(Reading {
                    turn,
                    lines,
                    size: (w, h),
                });
                score
            } else {
                Score::of(&self.read(&img, w, h, Some(SAMPLE_LINES))?)
            };
            scores.push((turn, score));
            // A hint is never taken on its own word: upside-down letters
            // can pass for words, and the hint is at its worst telling up
            // from down. It does get the direction of the lines right.
            if i == 1 && hint.is_some() {
                let (a, b) = (scores[0].1, scores[1].1);
                let (win, lose) = if a.good >= b.good { (a, b) } else { (b, a) };
                if win.sure() && leads(win.good, lose.good) {
                    break;
                }
            }
        }
        scores.sort_by_key(|s| std::cmp::Reverse(s.1.good));
        let (best, good) = (scores[0].0, scores[0].1.good);
        let next = scores.get(1).map_or(0, |s| s.1.good);
        if good < MIN_LETTERS || !leads(good, next) {
            return Ok(None);
        }
        if let Some(reading) = full.filter(|r| r.turn == best) {
            return Ok(Some(reading));
        }
        let (img, w, h) = turn_gray(gray, width, height, best);
        let lines = self.read(&img, w, h, None)?;
        Ok(Some(Reading {
            turn: best,
            lines,
            size: (w, h),
        }))
    }
}

/// Whether the best way up reads enough better than the next best.
fn leads(best: usize, next: usize) -> bool {
    best as f64 >= next as f64 * MIN_LEAD
}

/// How much of what was read looks like words.
#[derive(Debug, Clone, Copy, Default)]
struct Score {
    /// Letters in plausible words.
    good: usize,
    /// All letters and digits read.
    all: usize,
}

impl Score {
    fn of(lines: &[Line]) -> Score {
        let text: String = lines
            .iter()
            .map(|l| l.iter().map(|c| c.c).chain(['\n']).collect::<String>())
            .collect();
        let mut s = Score::default();
        for word in text.split_whitespace() {
            let w = word.trim_matches(|c: char| !c.is_alphanumeric());
            let n = w.chars().filter(|c| c.is_alphanumeric()).count();
            s.all += n;
            if plausible(w) {
                s.good += n;
                if COMMON.contains(&w.to_lowercase().as_str()) {
                    // Common words are strong evidence: count them twice.
                    s.good += n;
                }
            }
        }
        s
    }

    fn sure(&self) -> bool {
        self.good >= SURE_LETTERS && self.good as f64 >= SURE_SHARE * self.all as f64
    }
}

fn is_vowel(c: char) -> bool {
    "aeiouyàáâãäåæèéêëìíîïòóôõöøùúûüýœ".contains(c)
}

/// Whether a word looks like one: letters in lower case, or capitalised,
/// or all capitals, with vowels and consonants mixed the way words mix
/// them; or a number. Letters read upside down come out in runs no word
/// has ("TEEEEEEEEFTEE", "DZTHTHDNDDHHNNNT"), which this turns away.
fn plausible(w: &str) -> bool {
    let chars: Vec<char> = w.chars().collect();
    if chars.len() < 2 {
        return false;
    }
    if chars
        .iter()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '/' | '-'))
    {
        return chars.iter().filter(|c| c.is_ascii_digit()).count() >= 2;
    }
    if !chars
        .iter()
        .all(|c| c.is_alphabetic() || *c == '\'' || *c == '-')
    {
        return false;
    }
    let rest_lower = chars[1..].iter().all(|c| !c.is_uppercase());
    let all_upper = chars.iter().all(|c| !c.is_lowercase());
    if !(rest_lower || all_upper) || chars.len() > MAX_WORD {
        return false;
    }
    let letters: Vec<char> = chars
        .iter()
        .filter(|c| c.is_alphabetic())
        .flat_map(|c| c.to_lowercase())
        .collect();
    // The same letter three times running.
    if letters.windows(3).any(|w| w[0] == w[1] && w[1] == w[2]) {
        return false;
    }
    let vowels = letters.iter().filter(|c| is_vowel(**c)).count();
    if vowels == 0 {
        return false;
    }
    if letters.len() < 4 {
        return true;
    }
    // The longest runs of vowels and of consonants.
    let (mut vowel_run, mut consonant_run, mut run, mut last) = (0, 0, 0, None);
    for c in &letters {
        let v = is_vowel(*c);
        run = if last == Some(v) { run + 1 } else { 1 };
        last = Some(v);
        if v {
            vowel_run = vowel_run.max(run);
        } else {
            consonant_run = consonant_run.max(run);
        }
    }
    let share = vowels as f64 / letters.len() as f64;
    (MIN_VOWELS..=MAX_VOWELS).contains(&share)
        && vowel_run <= MAX_VOWEL_RUN
        && consonant_run <= MAX_CONSONANT_RUN
}

/// Very common short words in English and the other big Latin-script
/// languages. Upside-down text almost never reads as these.
const COMMON: &[&str] = &[
    "the", "and", "of", "to", "in", "is", "for", "that", "with", "on", "as", "are", "this", "be",
    "by", "or", "from", "at", "an", "it", "not", "was", "have", "has", "your", "you", "will",
    "all", "can", "date", "name", "page", "le", "la", "les", "de", "des", "et", "est", "un", "une",
    "du", "en", "pour", "par", "der", "die", "das", "und", "ist", "mit", "von", "zu", "el", "los",
    "las", "con", "por", "que", "del", "il", "di", "che", "per", "una",
];

/// A grey picture turned `turn` degrees clockwise (0, 90, 180 or 270), and
/// its new width and height.
pub fn turn_gray(gray: &[u8], w: usize, h: usize, turn: u16) -> (Vec<u8>, usize, usize) {
    match turn % 360 {
        90 => {
            let mut out = vec![0; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[x * h + (h - 1 - y)] = gray[y * w + x];
                }
            }
            (out, h, w)
        }
        180 => {
            let mut out = gray[..w * h].to_vec();
            out.reverse();
            (out, w, h)
        }
        270 => {
            let mut out = vec![0; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[(w - 1 - x) * h + y] = gray[y * w + x];
                }
            }
            (out, h, w)
        }
        _ => (gray[..w * h].to_vec(), w, h),
    }
}

/// A point in a picture turned `turn` degrees clockwise by [`turn_gray`],
/// taken back to the unturned picture, which is `w` by `h`.
pub fn unturn_point(x: f64, y: f64, w: f64, h: f64, turn: u16) -> (f64, f64) {
    match turn % 360 {
        90 => (y, h - x),
        180 => (w - x, h - y),
        270 => (w - y, x),
        _ => (x, y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_and_back() {
        // A 3 by 2 picture with distinct pixels.
        let g: Vec<u8> = (0..6).collect();
        for turn in [0, 90, 180, 270] {
            let (t, tw, th) = turn_gray(&g, 3, 2, turn);
            for ty in 0..th {
                for tx in 0..tw {
                    // The pixel centre, taken back, lands on the same pixel.
                    let (x, y) = unturn_point(tx as f64 + 0.5, ty as f64 + 0.5, 3.0, 2.0, turn);
                    let (x, y) = (x.floor() as usize, y.floor() as usize);
                    assert_eq!(t[ty * tw + tx], g[y * 3 + x], "turn {turn}");
                }
            }
        }
    }

    #[test]
    fn scores_words() {
        let line = |s: &str| -> Line { s.chars().map(|c| Char { c, rect: [0.0; 4] }).collect() };
        let good = Score::of(&[line("The quick brown fox jumps over the lazy dog")]);
        let bad = Score::of(&[line("bop ʎzɐl ǝɥʇ ɹǝʌo sdɯnɾ xoɟ uMoɹq")]);
        let junk = Score::of(&[line("pIo 8op ,Kzel aql lano sdwnl xoj")]);
        assert!(good.good > 3 * bad.good.max(1));
        assert!(good.good > junk.good);
    }

    #[test]
    fn capitals_read_upside_down_are_not_words() {
        let line = |s: &str| -> Line { s.chars().map(|c| Char { c, rect: [0.0; 4] }).collect() };
        // A page of capitals as OCR read it upright, and upside down.
        let good = Score::of(&[
            line("WEIGHT OF A PELLET BY DIVIDING THE"),
            line("TOTAL WEIGHT OBTAINED BY 15."),
            line("ELECTRONIC BALANCE, SATORIOUS"),
        ]);
        let junk = Score::of(&[
            line("HESEAEEENEENFEEENHEAA TEEEEEEEEFTEE HOEHORAM"),
            line("DZTHTHDNDDHHNNNT EZITEEFNHHHO ENHHHOND"),
            line("SOAAN REF EELOVENHO LVAH"),
        ]);
        assert!(good.sure());
        assert!(!junk.sure());
        assert!(leads(good.good, junk.good));
        for w in ["strength", "months", "queue", "l'eau", "IGNITER", "Schrift"] {
            assert!(plausible(w), "{w}");
        }
    }
}
