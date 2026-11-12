//! The cryptography that seals form profiles, which comes from one of
//! two places depending on how Atelier was built.
//!
//! The ordinary build uses Atelier's own code in `atelier-crypto`. The
//! `fips` build, for places that must use validated cryptography, does
//! every operation in the operating system's validated module instead
//! and runs none of Atelier's own: on Windows that is CNG's Cryptographic
//! Primitives Library (`bcryptprimitives.dll`).
//!
//! Both do the same thing, so a profile sealed by one build opens in the
//! other: PBKDF2-HMAC-SHA256 turns the passphrase into an encryption key
//! and an authentication key, AES-256-CBC encrypts with PKCS#7 padding,
//! and HMAC-SHA256 over the header, the IV and the ciphertext is checked
//! before anything is decrypted.
//!
//! Using a validated module does not by itself make Atelier validated or
//! compliant; that is for whoever accredits the system to decide.

#[cfg(all(feature = "fips", not(windows)))]
compile_error!(
    "The `fips` feature has a validated module wired up for Windows only so far. \
     Build without it on this platform."
);

/// What does the cryptography, for showing to the person.
#[cfg(not(feature = "fips"))]
pub const MODULE: &str = "Atelier's own code (not a validated module)";
#[cfg(feature = "fips")]
pub const MODULE: &str = "Windows CNG, the system's FIPS-validated module";

/// The fewest characters a new passphrase may have. The validated build
/// holds to the eight that guidance for password-based keys asks for.
#[cfg(not(feature = "fips"))]
pub const MIN_PASSPHRASE: usize = 1;
#[cfg(feature = "fips")]
pub const MIN_PASSPHRASE: usize = 8;

/// What the tag is worked out over: the header's length, the header, the
/// IV and the ciphertext.
#[cfg_attr(not(feature = "fips"), allow(dead_code))]
fn tagged(iv: &[u8; 16], header: &[u8], sealed: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(8 + header.len() + 16 + sealed.len());
    m.extend_from_slice(&(header.len() as u64).to_be_bytes());
    m.extend_from_slice(header);
    m.extend_from_slice(iv);
    m.extend_from_slice(sealed);
    m
}

#[cfg(not(feature = "fips"))]
mod software {
    use atelier_crypto::SealKey;

    /// Fills `bytes` from the system's random number generator.
    pub fn random(bytes: &mut [u8]) -> bool {
        getrandom::fill(bytes).is_ok()
    }

    /// The keys a passphrase unlocks, wiped when dropped.
    pub struct Key(SealKey);

    impl Key {
        pub fn derive(passphrase: &[u8], salt: &[u8], iterations: u32) -> Option<Self> {
            Some(Self(SealKey::derive(passphrase, salt, iterations)))
        }

        pub fn seal(
            &self,
            iv: &[u8; 16],
            header: &[u8],
            secret: &[u8],
        ) -> Option<(Vec<u8>, [u8; 32])> {
            Some(self.0.seal(iv, header, secret))
        }

        pub fn open(
            &self,
            iv: &[u8; 16],
            header: &[u8],
            sealed: &[u8],
            tag: &[u8],
        ) -> Option<Vec<u8>> {
            self.0.open(iv, header, sealed, tag)
        }
    }
}

#[cfg(all(feature = "fips", windows))]
mod validated {
    use atelier_crypto::{ct_eq, wipe};

    use crate::cng;

    pub fn random(bytes: &mut [u8]) -> bool {
        cng::random(bytes)
    }

    /// The keys a passphrase unlocks, wiped when dropped. They are worked
    /// out and used inside CNG; these are the bytes it hands back.
    pub struct Key {
        encrypt: [u8; 32],
        authenticate: [u8; 32],
    }

    impl Key {
        pub fn derive(passphrase: &[u8], salt: &[u8], iterations: u32) -> Option<Self> {
            let mut both = [0u8; 64];
            let done = cng::pbkdf2_hmac_sha256(passphrase, salt, iterations, &mut both);
            let mut key = Self {
                encrypt: [0; 32],
                authenticate: [0; 32],
            };
            key.encrypt.copy_from_slice(&both[..32]);
            key.authenticate.copy_from_slice(&both[32..]);
            wipe(&mut both);
            done.then_some(key)
        }

