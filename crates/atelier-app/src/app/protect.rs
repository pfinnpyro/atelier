//! Documents protected with OpenPGP: opening them, keeping them
//! protected when saved, exporting a protected copy, and looking after
//! the keys that takes.
//!
//! A protected file is a cover page around the whole document, encrypted
//! (see `atelier_pgp::pdf`). Opening one decrypts it into memory only.
//! Saving seals the changed document for the same readers and never
//! writes it out in the clear; an unprotected copy is made only by
//! asking for one.

use std::collections::HashSet;
use std::path::PathBuf;

use atelier_pgp::{Certificate, Curve, Error, Readers, SecretKey, Summary};
use eframe::egui::{self, Vec2};

use super::Viewer;
use crate::keys::{self, KeyStore};

/// Whether this build turns passphrases into keys for OpenPGP. The FIPS
/// build does not: OpenPGP's way of doing it is not an approved one.
const PASSPHRASES: bool = !cfg!(feature = "fips");

/// A protected file the unlock prompt is open for.
pub(super) struct Sealed {
    payload: Vec<u8>,
    name: String,
    path: Option<PathBuf>,
    summary: Summary,
    /// Who it says it is for.
    readers: Vec<String>,
    /// The person's own key is one of them.
    mine: bool,
    passphrase: String,
    key_passphrase: String,
    wrong: Option<String>,
    focus: bool,
}

impl Drop for Sealed {
    fn drop(&mut self) {
        atelier_form::wipe_string(&mut self.passphrase);
        atelier_form::wipe_string(&mut self.key_passphrase);
    }
}

pub(super) struct ProtectUi {
    pub keys: KeyStore,
    /// The OpenPGP window is open.
    pub open: bool,
    pub sealed: Option<Sealed>,
    /// The readers of a document just decrypted, on their way to the
    /// tab it opens in.
    pub pending: Option<Readers>,
    // Making or restoring a key.
    name: String,
    curve: Curve,
    key_passphrase: String,
    key_confirm: String,
    remove_armed: bool,
    // Protecting a copy.
    to_self: bool,
    to: HashSet<String>,
    passphrase: String,
    confirm: String,
    /// What went wrong in the window, shown there.
    problem: Option<String>,
}

impl Default for ProtectUi {
    fn default() -> Self {
        Self {
            keys: KeyStore::load(),
            open: false,
            sealed: None,
            pending: None,
            name: String::new(),
            curve: Curve::P256,
            key_passphrase: String::new(),
            key_confirm: String::new(),
            remove_armed: false,
            to_self: true,
            to: HashSet::new(),
            passphrase: String::new(),
            confirm: String::new(),
            problem: None,
        }
    }
}

impl ProtectUi {
    fn forget_passphrases(&mut self) {
        for secret in [
            &mut self.key_passphrase,
            &mut self.key_confirm,
            &mut self.passphrase,
            &mut self.confirm,
        ] {
            atelier_form::wipe_string(secret);
        }
    }
}

/// What a click in the OpenPGP window asks for, done once the window
/// has been drawn.
enum Act {
    CreateKey,
    RestoreKey,
    ExportCertificate,
    BackUpKey,
    RemoveKey,
    AddContact,
    RemoveContact(String),
    ExportProtected,
    ExportUnprotected,
}

/// A list of names as a phrase: "You, Bob and one other".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [most @ .., last] => format!("{} and {last}", most.join(", ")),
    }
}

/// Who a protected document's readers are, as a sentence.
fn readers_sentence(names: &[String], passphrase: bool) -> String {
    let mut who = names.to_vec();
    if passphrase {
        who.push("anyone with its passphrase".into());
    }
    listed(&who)
}

impl Viewer {
    pub(super) fn open_protect(&mut self) {
        self.commit_ink();
        self.finish_editing(true);
        self.protect.open = true;
        self.protect.problem = None;
    }

