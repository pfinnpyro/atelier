//! AES in Galois/Counter Mode (NIST SP 800-38D) with 96-bit nonces and
//! 128-bit tags: encryption that also says whether the ciphertext, and
//! some data sent beside it, are as they were written.
//!
//! The multiplication in GF(2^128) here takes the same steps whatever it
//! is given. The AES underneath still uses lookup tables; `seal.rs` says
//! why that is accepted for files opened on the owner's own machine.

use crate::aes::Aes;
use crate::seal::{ct_eq, wipe};

/// The most one nonce may encrypt: 2^32 - 2 blocks.
const MAX_LEN: u64 = ((1 << 32) - 2) * 16;

/// An AES key ready for GCM.
pub struct AesGcm {
    aes: Aes,
    /// The hash subkey: the encryption of the zero block.
    h: u128,
}

impl Drop for AesGcm {
    fn drop(&mut self) {
        self.h = 0;
        std::hint::black_box(&self.h);
    }
}

/// Multiplies two elements of GF(2^128) in GCM's bit order, where the
/// first bit of a block is the highest bit of the number.
fn gf_mul(x: u128, h: u128) -> u128 {
    const R: u128 = 0xe1 << 120;
    let (mut z, mut v) = (0u128, h);
    for i in (0..128).rev() {
        z ^= v & ((x >> i) & 1).wrapping_neg();
        v = (v >> 1) ^ (R & (v & 1).wrapping_neg());
    }
    z
}

impl AesGcm {
    /// Takes a 16-, 24- or 32-byte key; `None` for any other length.
    pub fn new(key: &[u8]) -> Option<Self> {
        let aes = Aes::new(key)?;
        let mut block = [0u8; 16];
        aes.encrypt_block(&mut block);
        Some(Self {
            aes,
            h: u128::from_be_bytes(block),
        })
    }

    fn ghash(&self, aad: &[u8], sealed: &[u8]) -> u128 {
        let mut y = 0u128;
        for part in [aad, sealed] {
            for chunk in part.chunks(16) {
                let mut block = [0u8; 16];
                block[..chunk.len()].copy_from_slice(chunk);
                y = gf_mul(y ^ u128::from_be_bytes(block), self.h);
            }
        }
        let lengths = ((aad.len() as u128 * 8) << 64) | (sealed.len() as u128 * 8);
        gf_mul(y ^ lengths, self.h)
    }

    fn counter_block(&self, nonce: &[u8; 12], counter: u32) -> [u8; 16] {
        let mut block = [0u8; 16];
        block[..12].copy_from_slice(nonce);
        block[12..].copy_from_slice(&counter.to_be_bytes());
        self.aes.encrypt_block(&mut block);
        block
    }

