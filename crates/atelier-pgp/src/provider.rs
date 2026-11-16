//! Where the cryptography is done.

use atelier_crypto::{
    Aes, AesGcm, Curve, Hash, Sha256, aes_key_unwrap, aes_key_wrap, ec_public_is_valid,
    ec_public_key, ec_secret_is_valid, ecdh, ecdsa_sign, ecdsa_verify, hkdf_sha256, self_test,
    wipe,
};

use crate::{Error, Result};

/// Bytes that are secret, overwritten when dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Vec<u8>);

impl Secret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn zeroed(len: usize) -> Self {
        Self(vec![0; len])
    }

    /// Empty, with room for `len` bytes. Asking for enough up front
    /// keeps the secret from being left behind in a buffer it outgrew.
    pub fn with_capacity(len: usize) -> Self {
        Self(Vec::with_capacity(len))
    }

    pub fn extend(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret({} bytes)", self.0.len())
    }
}

/// A key pair on a curve: the secret number at the curve's size and
/// the public point as `04 || x || y`.
#[derive(Clone, Copy)]
pub struct KeyPair<'a> {
    pub secret: &'a [u8],
    pub public: &'a [u8],
}

/// The cryptographic operations OpenPGP is built from here. Each is one
/// approved algorithm, except [`Provider::passphrase_key`].
pub trait Provider {
    /// What does the work, for showing to the person.
    fn name(&self) -> &'static str;

    /// Fills `out` from an approved random bit generator.
    fn random(&self, out: &mut [u8]) -> Result<()>;

    fn digest(&self, hash: Hash, data: &[u8]) -> Result<Vec<u8>>;

