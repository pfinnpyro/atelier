//! The two model files OCR needs. They are too big to ship inside the
//! program, so they are fetched once, on request, and checked against
//! fixed SHA-256 sums before every use: a changed or damaged file is never
//! loaded.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// One model file: where it comes from and what it must hash to.
#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// The text detection and recognition models from the ocrs project.
pub const MODELS: [ModelFile; 2] = [
    ModelFile {
        name: "text-detection.onnx",
        url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx",
        sha256: "a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe",
        size: 2_499_479,
    },
    ModelFile {
        name: "text-recognition.onnx",
        url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx",
        sha256: "86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5",
        size: 9_713_177,
    },
];

/// The bytes all model files take together.
pub fn total_size() -> u64 {
    MODELS.iter().map(|m| m.size).sum()
}

/// Whether every model file is in `dir` at its expected size. Their
/// contents are checked when they are loaded.
pub fn installed(dir: &Path) -> bool {
    MODELS.iter().all(|m| {
        fs::metadata(dir.join(m.name))
            .map(|md| md.len() == m.size)
            .unwrap_or(false)
    })
}

/// Reads a model file, refusing it unless it hashes to the expected sum.
pub(crate) fn read_checked(dir: &Path, model: &ModelFile) -> Result<Vec<u8>, String> {
    let path = dir.join(model.name);
    let mut data = Vec::new();
    fs::File::open(&path)
        .and_then(|mut f| f.read_to_end(&mut data))
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if hex(&atelier_crypto::sha256(&data)) != model.sha256 {
        return Err(format!(
            "{} is not the expected file; download the OCR models again.",
            model.name
        ));
    }
    Ok(data)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads any model file that is missing or wrong into `dir`, adding
/// the bytes received to `progress` as they arrive. Uses the system's
/// `curl`, which Windows 10 and later include, so no network code is
/// built in. Each file is checked before it takes its final name.
pub fn download(dir: &Path, progress: &AtomicU64) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    for model in &MODELS {
        if read_checked(dir, model).is_ok() {
            progress.fetch_add(model.size, Ordering::SeqCst);
            continue;
        }
        let part = dir.join(format!("{}.part", model.name));
        let _ = fs::remove_file(&part);
        fetch(model.url, &part, progress)?;
        let data = fs::read(&part).map_err(|e| format!("Could not read the download: {e}"))?;
        if hex(&atelier_crypto::sha256(&data)) != model.sha256 {
            let _ = fs::remove_file(&part);
            return Err(format!(
                "The downloaded {} did not match its checksum, so it was thrown away.",
                model.name
            ));
        }
        fs::rename(&part, dir.join(model.name))
            .map_err(|e| format!("Could not save {}: {e}", model.name))?;
    }
    Ok(())
}

/// Fetches `url` into `to` with curl, counting bytes as the file grows.
fn fetch(url: &str, to: &PathBuf, progress: &AtomicU64) -> Result<(), String> {
    let mut cmd = Command::new("curl");
    cmd.args(["--fail", "--silent", "--show-error", "--location"])
        // Only https, also after redirects.
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-time", "900", "--output"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start curl to download the models: {e}"))?;
    let start = progress.load(Ordering::SeqCst);
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        let got = fs::metadata(to).map_or(0, |m| m.len());
        progress.store(start + got, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(150));
    };
    let got = fs::metadata(to).map_or(0, |m| m.len());
    progress.store(start + got, Ordering::SeqCst);
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let _ = fs::remove_file(to);
        return Err(format!("The download failed: {}", err.trim()));
    }
    Ok(())
}