    /// Counter mode from block 2 on; block 1 masks the tag.
    fn stream(&self, nonce: &[u8; 12], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 16);
        for (i, chunk) in data.chunks(16).enumerate() {
            let mut pad = self.counter_block(nonce, i as u32 + 2);
            out.extend(chunk.iter().zip(pad).map(|(d, p)| d ^ p));
            wipe(&mut pad);
        }
        out
    }

    fn tag(&self, nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> [u8; 16] {
        let mask = u128::from_be_bytes(self.counter_block(nonce, 1));
        (self.ghash(aad, sealed) ^ mask).to_be_bytes()
    }

    /// Encrypts `secret` and returns the ciphertext followed by its
    /// 16-byte tag. `aad` is not hidden but is covered by the tag. A nonce
    /// must never be used twice with one key. `None` when there is more
    /// than one nonce may cover.
    pub fn seal(&self, nonce: &[u8; 12], aad: &[u8], secret: &[u8]) -> Option<Vec<u8>> {
        if secret.len() as u64 > MAX_LEN {
            return None;
        }
        let mut out = self.stream(nonce, secret);
        let tag = self.tag(nonce, aad, &out);
        out.extend_from_slice(&tag);
        Some(out)
    }

    /// Checks the tag at the end of `sealed`, in constant time, and only
    /// then decrypts. `None` when the key, nonce or `aad` is wrong or
    /// anything was changed.
    pub fn open(&self, nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let at = sealed.len().checked_sub(16)?;
        let (sealed, tag) = sealed.split_at(at);
        if sealed.len() as u64 > MAX_LEN || !ct_eq(&self.tag(nonce, aad, sealed), tag) {
            return None;
        }
        Some(self.stream(nonce, sealed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hex, unhex};

    #[test]
    fn nist_test_cases() {
        // "The Galois/Counter Mode of Operation", test cases 1, 2 and 4.
        let gcm = AesGcm::new(&[0; 16]).unwrap();
        assert_eq!(
            hex(&gcm.seal(&[0; 12], b"", b"").unwrap()),
            "58e2fccefa7e3061367f1d57a4e7455a"
        );
        assert_eq!(
            hex(&gcm.seal(&[0; 12], b"", &[0; 16]).unwrap()),
            "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf"
        );
        let gcm = AesGcm::new(&unhex("feffe9928665731c6d6a8f9467308308")).unwrap();
        let nonce: [u8; 12] = unhex("cafebabefacedbaddecaf888").try_into().unwrap();
        let aad = unhex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
        let plain = unhex(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72\
             1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
        );
        let sealed = gcm.seal(&nonce, &aad, &plain).unwrap();
        assert_eq!(
            hex(&sealed),
            "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e\
             21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091\
             5bc94fbc3221a5db94fae95ae7121a47"
        );
        assert_eq!(gcm.open(&nonce, &aad, &sealed), Some(plain));
    }

    #[test]
    fn agrees_with_openssl() {
        let key: Vec<u8> = (0..32).collect();
        let nonce: [u8; 12] = std::array::from_fn(|i| 100 + i as u8);
        let plain: Vec<u8> = (0..77u32).map(|i| (i * 7) as u8).collect();
        let sealed = AesGcm::new(&key)
            .unwrap()
            .seal(&nonce, b"header bytes", &plain)
            .unwrap();
        assert_eq!(
            hex(&sealed),
            "481cd07365ca7caf065d19a58e3e089432b5788f07ff69d30f7e1af53f687791\
             740eae35b0119115e5ec5e12c97988864b0cc6824c393ba9727e3a4b4b2a1733\
             c84b15db0aa8d1f47390eb4e05c39e3f10587980e819d4a44452c29125"
        );
        let short = AesGcm::new(&key[..16]).unwrap();
        assert_eq!(
            hex(&short.seal(&nonce, b"", &plain).unwrap()),
            "1a6540b0c72192cba301115dd291498540ff067a1e3a47fb5cc0072a7839252a\
             5a1bbcd1771b2d809247016bb9b9064e8bca4316f1753ba94b0197c06835d7d9\
             a45ddcd87054787bdf5838df26dc23497099836ba92f3f5d6cc179774b"
        );
        assert_eq!(
            hex(&AesGcm::new(&key)
                .unwrap()
                .seal(&nonce, b"header bytes", b"")
                .unwrap()),
            "227bd7ff31a32188c1aa416e3421f4b0"
        );
    }

    #[test]
    fn nothing_changed_opens() {
        let gcm = AesGcm::new(&[3; 32]).unwrap();
        let nonce = [5u8; 12];
        let sealed = gcm
            .seal(&nonce, b"aad", b"a secret of some length")
            .unwrap();
        assert_eq!(
            gcm.open(&nonce, b"aad", &sealed).as_deref(),
            Some(&b"a secret of some length"[..])
        );
        for i in 0..sealed.len() {
            let mut bent = sealed.clone();
            bent[i] ^= 0x10;
            assert_eq!(gcm.open(&nonce, b"aad", &bent), None, "byte {i}");
        }
        assert_eq!(gcm.open(&nonce, b"aae", &sealed), None);
        assert_eq!(gcm.open(&[6; 12], b"aad", &sealed), None);
        assert_eq!(gcm.open(&nonce, b"aad", &sealed[..sealed.len() - 1]), None);
        assert_eq!(gcm.open(&nonce, b"aad", &sealed[..10]), None);
        let other = AesGcm::new(&[4; 32]).unwrap();
        assert_eq!(other.open(&nonce, b"aad", &sealed), None);
        assert!(AesGcm::new(&[0; 20]).is_none());
    }
}
