//! Background signature inspection with independent integrity, chain and time results.
use super::{
    Viewer,
    security_provider::{Scratch, openssl, pem_certificates},
};
use atelier_syntax::{Document, SignatureInspection};
use eframe::egui;
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{Arc, mpsc},
};

#[derive(Default)]
pub(super) struct Signatures {
    source: Option<Arc<Document>>,
    trust_bundle: Option<PathBuf>,
    job: Option<mpsc::Receiver<Result<Vec<Report>, String>>>,
    result: Option<Result<Vec<Report>, String>>,
}

struct Report {
    label: String,
    revision: usize,
    later: bool,
    changes: Option<Arc<atelier_syntax::Result<atelier_syntax::SignedRevisionChanges>>>,
    integrity: Result<bool, String>,
    certificates: Vec<CertificateReport>,
}

#[derive(Debug)]
enum Check {
    Passed,
    Failed(String),
    Unknown(String),
}
impl Check {
    fn label(&self) -> String {
        match self {
            Self::Passed => "passed".into(),
            Self::Failed(reason) => format!("failed ({reason})"),
            Self::Unknown(reason) => format!("not checked ({reason})"),
        }
    }
}
struct CertificateReport {
    details: String,
    chain: Check,
    chain_with_dates: Check,
    validity: Check,
}
struct Verification {
    valid: bool,
    certificates: Vec<CertificateReport>,
}

