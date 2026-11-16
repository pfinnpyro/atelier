//! Known-answer tests for the approved algorithms, of the kind a
//! cryptographic module runs before it does any work (FIPS 140-3,
//! "pre-operational self-tests"). Passing them shows the code computes
//! what it should on this machine; it does not make the code a validated
//! module.

use std::sync::LazyLock;

use crate::aes::Aes;
use crate::ec::{Curve, ec_public_key, ecdh, ecdsa_sign, ecdsa_verify};
use crate::gcm::AesGcm;
use crate::kdf::{aes_key_unwrap, aes_key_wrap, hkdf_sha256};
use crate::seal::hmac_sha256;
use crate::sha2::{Hash, sha256};

fn starts(bytes: &[u8], with: &[u8]) -> bool {
    bytes.len() >= with.len() && bytes[..with.len()] == *with
}

fn run() -> Result<(), &'static str> {
    // FIPS 180-4, "abc".
    if !starts(
        &sha256(b"abc"),
        &[0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea],
    ) {
        return Err("SHA-256");
    }
    if !starts(
        &Hash::Sha384.digest(b"abc"),
        &[0xcb, 0x00, 0x75, 0x3f, 0x45, 0xa3, 0x5e, 0x8b],
    ) {
        return Err("SHA-384");
    }
    // RFC 4231, test case 2.
    if !starts(
        &hmac_sha256(b"Jefe", b"what do ya want for nothing?"),
        &[0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e],
    ) {
        return Err("HMAC-SHA256");
    }
    // RFC 5869, test case 3.
    let mut okm = [0u8; 42];
    if hkdf_sha256(&[0x0b; 22], b"", b"", &mut okm).is_none()
        || !starts(&okm, &[0x8d, 0xa4, 0xe7, 0x75, 0xa5, 0x63, 0xc1, 0x8f])
    {
        return Err("HKDF");
    }
    // The GCM specification's test case 2, both ways.
    let gcm = AesGcm::new(&[0; 16]).ok_or("AES-GCM")?;
    let sealed = gcm.seal(&[0; 12], b"", &[0; 16]).ok_or("AES-GCM")?;
    if !starts(&sealed, &[0x03, 0x88, 0xda, 0xce, 0x60, 0xb6, 0xa3, 0x92])
        || !starts(
            &sealed[16..],
            &[0xab, 0x6e, 0x47, 0xd4, 0x2c, 0xec, 0x13, 0xbd],
        )
        || gcm.open(&[0; 12], b"", &sealed).as_deref() != Some(&[0u8; 16][..])
    {
        return Err("AES-GCM");
    }
    // RFC 3394, section 4.1, both ways.
    let bytes: Vec<u8> = (0..16).collect();
    let key: Vec<u8> = (0..16u8).map(|i| i * 0x11).collect();
    let kek = Aes::new(&bytes).ok_or("AES key wrap")?;
    let wrapped = aes_key_wrap(&kek, &key).ok_or("AES key wrap")?;
    if !starts(&wrapped, &[0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47])
        || aes_key_unwrap(&kek, &wrapped).as_deref() != Some(&key[..])
    {
        return Err("AES key wrap");
    }
    // On each curve: two key pairs agree on a secret, and a signature
    // with a known answer verifies.
    for (curve, r) in [
        (Curve::P256, [0xc5, 0x96, 0xfa, 0xcf]),
        (Curve::P384, [0x4a, 0x28, 0x67, 0xa1]),
    ] {
        let a: Vec<u8> = (1..=curve.size() as u8).collect();
        let b: Vec<u8> = (101..=100 + curve.size() as u8).collect();
        let (Some(pa), Some(pb)) = (ec_public_key(curve, &a), ec_public_key(curve, &b)) else {
            return Err("ECDH");
        };
        let shared = ecdh(curve, &a, &pb).ok_or("ECDH")?;
        if ecdh(curve, &b, &pa) != Some(shared) {
            return Err("ECDH");
        }
        let digest = curve.hash().digest(b"self-test");
        let (sig_r, sig_s) = ecdsa_sign(curve, &a, &digest).ok_or("ECDSA")?;
        let mut other = digest.clone();
        other[0] ^= 1;
        if !starts(&sig_r, &r)
            || !ecdsa_verify(curve, &pa, &digest, &sig_r, &sig_s)
            || ecdsa_verify(curve, &pa, &other, &sig_r, &sig_s)
        {
            return Err("ECDSA");
        }
    }
    Ok(())
}

static RESULT: LazyLock<Result<(), &'static str>> = LazyLock::new(run);

/// Runs each approved algorithm on a known answer, once; later calls
/// give the same result again. The error names the algorithm that
/// failed, after which none of them should be used.
pub fn self_test() -> Result<(), &'static str> {
    *RESULT
}

#[cfg(test)]
mod tests {
    #[test]
    fn passes() {
        assert_eq!(super::self_test(), Ok(()));
    }
}
