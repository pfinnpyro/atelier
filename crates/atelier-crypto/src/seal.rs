//! Sealing a small secret under a passphrase, for files Atelier keeps
//! itself: PBKDF2-HMAC-SHA256 (RFC 8018) turns the passphrase into two
//! keys, AES-256 in CBC mode encrypts, and HMAC-SHA256 (RFC 2104) over
//! the ciphertext says whether it is intact and the passphrase right.
//!
//! The tag is checked, in constant time, before anything is decrypted.
//! The AES here uses lookup tables, so its timing depends on the key;
//! that matters to something answering strangers over a network and not
//! to a file opened on the owner's own machine.

use crate::aes::{Aes, aes_cbc_decrypt, aes_cbc_encrypt};
use crate::sha2::{INIT256, compress256, output256, sha256};

/// Overwrites bytes that held a secret. The compiler is told the zeros
/// are looked at, so it does not drop the writes as dead.
pub fn wipe(bytes: &mut [u8]) {
    bytes.fill(0);
    std::hint::black_box(bytes);
}

/// Whether two byte strings are equal, taking the same time wherever
/// they differ.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// Hashes the rest of a message into a state that has already taken in
/// `before` bytes of whole blocks.
fn finish(mut h: [u32; 8], data: &[u8], before: usize) -> [u8; 32] {
    let mut blocks = data.chunks_exact(64);
    for block in &mut blocks {
        compress256(&mut h, block);
    }
    let rest = blocks.remainder();
    let mut last = [0u8; 128];
    last[..rest.len()].copy_from_slice(rest);
    last[rest.len()] = 0x80;
    let n = if rest.len() < 56 { 64 } else { 128 };
    let bits = ((before + data.len()) as u64) * 8;
    last[n - 8..n].copy_from_slice(&bits.to_be_bytes());
    for block in last[..n].chunks_exact(64) {
        compress256(&mut h, block);
    }
    wipe(&mut last);
    output256(&h)
}

/// HMAC-SHA256 with the key already mixed in, so each message costs two
/// hash blocks more than its own length.
struct Hmac {
    inner: [u32; 8],
    outer: [u32; 8],
}

impl Hmac {
    fn new(key: &[u8]) -> Self {
        let mut block = [0u8; 64];
        if key.len() > 64 {
            block[..32].copy_from_slice(&sha256(key));
        } else {
            block[..key.len()].copy_from_slice(key);
        }
        let mut pad = [0u8; 64];
        let mut state = |with: u8| {
            for (p, k) in pad.iter_mut().zip(block) {
                *p = k ^ with;
            }
            let mut h = INIT256;
            compress256(&mut h, &pad);
            h
        };
        let hmac = Self {
            inner: state(0x36),
            outer: state(0x5c),
        };
        wipe(&mut block);
        wipe(&mut pad);
        hmac
    }

    fn mac(&self, data: &[u8]) -> [u8; 32] {
        let mut inner = finish(self.inner, data, 64);
        let out = finish(self.outer, &inner, 64);
        wipe(&mut inner);
        out
    }
}

impl Drop for Hmac {
    fn drop(&mut self) {
        self.inner.fill(0);
        self.outer.fill(0);
        std::hint::black_box(&self.inner);
        std::hint::black_box(&self.outer);
    }
}

/// HMAC-SHA256 of `data` under `key`.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    Hmac::new(key).mac(data)
}

