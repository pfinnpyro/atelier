//! File-backed identities; private keys stay in their source files and OpenSSL.
use super::security_provider::{Scratch, openssl, pem_certificates};
use atelier_syntax::Document;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

pub(super) enum Identity {
    Pem {
        certificate: PathBuf,
        key: PathBuf,
    },
    Pkcs12(PathBuf),
    #[cfg(windows)]
    WindowsStore(String),
}

fn run(command: &mut Command, password: &str, operation: &str) -> Result<(), String> {
    let output = command
        .args(["-passin", "env:ATELIER_SIGNING_PASSWORD"])
        .env("ATELIER_SIGNING_PASSWORD", password)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Could not start OpenSSL for {operation}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{operation} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub(super) fn sign_document(
    doc: &Document,
    author: &str,
    identity: &Identity,
    password: &str,
) -> Result<Vec<u8>, String> {
    let prepared = doc
        .prepare_signature(author, 65_536)
        .map_err(|e| e.to_string())?;
    let scratch = Scratch::new()?;
    let content = scratch.0.join("content.bin");
    let cms = scratch.0.join("signature.der");
    std::fs::write(&content, prepared.signing_bytes()).map_err(|e| e.to_string())?;
    let mut command = openssl();
    command
        .args([
            "cms",
            "-sign",
            "-binary",
            "-outform",
            "DER",
            "-md",
            "sha256",
            "-nosmimecap",
        ])
        .arg("-in")
        .arg(&content)
        .arg("-out")
        .arg(&cms);
    match identity {
        Identity::Pem { certificate, key } => {
            command
                .arg("-signer")
                .arg(certificate)
                .arg("-inkey")
                .arg(key);
        }
        Identity::Pkcs12(path) => {
            let certificate = scratch.0.join("certificate.pem");
            let chain = scratch.0.join("chain.pem");
            run(
                openssl()
                    .args(["pkcs12", "-in"])
                    .arg(path)
                    .args(["-nokeys", "-clcerts", "-out"])
                    .arg(&certificate),
                password,
                "Reading PKCS#12 identity",
            )?;
            let public = std::fs::read_to_string(&certificate).map_err(|e| e.to_string())?;
            if pem_certificates(&public)?.len() != 1 {
                return Err(
                    "Choose a PKCS#12 file containing exactly one signing identity.".into(),
                );
            }
            run(
                openssl()
                    .args(["pkcs12", "-in"])
                    .arg(path)
                    .args(["-nokeys", "-cacerts", "-out"])
                    .arg(&chain),
                password,
                "Reading PKCS#12 certificate chain",
            )?;
            command
                .arg("-signer")
                .arg(&certificate)
                .arg("-inkey")
                .arg(path)
                .args(["-keyform", "P12"]);
            if std::fs::read(&chain)
                .map_err(|e| e.to_string())?
                .windows(b"-----BEGIN CERTIFICATE-----".len())
                .any(|w| w == b"-----BEGIN CERTIFICATE-----")
            {
                command.arg("-certfile").arg(&chain);
            }
        }
        #[cfg(windows)]
        Identity::WindowsStore(thumbprint) => {
            super::windows_identity::sign(thumbprint, &content, &cms)?;
            return prepared
                .finish(&std::fs::read(cms).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string());
        }
    }
    run(&mut command, password, "Certificate signing")?;
    prepared
        .finish(&std::fs::read(cms).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        workspace: Scratch,
    }
    impl Fixture {
        fn new() -> Self {
            let fixture = Self {
                workspace: Scratch::new().unwrap(),
            };
            std::fs::write(fixture.workspace.0.join("openssl.cnf"), "[req]\ndistinguished_name=dn\n[dn]\n[ca]\nbasicConstraints=critical,CA:true\nkeyUsage=critical,keyCertSign,cRLSign\n[leaf]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\n").unwrap();
            fixture.run(&[
                "req",
                "-config",
                "openssl.cnf",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-x509",
                "-subj",
                "/CN=Identity root",
                "-days",
                "1",
                "-extensions",
                "ca",
                "-keyout",
                "root.key",
                "-out",
                "root.pem",
            ]);
            fixture.run(&[
                "req",
                "-config",
                "openssl.cnf",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-subj",
                "/CN=Identity signer",
                "-keyout",
                "leaf.key",
                "-out",
                "leaf.csr",
            ]);
            fixture.run(&[
                "x509",
                "-req",
                "-in",
                "leaf.csr",
                "-CA",
                "root.pem",
                "-CAkey",
                "root.key",
                "-set_serial",
                "2",
                "-days",
                "1",
                "-extfile",
                "openssl.cnf",
                "-extensions",
                "leaf",
                "-out",
                "leaf.pem",
            ]);
            fixture
        }
        fn run(&self, args: &[&str]) {
            let output = openssl()
                .args(args)
                .current_dir(&self.workspace.0)
                .env("ATELIER_TEST_PASSWORD", "test identity password")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fn path(&self, name: &str) -> PathBuf {
            self.workspace.0.join(name)
        }
        fn verify(&self, bytes: Vec<u8>, require_chain: bool) {
            let doc = Document::open(bytes).unwrap();
            let signature = doc.inspect_signatures().unwrap().remove(0).unwrap();
            assert!(!signature.has_subsequent_bytes);
            std::fs::write(self.path("content"), signature.signed_bytes).unwrap();
            std::fs::write(self.path("cms.der"), signature.cms).unwrap();
            let mut command = openssl();
            command.args([
                "cms", "-verify", "-binary", "-inform", "DER", "-in", "cms.der", "-content",
                "content",
            ]);
            if require_chain {
                command.args([
                    "-CAfile",
                    "root.pem",
                    "-no-CApath",
                    "-no-CAstore",
                    "-purpose",
                    "any",
                ]);
            } else {
                command.arg("-noverify");
            }
            let output = command
                .current_dir(&self.workspace.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    fn document() -> Document {
        Document::open(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n%%EOF".to_vec()).unwrap()
    }

    #[test]
    fn pkcs12_signs_with_chain_and_rejects_bad_credentials() {
        if openssl().arg("version").output().is_err() {
            return;
        }
        let f = Fixture::new();
        f.run(&[
            "pkcs12",
            "-export",
            "-in",
            "leaf.pem",
            "-inkey",
            "leaf.key",
            "-certfile",
            "root.pem",
            "-out",
            "identity.p12",
            "-passout",
            "env:ATELIER_TEST_PASSWORD",
        ]);
        let identity = Identity::Pkcs12(f.path("identity.p12"));
        let doc = document();
        f.verify(
            sign_document(&doc, "Test", &identity, "test identity password").unwrap(),
            true,
        );
        assert!(sign_document(&doc, "Test", &identity, "wrong password").is_err());
        assert!(sign_document(&doc, "Test", &Identity::Pkcs12(f.path("missing.p12")), "").is_err());
        std::fs::write(f.path("invalid.p12"), b"invalid identity").unwrap();
        assert!(sign_document(&doc, "Test", &Identity::Pkcs12(f.path("invalid.p12")), "").is_err());
        f.run(&[
            "pkcs12",
            "-export",
            "-in",
            "leaf.pem",
            "-inkey",
            "leaf.key",
            "-out",
            "empty-password.pfx",
            "-passout",
            "pass:",
        ]);
        f.verify(
            sign_document(
                &doc,
                "Test",
                &Identity::Pkcs12(f.path("empty-password.pfx")),
                "",
            )
            .unwrap(),
            false,
        );
        f.run(&[
            "pkcs12",
            "-export",
            "-nokeys",
            "-in",
            "leaf.pem",
            "-out",
            "no-key.p12",
            "-passout",
            "pass:",
        ]);
        assert!(sign_document(&doc, "Test", &Identity::Pkcs12(f.path("no-key.p12")), "").is_err());
    }

    #[test]
    fn pem_signing_supports_encrypted_keys_and_large_documents() {
        if openssl().arg("version").output().is_err() {
            return;
        }
        let f = Fixture::new();
        f.run(&[
            "pkey",
            "-in",
            "leaf.key",
            "-aes256",
            "-passout",
            "env:ATELIER_TEST_PASSWORD",
            "-out",
            "encrypted.key",
        ]);
        let identity = Identity::Pem {
            certificate: f.path("leaf.pem"),
            key: f.path("encrypted.key"),
        };
        let original = document();
        let mut update = original.update();
        update.add(atelier_syntax::Object::String(
            atelier_syntax::PdfString::literal(vec![b'x'; 2_000_000]),
        ));
        let doc = Document::open(update.to_bytes().unwrap()).unwrap();
        f.verify(
            sign_document(&doc, "Test", &identity, "test identity password").unwrap(),
            false,
        );
        assert!(sign_document(&doc, "Test", &identity, "wrong").is_err());
        let mismatch = Identity::Pem {
            certificate: f.path("leaf.pem"),
            key: f.path("root.key"),
        };
        assert!(sign_document(&doc, "Test", &mismatch, "").is_err());
    }
}