    /// HKDF with SHA-256 (RFC 5869).
    fn hkdf_sha256(&self, ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Result<()>;

    /// AES-GCM with a 16-, 24- or 32-byte key: the ciphertext followed by
    /// its 16-byte tag.
    fn gcm_seal(&self, key: &[u8], nonce: &[u8; 12], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>>;

    /// The inverse; [`Error::Tampered`] when the tag does not match.
    fn gcm_open(&self, key: &[u8], nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> Result<Secret>;

    /// The AES key wrap (RFC 3394).
    fn key_wrap(&self, kek: &[u8], key: &[u8]) -> Result<Vec<u8>>;

    /// The inverse; [`Error::Tampered`] when the check value is wrong.
    fn key_unwrap(&self, kek: &[u8], wrapped: &[u8]) -> Result<Secret>;

    /// A new key pair: the secret number, at the curve's size, and the
    /// public point as `04 || x || y`.
    fn ec_generate(&self, curve: Curve) -> Result<(Secret, Vec<u8>)>;

    /// Whether `secret` is the secret key of the public point `public`.
    fn ec_pair_matches(&self, curve: Curve, secret: &[u8], public: &[u8]) -> bool;

    /// Whether `public` is `04 || x || y` for a point on the curve.
    fn ec_public_is_valid(&self, curve: Curve, public: &[u8]) -> bool;

    /// One-pass Diffie-Hellman and the single-step key derivation of
    /// SP 800-56A: `hash(00 00 00 01 || shared x || other_info)`, of
    /// which the caller takes as many bytes from the front as its key
    /// needs. `ours` is our own key pair, `peer` the other side's point.
    fn ecdh_kek(
        &self,
        curve: Curve,
        hash: Hash,
        ours: KeyPair<'_>,
        peer: &[u8],
        other_info: &[u8],
    ) -> Result<Secret>;

    /// Signs a hash with ECDSA; `r` and `s` at the curve's size.
    fn ecdsa_sign(
        &self,
        curve: Curve,
        signer: KeyPair<'_>,
        digest: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>)>;

    fn ecdsa_verify(&self, curve: Curve, public: &[u8], digest: &[u8], r: &[u8], s: &[u8]) -> bool;

    /// OpenPGP's iterated and salted S2K with SHA-256 (RFC 9580,
    /// 3.7.1.3): `salt || passphrase`, repeated to `count` bytes, hashed.
    /// This is not an approved way to turn a passphrase into a key, so a
    /// provider held to approved algorithms returns
    /// [`Error::NotApproved`].
    fn passphrase_key(&self, salt: &[u8; 8], count: u32, passphrase: &[u8]) -> Result<Secret>;
}

/// The provider made from Atelier's own code in `atelier-crypto`: the
/// approved algorithms, tested against known answers before first use,
/// but not a validated module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Software;

fn tested() -> Result<()> {
    self_test().map_err(|what| Error::Crypto(format!("the {what} self-test failed")))
}

fn failed<T>(what: &str) -> Result<T> {
    Err(Error::Crypto(what.into()))
}

impl Provider for Software {
    fn name(&self) -> &'static str {
        "Atelier's own code (not a validated module)"
    }

    fn random(&self, out: &mut [u8]) -> Result<()> {
        getrandom::fill(out).map_err(|e| Error::Crypto(format!("no system randomness: {e}")))
    }

    fn digest(&self, hash: Hash, data: &[u8]) -> Result<Vec<u8>> {
        tested()?;
        Ok(hash.digest(data))
    }

    fn hkdf_sha256(&self, ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Result<()> {
        tested()?;
        hkdf_sha256(ikm, salt, info, out).ok_or(Error::Crypto("HKDF output too long".into()))
    }

    fn gcm_seal(&self, key: &[u8], nonce: &[u8; 12], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
        tested()?;
        let Some(gcm) = AesGcm::new(key) else {
            return failed("bad AES key size");
        };
        gcm.seal(nonce, aad, plain)
            .ok_or(Error::Crypto("too much for one GCM nonce".into()))
    }

    fn gcm_open(&self, key: &[u8], nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> Result<Secret> {
        tested()?;
        let Some(gcm) = AesGcm::new(key) else {
            return failed("bad AES key size");
        };
        gcm.open(nonce, aad, sealed)
            .map(Secret::new)
            .ok_or(Error::Tampered)
    }

    fn key_wrap(&self, kek: &[u8], key: &[u8]) -> Result<Vec<u8>> {
        tested()?;
        let Some(aes) = Aes::new(kek) else {
            return failed("bad AES key size");
        };
        aes_key_wrap(&aes, key).ok_or(Error::Crypto("key of a size that cannot be wrapped".into()))
    }

    fn key_unwrap(&self, kek: &[u8], wrapped: &[u8]) -> Result<Secret> {
        tested()?;
        let Some(aes) = Aes::new(kek) else {
            return failed("bad AES key size");
        };
        aes_key_unwrap(&aes, wrapped)
            .map(Secret::new)
            .ok_or(Error::Tampered)
    }

    fn ec_generate(&self, curve: Curve) -> Result<(Secret, Vec<u8>)> {
        tested()?;
        // FIPS 186-5, A.2.2: draw until the number is in range, which
        // for these curves the first one all but always is.
        for _ in 0..64 {
            let mut secret = Secret::zeroed(curve.size());
            self.random(secret.bytes_mut())?;
            if !ec_secret_is_valid(curve, secret.bytes()) {
                continue;
            }
            let Some(public) = ec_public_key(curve, secret.bytes()) else {
                continue;
            };
            // The pairwise consistency test a new key pair must pass.
            let digest = curve.hash().digest(b"pairwise consistency");
            let pair = KeyPair {
                secret: secret.bytes(),
                public: &public,
            };
            let (r, s) = self.ecdsa_sign(curve, pair, &digest)?;
            if !ecdsa_verify(curve, &public, &digest, &r, &s) {
                return failed("a new key pair did not verify its own signature");
            }
            return Ok((secret, public));
        }
        failed("the random number generator gave no usable key")
    }

    fn ec_pair_matches(&self, curve: Curve, secret: &[u8], public: &[u8]) -> bool {
        tested().is_ok()
            && ec_secret_is_valid(curve, secret)
            && ec_public_key(curve, secret).is_some_and(|p| p == public)
    }

    fn ec_public_is_valid(&self, curve: Curve, public: &[u8]) -> bool {
        ec_public_is_valid(curve, public)
    }

    fn ecdh_kek(
        &self,
        curve: Curve,
        hash: Hash,
        ours: KeyPair<'_>,
        peer: &[u8],
        other_info: &[u8],
    ) -> Result<Secret> {
        tested()?;
        let Some(shared) = ecdh(curve, ours.secret, peer) else {
            return crate::malformed("a public key that is not a point on its curve");
        };
        let shared = Secret::new(shared);
        let mut input = Secret::with_capacity(4 + shared.0.len() + other_info.len());
        input.extend(&[0, 0, 0, 1]);
        input.extend(shared.bytes());
        input.extend(other_info);
        Ok(Secret::new(hash.digest(input.bytes())))
    }

    fn ecdsa_sign(
        &self,
        curve: Curve,
        signer: KeyPair<'_>,
        digest: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        tested()?;
        ecdsa_sign(curve, signer.secret, digest)
            .ok_or(Error::Malformed("secret key out of range".into()))
    }

    fn ecdsa_verify(&self, curve: Curve, public: &[u8], digest: &[u8], r: &[u8], s: &[u8]) -> bool {
        tested().is_ok() && ecdsa_verify(curve, public, digest, r, s)
    }

    fn passphrase_key(&self, salt: &[u8; 8], count: u32, passphrase: &[u8]) -> Result<Secret> {
        tested()?;
        let mut unit = Secret::new(salt.to_vec());
        unit.0.extend_from_slice(passphrase);
        // At least one whole copy is hashed, however small the count.
        let mut left = (count as usize).max(unit.0.len());
        let mut hash = Sha256::new();
        while left > 0 {
            let n = left.min(unit.0.len());
            hash.update(&unit.0[..n]);
            left -= n;
        }
        Ok(Secret::new(hash.finish().to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s2k_matches_the_rfc_sample() {
        // RFC 9580, A.11: "password" with the sample salt and count 0xff
        // (65011712 bytes) gives an AES-128 key.
        let salt = [0xe9, 0xd3, 0x97, 0x85, 0xb2, 0x07, 0x00, 0x08];
        let key = Software
            .passphrase_key(&salt, 65_011_712, b"password")
            .unwrap();
        assert_eq!(
            crate::hex(&key.bytes()[..16]),
            "250281715BBA7828EF71EF64C4784753"
        );
    }

    #[test]
    fn a_small_count_still_hashes_everything_once() {
        let salt = [1u8; 8];
        let key = Software
            .passphrase_key(&salt, 4, b"a long passphrase")
            .unwrap();
        let mut whole = salt.to_vec();
        whole.extend_from_slice(b"a long passphrase");
        assert_eq!(key.bytes(), atelier_crypto::sha256(&whole));
    }

    #[test]
    fn new_keys_agree_on_a_secret() {
        for curve in [Curve::P256, Curve::P384] {
            let (a, public_a) = Software.ec_generate(curve).unwrap();
            let (b, public_b) = Software.ec_generate(curve).unwrap();
            assert_ne!(a, b);
            let hash = curve.hash();
            fn pair<'a>(secret: &'a Secret, public: &'a [u8]) -> KeyPair<'a> {
                KeyPair {
                    secret: secret.bytes(),
                    public,
                }
            }
            let one = Software
                .ecdh_kek(curve, hash, pair(&a, &public_a), &public_b, b"info")
                .unwrap();
            let other = Software
                .ecdh_kek(curve, hash, pair(&b, &public_b), &public_a, b"info")
                .unwrap();
            assert_eq!(one, other);
            assert_eq!(one.bytes().len(), hash.size());
            let apart = Software
                .ecdh_kek(curve, hash, pair(&b, &public_b), &public_a, b"inf0")
                .unwrap();
            assert_ne!(one, apart);
        }
    }
}