/// PBKDF2 with HMAC-SHA256: fills `out` with key bytes worked out from a
/// passphrase, taking `iterations` rounds over each 32 bytes.
pub fn pbkdf2_hmac_sha256(passphrase: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    let hmac = Hmac::new(passphrase);
    let mut first = Vec::with_capacity(salt.len() + 4);
    for (i, chunk) in out.chunks_mut(32).enumerate() {
        first.clear();
        first.extend_from_slice(salt);
        first.extend_from_slice(&(i as u32 + 1).to_be_bytes());
        let mut u = hmac.mac(&first);
        let mut t = u;
        for _ in 1..iterations.max(1) {
            u = hmac.mac(&u);
            for (a, b) in t.iter_mut().zip(u) {
                *a ^= b;
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
        wipe(&mut u);
        wipe(&mut t);
    }
}

/// The keys a passphrase unlocks: one to encrypt, one to authenticate.
/// They are overwritten when dropped.
pub struct SealKey {
    encrypt: [u8; 32],
    authenticate: [u8; 32],
}

impl SealKey {
    /// Works the keys out from a passphrase. Slow on purpose: `iterations`
    /// is what each guess at the passphrase costs.
    pub fn derive(passphrase: &[u8], salt: &[u8], iterations: u32) -> Self {
        let mut both = [0u8; 64];
        pbkdf2_hmac_sha256(passphrase, salt, iterations, &mut both);
        let mut key = Self {
            encrypt: [0; 32],
            authenticate: [0; 32],
        };
        key.encrypt.copy_from_slice(&both[..32]);
        key.authenticate.copy_from_slice(&both[32..]);
        wipe(&mut both);
        key
    }

    fn tag(&self, iv: &[u8; 16], header: &[u8], sealed: &[u8]) -> [u8; 32] {
        let mut m = Vec::with_capacity(8 + header.len() + 16 + sealed.len());
        m.extend_from_slice(&(header.len() as u64).to_be_bytes());
        m.extend_from_slice(header);
        m.extend_from_slice(iv);
        m.extend_from_slice(sealed);
        hmac_sha256(&self.authenticate, &m)
    }

    /// Encrypts `secret`. `iv` must be fresh random bytes each time.
    /// `header` is not hidden but is covered by the tag, so changing it
    /// makes the seal fail to open. Returns the ciphertext and its tag.
    pub fn seal(&self, iv: &[u8; 16], header: &[u8], secret: &[u8]) -> (Vec<u8>, [u8; 32]) {
        let fill = 16 - secret.len() % 16;
        let mut padded = Vec::with_capacity(secret.len() + fill);
        padded.extend_from_slice(secret);
        padded.resize(secret.len() + fill, fill as u8);
        let sealed = match Aes::new(&self.encrypt) {
            Some(aes) => aes_cbc_encrypt(&aes, iv, &padded),
            None => Vec::new(),
        };
        wipe(&mut padded);
        let tag = self.tag(iv, header, &sealed);
        (sealed, tag)
    }

    /// Checks the tag and decrypts. `None` when the passphrase was wrong
    /// or anything was changed.
    pub fn open(&self, iv: &[u8; 16], header: &[u8], sealed: &[u8], tag: &[u8]) -> Option<Vec<u8>> {
        if sealed.is_empty() || sealed.len() % 16 != 0 {
            return None;
        }
        if !ct_eq(&self.tag(iv, header, sealed), tag) {
            return None;
        }
        let aes = Aes::new(&self.encrypt)?;
        let mut secret = aes_cbc_decrypt(&aes, iv, sealed);
        let fill = usize::from(*secret.last()?);
        let good = (1..=16).contains(&fill)
            && fill <= secret.len()
            && secret[secret.len() - fill..]
                .iter()
                .all(|b| usize::from(*b) == fill);
        if !good {
            wipe(&mut secret);
            return None;
        }
        // Shortening leaves the padding in the buffer; clear it first.
        let keep = secret.len() - fill;
        wipe(&mut secret[keep..]);
        secret.truncate(keep);
        Some(secret)
    }
}

impl Drop for SealKey {
    fn drop(&mut self) {
        wipe(&mut self.encrypt);
        wipe(&mut self.authenticate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // A key longer than a block is hashed first.
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn hmac_at_every_length_around_a_block() {
        // Checked against Python's hmac module.
        let data: Vec<u8> = (0..200u8).collect();
        let mut all = Vec::new();
        for n in [0, 1, 55, 56, 57, 63, 64, 65, 119, 120, 128, 200] {
            all.extend_from_slice(&hmac_sha256(b"key", &data[..n]));
        }
        assert_eq!(
            hex(&sha256(&all)),
            "270ddabf0973b1a123b68f0b41d604bdd974ad010d92a3b215e6d4feced92fb4"
        );
    }

    #[test]
    fn pbkdf2_matches_known_answers() {
        // RFC 7914, section 11.
        let mut out = [0u8; 64];
        pbkdf2_hmac_sha256(b"passwd", b"salt", 1, &mut out);
        assert_eq!(
            hex(&out),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
             49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
        );
        // Checked against Python's hashlib.
        let mut out = [0u8; 40];
        pbkdf2_hmac_sha256(b"correct horse", b"battery staple", 4096, &mut out);
        assert_eq!(
            hex(&out),
            "148a2c0431e2c3fbb714416ef1c1ea334b1900af5cbd92eddfab3e9d20991f7e39a3e227084d496b"
        );
    }

    #[test]
    fn a_seal_opens_only_untouched_and_with_its_passphrase() {
        let key = SealKey::derive(b"open sesame", b"salt salt salt s", 1000);
        let iv = [7u8; 16];
        for secret in [
            &b""[..],
            b"a",
            b"exactly sixteen!",
            b"a little longer than a block",
        ] {
            let (sealed, tag) = key.seal(&iv, b"header", secret);
            assert_eq!(sealed.len() % 16, 0);
            assert!(sealed.len() > secret.len());
            assert_eq!(
                key.open(&iv, b"header", &sealed, &tag).as_deref(),
                Some(secret)
            );
            // Another passphrase, header or IV, or any changed byte.
            let other = SealKey::derive(b"open sesame!", b"salt salt salt s", 1000);
            assert_eq!(other.open(&iv, b"header", &sealed, &tag), None);
            assert_eq!(key.open(&iv, b"Header", &sealed, &tag), None);
            assert_eq!(key.open(&[8u8; 16], b"header", &sealed, &tag), None);
            let mut bent = sealed.clone();
            bent[0] ^= 1;
            assert_eq!(key.open(&iv, b"header", &bent, &tag), None);
            let mut bent = tag;
            bent[31] ^= 0x80;
            assert_eq!(key.open(&iv, b"header", &sealed, &bent), None);
            assert_eq!(
                key.open(&iv, b"header", &sealed[..sealed.len() - 16], &tag),
                None
            );
        }
    }

    #[test]
    fn the_ciphertext_does_not_show_the_secret() {
        let key = SealKey::derive(b"pass", b"salt", 10);
        let secret = b"1600 Pennsylvania Avenue NW, Washington";
        let (sealed, _) = key.seal(&[1u8; 16], b"", secret);
        assert!(!sealed.windows(6).any(|w| secret.windows(6).any(|s| s == w)));
        // A fresh IV gives a different ciphertext for the same secret.
        let (again, _) = key.seal(&[2u8; 16], b"", secret);
        assert_ne!(sealed, again);
    }
}
