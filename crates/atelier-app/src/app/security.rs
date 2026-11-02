//! First security workflow: export encrypted copies or sign a completed revision.
use super::Viewer;
use eframe::egui;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

#[derive(Default)]
pub(super) struct SecurityUi {
    pub open: bool,
    signing: bool,
    password: String,
    confirm: String,
    certificate: Option<PathBuf>,
    key: Option<PathBuf>,
    key_password: String,
    job: Option<Receiver<Result<String, String>>>,
}

impl Viewer {
    pub(super) fn open_security(&mut self, signing: bool) {
        self.commit_ink();
        self.finish_editing(true);
        self.security.open = true;
        self.security.signing = signing;
    }

    pub(super) fn security_window(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.security.job {
            match job.try_recv() {
                Ok(result) => {
                    self.security.job = None;
                    match result {
                        Ok(message) => self.notice = Some(message),
                        Err(e) => self.error = Some(e),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.security.job = None;
                    self.error = Some("The security export worker stopped unexpectedly.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
            }
        }
        if !self.security.open {
            return;
        }
        let mut open = true;
        let mut export = false;
        egui::Window::new("Document security")
            .open(&mut open).collapsible(false).resizable(false).default_width(460.0)
            .frame(egui::Frame::window(&ctx.global_style()).inner_margin(18.0))
            .show(ctx, |ui| {
                let s = &mut self.security;
                ui.add_enabled_ui(s.job.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut s.signing, false, "Encrypt a copy");
                        ui.selectable_value(&mut s.signing, true, "Sign with certificate");
                    });
                    ui.separator();
                    if s.signing {
                        ui.label("Signs the entire current revision, including any ink signatures you placed. Later edits will be a separate revision.");
                        ui.label("Uses OpenSSL and a PEM certificate/private key. Certificate trust and revocation are not checked by Atelier yet.");
                        for (label, path) in [("Certificate (PEM)…", &mut s.certificate), ("Private key (PEM)…", &mut s.key)] {
                            ui.horizontal(|ui| {
                                if ui.button(label).clicked() && let Some(p) = rfd::FileDialog::new().add_filter("PEM files", &["pem", "crt", "key"]).pick_file() { *path = Some(p); }
                                ui.label(path.as_ref().and_then(|p| p.file_name()).map(|p| p.to_string_lossy()).unwrap_or_default());
                            });
                        }
                        ui.label("Private key password (if encrypted)");
                        ui.add(egui::TextEdit::singleline(&mut s.key_password).password(true));
                    } else {
                        ui.label("AES-256 encryption (PDF 2.0, revision 6). A password will be required to open the exported copy.");
                        ui.label("Password (ASCII, up to 127 bytes)");
                        ui.add(egui::TextEdit::singleline(&mut s.password).password(true));
                        ui.label("Confirm password");
                        ui.add(egui::TextEdit::singleline(&mut s.confirm).password(true));
                        ui.weak("This pass uses the same password for opening and full access. Permissions are unrestricted after opening.");
                    }
                    ui.add_space(10.0);
                    let ready = self.open.is_some() && if s.signing { s.certificate.is_some() && s.key.is_some() }
                        else { !s.password.is_empty() && s.password == s.confirm && s.password.is_ascii() && s.password.len() <= 127 };
                    export = ui.add_enabled(ready, egui::Button::new(if s.signing { "Sign and export copy…" } else { "Export encrypted copy…" })).clicked();
                });
                if self.security.job.is_some() { ui.spinner(); ui.label("Preparing secure copy…"); }
            });
        self.security.open = open;
        if !open {
            self.security.password.clear();
            self.security.confirm.clear();
            self.security.key_password.clear();
        }
        if export {
            self.start_security_export();
        }
    }

    fn start_security_export(&mut self) {
        self.commit_ink();
        self.finish_editing(true);
        self.rebuild(&std::collections::HashSet::new(), false);
        let Some(o) = &self.open else { return };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .set_file_name(if self.security.signing {
                "signed.pdf"
            } else {
                "encrypted.pdf"
            })
            .save_file()
        else {
            return;
        };
        if o.path.as_ref().is_some_and(|source| {
            source == &path
                || source
                    .canonicalize()
                    .ok()
                    .zip(path.canonicalize().ok())
                    .is_some_and(|(a, b)| a == b)
        }) {
            self.error = Some("Choose a new file for the security export.".into());
            return;
        }
        let doc = o.doc.clone();
        let signing = self.security.signing;
        let password = std::mem::take(&mut self.security.password);
        self.security.confirm.clear();
        let key_password = std::mem::take(&mut self.security.key_password);
        let cert = self.security.certificate.clone();
        let key = self.security.key.clone();
        let author = self.settings.review_author.clone();
        let (tx, rx) = mpsc::channel();
        let wake = self.ctx.clone();
        let spawned = std::thread::Builder::new().name("security-export".into()).spawn(move || {
            let result = (|| -> Result<String, String> {
                let bytes = if signing {
                    let prepared = doc.prepare_signature(&author, 32_768).map_err(|e| e.to_string())?;
                    let cert = cert.ok_or("Choose a certificate")?;
                    let key = key.ok_or("Choose a private key")?;
                    let mut child = Command::new("openssl").args(["cms", "-sign", "-binary", "-outform", "DER", "-md", "sha256", "-nosmimecap", "-signer"])
                        .arg(cert).arg("-inkey").arg(key).args(["-passin", "env:ATELIER_SIGNING_PASSWORD"])
                        .env("ATELIER_SIGNING_PASSWORD", key_password)
                        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
                        .map_err(|e| format!("Could not start OpenSSL. Install OpenSSL to use PEM signing: {e}"))?;
                    let written = child.stdin.take().ok_or("Could not open signer input")?.write_all(&prepared.signing_bytes());
                    let output = child.wait_with_output().map_err(|e| format!("OpenSSL failed: {e}"))?;
                    if !output.status.success() { return Err(format!("Certificate signing failed: {}", String::from_utf8_lossy(&output.stderr))); }
                    written.map_err(|e| format!("Could not send the document to OpenSSL: {e}"))?;
                    prepared.finish(&output.stdout).map_err(|e| e.to_string())?
                } else {
                    doc.update().to_encrypted_bytes(&password, &password).map_err(|e| e.to_string())?
                };
                super::write_replacing(&path, &bytes).map_err(|e| format!("Could not export {}: {e}", path.display()))?;
                Ok(format!("{} copy exported to {}.", if signing { "Certificate-signed" } else { "Encrypted" }, path.display()))
            })();
            let _ = tx.send(result); wake.request_repaint();
        });
        match spawned {
            Ok(_) => self.security.job = Some(rx),
            Err(e) => self.error = Some(format!("Could not start security export: {e}")),
        }
    }
}