        pub fn seal(
            &self,
            iv: &[u8; 16],
            header: &[u8],
            secret: &[u8],
        ) -> Option<(Vec<u8>, [u8; 32])> {
            let sealed = cng::aes256_cbc(&self.encrypt, iv, secret, true)?;
            let tag = cng::hmac_sha256(&self.authenticate, &super::tagged(iv, header, &sealed))?;
            Some((sealed, tag))
        }

        pub fn open(
            &self,
            iv: &[u8; 16],
            header: &[u8],
            sealed: &[u8],
            tag: &[u8],
        ) -> Option<Vec<u8>> {
            if sealed.is_empty() || !sealed.len().is_multiple_of(16) {
                return None;
            }
            let want = cng::hmac_sha256(&self.authenticate, &super::tagged(iv, header, sealed))?;
            if !ct_eq(&want, tag) {
                return None;
            }
            cng::aes256_cbc(&self.encrypt, iv, sealed, false)
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            wipe(&mut self.encrypt);
            wipe(&mut self.authenticate);
        }
    }
}

#[cfg(not(feature = "fips"))]
pub use software::{Key, random};
#[cfg(all(feature = "fips", windows))]
pub use validated::{Key, random};

/// Whether the machine's policy asks for validated cryptography only,
/// where that can be told.
pub fn system_policy() -> Option<bool> {
    #[cfg(all(feature = "fips", windows))]
    {
        crate::cng::fips_policy()
    }
    #[cfg(not(all(feature = "fips", windows)))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whichever_module_seals_only_the_right_key_opens() {
        let key = Key::derive(b"open sesame", b"salt salt salt s", 1000).unwrap();
        let mut iv = [0u8; 16];
        assert!(random(&mut iv));
        assert_ne!(iv, [0u8; 16]);
        for secret in [
            &b""[..],
            b"a",
            b"exactly sixteen!",
            b"longer than one block of AES",
        ] {
            let (sealed, tag) = key.seal(&iv, b"header", secret).unwrap();
            assert_eq!(sealed.len(), secret.len() / 16 * 16 + 16);
            assert_eq!(
                key.open(&iv, b"header", &sealed, &tag).as_deref(),
                Some(secret)
            );
            let other = Key::derive(b"open sesame!", b"salt salt salt s", 1000).unwrap();
            assert_eq!(other.open(&iv, b"header", &sealed, &tag), None);
            assert_eq!(key.open(&iv, b"Header", &sealed, &tag), None);
            let mut bent = sealed.clone();
            bent[0] ^= 1;
            assert_eq!(key.open(&iv, b"header", &bent, &tag), None);
        }
    }

    /// The validated module and Atelier's own code must agree to the
    /// byte, or profiles would not move between the two builds.
    #[cfg(all(feature = "fips", windows))]
    #[test]
    fn the_validated_module_agrees_with_atelier_s_own_code() {
        let (passphrase, salt, iv) = (b"correct horse battery", [7u8; 16], [9u8; 16]);
        let ours = atelier_crypto::SealKey::derive(passphrase, &salt, 2500);
        let theirs = Key::derive(passphrase, &salt, 2500).unwrap();
        for n in [0, 1, 15, 16, 17, 100, 1000] {
            let secret: Vec<u8> = (0..n).map(|i| (i * 7) as u8).collect();
            let (sealed, tag) = ours.seal(&iv, b"some header", &secret);
            assert_eq!(
                theirs.seal(&iv, b"some header", &secret),
                Some((sealed.clone(), tag)),
                "{n} bytes"
            );
            // And each opens what the other sealed.
            assert_eq!(
                theirs.open(&iv, b"some header", &sealed, &tag),
                Some(secret.clone())
            );
            assert_eq!(ours.open(&iv, b"some header", &sealed, &tag), Some(secret));
        }
        // The pieces, against published answers (RFC 4231 and RFC 7914).
        let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(&crate::cng::hmac_sha256(b"Jefe", b"what do ya want for nothing?").unwrap()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let mut out = [0u8; 64];
        assert!(crate::cng::pbkdf2_hmac_sha256(
            b"passwd", b"salt", 1, &mut out
        ));
        assert_eq!(
            hex(&out),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
             49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
        );
        assert!(system_policy().is_some());
    }
}