impl Viewer {
    pub(super) fn poll_signatures(&mut self, ctx: &egui::Context) {
        let Some(open) = &mut self.open else { return };
        let state = &mut open.signatures;
        if state
            .source
            .as_ref()
            .is_none_or(|d| !Arc::ptr_eq(d, &open.doc))
        {
            let bundle = state.trust_bundle.clone();
            *state = Signatures {
                trust_bundle: bundle,
                ..Default::default()
            };
            state.source = Some(open.doc.clone());
            let doc = open.doc.clone();
            let bundle = state.trust_bundle.clone();
            let wake = ctx.clone();
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new()
                .name("signature-verification".into())
                .spawn(move || {
                    let result = inspect_document(&doc, bundle.as_deref());
                    let _ = tx.send(result);
                    wake.request_repaint();
                }) {
                Ok(_) => state.job = Some(rx),
                Err(e) => {
                    state.result = Some(Err(format!("Could not start signature verification: {e}")))
                }
            }
        }
        if let Some(job) = &state.job {
            match job.try_recv() {
                Ok(result) => {
                    state.result = Some(result);
                    state.job = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    state.result = Some(Err("Signature verification worker stopped.".into()));
                    state.job = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
            }
        }
    }

    pub(super) fn signatures_panel(&mut self, ui: &mut egui::Ui) {
        let Some(open) = &mut self.open else { return };
        let state = &mut open.signatures;
        let mut retry = ui.button("Verify again").clicked();
        ui.horizontal_wrapped(|ui| {
            if ui.button("Choose CA bundle...").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("PEM certificates", &["pem", "crt"])
                    .pick_file()
            {
                state.trust_bundle = Some(path);
                retry = true;
            }
            if state.trust_bundle.is_some() && ui.button("Use default CA store").clicked() {
                state.trust_bundle = None;
                retry = true;
            }
        });
        ui.label(match &state.trust_bundle {
            Some(path) => format!("Trust source: {}", path.display()),
            None => {
                "Trust source: OpenSSL configured CA store (may differ from the OS store)".into()
            }
        });
        if retry {
            let bundle = state.trust_bundle.clone();
            *state = Signatures {
                trust_bundle: bundle,
                ..Default::default()
            };
            ui.ctx().request_repaint();
        }
        ui.label("Integrity, certificate chain trust and current certificate dates are checked separately. Revocation and timestamps are not checked.");
        ui.weak("Chain validation uses any certificate purpose; document-signing policy is not evaluated.");
        ui.weak("Includes indirect signature dictionaries from earlier PDF revisions. Later revisions are compared structurally. Categories do not establish whether changes are permitted. Declared restrictions remain enforced conservatively.");
        if open.signatures.result.is_some() {
            let policy = open.doc.signature_write_policy();
            for restriction in &policy.restrictions {
                ui.label(format!("Declared restriction: {restriction}"));
            }
            if let Err(error) = policy.check(false) {
                ui.colored_label(ui.visuals().warn_fg_color, error.to_string());
                ui.weak("Saving an unchanged copy remains available. These declarations are not a cryptographic trust result.");
            }
            if policy.has_signatures {
                ui.weak("Full rewrites, page extraction and encryption export are blocked to preserve signed bytes.");
            }
        }
        match &open.signatures.result {
            None => {
                ui.spinner();
                ui.label("Inspecting signatures…");
            }
            Some(Err(error)) => {
                ui.label(error);
            }
            Some(Ok(reports)) if reports.is_empty() => {
                ui.label("No signature dictionaries found.");
            }
            Some(Ok(reports)) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for report in reports {
                        ui.group(|ui| {
                            ui.label(&report.label);
                            match &report.integrity {
                                Ok(true) => {
                                    ui.label("Signed-byte integrity: valid");
                                }
                                Ok(false) => {
                                    ui.label("Signed-byte integrity: verification failed");
                                }
                                Err(e) => {
                                    ui.label(format!("Signed-byte integrity: not verified ({e})"));
                                }
                            }
                            if report.revision > 0 {
                                ui.label(format!(
                                    "Signed coverage ends at byte {}",
                                    report.revision
                                ));
                                ui.label(if report.later {
                                    "Later bytes exist outside this signature's coverage."
                                } else {
                                    "Coverage reaches the current file end."
                                });
                            }
                            if let Some(changes) = &report.changes {
                                show_revision_changes(ui, changes.as_ref(), report.later);
                            }
                            ui.label("Revocation: not checked");
                            if report.certificates.is_empty() {
                                ui.label("Certificate chain and dates: not checked");
                            }
                            for (index, certificate) in report.certificates.iter().enumerate() {
                                ui.collapsing(format!("Verifying signer {}", index + 1), |ui| {
                                    ui.label(format!(
                                        "Chain trust (dates excluded): {}",
                                        certificate.chain.label()
                                    ));
                                    ui.label(format!(
                                        "Chain validation with dates: {}",
                                        certificate.chain_with_dates.label()
                                    ));
                                    ui.label(format!(
                                        "Signer validity at verification time: {}",
                                        certificate.validity.label()
                                    ));
                                    ui.label(&certificate.details);
                                });
                            }
                        });
                    }
                });
            }
        }
    }
}

fn show_revision_changes(
    ui: &mut egui::Ui,
    changes: &atelier_syntax::Result<atelier_syntax::SignedRevisionChanges>,
    later: bool,
) {
    let changes = match changes {
        Ok(changes) => changes,
        Err(error) => {
            ui.label(format!("Later-change analysis: unknown ({error})"));
            return;
        }
    };
    if changes.unclassified_trailing_bytes {
        ui.label("Additional trailing bytes are not fully accounted for by parsed revisions.");
    }
    if later && changes.revisions.is_empty() {
        ui.label(
            "No subsequent cross-reference revision was found. Later bytes remain unclassified.",
        );
    }
    for (index, revision) in changes.revisions.iter().enumerate() {
        ui.collapsing(
            format!("Later revision {}: {} object changes", index + 1, revision.objects.len()),
            |ui| {
                ui.label(format!("Cross-reference offset: {}", revision.xref_offset));
                if !revision.trailer_keys.is_empty() {
                    ui.label(format!("Trailer keys: {}", revision.trailer_keys.join(", ")));
                }
                for change in revision.objects.iter().take(100) {
                    let keys = if change.keys.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", change.keys.join(", "))
                    };
                    ui.label(format!("{}: {} - {}{keys}",
                        change.object, change.kind.label(), change.category.label()));
                    if change.references_before == change.references_after {
                        ui.weak(format!("Reference contexts: {}", reference_contexts(&change.references_after)));
                    } else {
                        ui.weak(format!("Before: {}", reference_contexts(&change.references_before)));
                        ui.weak(format!("After: {}", reference_contexts(&change.references_after)));
                    }
                }
                if revision.objects.len() > 100 {
                    ui.label("Only the first 100 object changes are displayed.");
                }
                ui.weak("Reference contexts show forward reachability and conservative field inheritance, excluding recognized page, field and annotation backlinks. They do not prove visual effects, exclusive ownership or edit permission. Reverted changes remain listed.");
            },
        );
    }
}

