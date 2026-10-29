//! The hashes and ciphers PDF encryption needs (ISO 32000-2, 7.6): MD5,
//! SHA-256/384/512, RC4 and AES-128/256 in CBC mode.
//!
//! These exist to read and write PDF files, which are protected with
//! these algorithms whatever their strength. Nothing here is constant-time,
//! and none of it should be used to protect anything new.

mod aes;
mod md5;
mod rc4;
mod sha2;

pub use aes::{Aes, aes_cbc_decrypt, aes_cbc_encrypt};
pub use md5::{Md5, md5};
pub use rc4::rc4;
pub use sha2::{sha256, sha384, sha512};

#[cfg(test)]
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
