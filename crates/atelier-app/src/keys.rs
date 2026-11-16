//! The OpenPGP keys Atelier keeps: the person's own key, and the
//! certificates of people they encrypt documents for, in an `openpgp`
//! folder beside the settings file.
//!
//! The person's own key is sealed under a passphrase the same way form
//! profiles are (see `sealing.rs`), so in the FIPS build it is the
//! system's validated module that protects it. It is decrypted only into
//! memory, when a document needs it, and the passphrase is not kept.
//! Its certificate, the public half, is kept beside it unsealed, so
//! documents can be encrypted to oneself without unlocking anything.
//! Other people's certificates are public and kept as they came.

use std::path::{Path, PathBuf};

use atelier_pgp::{Certificate, Curve, SecretKey, Summary};

use crate::sealing;
use crate::settings::Settings;

const SEALED: &str = "atelier-openpgp-key 1";
const KDF: &str = "pbkdf2-hmac-sha256";
/// What each guess at the passphrase costs.
const ITERATIONS: u32 = 600_000;
/// A file asking for more work than this is not one Atelier wrote.
const MOST_ITERATIONS: u32 = 20_000_000;

const OWN_KEY: &str = "own.key";
const OWN_CERTIFICATE: &str = "own.cert.asc";
const CONTACTS: &str = "contacts";

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// A fingerprint in groups of four, for reading aloud and comparing.
pub fn grouped(fingerprint: &str) -> String {
    fingerprint
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What to call a certificate: its first name, or the start of its
/// fingerprint when it has none.
pub fn label(cert: &Certificate) -> String {
    match cert.user_ids().first() {
        Some(name) => name.clone(),
        None => format!("Key {}…", &cert.fingerprint_hex()[..16]),
    }
}

/// What the tag covers besides the ciphertext.
fn header(fingerprint: &str, iterations: u32, salt: &[u8; 16]) -> Vec<u8> {
    format!(
        "{SEALED}\n{fingerprint}\n{KDF}\n{iterations}\n{}",
        hex(salt)
    )
    .into_bytes()
}

/// Seals a secret key under a passphrase, as the text of its file.
fn seal(key: &SecretKey, passphrase: &str, iterations: u32) -> Result<String, String> {
    let module = || "The system's cryptography could not be used.".to_string();
    let (mut salt, mut iv) = ([0u8; 16], [0u8; 16]);
    if !sealing::random(&mut salt) || !sealing::random(&mut iv) {
        return Err("The system gave no random numbers to encrypt with.".into());
    }
    let fingerprint = key.certificate().fingerprint_hex();
    let sealer =
        sealing::Key::derive(passphrase.as_bytes(), &salt, iterations).ok_or_else(module)?;
    let (data, tag) = sealer
        .seal(
            &iv,
            &header(&fingerprint, iterations, &salt),
            key.to_bytes(),
        )
        .ok_or_else(module)?;
    Ok(format!(
        "{SEALED}\nfingerprint = {fingerprint}\nkdf = {KDF}\niterations = {iterations}\n\
         salt = {}\niv = {}\ndata = {}\ntag = {}\n",
        hex(&salt),
        hex(&iv),
        hex(&data),
        hex(&tag)
    ))
}

/// Opens what [`seal`] made. The error says whether the file or the
/// passphrase is at fault.
fn unseal(text: &str, passphrase: &str) -> Result<SecretKey, String> {
    let damaged = || "The key's file is damaged.".to_string();
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some(SEALED) {
        return Err(damaged());
    }
    let (mut fingerprint, mut iterations, mut salt, mut iv, mut data, mut tag) =
        (None, None, None, None, None, None);
    for line in lines {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "fingerprint" => fingerprint = Some(value.to_string()),
            "kdf" if value != KDF => return Err(damaged()),
            "iterations" => iterations = value.parse::<u32>().ok(),
            "salt" => salt = unhex(value),
            "iv" => iv = unhex(value),
            "data" => data = unhex(value),
            "tag" => tag = unhex(value),
            _ => {}
        }
    }
    let fingerprint = fingerprint.ok_or_else(damaged)?;
    let iterations = iterations
        .filter(|n| (1..=MOST_ITERATIONS).contains(n))
        .ok_or_else(damaged)?;
    let salt: [u8; 16] = salt.and_then(|s| s.try_into().ok()).ok_or_else(damaged)?;
    let iv: [u8; 16] = iv.and_then(|s| s.try_into().ok()).ok_or_else(damaged)?;
    let (data, tag) = (data.ok_or_else(damaged)?, tag.ok_or_else(damaged)?);
    let sealer = sealing::Key::derive(passphrase.as_bytes(), &salt, iterations)
        .ok_or("The system's cryptography could not be used.")?;
    let mut bytes = sealer
        .open(&iv, &header(&fingerprint, iterations, &salt), &data, &tag)
        .ok_or("That passphrase is not right.")?;
    let key = SecretKey::parse(atelier_pgp::provider(), &bytes);
    atelier_crypto::wipe(&mut bytes);
    let key = key.map_err(|_| damaged())?;
    if key.certificate().fingerprint_hex() != fingerprint {
        return Err(damaged());
    }
    Ok(key)
}

