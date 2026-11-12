//! Windows CNG (Cryptography API: Next Generation), for the `fips` build.
//!
//! The primitives here run inside the system's Cryptographic Primitives
//! Library, the module Microsoft submits for FIPS 140 validation. These
//! are raw system calls, so this is the one place in the `fips` build
//! that allows unsafe code; every call checks its status and the lengths
//! passed are those of the slices given.
#![allow(unsafe_code)]

use std::ptr::{null, null_mut};

use atelier_crypto::wipe;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_AES_ALGORITHM, BCRYPT_ALG_HANDLE, BCRYPT_ALG_HANDLE_HMAC_FLAG, BCRYPT_BLOCK_PADDING,
    BCRYPT_CHAIN_MODE_CBC, BCRYPT_CHAINING_MODE, BCRYPT_KEY_HANDLE, BCRYPT_SHA256_ALGORITHM,
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptCloseAlgorithmProvider, BCryptDecrypt,
    BCryptDeriveKeyPBKDF2, BCryptDestroyKey, BCryptEncrypt, BCryptGenRandom,
    BCryptGenerateSymmetricKey, BCryptGetFipsAlgorithmMode, BCryptHash,
    BCryptOpenAlgorithmProvider, BCryptSetProperty,
};

/// NTSTATUS values below zero are failures.
fn ok(status: i32) -> bool {
    status >= 0
}

fn len(bytes: &[u8]) -> Option<u32> {
    u32::try_from(bytes.len()).ok()
}

/// An algorithm provider, closed when dropped.
struct Algorithm(BCRYPT_ALG_HANDLE);

impl Algorithm {
    fn open(id: *const u16, flags: u32) -> Option<Self> {
        let mut handle: BCRYPT_ALG_HANDLE = null_mut();
        // SAFETY: `handle` is a valid place for the handle, and `id` is one
        // of the NUL-terminated names windows-sys defines.
        let status = unsafe { BCryptOpenAlgorithmProvider(&mut handle, id, null(), flags) };
        (ok(status) && !handle.is_null()).then_some(Self(handle))
    }
}

impl Drop for Algorithm {
    fn drop(&mut self) {
        // SAFETY: the handle came from BCryptOpenAlgorithmProvider and is
        // closed once.
        unsafe {
            BCryptCloseAlgorithmProvider(self.0, 0);
        }
    }
}

/// A symmetric key, destroyed when dropped.
struct SymmetricKey(BCRYPT_KEY_HANDLE);

impl Drop for SymmetricKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from BCryptGenerateSymmetricKey and is
        // destroyed once.
        unsafe {
            BCryptDestroyKey(self.0);
        }
    }
}

/// Fills `bytes` from the system's approved random number generator.
pub fn random(bytes: &mut [u8]) -> bool {
    let Some(n) = len(bytes) else { return false };
    // SAFETY: the buffer is valid for `n` bytes; a null algorithm handle
    // with this flag asks for the system-preferred generator.
    let status = unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            n,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    ok(status)
}

/// Whether Windows is set to require FIPS-approved algorithms (the
/// "System cryptography: Use FIPS compliant algorithms" policy).
pub fn fips_policy() -> Option<bool> {
    let mut enabled = 0u8;
    // SAFETY: `enabled` is a valid place for the one-byte answer.
    let status = unsafe { BCryptGetFipsAlgorithmMode(&mut enabled as *mut u8 as *mut _) };
    ok(status).then_some(enabled != 0)
}

/// HMAC-SHA256 of `data` under `key`.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Option<[u8; 32]> {
    let alg = Algorithm::open(BCRYPT_SHA256_ALGORITHM, BCRYPT_ALG_HANDLE_HMAC_FLAG)?;
    let mut out = [0u8; 32];
    // SAFETY: each pointer is valid for the length passed with it.
    let status = unsafe {
        BCryptHash(
            alg.0,
            key.as_ptr(),
            len(key)?,
            data.as_ptr(),
            len(data)?,
            out.as_mut_ptr(),
            32,
        )
    };
    ok(status).then_some(out)
}

