//! HKDF with SHA-256 (RFC 5869; NIST SP 800-56C) and the AES key wrap
//! (RFC 3394; NIST SP 800-38F).

use crate::aes::Aes;
use crate::seal::{ct_eq, hmac_sha256, wipe};

/// Fills `out` with key bytes worked out from `ikm`, which must already
/// be hard to guess: this does not slow a guesser down as PBKDF2 does.
/// `info` says what the key is for, so one secret gives unrelated keys.
/// `None` when asked for more than 255 hashes' worth.
pub fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Option<()> {
    if out.len() > 255 * 32 {
        return None;
    }
    let mut prk = hmac_sha256(if salt.is_empty() { &[0; 32] } else { salt }, ikm);
    let mut t = [0u8; 32];
    let mut input = Vec::with_capacity(32 + info.len() + 1);
    for (i, chunk) in out.chunks_mut(32).enumerate() {
        input.clear();
        if i > 0 {
            input.extend_from_slice(&t);
        }
        input.extend_from_slice(info);
        input.push(i as u8 + 1);
        t = hmac_sha256(&prk, &input);
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
    wipe(&mut prk);
    wipe(&mut t);
    wipe(&mut input);
    Some(())
}

/// The value every wrapped key starts from.
const WRAP_IV: u64 = 0xa6a6_a6a6_a6a6_a6a6;

fn halves(a: u64, r: u64) -> [u8; 16] {
    let mut block = [0u8; 16];
    block[..8].copy_from_slice(&a.to_be_bytes());
    block[8..].copy_from_slice(&r.to_be_bytes());
    block
}

fn word(bytes: &[u8]) -> u64 {
    let mut w = [0u8; 8];
    w.copy_from_slice(&bytes[..8]);
    u64::from_be_bytes(w)
}

fn finish(mut r: Vec<u64>) -> Vec<u8> {
    let out = r.iter().flat_map(|w| w.to_be_bytes()).collect();
    r.fill(0);
    std::hint::black_box(&r);
    out
}

/// Wraps `key`, whose length must be a multiple of 8 and at least 16,
/// under `kek`. The result is 8 bytes longer.
pub fn aes_key_wrap(kek: &Aes, key: &[u8]) -> Option<Vec<u8>> {
    if key.len() < 16 || key.len() % 8 != 0 {
        return None;
    }
    let mut r: Vec<u64> = key.chunks_exact(8).map(word).collect();
    let n = r.len() as u64;
    let mut a = WRAP_IV;
    for j in 0..6 {
        for (i, w) in r.iter_mut().enumerate() {
            let mut block = halves(a, *w);
            kek.encrypt_block(&mut block);
            a = word(&block) ^ (n * j + i as u64 + 1);
            *w = word(&block[8..]);
            wipe(&mut block);
        }
    }
    let mut out = a.to_be_bytes().to_vec();
    out.extend(finish(r));
    Some(out)
}

/// Unwraps what [`aes_key_wrap`] made. `None` when the key-encryption key
/// is wrong or the wrapped key was changed.
pub fn aes_key_unwrap(kek: &Aes, wrapped: &[u8]) -> Option<Vec<u8>> {
    if wrapped.len() < 24 || wrapped.len() % 8 != 0 {
        return None;
    }
    let mut r: Vec<u64> = wrapped[8..].chunks_exact(8).map(word).collect();
    let n = r.len() as u64;
    let mut a = word(wrapped);
    for j in (0..6).rev() {
        for (i, w) in r.iter_mut().enumerate().rev() {
            let mut block = halves(a ^ (n * j + i as u64 + 1), *w);
            kek.decrypt_block(&mut block);
            a = word(&block);
            *w = word(&block[8..]);
            wipe(&mut block);
        }
    }
    let mut out = finish(r);
    if !ct_eq(&a.to_be_bytes(), &WRAP_IV.to_be_bytes()) {
        wipe(&mut out);
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hex, unhex};

    #[test]
    fn hkdf_matches_rfc_5869() {
        // Test case 1.
        let mut out = [0u8; 42];
        hkdf_sha256(
            &[0x0b; 22],
            &unhex("000102030405060708090a0b0c"),
            &unhex("f0f1f2f3f4f5f6f7f8f9"),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            hex(&out),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
             34007208d5b887185865"
        );
        // Test case 3: no salt and no info.
        hkdf_sha256(&[0x0b; 22], b"", b"", &mut out).unwrap();
        assert_eq!(
            hex(&out),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d\
             9d201395faa4b61a96c8"
        );
    }

    #[test]
    fn hkdf_agrees_with_openssl() {
        let mut out = [0u8; 70];
        hkdf_sha256(b"ikm", b"", b"", &mut out).unwrap();
        assert_eq!(
            hex(&out),
            "d58e7c1c4394984eb3ca168ff7b709d44f8db0e9f6f00a0ac1c01a2e4d0ad350\
             c758da95053f1ac71c199ee45620e340f7cb84a36ef99050f0506b8f6546d573\
             e33165764040"
        );
        let salt: Vec<u8> = (0..13).collect();
        let mut out = [0u8; 42];
        hkdf_sha256(b"input keying material", &salt, b"info!", &mut out).unwrap();
        assert_eq!(
            hex(&out),
            "bdc90b7164f8662070e00bb84bb158ebcd0b219c52f3d3d364f60899c590df66\
             f33da906d2643820ca2c"
        );
        assert!(hkdf_sha256(b"ikm", b"", b"", &mut vec![0; 255 * 32 + 1]).is_none());
    }

    #[test]
    fn key_wrap_matches_rfc_3394() {
        // Section 4.1: 128 bits of key data under a 128-bit KEK.
        let kek = Aes::new(&unhex("000102030405060708090a0b0c0d0e0f")).unwrap();
        let key = unhex("00112233445566778899aabbccddeeff");
        let wrapped = aes_key_wrap(&kek, &key).unwrap();
        assert_eq!(
            hex(&wrapped),
            "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5"
        );
        assert_eq!(aes_key_unwrap(&kek, &wrapped), Some(key));
    }

    #[test]
    fn key_wrap_agrees_with_openssl() {
        let bytes: Vec<u8> = (0..40).collect();
        for (kek, data, want) in [
            (
                16,
                40,
                "afbd0c1a31dafc25eeb6402532d8b6ff5116f7f2474462d7218e942e144eeb0c\
                 b300e6663191948935a19cd3b85679c1",
            ),
            (
                24,
                40,
                "7fedba2d383fce05650368d56cdbf642360c2dd51b927376ef0c1a6badf0920e\
                 1c6f006abb67e7bf1dba3b84e8015fea",
            ),
            (32, 16, "8cc4bfeca8a9f2f38c8b28392f1e8f4b3e5a5fd2f20bd688"),
        ] {
            let aes = Aes::new(&bytes[..kek]).unwrap();
            let wrapped = aes_key_wrap(&aes, &bytes[..data]).unwrap();
            assert_eq!(hex(&wrapped), want);
            assert_eq!(
                aes_key_unwrap(&aes, &wrapped).as_deref(),
                Some(&bytes[..data])
            );
            // Any change, or another KEK, and nothing comes back.
            let mut bent = wrapped.clone();
            bent[9] ^= 1;
            assert_eq!(aes_key_unwrap(&aes, &bent), None);
            let other = Aes::new(&[9; 16]).unwrap();
            assert_eq!(aes_key_unwrap(&other, &wrapped), None);
        }
        let aes = Aes::new(&[0; 16]).unwrap();
        assert!(aes_key_wrap(&aes, &[0; 8]).is_none());
        assert!(aes_key_wrap(&aes, &[0; 20]).is_none());
        assert!(aes_key_unwrap(&aes, &[0; 16]).is_none());
    }
}