    /// A protected file was read: opens it with what is at hand, or
    /// asks for what it needs.
    pub(super) fn open_protected(&mut self, payload: Vec<u8>, name: String, path: Option<PathBuf>) {
        let summary = match atelier_pgp::payload_summary(&payload) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(format!("Could not open {name}: {e}"));
                return;
            }
        };
        self.protect.sealed = Some(Sealed {
            readers: self.protect.keys.reader_names(&summary),
            mine: self.protect.keys.is_reader(&summary),
            payload,
            name,
            path,
            summary,
            passphrase: String::new(),
            key_passphrase: String::new(),
            wrong: None,
            focus: true,
        });
    }

    /// Tries what was typed into the unlock prompt.
    fn unseal(&mut self) {
        let Some(mut sealed) = self.protect.sealed.take() else {
            return;
        };
        if sealed.mine
            && !sealed.key_passphrase.is_empty()
            && let Err(e) = self.protect.keys.unlock(&sealed.key_passphrase)
        {
            sealed.wrong = Some(e);
            sealed.focus = true;
            self.protect.sealed = Some(sealed);
            return;
        }
        let result = {
            let keys: Vec<&SecretKey> = self.protect.keys.secret().into_iter().collect();
            let passphrase = (PASSPHRASES && !sealed.passphrase.is_empty())
                .then_some(sealed.passphrase.as_bytes());
            atelier_pgp::unprotect_pdf_keeping_readers(
                atelier_pgp::provider(),
                &sealed.payload,
                &keys,
                passphrase,
            )
        };
        // The key was needed for this and is not kept decrypted.
        self.protect.keys.lock();
        match result {
            Ok((pdf, readers)) => {
                let who = readers_sentence(&sealed.readers, sealed.summary.passphrase);
                self.protect.pending = Some(readers);
                let (name, path) = (std::mem::take(&mut sealed.name), sealed.path.take());
                self.open_with_password(pdf, name, "", path);
                self.protect.pending = None;
                if self.error.is_none() && self.notice.is_none() {
                    self.notice = Some(format!(
                        "This document is protected with OpenPGP for {who}. Saving keeps it \
                         protected for the same readers."
                    ));
                }
            }
            Err(Error::NoKey) => {
                sealed.wrong = Some(
                    if sealed.mine && sealed.summary.passphrase && PASSPHRASES {
                        "Neither your key nor that passphrase opens it."
                    } else if sealed.mine {
                        "Your key does not open it."
                    } else {
                        "That passphrase is not right."
                    }
                    .into(),
                );
                sealed.focus = true;
                self.protect.sealed = Some(sealed);
            }
            Err(e) => self.error = Some(format!("Could not open {}: {e}", sealed.name)),
        }
    }

    /// The prompt for a protected file's key or passphrase.
    pub(super) fn sealed_window(&mut self, ctx: &egui::Context) {
        let Some(sealed) = &mut self.protect.sealed else {
            return;
        };
        let by_passphrase = sealed.summary.passphrase && PASSPHRASES;
        let can_open = sealed.mine || by_passphrase;
        let (mut submit, mut cancel) = (false, false);
        egui::Window::new("Protected document")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(format!("{} is encrypted with OpenPGP.", sealed.name));
                let who = readers_sentence(&sealed.readers, sealed.summary.passphrase);
                if !who.is_empty() {
                    ui.label(format!("It can be opened by {who}."));
                }
                ui.add_space(4.0);
                let mut fields = Vec::new();
                if sealed.mine {
                    ui.label("The passphrase of your key");
                    fields.push(
                        ui.add(
                            egui::TextEdit::singleline(&mut sealed.key_passphrase)
                                .password(true)
                                .desired_width(280.0),
                        ),
                    );
                }
                if by_passphrase {
                    ui.label(if sealed.mine {
                        "Or the document's passphrase"
                    } else {
                        "The document's passphrase"
                    });
                    fields.push(
                        ui.add(
                            egui::TextEdit::singleline(&mut sealed.passphrase)
                                .password(true)
                                .desired_width(280.0),
                        ),
                    );
                } else if sealed.summary.passphrase {
                    ui.weak(
                        "It also opens with a passphrase, but this FIPS build of Atelier does \
                         not use OpenPGP passphrases: the way they become keys is not an \
                         approved one.",
                    );
                }
                if !can_open {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        "Your key is not one it was encrypted for, so it cannot be opened here.",
                    );
                }
                if sealed.focus {
                    if let Some(first) = fields.first() {
                        first.request_focus();
                    }
                    sealed.focus = false;
                }
                let entered = fields.iter().any(|f| f.lost_focus())
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if let Some(wrong) = &sealed.wrong {
                    ui.colored_label(ui.visuals().error_fg_color, wrong);
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let ready = !sealed.key_passphrase.is_empty() || !sealed.passphrase.is_empty();
                    if ui
                        .add_enabled(can_open && ready, egui::Button::new("Open"))
                        .clicked()
                        || (entered && ready)
                    {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.protect.sealed = None;
        } else if submit {
            self.unseal();
        }
    }

    /// The bytes to write when saving the document in front: sealed for
    /// its readers when it was opened from a protected file, and as they
    /// are otherwise. An error means nothing must be written.
    pub(super) fn bytes_to_save(&self) -> Result<std::borrow::Cow<'_, [u8]>, String> {
        let Some(o) = &self.open else {
            return Err("No document is open.".into());
        };
        match &o.readers {
            Some(readers) => {
                atelier_pgp::protect_pdf_for(atelier_pgp::provider(), o.doc.data(), readers)
                    .map(std::borrow::Cow::Owned)
                    .map_err(|e| {
                        format!("Could not encrypt the document, so it was not saved: {e}")
                    })
            }
            None => Ok(std::borrow::Cow::Borrowed(o.doc.data())),
        }
    }

    /// The window for keys and for protecting a copy.
    pub(super) fn protect_window(&mut self, ctx: &egui::Context) {
        if !self.protect.open {
            return;
        }
        let mut open = true;
        let mut act = None;
        let protected = self.open.as_ref().and_then(|o| {
            let readers = o.readers.as_ref()?;
            let summary = readers.summary();
            Some(readers_sentence(
                &self.protect.keys.reader_names(summary),
                summary.passphrase,
            ))
        });
        let has_document = self.open.is_some();
        egui::Window::new("OpenPGP protection")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .frame(egui::Frame::window(&ctx.global_style()).inner_margin(18.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(560.0)
                    .show(ui, |ui| {
                        let p = &mut self.protect;
                        ui.strong("This document");
                        match &protected {
                            Some(who) => {
                                ui.label(format!(
                                    "Protected for {who}. Saving keeps it protected for the \
                                     same readers; to change who can read it, export a new \
                                     protected copy."
                                ));
                                if ui.button("Export an unprotected copy…").clicked() {
                                    act = Some(Act::ExportUnprotected);
                                }
                            }
                            None if has_document => {
                                ui.label(
                                    "A protected copy holds the whole document encrypted, \
                                     behind a cover page. Other PDF viewers show only the \
                                     cover; signatures inside stay valid.",
                                );
                            }
                            None => {
                                ui.weak("Open a document to protect a copy of it.");
                            }
                        }
                        if has_document {
                            ui.add_space(4.0);
                            ui.label("Who can open the copy");
                            match &p.keys.own {
                                Some(own) => {
                                    ui.checkbox(&mut p.to_self, format!("You ({})", keys::label(own)));
                                }
                                None => {
                                    ui.weak("You have no key yet, so the copy cannot be for you.");
                                }
                            }
                            for cert in &p.keys.contacts {
                                let id = cert.fingerprint_hex();
                                let mut on = p.to.contains(&id);
                                if ui.checkbox(&mut on, keys::label(cert)).changed() {
                                    if on {
                                        p.to.insert(id);
                                    } else {
                                        p.to.remove(&id);
                                    }
                                }
                            }
                            if PASSPHRASES {
                                ui.label("And anyone with this passphrase (optional)");
                                ui.add(egui::TextEdit::singleline(&mut p.passphrase).password(true));
                                if !p.passphrase.is_empty() {
                                    ui.label("The passphrase again");
                                    ui.add(egui::TextEdit::singleline(&mut p.confirm).password(true));
                                }
                            } else {
                                ui.weak(
                                    "This FIPS build encrypts to keys only: the way OpenPGP \
                                     turns a passphrase into a key is not an approved one.",
                                );
                            }
                            let anyone = (p.to_self && p.keys.own.is_some())
                                || p.keys.contacts.iter().any(|c| p.to.contains(&c.fingerprint_hex()))
                                || !p.passphrase.is_empty();
                            let ready = anyone && p.passphrase == p.confirm_or_empty();
                            if ui
                                .add_enabled(ready, egui::Button::new("Export a protected copy…"))
                                .clicked()
                            {
                                act = Some(Act::ExportProtected);
                            }
                        }

                        ui.separator();
                        ui.strong("Your key");
                        match &p.keys.own {
                            Some(own) => {
                                ui.label(keys::label(own));
                                ui.monospace(keys::grouped(&own.fingerprint_hex()));
                                ui.weak(
                                    "Give people your certificate so they can encrypt for you, \
                                     and let them check this fingerprint with you. The key \
                                     itself stays here, encrypted under its passphrase.",
                                );
                                ui.horizontal_wrapped(|ui| {
                                    if ui.button("Export certificate…").clicked() {
                                        act = Some(Act::ExportCertificate);
                                    }
                                    if ui
                                        .button("Back up key…")
                                        .on_hover_text(
                                            "Saves the key still encrypted under its passphrase, \
                                             to restore on another computer",
                                        )
                                        .clicked()
                                    {
                                        act = Some(Act::BackUpKey);
                                    }
                                });
                                ui.horizontal_wrapped(|ui| {
                                    ui.checkbox(
                                        &mut p.remove_armed,
                                        "Remove it: documents encrypted for it will no longer open",
                                    );
                                    if ui
                                        .add_enabled(p.remove_armed, egui::Button::new("Remove key"))
                                        .clicked()
                                    {
                                        act = Some(Act::RemoveKey);
                                    }
                                });
                            }
                            None => {
                                ui.label("Your name and address, as others will see them");
                                ui.add(
                                    egui::TextEdit::singleline(&mut p.name)
                                        .hint_text("Ada Lovelace <ada@example.org>"),
                                );
                                ui.horizontal(|ui| {
                                    ui.label("Curve");
                                    ui.selectable_value(&mut p.curve, Curve::P256, "NIST P-256");
                                    ui.selectable_value(&mut p.curve, Curve::P384, "NIST P-384");
                                });
                                ui.label(format!(
                                    "A passphrase to keep the key under (at least {})",
                                    crate::sealing::MIN_PASSPHRASE.max(1)
                                ));
                                ui.add(egui::TextEdit::singleline(&mut p.key_passphrase).password(true));
                                ui.label("The passphrase again");
                                ui.add(egui::TextEdit::singleline(&mut p.key_confirm).password(true));
                                ui.weak(
                                    "The passphrase is not kept. Without it the key, and what \
                                     is encrypted for it, cannot be recovered.",
                                );
                                ui.horizontal_wrapped(|ui| {
                                    let ready = !p.key_passphrase.is_empty()
                                        && p.key_passphrase == p.key_confirm;
                                    if ui
                                        .add_enabled(ready, egui::Button::new("Make a key"))
                                        .clicked()
                                    {
                                        act = Some(Act::CreateKey);
                                    }
                                    if ui
                                        .add_enabled(
                                            !p.key_passphrase.is_empty(),
                                            egui::Button::new("Restore from a backup…"),
                                        )
                                        .on_hover_text("Type the backup's passphrase above first")
                                        .clicked()
                                    {
                                        act = Some(Act::RestoreKey);
                                    }
                                });
                            }
                        }

                        ui.separator();
                        ui.strong("Other people's certificates");
                        if p.keys.contacts.is_empty() {
                            ui.weak("None yet. Add the certificate someone gave you to encrypt for them.");
                        }
                        for cert in &p.keys.contacts {
                            ui.horizontal(|ui| {
                                ui.label(keys::label(cert));
                                if ui.small_button("Remove").clicked() {
                                    act = Some(Act::RemoveContact(cert.fingerprint_hex()));
                                }
                            });
                            ui.weak(keys::grouped(&cert.fingerprint_hex()));
                        }
                        if ui.button("Add a certificate…").clicked() {
                            act = Some(Act::AddContact);
                        }

                        if let Some(problem) = &p.problem {
                            ui.add_space(6.0);
                            ui.colored_label(ui.visuals().error_fg_color, problem);
                        }
                        ui.add_space(6.0);
                        ui.weak(format!(
                            "OpenPGP (RFC 9580) with NIST P-256 or P-384 keys and AES-256 in \
                             GCM. Cryptography by {}.",
                            atelier_pgp::provider().name()
                        ));
                    });
            });
        self.protect.open = open;
        if !open {
            self.protect.forget_passphrases();
            self.protect.remove_armed = false;
        }
        if let Some(act) = act {
            self.protect.problem = None;
            if let Err(problem) = self.protect_act(act) {
                self.protect.problem = Some(problem);
            }
        }
    }

    fn protect_act(&mut self, act: Act) -> Result<(), String> {
        let io = |what: &str, e: std::io::Error| format!("Could not {what}: {e}");
        match act {
            Act::CreateKey => {
                let p = &mut self.protect;
                p.keys.create(&p.name, p.curve, &p.key_passphrase)?;
                p.keys.lock();
                p.forget_passphrases();
                p.name.clear();
                self.notice = Some(
                    "Your key is made. Export its certificate and give that to the people \
                     who will encrypt documents for you."
                        .into(),
                );
            }
            Act::RestoreKey => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Atelier key backups", &["atelier-key"])
                    .add_filter("All files", &["*"])
                    .pick_file()
                else {
                    return Ok(());
                };
                let backup =
                    std::fs::read_to_string(&path).map_err(|e| io("read the backup", e))?;
                let p = &mut self.protect;
                p.keys.restore(&backup, &p.key_passphrase)?;
                p.keys.lock();
                p.forget_passphrases();
                self.notice = Some("Your key is restored.".into());
            }
            Act::ExportCertificate => {
                let Some(own) = &self.protect.keys.own else {
                    return Ok(());
                };
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("OpenPGP certificates", &["asc"])
                    .set_file_name("my-certificate.asc")
                    .save_file()
                else {
                    return Ok(());
                };
                super::write_replacing(&path, own.to_armored().as_bytes())
                    .map_err(|e| io("save the certificate", e))?;
                self.notice = Some(format!(
                    "Certificate saved as {}. It holds nothing secret.",
                    super::display_name(&path)
                ));
            }
            Act::BackUpKey => {
                let backup = self.protect.keys.backup()?;
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Atelier key backups", &["atelier-key"])
                    .set_file_name("my-key.atelier-key")
                    .save_file()
                else {
                    return Ok(());
                };
                super::write_replacing(&path, backup.as_bytes())
                    .map_err(|e| io("save the backup", e))?;
                self.notice = Some(format!(
                    "Key backed up as {}, still encrypted under its passphrase.",
                    super::display_name(&path)
                ));
            }
            Act::RemoveKey => {
                self.protect.keys.remove_own()?;
                self.protect.remove_armed = false;
            }
            Act::AddContact => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        "OpenPGP certificates",
                        &["asc", "pgp", "gpg", "key", "cert"],
                    )
                    .add_filter("All files", &["*"])
                    .pick_file()
                else {
                    return Ok(());
                };
                let data = std::fs::read(&path).map_err(|e| io("read the certificate", e))?;
                let name = self.protect.keys.import(&data)?;
                self.notice = Some(format!(
                    "Added {name}. Check the fingerprint with them before relying on it."
                ));
            }
            Act::RemoveContact(fingerprint) => {
                self.protect.keys.remove_contact(&fingerprint)?;
                self.protect.to.remove(&fingerprint);
            }
            Act::ExportProtected => self.export_protected()?,
            Act::ExportUnprotected => {
                self.settle_for_export();
                let Some(o) = &self.open else { return Ok(()) };
                let stem = o.name.strip_suffix(".pdf").unwrap_or(&o.name);
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("PDF documents", &["pdf"])
                    .set_file_name(format!("{stem} (unprotected).pdf"))
                    .save_file()
                else {
                    return Ok(());
                };
                self.not_over_the_open_file(&path)?;
                super::write_replacing(&path, o.doc.data())
                    .map_err(|e| io("export the copy", e))?;
                self.notice = Some(format!(
                    "Unprotected copy exported to {}. Anyone can read that file.",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    /// Puts pending ink, edits and OCR text into the document, as an
    /// export of it should have them.
    fn settle_for_export(&mut self) {
        self.commit_ink();
        self.finish_editing(true);
        self.rebuild(&HashSet::new(), false);
    }

    fn not_over_the_open_file(&self, path: &std::path::Path) -> Result<(), String> {
        let same = self
            .open
            .as_ref()
            .and_then(|o| o.path.as_ref())
            .is_some_and(|source| {
                source == path
                    || source
                        .canonicalize()
                        .ok()
                        .zip(path.canonicalize().ok())
                        .is_some_and(|(a, b)| a == b)
            });
        if same {
            return Err("Choose a new file for the copy.".into());
        }
        Ok(())
    }

    fn export_protected(&mut self) -> Result<(), String> {
        self.settle_for_export();
        let Some(o) = &self.open else { return Ok(()) };
        let p = &self.protect;
        let mut to: Vec<&Certificate> = Vec::new();
        if p.to_self {
            to.extend(&p.keys.own);
        }
        to.extend(
            p.keys
                .contacts
                .iter()
                .filter(|c| p.to.contains(&c.fingerprint_hex())),
        );
        let passphrase =
            (PASSPHRASES && !p.passphrase.is_empty()).then_some(p.passphrase.as_bytes());
        let stem = o.name.strip_suffix(".pdf").unwrap_or(&o.name);
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .set_file_name(format!("{stem} (protected).pdf"))
            .save_file()
        else {
            return Ok(());
        };
        self.not_over_the_open_file(&path)?;
        let bytes = atelier_pgp::protect_pdf(
            atelier_pgp::provider(),
            o.doc.data(),
            &to,
            passphrase,
            keys::now(),
        )
        .map_err(|e| e.to_string())?;
        super::write_replacing(&path, &bytes)
            .map_err(|e| format!("Could not export {}: {e}", path.display()))?;
        let mut names: Vec<String> = to
            .iter()
            .map(|c| {
                if p.keys
                    .own
                    .as_ref()
                    .is_some_and(|own| own.fingerprint() == c.fingerprint())
                {
                    "you".to_string()
                } else {
                    keys::label(c)
                }
            })
            .collect();
        if passphrase.is_some() {
            names.push("anyone with the passphrase".into());
        }
        self.notice = Some(format!(
            "Protected copy for {} exported to {}.",
            listed(&names),
            path.display()
        ));
        self.protect.forget_passphrases();
        Ok(())
    }
}

impl ProtectUi {
    /// What the confirmation must equal: the passphrase when one was
    /// typed, and nothing otherwise.
    fn confirm_or_empty(&self) -> &str {
        if self.passphrase.is_empty() {
            ""
        } else {
            &self.confirm
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_syntax::Document;

    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
        4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
        5 0 obj << /Title (A secret title) >> endobj\n\
        trailer << /Root 1 0 R /Info 5 0 R >>\n%%EOF";

    fn viewer(test: &str) -> Viewer {
        let mut viewer = Viewer::initialize(
            &eframe::CreationContext::_new_kittest(egui::Context::default()),
            crate::settings::Settings {
                check_updates: false,
                ocr: false,
                orient_on_open: false,
                ..Default::default()
            },
        );
        viewer.protect.keys = KeyStore::for_test(test);
        viewer
    }

    fn has(bytes: &[u8], what: &[u8]) -> bool {
        bytes.windows(what.len()).any(|w| w == what)
    }

    /// The payload of a wrapper, decrypted with the viewer's own key.
    fn inside(viewer: &mut Viewer, wrapper: &[u8], passphrase: &str) -> Vec<u8> {
        let doc = Document::open(wrapper.to_vec()).unwrap();
        assert_eq!(doc.pages().unwrap().len(), 1, "the cover page");
        let payload = atelier_pgp::protected_payload(&doc).unwrap().unwrap();
        viewer.protect.keys.unlock(passphrase).unwrap();
        let key = viewer.protect.keys.secret().unwrap();
        let pdf =
            atelier_pgp::unprotect_pdf(atelier_pgp::provider(), &payload, &[key], None).unwrap();
        viewer.protect.keys.lock();
        pdf
    }

    #[test]
    fn a_protected_file_opens_with_the_key_and_is_saved_protected() {
        let mut viewer = viewer("open-save");
        viewer
            .protect
            .keys
            .create("Ada <ada@example.org>", Curve::P256, "key passphrase")
            .unwrap();
        viewer.protect.keys.lock();
        let own = viewer.protect.keys.own.clone().unwrap();
        let wrapped =
            atelier_pgp::protect_pdf(atelier_pgp::provider(), PDF, &[&own], None, keys::now())
                .unwrap();

        // Opening shows the prompt and nothing of the document.
        viewer.open_with_password(wrapped.clone(), "mine.pdf".into(), "", None);
        assert!(viewer.open.is_none());
        let sealed = viewer.protect.sealed.as_mut().unwrap();
        assert!(sealed.mine);
        assert_eq!(sealed.readers, ["You"]);
        sealed.key_passphrase = "not the passphrase".into();
        viewer.unseal();
        assert!(viewer.open.is_none());
        let sealed = viewer.protect.sealed.as_mut().unwrap();
        assert_eq!(
            sealed.wrong.as_deref(),
            Some("That passphrase is not right.")
        );

        sealed.key_passphrase = "key passphrase".into();
        viewer.unseal();
        assert!(viewer.protect.sealed.is_none());
        assert!(viewer.error.is_none(), "{:?}", viewer.error);
        let open = viewer.open.as_ref().unwrap();
        assert_eq!(open.doc.pages().unwrap().len(), 2);
        assert!(open.readers.is_some());
        assert!(
            viewer
                .notice
                .as_deref()
                .unwrap()
                .contains("protected with OpenPGP for You")
        );
        // The key does not stay decrypted.
        assert!(viewer.protect.keys.secret().is_none());

        // What saving writes is a protected file again, for the same
        // reader, sealed afresh. (No path is used here: opening by path
        // would put the file on the list of recent files.)
        let saved = viewer.bytes_to_save().unwrap().into_owned();
        assert!(!has(&saved, b"A secret title"));
        assert_ne!(saved, wrapped);
        assert_eq!(inside(&mut viewer, &saved, "key passphrase"), PDF);
    }

    #[test]
    fn a_file_for_someone_else_says_so() {
        let mut viewer = viewer("not-mine");
        viewer
            .protect
            .keys
            .create("Ada", Curve::P256, "key passphrase")
            .unwrap();
        let bob =
            SecretKey::generate(atelier_pgp::provider(), Curve::P384, "Bob", keys::now()).unwrap();
        let wrapped = atelier_pgp::protect_pdf(
            atelier_pgp::provider(),
            PDF,
            &[bob.certificate()],
            None,
            keys::now(),
        )
        .unwrap();
        viewer.open_with_password(wrapped.clone(), "theirs.pdf".into(), "", None);
        assert!(viewer.open.is_none());
        let sealed = viewer.protect.sealed.as_ref().unwrap();
        assert!(!sealed.mine);
        assert!(sealed.readers[0].starts_with("an unknown key ("));
        // Once Bob's certificate is known, he is named.
        viewer.protect.sealed = None;
        viewer
            .protect
            .keys
            .import(bob.certificate().to_bytes())
            .unwrap();
        viewer.open_with_password(wrapped, "theirs.pdf".into(), "", None);
        assert_eq!(viewer.protect.sealed.as_ref().unwrap().readers, ["Bob"]);
    }

    #[cfg(not(feature = "fips"))]
    #[test]
    fn a_passphrase_opens_it_and_saving_keeps_every_reader() {
        let mut viewer = viewer("passphrase");
        viewer
            .protect
            .keys
            .create("Ada", Curve::P256, "key passphrase")
            .unwrap();
        viewer.protect.keys.lock();
        let own = viewer.protect.keys.own.clone().unwrap();
        let wrapped = atelier_pgp::protect_pdf(
            atelier_pgp::provider(),
            PDF,
            &[&own],
            Some(b"document passphrase"),
            keys::now(),
        )
        .unwrap();
        viewer.open_with_password(wrapped, "shared.pdf".into(), "", None);
        let sealed = viewer.protect.sealed.as_mut().unwrap();
        sealed.passphrase = "document passphrase".into();
        viewer.unseal();
        assert!(viewer.open.is_some(), "{:?}", viewer.error);
        // Opened without the key, it is still saved for the key.
        let saved = viewer.bytes_to_save().unwrap().into_owned();
        assert!(!has(&saved, b"A secret title"));
        assert_eq!(inside(&mut viewer, &saved, "key passphrase"), PDF);
    }

    /// Draws the windows in each of their states, which catches a
    /// panic in the interface code; how they look is not checked.
    #[test]
    fn the_windows_draw_in_every_state() {
        let mut viewer = viewer("draw");
        let draw = |viewer: &mut Viewer| {
            let ctx = viewer.ctx.clone();
            let mut drawn = ctx.run_ui(egui::RawInput::default(), |ui| {
                let ctx = ui.ctx().clone();
                viewer.sealed_window(&ctx);
                viewer.protect_window(&ctx);
            });
            assert!(!drawn.shapes.is_empty());
            // There is no screen to hand the font texture to.
            drawn.textures_delta.clear();
        };
        // No key, no document.
        viewer.open_protect();
        draw(&mut viewer);
        // A key, someone to write to, and a document.
        viewer
            .protect
            .keys
            .create("Ada", Curve::P256, "key passphrase")
            .unwrap();
        let bob =
            SecretKey::generate(atelier_pgp::provider(), Curve::P384, "Bob", keys::now()).unwrap();
        viewer
            .protect
            .keys
            .import(bob.certificate().to_bytes())
            .unwrap();
        viewer.open_with_password(PDF.to_vec(), "plain.pdf".into(), "", None);
        viewer.protect.passphrase = "typed".into();
        draw(&mut viewer);
        // The prompt for a protected file, then the file open.
        let own = viewer.protect.keys.own.clone().unwrap();
        let wrapped = atelier_pgp::protect_pdf(
            atelier_pgp::provider(),
            PDF,
            &[&own, bob.certificate()],
            None,
            keys::now(),
        )
        .unwrap();
        viewer.open_with_password(wrapped, "mine.pdf".into(), "", None);
        draw(&mut viewer);
        let sealed = viewer.protect.sealed.as_mut().unwrap();
        sealed.key_passphrase = "wrong".into();
        viewer.unseal();
        draw(&mut viewer);
        viewer.protect.sealed.as_mut().unwrap().key_passphrase = "key passphrase".into();
        viewer.unseal();
        assert!(viewer.open.as_ref().unwrap().readers.is_some());
        draw(&mut viewer);
    }

    #[test]
    fn an_ordinary_file_is_saved_as_it_is() {
        let mut viewer = viewer("plain");
        viewer.open_with_password(PDF.to_vec(), "plain.pdf".into(), "", None);
        assert!(viewer.protect.sealed.is_none());
        assert!(viewer.open.as_ref().unwrap().readers.is_none());
        assert_eq!(viewer.bytes_to_save().unwrap(), PDF);
    }

    #[test]
    fn readers_read_as_a_sentence() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(readers_sentence(&names(&["You"]), false), "You");
        assert_eq!(
            readers_sentence(&names(&["You", "Bob"]), false),
            "You and Bob"
        );
        assert_eq!(
            readers_sentence(&names(&["You", "Bob"]), true),
            "You, Bob and anyone with its passphrase"
        );
        assert_eq!(readers_sentence(&[], true), "anyone with its passphrase");
        assert_eq!(readers_sentence(&[], false), "");
    }
}