fn reference_contexts(impact: &atelier_syntax::ReferenceImpact) -> String {
    fn abbreviated(values: impl IntoIterator<Item = String>, count: usize) -> String {
        let mut result = values.into_iter().take(8).collect::<Vec<_>>().join(", ");
        if count > 8 {
            result.push_str(&format!(", ... ({} total)", count));
        }
        if result.is_empty() {
            result.push_str("none mapped");
        }
        result
    }
    format!(
        "pages [{}]; fields [{}]; direct referrers [{}]{}",
        abbreviated(
            impact.page_contexts.iter().map(ToString::to_string),
            impact.page_contexts.len()
        ),
        abbreviated(
            impact.field_contexts.iter().cloned(),
            impact.field_contexts.len()
        ),
        abbreviated(
            impact.direct_referrers.iter().map(ToString::to_string),
            impact.direct_referrers.len()
        ),
        if impact.trailer_reference {
            "; referenced by trailer"
        } else {
            ""
        }
    )
}

fn inspect_document(
    doc: &Document,
    bundle: Option<&std::path::Path>,
) -> Result<Vec<Report>, String> {
    let _policy = doc.signature_write_policy();
    Ok(doc
        .inspect_signatures()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|candidate| match candidate {
            Err(e) => Report {
                label: "Signature inspection incomplete".into(),
                revision: 0,
                later: false,
                changes: None,
                integrity: Err(e.to_string()),
                certificates: Vec::new(),
            },
            Ok(signature) => {
                let mut report = Report {
                    label: format!(
                        "Signature object {} (PDF name: {}, untrusted)",
                        signature.object,
                        signature.signer.as_deref().unwrap_or("unspecified")
                    ),
                    revision: signature.signed_revision_len,
                    later: signature.has_subsequent_bytes,
                    changes: Some(doc.changes_since_signed_revision(signature.signed_revision_len)),
                    integrity: Err("not checked".into()),
                    certificates: Vec::new(),
                };
                match verify(&signature, bundle) {
                    Ok(verified) => {
                        report.integrity = Ok(verified.valid);
                        report.certificates = verified.certificates;
                    }
                    Err(e) => report.integrity = Err(e),
                }
                report
            }
        })
        .collect())
}

