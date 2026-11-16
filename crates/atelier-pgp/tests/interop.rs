//! Keys and messages made by another implementation of RFC 9580 (rpgp
//! 0.16): version 6 ECDSA keys with ECDH subkeys, and messages to them
//! sealed with AES-256 in GCM. The same run checked the other direction,
//! that rpgp verifies keys and opens messages made here.

use atelier_pgp::{Certificate, Curve, SecretKey, Software, decrypt, summary};

const TEXT: &[u8] = b"Atelier and rpgp agree about this sentence.";

fn check(curve: Curve, key: &[u8], message: &[u8]) {
    let key = SecretKey::parse(&Software, key).unwrap();
    let cert = key.certificate();
    assert_eq!(cert.curve(), curve);
    assert_eq!(cert.user_ids(), ["Bob <bob@example.org>"]);
    assert_eq!(
        Certificate::parse(&Software, cert.to_armored().as_bytes()).unwrap(),
        *cert
    );
    assert_eq!(decrypt(&Software, message, &[&key], None).unwrap(), TEXT);
    let said = summary(message).unwrap();
    assert!(!said.passphrase);
    assert_eq!(said.recipients, [cert.key_fingerprints()[1].to_vec()]);
}

#[test]
fn rpgp_p256() {
    check(
        Curve::P256,
        include_bytes!("data/rpgp-p256.key"),
        include_bytes!("data/rpgp-p256.pgp"),
    );
}

#[test]
fn rpgp_p384() {
    check(
        Curve::P384,
        include_bytes!("data/rpgp-p384.key"),
        include_bytes!("data/rpgp-p384.pgp"),
    );
}
