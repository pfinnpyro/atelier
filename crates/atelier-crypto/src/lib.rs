//! The hashes and ciphers PDF encryption needs (ISO 32000-2, 7.6): MD5,
//! SHA-256/384/512, RC4 and AES-128/256 in CBC mode.
//!
//! These exist to read and write PDF files, which are protected with
//! these algorithms whatever their strength. Nothing here is constant-time,
//! and MD5, RC4 and bare CBC should not be used to protect anything new.
//!
//! The one exception is [`SealKey`], which seals files Atelier keeps
//! itself under a passphrase; `seal.rs` says what it is built from and
//! why the timing of the AES underneath does not matter there.

mod aes;
mod md5;
mod rc4;
mod seal;
mod sha2;

pub use aes::{Aes, aes_cbc_decrypt, aes_cbc_encrypt};
pub use md5::{Md5, md5};
pub use rc4::rc4;
pub use seal::{SealKey, ct_eq, hmac_sha256, pbkdf2_hmac_sha256, wipe};
pub use sha2::{sha256, sha384, sha512};

#[cfg(test)]
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
