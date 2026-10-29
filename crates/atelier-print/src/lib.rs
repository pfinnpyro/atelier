//! Printing. The choices (copies, orientation, colour, sizing, two-sided,
//! which pages) and where a page lands on the paper are worked out here
//! the same way everywhere; the job itself goes to the Windows print
//! spooler as pictures of the pages this program draws, or elsewhere to
//! `lp` as the PDF itself.

use std::sync::atomic::AtomicBool;

#[cfg(not(windows))]
mod lp;
#[cfg(windows)]
mod windows;

/// Which way pages go on the paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Each page the way that fits it best: wide pages landscape.
    Auto,
    Portrait,
    Landscape,
}

/// How big pages print.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sizing {
    /// Scaled to fill the printable area.
    Fit,
    /// At their own size, cut off if bigger than the paper.
    Actual,
    /// At their own size, or smaller to fit.
    Shrink,
    /// At this percentage of their own size.
    Custom(f64),
}

/// Printing on both sides of the paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplex {
    Off,
    /// Flipped on the long edge, like a book.
    LongEdge,
    /// Flipped on the short edge, like a notepad.
    ShortEdge,
}

/// Everything about one print job.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub printer: String,
    pub copies: u32,
    /// Whole copies one after another (1 2 3, 1 2 3) rather than each page
    /// several times (1 1, 2 2, 3 3).
    pub collate: bool,
    pub orientation: Orientation,
    pub color: bool,
    pub sizing: Sizing,
    pub duplex: Duplex,
    /// Pages to print, from 0, in order.
    pub pages: Vec<usize>,
}

/// A printer the system knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Printer {
    pub name: String,
    pub default: bool,
}

/// A sheet of paper held upright, in points: its size and the part the
/// printer can reach (left, top, right, bottom from the top left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    pub size: (f64, f64),
    pub printable: (f64, f64, f64, f64),
}

impl Paper {
    /// US Letter with quarter-inch margins, for when the printer does not
    /// say.
    pub const LETTER: Paper = Paper {
        size: (612.0, 792.0),
        printable: (18.0, 18.0, 594.0, 774.0),
    };

    /// The same sheet turned on its side.
    pub fn turned(&self) -> Paper {
        let (w, h) = self.size;
        let (l, t, r, b) = self.printable;
        // Turned a quarter anticlockwise: the old right edge is the top.
        Paper {
            size: (h, w),
            printable: (t, w - r, b, w - l),
        }
    }
}

/// Where a page goes on the paper, in points from the paper's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Whether a page of size `page` (points, as shown) prints landscape.
pub fn landscape(page: (f64, f64), orientation: Orientation) -> bool {
    match orientation {
        Orientation::Auto => page.0 > page.1,
        Orientation::Portrait => false,
        Orientation::Landscape => true,
    }
}

/// Where a page of size `page` (points, as shown) lands on `paper`
/// (already turned for landscape).
pub fn place(page: (f64, f64), paper: &Paper, sizing: Sizing) -> Placement {
    let (pw, ph) = (page.0.max(1.0), page.1.max(1.0));
    let (l, t, r, b) = paper.printable;
    let (aw, ah) = ((r - l).max(1.0), (b - t).max(1.0));
    let fit = (aw / pw).min(ah / ph);
    let scale = match sizing {
        Sizing::Fit => fit,
        Sizing::Actual => 1.0,
        Sizing::Shrink => fit.min(1.0),
        Sizing::Custom(pct) => (pct / 100.0).clamp(0.01, 10.0),
    };
    let (w, h) = (pw * scale, ph * scale);
    // Centred on the printable area.
    Placement {
        x: l + (aw - w) / 2.0,
        y: t + (ah - h) / 2.0,
        w,
        h,
    }
}

/// Reads a page list such as "1-3, 5, 8-" (pages from 1) into page
/// indexes from 0. Pages past the end are an error.
pub fn parse_pages(text: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in text
        .split([',', ';'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let num = |s: &str| -> Result<usize, String> {
            s.trim()
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=count).contains(n))
                .ok_or_else(|| format!("“{}” is not a page from 1 to {count}.", s.trim()))
        };
        match part.split_once(['-', '–']) {
            Some((a, b)) => {
                let a = if a.trim().is_empty() { 1 } else { num(a)? };
                let b = if b.trim().is_empty() { count } else { num(b)? };
                if a > b {
                    return Err(format!("“{part}” runs backwards."));
                }
                out.extend(a - 1..b);
            }
            None => out.push(num(part)? - 1),
        }
    }
    if out.is_empty() {
        return Err("Say which pages to print, such as 1-3, 5.".into());
    }
    Ok(out)
}

/// Page indexes from 0 written as ranges from 1: "1-3,5".
pub fn ranges(pages: &[usize]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let start = pages[i];
        let mut end = start;
        while i + 1 < pages.len() && pages[i + 1] == end + 1 {
            i += 1;
            end += 1;
        }
        out.push(if start == end {
            format!("{}", start + 1)
        } else {
            format!("{}-{}", start + 1, end + 1)
        });
        i += 1;
    }
    out.join(",")
}

