//! Shared OpenSSL process and temporary workspace handling.
use std::{path::PathBuf, process::Command};

// A fresh directory is created atomically; never reuse a pre-existing location.
// It holds document bytes, so restrict Unix access and remove it on every exit.
pub(super) struct Scratch(pub(super) PathBuf);
impl Scratch {
    pub(super) fn new() -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "atelier-security-{}-{time}-{sequence}",
            std::process::id()
        ));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|e| format!("Could not create security workspace: {e}"))?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn openssl() -> Command {
    let command = Command::new("openssl");
    #[cfg(windows)]
    let mut command = command;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// LibreSSL (the macOS system `openssl`) cannot exclude its default CA store,
/// so a check limited to a selected bundle is impossible with it.
pub(super) fn openssl_is_libressl() -> bool {
    static LIBRESSL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LIBRESSL.get_or_init(|| {
        openssl()
            .arg("version")
            .stdin(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).starts_with("LibreSSL"))
            .unwrap_or(false)
    })
}

pub(super) fn pem_certificates(pem: &str) -> Result<Vec<String>, String> {
    let mut certificates = Vec::new();
    let mut remaining = pem;
    while let Some(start) = remaining.find("-----BEGIN CERTIFICATE-----") {
        remaining = &remaining[start..];
        let end = remaining
            .find("-----END CERTIFICATE-----")
            .ok_or("Incomplete signer certificate")?
            + "-----END CERTIFICATE-----".len();
        certificates.push(format!("{}\n", &remaining[..end]));
        remaining = &remaining[end..];
    }
    if certificates.is_empty() {
        return Err("No verifying signer certificates returned".into());
    }
    Ok(certificates)
}
