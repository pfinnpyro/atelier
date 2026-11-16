//! The hashes and ciphers PDF encryption needs (ISO 32000-2, 7.6): MD5,
//! SHA-256/384/512, RC4 and AES-128/256 in CBC mode.
//!
//! These exist to read and write PDF files, which are protected with
//! these algorithms whatever their strength. Nothing here is constant-time,
//! and MD5, RC4 and bare CBC should not be used to protect anything new.
//!
//! The exceptions protect new things. [`SealKey`] seals files Atelier
//! keeps itself under a passphrase; `seal.rs` says what it is built from
//! and why the timing of the AES underneath does not matter there. And
//! OpenPGP encryption is built from [`AesGcm`], [`hkdf_sha256`], the AES
//! key wrap and the NIST curves in `ec.rs`, all of them algorithms NIST
//! approves, written to their specifications and checked against
//! published answers and against OpenSSL. [`self_test`] runs a known
//! answer through each. This code is not a validated module: builds that
//! must use one do this work in the operating system's instead.

mod aes;
mod ec;
mod gcm;
mod kdf;
mod md5;
mod rc4;
mod seal;
mod selftest;
mod sha2;

pub use aes::{Aes, aes_cbc_decrypt, aes_cbc_encrypt};
pub use ec::{
    Curve, ec_public_is_valid, ec_public_key, ec_secret_is_valid, ecdh, ecdsa_sign, ecdsa_verify,
};
pub use gcm::AesGcm;
pub use kdf::{aes_key_unwrap, aes_key_wrap, hkdf_sha256};
pub use md5::{Md5, md5};
pub use rc4::rc4;
pub use seal::{SealKey, ct_eq, hmac_sha256, pbkdf2_hmac_sha256, wipe};
pub use selftest::self_test;
pub use sha2::{Hash, Sha256, sha256, sha384, sha512};

#[cfg(test)]
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
pub(crate) fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