fn verify(
    signature: &SignatureInspection,
    bundle: Option<&std::path::Path>,
) -> Result<Verification, String> {
    if !matches!(
        signature.subfilter.as_deref(),
        Some("adbe.pkcs7.detached" | "ETSI.CAdES.detached")
    ) {
        return Err("unsupported signature format".into());
    }
    let scratch = Scratch::new()?;
    let cms = scratch.0.join("signature.der");
    let content = scratch.0.join("content.bin");
    let signer = scratch.0.join("signer.pem");
    let embedded = scratch.0.join("embedded.pem");
    std::fs::write(&cms, &signature.cms).map_err(|e| e.to_string())?;
    std::fs::write(&content, &signature.signed_bytes).map_err(|e| e.to_string())?;
    let output = openssl()
        .args([
            "cms",
            "-verify",
            "-binary",
            "-inform",
            "DER",
            "-noverify",
            "-in",
        ])
        .arg(&cms)
        .arg("-content")
        .arg(&content)
        .arg("-signer")
        .arg(&signer)
        .arg("-certsout")
        .arg(&embedded)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("OpenSSL unavailable: {e}"))?;
    if !output.success() {
        return Ok(Verification {
            valid: false,
            certificates: Vec::new(),
        });
    }
    let pem = std::fs::read_to_string(&signer).map_err(|e| e.to_string())?;
    let signers = pem_certificates(&pem)?;
    let mut certificates = Vec::new();
    for (index, pem) in signers.iter().enumerate() {
        let leaf = scratch.0.join(format!("signer-{index}.pem"));
        std::fs::write(&leaf, pem).map_err(|e| e.to_string())?;
        certificates.push(check_certificate(&leaf, &embedded, bundle, now_seconds()?));
    }
    Ok(Verification {
        valid: true,
        certificates,
    })
}

fn now_seconds() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| e.to_string())
}

fn check_certificate(
    leaf: &std::path::Path,
    embedded: &std::path::Path,
    bundle: Option<&std::path::Path>,
    now: u64,
) -> CertificateReport {
    let check_chain = |ignore_time: bool| {
        let mut command = openssl();
        command.args(["verify", "-show_chain", "-purpose", "any"]);
        if ignore_time {
            command.arg("-no_check_time");
        }
        if let Some(bundle) = bundle {
            command
                .args(["-no-CApath", "-no-CAstore", "-CAfile"])
                .arg(bundle);
        }
        command
            .arg("-untrusted")
            .arg(embedded)
            .arg(leaf)
            .stdin(Stdio::null())
            .env("LC_ALL", "C");
        match command.output() {
            Ok(o) if o.status.success() => (
                Check::Passed,
                String::from_utf8_lossy(&o.stdout).into_owned(),
            ),
            Ok(o) => (
                Check::Failed(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                String::new(),
            ),
            Err(e) => (Check::Unknown(e.to_string()), String::new()),
        }
    };
    let (chain, chain_details) = check_chain(true);
    let (chain_with_dates, _) = check_chain(false);
    let output = openssl()
        .args(["x509", "-in"])
        .arg(leaf)
        .args([
            "-noout",
            "-subject",
            "-issuer",
            "-dates",
            "-fingerprint",
            "-sha256",
        ])
        .stdin(Stdio::null())
        .env("LC_ALL", "C")
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let details = String::from_utf8_lossy(&o.stdout).into_owned();
            let validity = certificate_validity(&details, now);
            CertificateReport {
                details: format!("{details}{chain_details}"),
                chain,
                chain_with_dates,
                validity,
            }
        }
        _ => CertificateReport {
            details: "Certificate details unavailable".into(),
            chain,
            chain_with_dates,
            validity: Check::Unknown("could not read certificate dates".into()),
        },
    }
}

fn certificate_validity(details: &str, now: u64) -> Check {
    let date = |prefix: &str| {
        details
            .lines()
            .find_map(|l| l.strip_prefix(prefix))
            .and_then(parse_certificate_time)
    };
    match (date("notBefore="), date("notAfter=")) {
        (Some(start), Some(end)) if start <= end => {
            let now = i128::from(now);
            if now < start {
                Check::Failed("not yet valid".into())
            } else if now >= end {
                Check::Failed("expired".into())
            } else {
                Check::Passed
            }
        }
        _ => Check::Unknown("unsupported certificate dates".into()),
    }
}