/// PBKDF2 with HMAC-SHA256; fills `out`. Returns whether it worked.
pub fn pbkdf2_hmac_sha256(passphrase: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) -> bool {
    let Some(alg) = Algorithm::open(BCRYPT_SHA256_ALGORITHM, BCRYPT_ALG_HANDLE_HMAC_FLAG) else {
        return false;
    };
    let (Some(p), Some(s), Some(o)) = (len(passphrase), len(salt), len(out)) else {
        return false;
    };
    // SAFETY: each pointer is valid for the length passed with it.
    let status = unsafe {
        BCryptDeriveKeyPBKDF2(
            alg.0,
            passphrase.as_ptr(),
            p,
            salt.as_ptr(),
            s,
            u64::from(iterations.max(1)),
            out.as_mut_ptr(),
            o,
            0,
        )
    };
    ok(status)
}

/// AES-256 in CBC mode with PKCS#7 padding: encrypts, or decrypts and
/// takes the padding off. `None` when CNG refuses, as it does for
/// ciphertext whose padding is wrong.
pub fn aes256_cbc(key: &[u8; 32], iv: &[u8; 16], input: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    let alg = Algorithm::open(BCRYPT_AES_ALGORITHM, 0)?;
    // CBC is the default; say so anyway. The mode is a NUL-terminated
    // UTF-16 name, passed with its size in bytes.
    let mut mode_len = 0usize;
    // SAFETY: BCRYPT_CHAIN_MODE_CBC is a NUL-terminated constant, so the
    // walk stops inside it.
    unsafe {
        while *BCRYPT_CHAIN_MODE_CBC.add(mode_len) != 0 {
            mode_len += 1;
        }
    }
    let mode_bytes = u32::try_from((mode_len + 1) * 2).ok()?;
    // SAFETY: the property name and value are NUL-terminated constants of
    // the size given.
    let status = unsafe {
        BCryptSetProperty(
            alg.0,
            BCRYPT_CHAINING_MODE,
            BCRYPT_CHAIN_MODE_CBC as *const u8,
            mode_bytes,
            0,
        )
    };
    if !ok(status) {
        return None;
    }
    let mut handle: BCRYPT_KEY_HANDLE = null_mut();
    // SAFETY: `handle` is a valid place for the key handle, the key is 32
    // readable bytes, and CNG allocates the key object itself when given
    // none.
    let status = unsafe {
        BCryptGenerateSymmetricKey(alg.0, &mut handle, null_mut(), 0, key.as_ptr(), 32, 0)
    };
    if !ok(status) || handle.is_null() {
        return None;
    }
    let key = SymmetricKey(handle);
    // CNG updates the IV as it goes, so it gets a copy.
    let mut chain = *iv;
    let mut out = vec![0u8; input.len() + 16];
    let mut written = 0u32;
    let (n, room) = (len(input)?, len(&out)?);
    // SAFETY: input and output are valid for the lengths given, the IV
    // copy is 16 writable bytes, and `written` receives the result size.
    let status = unsafe {
        if encrypt {
            BCryptEncrypt(
                key.0,
                input.as_ptr(),
                n,
                null(),
                chain.as_mut_ptr(),
                16,
                out.as_mut_ptr(),
                room,
                &mut written,
                BCRYPT_BLOCK_PADDING,
            )
        } else {
            BCryptDecrypt(
                key.0,
                input.as_ptr(),
                n,
                null(),
                chain.as_mut_ptr(),
                16,
                out.as_mut_ptr(),
                room,
                &mut written,
                BCRYPT_BLOCK_PADDING,
            )
        }
    };
    wipe(&mut chain);
    let written = written as usize;
    if !ok(status) || written > out.len() {
        wipe(&mut out);
        return None;
    }
    // What lies past the result may be decrypted padding.
    wipe(&mut out[written..]);
    out.truncate(written);
    Some(out)
}