/// Writes a file so that only its owner can read it.
fn write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    std::io::Write::write_all(&mut file, data)?;
    file.sync_all()
}

pub struct KeyStore {
    dir: Option<PathBuf>,
    iterations: u32,
    /// The person's own certificate, if they have made or brought a key.
    pub own: Option<Certificate>,
    /// Their key, while it is unlocked.
    unlocked: Option<SecretKey>,
    /// Other people's certificates, by name.
    pub contacts: Vec<Certificate>,
}

impl KeyStore {
    /// The keys in the user's settings folder.
    pub fn load() -> Self {
        Self::at(Settings::path().map(|p| p.with_file_name("openpgp")))
    }

    pub fn at(dir: Option<PathBuf>) -> Self {
        let mut store = Self {
            dir,
            iterations: ITERATIONS,
            own: None,
            unlocked: None,
            contacts: Vec::new(),
        };
        let Some(dir) = store.dir.clone() else {
            return store;
        };
        let read = |path: PathBuf| {
            let data = std::fs::read(path).ok()?;
            Certificate::parse(atelier_pgp::provider(), &data).ok()
        };
        // Without its sealed key the certificate is of no use as "own".
        if dir.join(OWN_KEY).is_file() {
            store.own = read(dir.join(OWN_CERTIFICATE));
        }
        if let Ok(entries) = std::fs::read_dir(dir.join(CONTACTS)) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|e| e == "asc") {
                    store.contacts.extend(read(entry.path()));
                }
            }
        }
        store.sort();
        store
    }

    #[cfg(test)]
    pub fn for_test(test: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("atelier-keys-test-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Self::at(Some(dir));
        store.iterations = 1000;
        store
    }

    #[cfg(test)]
    pub fn reopened(&self) -> Self {
        let mut store = Self::at(self.dir.clone());
        store.iterations = self.iterations;
        store
    }

    fn sort(&mut self) {
        self.contacts.sort_by_key(|c| label(c).to_lowercase());
    }

    fn dir(&self) -> Result<&Path, String> {
        self.dir
            .as_deref()
            .ok_or_else(|| "There is no settings folder to keep keys in.".to_string())
    }

    fn check_passphrase(passphrase: &str) -> Result<(), String> {
        let least = sealing::MIN_PASSPHRASE.max(1);
        if passphrase.chars().count() < least {
            return Err(format!(
                "The passphrase needs at least {least} character{}.",
                if least == 1 { "" } else { "s" }
            ));
        }
        Ok(())
    }

    /// Keeps `key` as the person's own, sealed under `passphrase`, and
    /// leaves it unlocked.
    fn keep(&mut self, key: SecretKey, passphrase: &str) -> Result<(), String> {
        Self::check_passphrase(passphrase)?;
        let dir = self.dir()?.to_path_buf();
        let sealed = seal(&key, passphrase, self.iterations)?;
        let io = |e: std::io::Error| format!("Could not save the key: {e}");
        write(&dir.join(OWN_KEY), sealed.as_bytes()).map_err(io)?;
        write(
            &dir.join(OWN_CERTIFICATE),
            key.certificate().to_armored().as_bytes(),
        )
        .map_err(io)?;
        self.own = Some(key.certificate().clone());
        self.unlocked = Some(key);
        Ok(())
    }

    /// Makes the person a key. Refused when they already have one: a
    /// key that is replaced can no longer open what was encrypted to it.
    pub fn create(&mut self, name: &str, curve: Curve, passphrase: &str) -> Result<(), String> {
        if self.own.is_some() {
            return Err("There is already a key. Remove it first to make another.".into());
        }
        Self::check_passphrase(passphrase)?;
        self.dir()?;
        let key = SecretKey::generate(atelier_pgp::provider(), curve, name.trim(), now())
            .map_err(|e| e.to_string())?;
        self.keep(key, passphrase)
    }

    /// The sealed key as its file has it, for copying to another
    /// computer. It stays sealed under its passphrase.
    pub fn backup(&self) -> Result<String, String> {
        std::fs::read_to_string(self.dir()?.join(OWN_KEY))
            .map_err(|e| format!("Could not read the key: {e}"))
    }

    /// Brings in a key from a backup made by [`KeyStore::backup`].
    pub fn restore(&mut self, backup: &str, passphrase: &str) -> Result<(), String> {
        if self.own.is_some() {
            return Err("There is already a key. Remove it first to bring in another.".into());
        }
        let key = unseal(backup, passphrase)?;
        self.keep(key, passphrase)
    }

    /// Forgets the person's own key for good.
    pub fn remove_own(&mut self) -> Result<(), String> {
        let dir = self.dir()?.to_path_buf();
        for file in [OWN_KEY, OWN_CERTIFICATE] {
            match std::fs::remove_file(dir.join(file)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(format!("Could not remove the key: {e}"));
                }
                _ => {}
            }
        }
        self.own = None;
        self.unlocked = None;
        Ok(())
    }

    /// Decrypts the person's key into memory.
    pub fn unlock(&mut self, passphrase: &str) -> Result<(), String> {
        let key = unseal(&self.backup()?, passphrase)?;
        self.unlocked = Some(key);
        Ok(())
    }

    /// Forgets the decrypted key, which overwrites it.
    pub fn lock(&mut self) {
        self.unlocked = None;
    }

    /// The person's key, when it is unlocked.
    pub fn secret(&self) -> Option<&SecretKey> {
        self.unlocked.as_ref()
    }

    /// Adds someone's certificate, or replaces the one already kept
    /// for the same key. Returns what to call it.
    pub fn import(&mut self, data: &[u8]) -> Result<String, String> {
        let cert = Certificate::parse(atelier_pgp::provider(), data).map_err(|e| e.to_string())?;
        // Say now, not at the moment of encrypting, if it cannot be used.
        cert.usable_at(now()).map_err(|e| e.to_string())?;
        if self
            .own
            .as_ref()
            .is_some_and(|o| o.fingerprint() == cert.fingerprint())
        {
            return Err("That is your own certificate.".into());
        }
        let path = self
            .dir()?
            .join(CONTACTS)
            .join(format!("{}.asc", cert.fingerprint_hex()));
        write(&path, cert.to_armored().as_bytes())
            .map_err(|e| format!("Could not save the certificate: {e}"))?;
        let name = label(&cert);
        self.contacts
            .retain(|c| c.fingerprint() != cert.fingerprint());
        self.contacts.push(cert);
        self.sort();
        Ok(name)
    }

    pub fn remove_contact(&mut self, fingerprint: &str) -> Result<(), String> {
        let path = self
            .dir()?
            .join(CONTACTS)
            .join(format!("{fingerprint}.asc"));
        std::fs::remove_file(path).map_err(|e| format!("Could not remove the certificate: {e}"))?;
        self.contacts.retain(|c| c.fingerprint_hex() != fingerprint);
        Ok(())
    }

    /// Whether a message names the person's own key among its readers.
    pub fn is_reader(&self, summary: &Summary) -> bool {
        self.own.as_ref().is_some_and(|own| {
            let keys = own.key_fingerprints();
            summary
                .recipients
                .iter()
                .any(|r| keys.iter().any(|k| k[..] == r[..]))
        })
    }

    /// The readers a message names, as names where the certificates are
    /// known and as the start of a fingerprint where they are not.
    pub fn reader_names(&self, summary: &Summary) -> Vec<String> {
        summary
            .recipients
            .iter()
            .map(|r| {
                let known = self
                    .own
                    .iter()
                    .map(|c| (c, true))
                    .chain(self.contacts.iter().map(|c| (c, false)))
                    .find(|(c, _)| c.key_fingerprints().iter().any(|k| k[..] == r[..]));
                match known {
                    Some((_, true)) => "You".to_string(),
                    Some((cert, false)) => label(cert),
                    None => format!(
                        "an unknown key ({}…)",
                        hex(&r[..r.len().min(8)]).to_uppercase()
                    ),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(store: KeyStore) {
        if let Some(dir) = &store.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_key_is_kept_sealed_and_comes_back_with_its_passphrase() {
        let mut store = KeyStore::for_test("own");
        assert!(store.own.is_none());
        store
            .create("Ada <ada@example.org>", Curve::P256, "correct horse")
            .unwrap();
        let cert = store.own.clone().unwrap();
        assert!(store.secret().is_some());
        // A second key would orphan what the first can open.
        assert!(store.create("Ada", Curve::P256, "correct horse").is_err());

        // On disk the secret key is sealed; the certificate is not.
        let dir = store.dir.clone().unwrap();
        let sealed = std::fs::read_to_string(dir.join(OWN_KEY)).unwrap();
        assert!(sealed.starts_with(SEALED));
        let secret = store.secret().unwrap().to_bytes().to_vec();
        let tail = hex(&secret[secret.len() - 24..]);
        assert!(!sealed.contains(&tail));
        assert!(
            std::fs::read_to_string(dir.join(OWN_CERTIFICATE))
                .unwrap()
                .starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----")
        );

        // A fresh start knows the certificate and has the key locked.
        let mut again = store.reopened();
        assert_eq!(again.own.as_ref(), Some(&cert));
        assert!(again.secret().is_none());
        assert_eq!(
            again.unlock("wrong horse").unwrap_err(),
            "That passphrase is not right."
        );
        again.unlock("correct horse").unwrap();
        assert_eq!(again.secret().unwrap().to_bytes(), secret);
        again.lock();
        assert!(again.secret().is_none());

        // The backup restores elsewhere, under the same passphrase only.
        let backup = again.backup().unwrap();
        let mut elsewhere = KeyStore::for_test("own-elsewhere");
        assert!(elsewhere.restore(&backup, "wrong horse").is_err());
        elsewhere.restore(&backup, "correct horse").unwrap();
        assert_eq!(elsewhere.own.as_ref(), Some(&cert));
        // A changed file does not open.
        let bent = backup.replace("iterations = 1000", "iterations = 1001");
        elsewhere.remove_own().unwrap();
        assert!(elsewhere.restore(&bent, "correct horse").is_err());
        assert!(elsewhere.own.is_none());

        again.remove_own().unwrap();
        assert!(again.reopened().own.is_none());
        clean(elsewhere);
        clean(store);
    }

    #[test]
    fn contacts_are_kept_and_named() {
        let provider = atelier_pgp::provider();
        let mut store = KeyStore::for_test("contacts");
        store.create("Me", Curve::P256, "passphrase").unwrap();
        let bob =
            SecretKey::generate(provider, Curve::P384, "Bob <bob@example.org>", now()).unwrap();
        let zed = SecretKey::generate(provider, Curve::P256, "", now()).unwrap();
        assert_eq!(
            store
                .import(bob.certificate().to_armored().as_bytes())
                .unwrap(),
            "Bob <bob@example.org>"
        );
        // Twice is once; a secret key gives up only its certificate.
        store.import(bob.to_bytes()).unwrap();
        store.import(zed.certificate().to_bytes()).unwrap();
        assert_eq!(store.contacts.len(), 2);
        assert!(store.import(b"not a certificate").is_err());
        let own = store.own.clone().unwrap();
        assert!(store.import(own.to_bytes()).is_err());
        let file = store
            .dir
            .clone()
            .unwrap()
            .join(CONTACTS)
            .join(format!("{}.asc", bob.certificate().fingerprint_hex()));
        assert!(!std::fs::read_to_string(file).unwrap().contains("PRIVATE"));

        let message =
            atelier_pgp::encrypt(provider, b"x", &[&own, bob.certificate()], None, now()).unwrap();
        let summary = atelier_pgp::summary(&message).unwrap();
        assert!(store.is_reader(&summary));
        assert_eq!(
            store.reader_names(&summary),
            ["You", "Bob <bob@example.org>"]
        );

        let mut again = store.reopened();
        assert_eq!(again.contacts.len(), 2);
        again
            .remove_contact(&bob.certificate().fingerprint_hex())
            .unwrap();
        let names = again.reader_names(&summary);
        assert!(names[1].starts_with("an unknown key ("), "{names:?}");
        let other =
            atelier_pgp::encrypt(provider, b"x", &[bob.certificate()], None, now()).unwrap();
        assert!(!again.is_reader(&atelier_pgp::summary(&other).unwrap()));
        assert_eq!(grouped("ABCDEF0123"), "ABCD EF01 23");
        clean(store);
    }
}
