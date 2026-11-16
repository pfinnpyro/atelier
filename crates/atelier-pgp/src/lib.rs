//! OpenPGP (RFC 9580), cut down to the algorithms NIST approves.
//!
//! What is written and read here:
//!
//! - version 6 certificates whose primary key signs with ECDSA and whose
//!   subkey takes ECDH, both on NIST P-256 or P-384;
//! - messages whose session key is wrapped for such a subkey (a version 6
//!   public-key encrypted session key packet: one-pass ECDH, the
//!   single-step SHA-2 key derivation of SP 800-56A and the AES key wrap)
//!   or locked under a passphrase (version 6 symmetric-key packet), and
//!   whose data is a version 2 integrity-protected packet sealed with
//!   AES-GCM under a key from HKDF-SHA256.
//!
//! Everything else in OpenPGP is refused by name, not guessed at: RSA,
//! Curve25519 and Ed25519 keys, version 4 keys, CFB encryption, the OCB
//! and EAX modes, compression and Argon2. Other implementations of
//! RFC 9580 read what this writes, and this reads what they write when
//! they keep to the same algorithms.
//!
//! No cryptography happens in this crate. It all goes through a
//! [`Provider`], so a build that must use a validated module can supply
//! one that calls it; [`Software`] is the provider made from Atelier's
//! own code.
//!
//! One part of the standard is not approved: a passphrase becomes a key
//! through OpenPGP's own iterated and salted S2K, which is not one of
//! NIST's password-based key derivation functions. A provider held to
//! approved algorithms refuses it ([`Error::NotApproved`]), which leaves
//! public keys as the way to encrypt there.

#[cfg(all(feature = "fips", not(windows)))]
compile_error!(
    "The `fips` feature has a validated module wired up for Windows only so far.      Build without it on this platform."
);

mod armor;
mod cert;
#[cfg(all(feature = "fips", windows))]
mod cng;
mod message;
mod packet;
mod pdf;
mod provider;

pub use armor::{armor, dearmor};
pub use atelier_crypto::Curve;
pub use cert::{Certificate, Fingerprint, SecretKey};
pub use message::{Readers, Summary, decrypt, decrypt_keeping_readers, encrypt, summary};
pub use pdf::{
    payload_summary, protect_pdf, protect_pdf_for, protected_payload, unprotect_pdf,
    unprotect_pdf_keeping_readers,
};
pub use provider::{KeyPair, Provider, Secret, Software};

#[cfg(all(feature = "fips", windows))]
pub use cng::Cng;

/// The provider this build is meant to use: the operating system's
/// validated module under the `fips` feature, and otherwise
/// [`Software`].
pub fn provider() -> &'static dyn Provider {
    #[cfg(all(feature = "fips", windows))]
    {
        &Cng
    }
    #[cfg(not(all(feature = "fips", windows)))]
    {
        &Software
    }
}

use atelier_crypto::Hash;

/// Why something could not be done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not the OpenPGP they should be.
    Malformed(String),
    /// Valid OpenPGP that uses something this crate leaves out.
    Unsupported(String),
    /// The provider does not do this because NIST does not approve it.
    NotApproved(String),
    /// None of the keys given, nor the passphrase, opens the message.
    NoKey,
    /// The message was changed after it was sealed, or cut short.
    Tampered,
    /// The certificate cannot be encrypted to: revoked, expired, or
    /// without a usable encryption key.
    Unusable(String),
    /// The provider failed.
    Crypto(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(m) => write!(f, "not valid OpenPGP: {m}"),
            Self::Unsupported(m) => write!(f, "not supported: {m}"),
            Self::NotApproved(m) => write!(f, "not available with validated cryptography: {m}"),
            Self::NoKey => write!(f, "none of the keys or the passphrase given opens this"),
            Self::Tampered => write!(f, "the encrypted data was changed or cut short"),
            Self::Unusable(m) => write!(f, "this certificate cannot be used: {m}"),
            Self::Crypto(m) => write!(f, "cryptography failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn malformed<T>(what: &str) -> Result<T> {
    Err(Error::Malformed(what.into()))
}

/// OpenPGP's numbers for the things used here (RFC 9580, section 9).
pub(crate) mod id {
    pub const ECDH: u8 = 18;
    pub const ECDSA: u8 = 19;
    pub const AES128: u8 = 7;
    pub const AES192: u8 = 8;
    pub const AES256: u8 = 9;
    pub const GCM: u8 = 3;
    pub const SHA256: u8 = 8;
    pub const SHA384: u8 = 9;
    pub const SHA512: u8 = 10;
}

/// The key size of a symmetric algorithm, for the three AES sizes.
pub(crate) fn aes_key_size(algorithm: u8) -> Result<usize> {
    match algorithm {
        id::AES128 => Ok(16),
        id::AES192 => Ok(24),
        id::AES256 => Ok(32),
        other => Err(Error::Unsupported(format!(
            "symmetric algorithm {other} (only AES is used)"
        ))),
    }
}

pub(crate) fn hash_from_id(algorithm: u8) -> Option<Hash> {
    match algorithm {
        id::SHA256 => Some(Hash::Sha256),
        id::SHA384 => Some(Hash::Sha384),
        id::SHA512 => Some(Hash::Sha512),
        _ => None,
    }
}

pub(crate) fn hash_id(hash: Hash) -> u8 {
    match hash {
        Hash::Sha256 => id::SHA256,
        Hash::Sha384 => id::SHA384,
        Hash::Sha512 => id::SHA512,
    }
}

/// How a curve is named in a key (its object identifier).
pub(crate) fn curve_oid(curve: Curve) -> &'static [u8] {
    match curve {
        Curve::P256 => &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07],
        Curve::P384 => &[0x2b, 0x81, 0x04, 0x00, 0x22],
    }
}

/// The hash and the key-wrapping cipher a version 6 ECDH key must use
/// on each curve (RFC 9580, section 11.5.1).
pub(crate) fn ecdh_parameters(curve: Curve) -> (Hash, u8) {
    match curve {
        Curve::P256 => (Hash::Sha256, id::AES128),
        Curve::P384 => (Hash::Sha384, id::AES192),
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}