/// A page drawn for printing: premultiplied RGBA rows.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// What is being printed.
pub trait Source {
    /// A page's size in points, as shown (after /Rotate).
    fn page_size(&self, page: usize) -> (f64, f64);
    /// A page drawn at `scale` pixels per point, or None if it cannot be.
    fn draw(&self, page: usize, scale: f64) -> Option<Picture>;
    /// The document as a PDF file, with any changes.
    fn pdf(&self) -> &[u8];
}

/// The printers there are, the default first.
pub fn printers() -> Vec<Printer> {
    #[cfg(windows)]
    let mut list = windows::printers();
    #[cfg(not(windows))]
    let mut list = lp::printers();
    list.sort_by_key(|p| !p.default);
    list
}

/// The paper a printer is set up with, upright, if it says.
pub fn paper(printer: &str) -> Option<Paper> {
    #[cfg(windows)]
    return windows::paper(printer);
    #[cfg(not(windows))]
    return lp::paper(printer);
}

/// Opens the printer's own settings dialog, where it has one.
pub fn printer_properties(printer: &str) -> Result<(), String> {
    #[cfg(windows)]
    return windows::properties(printer);
    #[cfg(not(windows))]
    {
        let _ = printer;
        Err("This system has no printer settings dialog to open from here.".into())
    }
}

/// Prints. `progress` hears (sheets sent, sheets in all); setting `cancel`
/// stops the job between pages. `title` names the job in the queue.
pub fn print(
    options: &Options,
    title: &str,
    source: &dyn Source,
    progress: &dyn Fn(usize, usize),
    cancel: &AtomicBool,
) -> Result<(), String> {
    if options.pages.is_empty() {
        return Err("There are no pages to print.".into());
    }
    #[cfg(windows)]
    return windows::print(options, title, source, progress, cancel);
    #[cfg(not(windows))]
    return lp::print(options, title, source, progress, cancel);
}

/// The order pages go out in, copies included.
pub fn sheets(options: &Options) -> Vec<usize> {
    let copies = options.copies.max(1) as usize;
    if options.collate {
        (0..copies)
            .flat_map(|_| options.pages.iter().copied())
            .collect()
    } else {
        options
            .pages
            .iter()
            .flat_map(|&p| std::iter::repeat_n(p, copies))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_page_lists() {
        assert_eq!(parse_pages("1-3, 5", 9), Ok(vec![0, 1, 2, 4]));
        assert_eq!(parse_pages("8-", 9), Ok(vec![7, 8]));
        assert_eq!(parse_pages("-2", 9), Ok(vec![0, 1]));
        assert!(parse_pages("10", 9).is_err());
        assert!(parse_pages("4-2", 9).is_err());
        assert!(parse_pages(" ", 9).is_err());
        assert_eq!(ranges(&[0, 1, 2, 4, 7, 8]), "1-3,5,8-9");
    }

    #[test]
    fn places_pages() {
        let paper = Paper {
            size: (600.0, 800.0),
            printable: (0.0, 0.0, 600.0, 800.0),
        };
        // Fit: a half-size page doubles and is centred across.
        let p = place((200.0, 400.0), &paper, Sizing::Fit);
        assert_eq!((p.x, p.y, p.w, p.h), (100.0, 0.0, 400.0, 800.0));
        // Shrink leaves a small page alone; Actual keeps a big one big.
        let p = place((200.0, 400.0), &paper, Sizing::Shrink);
        assert_eq!((p.w, p.h), (200.0, 400.0));
        let p = place((1200.0, 1600.0), &paper, Sizing::Actual);
        assert_eq!((p.x, p.y, p.w, p.h), (-300.0, -400.0, 1200.0, 1600.0));
        let p = place((200.0, 400.0), &paper, Sizing::Custom(50.0));
        assert_eq!((p.w, p.h), (100.0, 200.0));
        assert!(landscape((800.0, 600.0), Orientation::Auto));
        assert!(!landscape((800.0, 600.0), Orientation::Portrait));
    }

    #[test]
    fn turns_paper() {
        let paper = Paper {
            size: (600.0, 800.0),
            printable: (10.0, 20.0, 590.0, 770.0),
        };
        let t = paper.turned();
        assert_eq!(t.size, (800.0, 600.0));
        // Margins: left 10, top 20, right 10, bottom 30 become
        // left 20, top 10, right 30, bottom 10.
        assert_eq!(t.printable, (20.0, 10.0, 770.0, 590.0));
        assert_eq!(t.turned().turned().turned().turned(), t);
    }

    #[test]
    fn orders_copies() {
        let mut o = Options {
            printer: String::new(),
            copies: 2,
            collate: true,
            orientation: Orientation::Auto,
            color: true,
            sizing: Sizing::Fit,
            duplex: Duplex::Off,
            pages: vec![0, 1],
        };
        assert_eq!(sheets(&o), vec![0, 1, 0, 1]);
        o.collate = false;
        assert_eq!(sheets(&o), vec![0, 0, 1, 1]);
    }
}
