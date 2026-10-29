//! Printing through CUPS's `lp`, on Linux and macOS: the PDF goes to the
//! printer as it is, with the choices as `lp` options.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use crate::{Duplex, Options, Orientation, Paper, Printer, Sizing, Source, ranges};

pub fn printers() -> Vec<Printer> {
    let default = Command::new("lpstat")
        .arg("-d")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_once(':')
                .map(|(_, n)| n.trim().to_string())
        })
        .unwrap_or_default();
    Command::new("lpstat")
        .arg("-e")
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(|n| Printer {
                    name: n.to_string(),
                    default: n == default,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn paper(_printer: &str) -> Option<Paper> {
    None
}

/// The `lp` arguments for a job, before the file name.
pub fn args(options: &Options, title: &str) -> Vec<String> {
    let mut a = vec![
        "-d".to_string(),
        options.printer.clone(),
        "-t".into(),
        title.into(),
        "-n".into(),
        options.copies.max(1).to_string(),
        "-P".into(),
        ranges(&options.pages),
    ];
    let mut opt = |o: &str| {
        a.push("-o".into());
        a.push(o.into());
    };
    if options.copies > 1 {
        opt(if options.collate {
            "collate=true"
        } else {
            "collate=false"
        });
    }
    match options.orientation {
        Orientation::Auto => {}
        Orientation::Portrait => opt("orientation-requested=3"),
        Orientation::Landscape => opt("orientation-requested=4"),
    }
    if !options.color {
        opt("print-color-mode=monochrome");
        opt("ColorModel=Gray");
    }
    match options.sizing {
        Sizing::Fit => opt("print-scaling=fit"),
        Sizing::Actual => opt("print-scaling=none"),
        Sizing::Shrink => opt("print-scaling=auto"),
        Sizing::Custom(p) => {
            opt("print-scaling=none");
            opt(&format!("scaling={}", p.round() as i64));
        }
    }
    opt(match options.duplex {
        Duplex::Off => "sides=one-sided",
        Duplex::LongEdge => "sides=two-sided-long-edge",
        Duplex::ShortEdge => "sides=two-sided-short-edge",
    });
    a
}

pub fn print(
    options: &Options,
    title: &str,
    source: &dyn Source,
    progress: &dyn Fn(usize, usize),
    _cancel: &AtomicBool,
) -> Result<(), String> {
    let total = options.pages.len() * options.copies.max(1) as usize;
    progress(0, total);
    // The file goes in on standard input, so nothing is left on disk.
    let mut child = Command::new("lp")
        .args(args(options, title))
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start lp: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(source.pdf())
            .map_err(|e| format!("Could not send the file to lp: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "The printer refused the job: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    progress(total, total);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_lp_options() {
        let o = Options {
            printer: "Office".into(),
            copies: 2,
            collate: false,
            orientation: Orientation::Landscape,
            color: false,
            sizing: Sizing::Custom(80.0),
            duplex: Duplex::LongEdge,
            pages: vec![0, 1, 2, 5],
        };
        let a = args(&o, "Report").join(" ");
        assert_eq!(
            a,
            "-d Office -t Report -n 2 -P 1-3,6 -o collate=false -o orientation-requested=4 \
             -o print-color-mode=monochrome -o ColorModel=Gray -o print-scaling=none \
             -o scaling=80 -o sides=two-sided-long-edge"
        );
    }
}
