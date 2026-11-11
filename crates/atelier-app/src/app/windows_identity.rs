//! Current-user Windows Personal-store identities. Key access stays with Windows.
use super::security_provider::Scratch;
use eframe::egui;
use std::{
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
};
const SCRIPT: &str = include_str!("windows_identity.ps1");

#[derive(Clone)]
struct Certificate {
    thumbprint: String,
    subject: String,
    issuer: String,
    expires: String,
}
#[derive(Default)]
pub(super) struct StoreUi {
    certificates: Vec<Certificate>,
    pub selected: Option<String>,
    loaded: bool,
    error: Option<String>,
    job: Option<mpsc::Receiver<Result<Vec<Certificate>, String>>>,
}
impl StoreUi {
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        if (!self.loaded || ui.button("Refresh identities").clicked()) && self.job.is_none() {
            self.loaded = true;
            self.error = None;
            let (tx, rx) = mpsc::channel();
            let wake = ui.ctx().clone();
            match std::thread::Builder::new()
                .name("windows-identities".into())
                .spawn(move || {
                    let _ = tx.send(list());
                    wake.request_repaint();
                }) {
                Ok(_) => self.job = Some(rx),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        if let Some(job) = &self.job {
            match job.try_recv() {
                Ok(result) => {
                    self.job = None;
                    match result {
                        Ok(certificates) => {
                            if !certificates
                                .iter()
                                .any(|c| Some(&c.thumbprint) == self.selected.as_ref())
                            {
                                self.selected = None;
                            }
                            self.certificates = certificates;
                        }
                        Err(e) => {
                            self.certificates.clear();
                            self.selected = None;
                            self.error = Some(e);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.error = Some("Identity discovery worker stopped.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.spinner();
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        ui.label("Current user / Personal. Private keys stay with Windows; key access may require a Windows prompt. Only the signer certificate is included.");
        if let Some(error) = &self.error {
            ui.label(error);
        }
        if self.job.is_none() && self.certificates.is_empty() && self.error.is_none() {
            ui.label("No certificates with private keys found.");
        }
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .show(ui, |ui| {
                for certificate in &self.certificates {
                    ui.selectable_value(
                        &mut self.selected,
                        Some(certificate.thumbprint.clone()),
                        format!("{} (expires {})", certificate.subject, certificate.expires),
                    )
                    .on_hover_text(format!(
                        "Issuer: {}\nThumbprint: {}",
                        certificate.issuer, certificate.thumbprint
                    ));
                }
            });
    }
}
fn powershell() -> Command {
    use std::os::windows::process::CommandExt;
    let executable = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let mut command = Command::new(executable);
    command
        .creation_flags(0x08000000)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
        ])
        .stdin(Stdio::null());
    command
}
fn invoke(
    mode: &str,
    thumbprint: Option<&str>,
    input: Option<&Path>,
    output: Option<&Path>,
) -> Result<Vec<u8>, String> {
    let scratch = Scratch::new()?;
    let script = scratch.0.join("windows-identity.ps1");
    std::fs::write(&script, SCRIPT).map_err(|e| e.to_string())?;
    let mut command = powershell();
    command.arg("-File").arg(script).arg("-Mode").arg(mode);
    if let Some(thumbprint) = thumbprint {
        command.env("ATELIER_NATIVE_THUMBPRINT", thumbprint);
    }
    if let Some(input) = input {
        command.env("ATELIER_NATIVE_INPUT", input);
    }
    if let Some(output) = output {
        command.env("ATELIER_NATIVE_OUTPUT", output);
    }
    let result = command
        .output()
        .map_err(|e| format!("Could not start Windows certificate provider: {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "Windows certificate provider failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(result.stdout)
}
fn valid_thumbprint(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
}
fn decode_text(text: &str) -> Result<String, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err("Invalid certificate metadata encoding".into());
    }
    let bytes = text
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).map_err(|e| e.to_string())?, 16)
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}
fn parse_list(data: &[u8]) -> Result<Vec<Certificate>, String> {
    let text = std::str::from_utf8(data).map_err(|e| e.to_string())?;
    let mut certificates = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 4 || !valid_thumbprint(fields[0]) {
            return Err("Invalid certificate discovery response".into());
        }
        certificates.push(Certificate {
            thumbprint: fields[0].to_uppercase(),
            subject: decode_text(fields[1])?,
            issuer: decode_text(fields[2])?,
            expires: fields[3].into(),
        });
    }
    certificates.sort_by(|a, b| {
        a.subject
            .cmp(&b.subject)
            .then(a.thumbprint.cmp(&b.thumbprint))
    });
    Ok(certificates)
}
fn list() -> Result<Vec<Certificate>, String> {
    parse_list(&invoke("List", None, None, None)?)
}
pub(super) fn sign(thumbprint: &str, input: &Path, output: &Path) -> Result<(), String> {
    if !valid_thumbprint(thumbprint) {
        return Err("Invalid Windows certificate thumbprint".into());
    }
    invoke("Sign", Some(thumbprint), Some(input), Some(output)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::super::security_provider::openssl;
    use super::*;

    #[test]
    fn discovery_protocol_preserves_unicode_and_rejects_invalid_metadata() {
        let thumbprint = "0123456789012345678901234567890123456789";
        let rows = format!("{thumbprint}\t434E3DC3A9\t434E3D4341\t2027-01-01\r\n");
        let certificates = parse_list(rows.as_bytes()).unwrap();
        assert_eq!(certificates[0].subject, "CN=\u{e9}");
        assert_eq!(certificates[0].issuer, "CN=CA");
        for bad in [
            "invalid\t\t\t2027-01-01",
            "junk",
            "",
            "0123456789012345678901234567890123456789\tFF\t\t2027-01-01",
        ] {
            if bad.is_empty() {
                assert!(parse_list(bad.as_bytes()).unwrap().is_empty());
            } else {
                assert!(parse_list(bad.as_bytes()).is_err());
            }
        }
        assert!(decode_text("0").is_err());
        assert!(!valid_thumbprint("'; Remove-Item"));
        assert!(sign("bad", Path::new("unused"), Path::new("unused")).is_err());
    }

    #[test]
    fn current_user_discovery_is_read_only_and_missing_selection_fails() {
        list().unwrap();
        let scratch = Scratch::new().unwrap();
        let output = scratch.0.join("must-not-exist.der");
        assert!(sign(&"0".repeat(40), &scratch.0.join("missing"), &output).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn native_cms_signs_with_ephemeral_key_and_verifies_independently() {
        if openssl().arg("version").output().is_err() {
            return;
        }
        let scratch = Scratch::new().unwrap();
        let run = |args: &[&str]| {
            let result = openssl()
                .args(args)
                .current_dir(&scratch.0)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        std::fs::write(
            scratch.0.join("config.cnf"),
            "[req]\ndistinguished_name=dn\n[dn]\n",
        )
        .unwrap();
        run(&[
            "req",
            "-config",
            "config.cnf",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-x509",
            "-subj",
            "/CN=Native test",
            "-days",
            "1",
            "-keyout",
            "key.pem",
            "-out",
            "cert.pem",
        ]);
        run(&[
            "pkcs12",
            "-export",
            "-in",
            "cert.pem",
            "-inkey",
            "key.pem",
            "-out",
            "test.p12",
            "-passout",
            "pass:test-only",
        ]);
        let doc = atelier_syntax::Document::open(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n%%EOF".to_vec()).unwrap();
        let prepared = doc.prepare_signature("Native test", 65536).unwrap();
        let content = scratch.0.join("content.bin");
        let cms = scratch.0.join("cms.der");
        std::fs::write(&content, prepared.signing_bytes()).unwrap();
        std::fs::write(scratch.0.join("provider.ps1"), SCRIPT).unwrap();
        // Import only into process memory. Never add a fixture to the user's stores.
        std::fs::write(scratch.0.join("test.ps1"), r#"
$ErrorActionPreference = 'Stop'
. (Join-Path $env:ATELIER_TEST_DIR 'provider.ps1') -Mode Library
$certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new((Join-Path $env:ATELIER_TEST_DIR 'test.p12'), 'test-only', [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
try {
    Write-DetachedSignature $certificate (Join-Path $env:ATELIER_TEST_DIR 'content.bin') (Join-Path $env:ATELIER_TEST_DIR 'cms.der')
} finally { $certificate.Reset() }
"#).unwrap();
        let output = powershell()
            .arg("-File")
            .arg(scratch.0.join("test.ps1"))
            .env("ATELIER_TEST_DIR", &scratch.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = prepared.finish(&std::fs::read(&cms).unwrap()).unwrap();
        let saved = atelier_syntax::Document::open(bytes).unwrap();
        let signature = saved.inspect_signatures().unwrap().remove(0).unwrap();
        std::fs::write(&content, &signature.signed_bytes).unwrap();
        std::fs::write(&cms, &signature.cms).unwrap();
        run(&[
            "cms",
            "-verify",
            "-binary",
            "-inform",
            "DER",
            "-noverify",
            "-in",
            "cms.der",
            "-content",
            "content.bin",
            "-out",
            "verified.bin",
        ]);
        let mut tampered = signature.signed_bytes;
        tampered[10] ^= 1;
        std::fs::write(&content, tampered).unwrap();
        let rejected = openssl()
            .args([
                "cms",
                "-verify",
                "-binary",
                "-inform",
                "DER",
                "-noverify",
                "-in",
                "cms.der",
                "-content",
                "content.bin",
            ])
            .current_dir(&scratch.0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!rejected.success());
    }
}