// OpenSSL's default date output is fixed English UTC, e.g. Nov  3 12:00:00 2026 GMT.
fn parse_certificate_time(text: &str) -> Option<i128> {
    let parts: Vec<_> = text.split_whitespace().collect();
    if parts.len() != 5 || parts[4] != "GMT" {
        return None;
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| *m == parts[0])?;
    let day: u32 = parts[1].parse().ok()?;
    let year: i32 = parts[3].parse().ok()?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    let leap = |y: i32| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mut months = [31u32, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if leap(year) {
        months[1] = 29;
    }
    if day == 0 || day > months[month] {
        return None;
    }
    let time: Vec<u32> = parts[2]
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if time.len() != 3 || time[0] > 23 || time[1] > 59 || time[2] > 59 {
        return None;
    }
    let days_before = |y: i32| {
        let y = i128::from(y - 1);
        365 * y + y / 4 - y / 100 + y / 400
    };
    let days = days_before(year) - days_before(1970)
        + i128::from(months[..month].iter().sum::<u32>() + day - 1);
    Some(days * 86400 + i128::from(time[0] * 3600 + time[1] * 60 + time[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_save_preserves_pending_work_before_opening_a_dialog() {
        let ctx = egui::Context::default();
        let mut viewer = Viewer::initialize(
            &eframe::CreationContext::_new_kittest(ctx),
            crate::settings::Settings {
                check_updates: false,
                ocr: false,
                orient_on_open: false,
                ..Default::default()
            },
        );
        let bytes = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n4 0 obj << /Type /Sig /Reference [<< /TransformMethod /DocMDP /TransformParams << /P 1 >> >>] >> endobj\n%%EOF".to_vec();
        viewer.open_with_password(bytes, "restricted.pdf".into(), "", None);
        let original = viewer.open.as_ref().unwrap().doc.clone();
        viewer.open.as_mut().unwrap().layers_pending = true;
        viewer.save(crate::app::SaveTo::Copy);
        assert!(viewer.error.as_ref().is_some_and(|e| e.contains("DocMDP")));
        let open = viewer.open.as_ref().unwrap();
        assert!(open.layers_pending);
        assert!(Arc::ptr_eq(&original, &open.doc));
        assert!(open.path.is_none());
    }

    #[test]
    fn unsupported_format_never_reaches_openssl() {
        let signature = SignatureInspection {
            object: atelier_syntax::ObjRef::new(1, 0),
            signer: None,
            subfilter: Some("adbe.pkcs7.sha1".into()),
            signed_revision_len: 0,
            has_subsequent_bytes: false,
            cms: vec![],
            signed_bytes: vec![],
        };
        assert!(
            verify(&signature, None)
                .err()
                .unwrap()
                .contains("unsupported")
        );
    }

    #[test]
    fn runtime_verification_rejects_tampering_and_preserves_revision_scope() {
        if openssl().arg("version").output().is_err() {
            eprintln!("OpenSSL unavailable: runtime integration test skipped");
            return;
        }
        let scratch = Scratch::new().unwrap();
        let config = scratch.0.join("openssl.cnf");
        let key = scratch.0.join("key.pem");
        let cert = scratch.0.join("cert.pem");
        std::fs::write(&config, "[req]\ndistinguished_name=dn\n[dn]\n").unwrap();
        let generated = openssl()
            .args(["req", "-config"])
            .arg(&config)
            .args([
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-x509",
                "-subj",
                "/CN=Atelier runtime test",
                "-days",
                "1",
                "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let doc = Document::open(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n%%EOF".to_vec()).unwrap();
        let prepared = doc.prepare_signature("Untrusted PDF name", 32768).unwrap();
        let content = scratch.0.join("content");
        let cms = scratch.0.join("cms.der");
        std::fs::write(&content, prepared.signing_bytes()).unwrap();
        let signed = openssl()
            .args(["cms", "-sign", "-binary", "-outform", "DER", "-signer"])
            .arg(&cert)
            .arg("-inkey")
            .arg(&key)
            .arg("-in")
            .arg(&content)
            .arg("-out")
            .arg(&cms)
            .output()
            .unwrap();
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
        let bytes = prepared.finish(&std::fs::read(cms).unwrap()).unwrap();
        let saved = Document::open(bytes).unwrap();
        let reports = inspect_document(&saved, None).unwrap();
        assert!(matches!(reports[0].integrity, Ok(true)));
        assert!(
            reports[0].certificates[0]
                .details
                .contains("Atelier runtime test")
        );
        assert!(!reports[0].later);
        let mut signature = saved.inspect_signatures().unwrap().remove(0).unwrap();
        let trusted = verify(&signature, Some(&cert)).unwrap();
        assert!(trusted.valid);
        assert!(matches!(trusted.certificates[0].chain, Check::Passed));
        assert!(matches!(
            trusted.certificates[0].chain_with_dates,
            Check::Passed
        ));
        assert!(matches!(trusted.certificates[0].validity, Check::Passed));
        let missing = scratch.0.join("missing-ca.pem");
        let unavailable = verify(&signature, Some(&missing)).unwrap();
        assert!(
            unavailable.valid,
            "CA configuration failure must not alter integrity"
        );
        assert!(!matches!(unavailable.certificates[0].chain, Check::Passed));
        let expired = check_certificate(&cert, &cert, Some(&cert), now_seconds().unwrap() + 172800);
        assert!(matches!(expired.chain, Check::Passed));
        assert!(matches!(expired.validity, Check::Failed(ref e) if e == "expired"));
        signature.signed_bytes[10] ^= 1;
        assert!(!verify(&signature, None).unwrap().valid);
        let mut update = saved.update();
        update.add(atelier_syntax::Object::Integer(42));
        let changed = Document::open(update.to_bytes().unwrap()).unwrap();
        let reports = inspect_document(&changed, None).unwrap();
        assert!(matches!(reports[0].integrity, Ok(true)));
        assert!(reports[0].later);
        let changes = reports[0].changes.as_ref().unwrap();
        let changes = changes.as_ref().as_ref().unwrap();
        assert_eq!(changes.revisions.len(), 1);
        assert!(
            changes.revisions[0]
                .objects
                .iter()
                .any(|c| c.kind == atelier_syntax::ChangeKind::Added)
        );
    }

    #[test]
    fn chain_requires_selected_root_and_embedded_intermediate() {
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
        std::fs::write(scratch.0.join("config.cnf"), "[req]\ndistinguished_name=dn\n[dn]\n[ca]\nbasicConstraints=critical,CA:true\nkeyUsage=critical,keyCertSign,cRLSign\n[leaf]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\n").unwrap();
        run(&[
            "req",
            "-config",
            "config.cnf",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-x509",
            "-subj",
            "/CN=Root",
            "-days",
            "2",
            "-extensions",
            "ca",
            "-keyout",
            "root.key",
            "-out",
            "root.pem",
        ]);
        for (name, issuer, extension) in [
            ("intermediate", "root", "ca"),
            ("leaf", "intermediate", "leaf"),
        ] {
            run(&[
                "req",
                "-config",
                "config.cnf",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-subj",
                &format!("/CN={name}"),
                "-keyout",
                &format!("{name}.key"),
                "-out",
                &format!("{name}.csr"),
            ]);
            run(&[
                "x509",
                "-req",
                "-in",
                &format!("{name}.csr"),
                "-CA",
                &format!("{issuer}.pem"),
                "-CAkey",
                &format!("{issuer}.key"),
                "-set_serial",
                if name == "leaf" { "3" } else { "2" },
                "-days",
                "1",
                "-extfile",
                "config.cnf",
                "-extensions",
                extension,
                "-out",
                &format!("{name}.pem"),
            ]);
        }
        let leaf = scratch.0.join("leaf.pem");
        let intermediate = scratch.0.join("intermediate.pem");
        let root = scratch.0.join("root.pem");
        let now = now_seconds().unwrap();
        let report = check_certificate(&leaf, &intermediate, Some(&root), now);
        assert!(
            matches!(report.chain, Check::Passed),
            "{}",
            report.chain.label()
        );
        assert!(matches!(report.chain_with_dates, Check::Passed));
        assert!(matches!(report.validity, Check::Passed));
        assert!(report.details.contains("intermediate"));
        let missing_intermediate = check_certificate(&leaf, &leaf, Some(&root), now);
        assert!(matches!(missing_intermediate.chain, Check::Failed(_)));
        assert!(matches!(missing_intermediate.validity, Check::Passed));
        // An embedded chain, even including its root, must not establish trust.
        let other_root = scratch.0.join("other-root.pem");
        run(&[
            "req",
            "-config",
            "config.cnf",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-x509",
            "-subj",
            "/CN=OtherRoot",
            "-days",
            "2",
            "-extensions",
            "ca",
            "-keyout",
            "other.key",
            "-out",
            "other-root.pem",
        ]);
        let embedded = scratch.0.join("all.pem");
        std::fs::write(
            &embedded,
            [
                std::fs::read(&intermediate).unwrap(),
                std::fs::read(&root).unwrap(),
            ]
            .concat(),
        )
        .unwrap();
        let untrusted = check_certificate(&leaf, &embedded, Some(&other_root), now);
        assert!(matches!(untrusted.chain, Check::Failed(_)));
    }

    #[test]
    fn certificate_dates_cover_boundaries_and_bad_inputs() {
        let details = "notBefore=Jan  1 00:00:00 1970 GMT\nnotAfter=Jan  2 00:00:00 1970 GMT\n";
        assert!(matches!(certificate_validity(details, 0), Check::Passed));
        assert!(matches!(
            certificate_validity(details, 86399),
            Check::Passed
        ));
        assert!(matches!(
            certificate_validity(details, 86400),
            Check::Failed(_)
        ));
        let future = "notBefore=Jan  2 00:00:00 1970 GMT\nnotAfter=Jan  3 00:00:00 1970 GMT\n";
        assert!(
            matches!(certificate_validity(future, 0), Check::Failed(ref e) if e == "not yet valid")
        );
        assert!(matches!(
            certificate_validity("bad dates", 0),
            Check::Unknown(_)
        ));
        assert_eq!(parse_certificate_time("Jan  1 00:00:00 1970 GMT"), Some(0));
        assert_eq!(parse_certificate_time("Dec 31 23:59:59 1969 GMT"), Some(-1));
        assert_eq!(
            parse_certificate_time("Jan  1 00:00:00 2000 GMT"),
            Some(946684800)
        );
        assert!(parse_certificate_time("Feb 29 12:00:00 2000 GMT").is_some());
        for date in [
            "Feb 29 12:00:00 1900 GMT",
            "Feb 30 12:00:00 2024 GMT",
            "Jan 0 00:00:00 2024 GMT",
            "Jan 1 24:00:00 2024 GMT",
            "Jan 1 00:00:00 2024 UTC",
        ] {
            assert!(parse_certificate_time(date).is_none(), "{date}");
        }
    }

    #[test]
    fn all_signer_certificates_are_retained() {
        let pem = "-----BEGIN CERTIFICATE-----\nONE\n-----END CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\nTWO\n-----END CERTIFICATE-----";
        let certificates = pem_certificates(pem).unwrap();
        assert_eq!(certificates.len(), 2);
        assert!(certificates[1].contains("TWO"));
        assert!(pem_certificates("-----BEGIN CERTIFICATE-----").is_err());
        assert!(pem_certificates("").is_err());
    }

    #[test]
    fn scratch_is_unique_and_removed() {
        let first = Scratch::new().unwrap();
        let second = Scratch::new().unwrap();
        assert_ne!(first.0, second.0);
        let path = first.0.clone();
        std::fs::write(path.join("content"), b"private bytes").unwrap();
        drop(first);
        assert!(!path.exists());
    }
}
