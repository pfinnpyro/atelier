//! The provider for builds that must use validated cryptography on
//! Windows: every operation runs inside CNG's Cryptographic Primitives
//! Library (`bcryptprimitives.dll`), the module Microsoft submits for
//! FIPS 140 validation, and none in Atelier's own code.
//!
//! Two things are assembled here from the module's operations, because
//! CNG does not offer them whole: the AES key wrap (RFC 3394) is the
//! standard's loop around CNG's AES block operation, since CNG wraps
//! only its own key objects and OpenPGP wraps a padded session key; and
//! the left bits of a hash are taken here before ECDSA. Passphrases are
//! refused: OpenPGP's way of stretching them is not an approved one.
//!
//! Using a validated module does not by itself make Atelier validated or
//! compliant; that is for whoever accredits the system to decide.
//!
//! These are raw system calls, so this is the one module in the crate
//! that allows unsafe code; every call checks its status and the lengths
//! passed are those of the slices given.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use atelier_crypto::{Curve, Hash, ct_eq, wipe};
use windows_sys::Win32::Foundation::STATUS_AUTH_TAG_MISMATCH;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_AES_ALGORITHM, BCRYPT_ALG_HANDLE, BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO,
    BCRYPT_CHAIN_MODE_ECB, BCRYPT_CHAIN_MODE_GCM, BCRYPT_CHAINING_MODE, BCRYPT_ECCPRIVATE_BLOB,
    BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDH_P256_ALGORITHM, BCRYPT_ECDH_P384_ALGORITHM,
    BCRYPT_ECDH_PRIVATE_P256_MAGIC, BCRYPT_ECDH_PRIVATE_P384_MAGIC, BCRYPT_ECDH_PUBLIC_P256_MAGIC,
    BCRYPT_ECDH_PUBLIC_P384_MAGIC, BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_ECDSA_P384_ALGORITHM,
    BCRYPT_ECDSA_PRIVATE_P256_MAGIC, BCRYPT_ECDSA_PRIVATE_P384_MAGIC,
    BCRYPT_ECDSA_PUBLIC_P256_MAGIC, BCRYPT_ECDSA_PUBLIC_P384_MAGIC, BCRYPT_HKDF_ALGORITHM,
    BCRYPT_HKDF_HASH_ALGORITHM, BCRYPT_HKDF_SALT_AND_FINALIZE, BCRYPT_KDF_HASH, BCRYPT_KEY_HANDLE,
    BCRYPT_SECRET_HANDLE, BCRYPT_SHA256_ALGORITHM, BCRYPT_SHA384_ALGORITHM,
    BCRYPT_SHA512_ALGORITHM, BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCRYPTBUFFER_VERSION, BCryptBuffer,
    BCryptBufferDesc, BCryptCloseAlgorithmProvider, BCryptDecrypt, BCryptDeriveKey,
    BCryptDestroyKey, BCryptDestroySecret, BCryptEncrypt, BCryptExportKey, BCryptFinalizeKeyPair,
    BCryptGenRandom, BCryptGenerateKeyPair, BCryptGenerateSymmetricKey, BCryptHash,
    BCryptImportKeyPair, BCryptKeyDerivation, BCryptOpenAlgorithmProvider, BCryptSecretAgreement,
    BCryptSetProperty, BCryptSignHash, BCryptVerifySignature, KDF_HASH_ALGORITHM, KDF_HKDF_INFO,
    KDF_SECRET_APPEND, KDF_SECRET_PREPEND,
};
use windows_sys::core::PCWSTR;

use crate::provider::{KeyPair, Provider, Secret};
use crate::{Error, Result};

/// NTSTATUS values below zero are failures.
fn ok(status: i32) -> bool {
    status >= 0
}

fn len(bytes: &[u8]) -> Option<u32> {
    u32::try_from(bytes.len()).ok()
}

/// The size in bytes of one of windows-sys's wide strings, with its
/// terminator.
fn wide_size(name: PCWSTR) -> u32 {
    let mut n = 0usize;
    // SAFETY: `name` is one of the NUL-terminated constants windows-sys
    // defines, so the walk stops inside it.
    unsafe {
        while *name.add(n) != 0 {
            n += 1;
        }
    }
    ((n + 1) * 2) as u32
}

/// An algorithm provider, closed when dropped.
struct Algorithm(BCRYPT_ALG_HANDLE);

impl Algorithm {
    fn open(id: PCWSTR) -> Option<Self> {
        let mut handle: BCRYPT_ALG_HANDLE = null_mut();
        // SAFETY: `handle` is a valid place for the handle, and `id` is one
        // of the NUL-terminated names windows-sys defines.
        let status = unsafe { BCryptOpenAlgorithmProvider(&mut handle, id, null(), 0) };
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

/// A key, destroyed when dropped. It holds its algorithm provider open,
/// which CNG needs for as long as the key lives.
struct Key {
    handle: BCRYPT_KEY_HANDLE,
    _algorithm: Algorithm,
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from CNG and is destroyed once, before
        // its algorithm provider is closed.
        unsafe {
            BCryptDestroyKey(self.handle);
        }
    }
}

/// An AES key in the chaining mode named.
fn aes(mode: PCWSTR, key: &[u8]) -> Option<Key> {
    if ![16, 24, 32].contains(&key.len()) {
        return None;
    }
    let algorithm = Algorithm::open(BCRYPT_AES_ALGORITHM)?;
    // SAFETY: the property name and value are NUL-terminated constants of
    // the size given.
    let status = unsafe {
        BCryptSetProperty(
            algorithm.0,
            BCRYPT_CHAINING_MODE,
            mode as *const u8,
            wide_size(mode),
            0,
        )
    };
    if !ok(status) {
        return None;
    }
    let mut handle: BCRYPT_KEY_HANDLE = null_mut();
    // SAFETY: `handle` is a valid place for the key handle, the key is
    // readable for its length, and CNG allocates the key object itself
    // when given none.
    let status = unsafe {
        BCryptGenerateSymmetricKey(
            algorithm.0,
            &mut handle,
            null_mut(),
            0,
            key.as_ptr(),
            len(key)?,
            0,
        )
    };
    (ok(status) && !handle.is_null()).then_some(Key {
        handle,
        _algorithm: algorithm,
    })
}

/// AES-GCM one way or the other. `tag` is written when encrypting and
/// checked when decrypting. `Err(true)` is a tag that does not match.
fn gcm(
    key: &[u8],
    nonce: &[u8; 12],
    aad: &[u8],
    input: &[u8],
    tag: &mut [u8; 16],
    encrypt: bool,
) -> std::result::Result<Vec<u8>, bool> {
    let key = aes(BCRYPT_CHAIN_MODE_GCM, key).ok_or(false)?;
    let mut nonce = *nonce;
    let mut info = BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
        cbSize: size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32,
        dwInfoVersion: 1,
        pbNonce: nonce.as_mut_ptr(),
        cbNonce: 12,
        pbAuthData: if aad.is_empty() {
            null_mut()
        } else {
            aad.as_ptr().cast_mut()
        },
        cbAuthData: len(aad).ok_or(false)?,
        pbTag: tag.as_mut_ptr(),
        cbTag: 16,
        ..Default::default()
    };
    let mut out = vec![0u8; input.len()];
    let mut written = 0u32;
    let n = len(input).ok_or(false)?;
    let info_ptr = (&raw mut info).cast::<c_void>().cast_const();
    // SAFETY: input and output are valid for `n` bytes; the mode
    // information points at the nonce, the associated data (which CNG
    // only reads) and the 16-byte tag, all of which outlive the call.
    let status = unsafe {
        if encrypt {
            BCryptEncrypt(
                key.handle,
                input.as_ptr(),
                n,
                info_ptr,
                null_mut(),
                0,
                out.as_mut_ptr(),
                n,
                &mut written,
                0,
            )
        } else {
            BCryptDecrypt(
                key.handle,
                input.as_ptr(),
                n,
                info_ptr,
                null_mut(),
                0,
                out.as_mut_ptr(),
                n,
                &mut written,
                0,
            )
        }
    };
    if !ok(status) || written as usize != out.len() {
        wipe(&mut out);
        return Err(status == STATUS_AUTH_TAG_MISMATCH);
    }
    Ok(out)
}

/// One AES block through CNG, in place.
fn aes_block(key: &Key, block: &mut [u8; 16], encrypt: bool) -> Option<()> {
    let input = *block;
    let mut written = 0u32;
    // SAFETY: input and output are 16 valid bytes each; ECB takes no IV.
    let status = unsafe {
        if encrypt {
            BCryptEncrypt(
                key.handle,
                input.as_ptr(),
                16,
                null(),
                null_mut(),
                0,
                block.as_mut_ptr(),
                16,
                &mut written,
                0,
            )
        } else {
            BCryptDecrypt(
                key.handle,
                input.as_ptr(),
                16,
                null(),
                null_mut(),
                0,
                block.as_mut_ptr(),
                16,
                &mut written,
                0,
            )
        }
    };
    (ok(status) && written == 16).then_some(())
}

fn hash_name(hash: Hash) -> PCWSTR {
    match hash {
        Hash::Sha256 => BCRYPT_SHA256_ALGORITHM,
        Hash::Sha384 => BCRYPT_SHA384_ALGORITHM,
        Hash::Sha512 => BCRYPT_SHA512_ALGORITHM,
    }
}

/// What CNG calls a curve for each use, and the numbers that head its
/// key blobs.
struct Names {
    ecdh: PCWSTR,
    ecdsa: PCWSTR,
    ecdh_private: u32,
    ecdh_public: u32,
    ecdsa_private: u32,
    ecdsa_public: u32,
}

fn names(curve: Curve) -> Names {
    match curve {
        Curve::P256 => Names {
            ecdh: BCRYPT_ECDH_P256_ALGORITHM,
            ecdsa: BCRYPT_ECDSA_P256_ALGORITHM,
            ecdh_private: BCRYPT_ECDH_PRIVATE_P256_MAGIC,
            ecdh_public: BCRYPT_ECDH_PUBLIC_P256_MAGIC,
            ecdsa_private: BCRYPT_ECDSA_PRIVATE_P256_MAGIC,
            ecdsa_public: BCRYPT_ECDSA_PUBLIC_P256_MAGIC,
        },
        Curve::P384 => Names {
            ecdh: BCRYPT_ECDH_P384_ALGORITHM,
            ecdsa: BCRYPT_ECDSA_P384_ALGORITHM,
            ecdh_private: BCRYPT_ECDH_PRIVATE_P384_MAGIC,
            ecdh_public: BCRYPT_ECDH_PUBLIC_P384_MAGIC,
            ecdsa_private: BCRYPT_ECDSA_PRIVATE_P384_MAGIC,
            ecdsa_public: BCRYPT_ECDSA_PUBLIC_P384_MAGIC,
        },
    }
}

/// Imports a key: the public point `04 || x || y`, with the secret
/// number for a private key. CNG checks that the point is on the curve.
fn import(
    algorithm: PCWSTR,
    magic: u32,
    curve: Curve,
    public: &[u8],
    secret: Option<&[u8]>,
) -> Option<Key> {
    let size = curve.size();
    if public.len() != 1 + 2 * size || public[0] != 4 {
        return None;
    }
    let mut blob = Secret::with_capacity(8 + 3 * size);
    blob.extend(&magic.to_le_bytes());
    blob.extend(&(size as u32).to_le_bytes());
    blob.extend(&public[1..]);
    if let Some(secret) = secret {
        if secret.len() != size {
            return None;
        }
        blob.extend(secret);
    }
    let kind = if secret.is_some() {
        BCRYPT_ECCPRIVATE_BLOB
    } else {
        BCRYPT_ECCPUBLIC_BLOB
    };
    let algorithm = Algorithm::open(algorithm)?;
    let mut handle: BCRYPT_KEY_HANDLE = null_mut();
    // SAFETY: `handle` is a valid place for the key handle and the blob
    // is readable for its length.
    let status = unsafe {
        BCryptImportKeyPair(
            algorithm.0,
            null_mut(),
            kind,
            &mut handle,
            blob.bytes().as_ptr(),
            len(blob.bytes())?,
            0,
        )
    };
    (ok(status) && !handle.is_null()).then_some(Key {
        handle,
        _algorithm: algorithm,
    })
}

/// A shared secret, destroyed when dropped.
struct Agreed(BCRYPT_SECRET_HANDLE);

impl Drop for Agreed {
    fn drop(&mut self) {
        // SAFETY: the handle came from BCryptSecretAgreement and is
        // destroyed once.
        unsafe {
            BCryptDestroySecret(self.0);
        }
    }
}

fn buffer(kind: u32, bytes: &[u8]) -> Option<BCryptBuffer> {
    Some(BCryptBuffer {
        cbBuffer: len(bytes)?,
        BufferType: kind,
        pvBuffer: bytes.as_ptr().cast_mut().cast(),
    })
}

/// Windows CNG, the system's FIPS-validated module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cng;

fn failed<T>(what: &str) -> Result<T> {
    Err(Error::Crypto(format!("Windows CNG: {what}")))
}

impl Provider for Cng {
    fn name(&self) -> &'static str {
        "Windows CNG, the system's FIPS-validated module"
    }

    fn random(&self, out: &mut [u8]) -> Result<()> {
        let Some(n) = len(out) else {
            return failed("too much randomness asked for");
        };
        // SAFETY: the buffer is valid for `n` bytes; a null algorithm
        // handle with this flag asks for the system-preferred generator.
        let status = unsafe {
            BCryptGenRandom(
                null_mut(),
                out.as_mut_ptr(),
                n,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if ok(status) {
            Ok(())
        } else {
            failed("no random numbers")
        }
    }

    fn digest(&self, hash: Hash, data: &[u8]) -> Result<Vec<u8>> {
        let (Some(algorithm), Some(n)) = (Algorithm::open(hash_name(hash)), len(data)) else {
            return failed("no hash");
        };
        let mut out = vec![0u8; hash.size()];
        // SAFETY: each pointer is valid for the length passed with it; no
        // key is given, so this is a plain hash.
        let status = unsafe {
            BCryptHash(
                algorithm.0,
                null(),
                0,
                data.as_ptr(),
                n,
                out.as_mut_ptr(),
                out.len() as u32,
            )
        };
        if ok(status) {
            Ok(out)
        } else {
            failed("hashing")
        }
    }

    fn hkdf_sha256(&self, ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Result<()> {
        let done = (|| {
            let algorithm = Algorithm::open(BCRYPT_HKDF_ALGORITHM)?;
            let mut handle: BCRYPT_KEY_HANDLE = null_mut();
            // SAFETY: `handle` is a valid place for the key handle and the
            // keying material is readable for its length.
            let status = unsafe {
                BCryptGenerateSymmetricKey(
                    algorithm.0,
                    &mut handle,
                    null_mut(),
                    0,
                    ikm.as_ptr(),
                    len(ikm)?,
                    0,
                )
            };
            if !ok(status) || handle.is_null() {
                return None;
            }
            let key = Key {
                handle,
                _algorithm: algorithm,
            };
            // SAFETY: the property name and the hash's name are
            // NUL-terminated constants of the size given.
            let status = unsafe {
                BCryptSetProperty(
                    key.handle,
                    BCRYPT_HKDF_HASH_ALGORITHM,
                    BCRYPT_SHA256_ALGORITHM as *const u8,
                    wide_size(BCRYPT_SHA256_ALGORITHM),
                    0,
                )
            };
            if !ok(status) {
                return None;
            }
            let salt_ptr = if salt.is_empty() {
                null()
            } else {
                salt.as_ptr()
            };
            // SAFETY: the salt is readable for its length, or absent.
            let status = unsafe {
                BCryptSetProperty(
                    key.handle,
                    BCRYPT_HKDF_SALT_AND_FINALIZE,
                    salt_ptr,
                    len(salt)?,
                    0,
                )
            };
            if !ok(status) {
                return None;
            }
            let mut buffers = [buffer(KDF_HKDF_INFO, info)?];
            let parameters = BCryptBufferDesc {
                ulVersion: BCRYPTBUFFER_VERSION,
                cBuffers: 1,
                pBuffers: buffers.as_mut_ptr(),
            };
            let mut written = 0u32;
            // SAFETY: the parameter list points at one buffer that outlives
            // the call, and the output is writable for its length.
            let status = unsafe {
                BCryptKeyDerivation(
                    key.handle,
                    &parameters,
                    out.as_mut_ptr(),
                    len(out)?,
                    &mut written,
                    0,
                )
            };
            (ok(status) && written as usize == out.len()).then_some(())
        })();
        done.ok_or(Error::Crypto("Windows CNG: HKDF".into()))
    }

    fn gcm_seal(&self, key: &[u8], nonce: &[u8; 12], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
        let mut tag = [0u8; 16];
        let Ok(mut out) = gcm(key, nonce, aad, plain, &mut tag, true) else {
            return failed("AES-GCM");
        };
        out.extend_from_slice(&tag);
        Ok(out)
    }

    fn gcm_open(&self, key: &[u8], nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> Result<Secret> {
        let Some(at) = sealed.len().checked_sub(16) else {
            return Err(Error::Tampered);
        };
        let (sealed, tag) = sealed.split_at(at);
        let mut tag: [u8; 16] = tag.try_into().expect("16 bytes");
        match gcm(key, nonce, aad, sealed, &mut tag, false) {
            Ok(plain) => Ok(Secret::new(plain)),
            Err(true) => Err(Error::Tampered),
            Err(false) => failed("AES-GCM"),
        }
    }

    fn key_wrap(&self, kek: &[u8], key: &[u8]) -> Result<Vec<u8>> {
        if key.len() < 16 || key.len() % 8 != 0 {
            return failed("a key of a size that cannot be wrapped");
        }
        let Some(aes) = aes(BCRYPT_CHAIN_MODE_ECB, kek) else {
            return failed("AES");
        };
        let n = key.len() / 8;
        let mut a = [0xa6u8; 8];
        let mut r = Secret::new(key.to_vec());
        for j in 0..6 {
            for i in 0..n {
                let mut block = [0u8; 16];
                block[..8].copy_from_slice(&a);
                block[8..].copy_from_slice(&r.bytes()[i * 8..i * 8 + 8]);
                if aes_block(&aes, &mut block, true).is_none() {
                    return failed("AES");
                }
                let t = ((n * j + i + 1) as u64).to_be_bytes();
                for (x, (b, t)) in a.iter_mut().zip(block.iter().zip(t)) {
                    *x = b ^ t;
                }
                r.bytes_mut()[i * 8..i * 8 + 8].copy_from_slice(&block[8..]);
                wipe(&mut block);
            }
        }
        let mut out = a.to_vec();
        out.extend_from_slice(r.bytes());
        Ok(out)
    }

    fn key_unwrap(&self, kek: &[u8], wrapped: &[u8]) -> Result<Secret> {
        if wrapped.len() < 24 || wrapped.len() % 8 != 0 {
            return Err(Error::Tampered);
        }
        let Some(aes) = aes(BCRYPT_CHAIN_MODE_ECB, kek) else {
            return failed("AES");
        };
        let n = wrapped.len() / 8 - 1;
        let mut a: [u8; 8] = wrapped[..8].try_into().expect("8 bytes");
        let mut r = Secret::new(wrapped[8..].to_vec());
        for j in (0..6).rev() {
            for i in (0..n).rev() {
                let t = ((n * j + i + 1) as u64).to_be_bytes();
                let mut block = [0u8; 16];
                for (x, (b, t)) in block.iter_mut().zip(a.iter().zip(t)) {
                    *x = b ^ t;
                }
                block[8..].copy_from_slice(&r.bytes()[i * 8..i * 8 + 8]);
                if aes_block(&aes, &mut block, false).is_none() {
                    return failed("AES");
                }
                a.copy_from_slice(&block[..8]);
                r.bytes_mut()[i * 8..i * 8 + 8].copy_from_slice(&block[8..]);
                wipe(&mut block);
            }
        }
        if !ct_eq(&a, &[0xa6; 8]) {
            return Err(Error::Tampered);
        }
        Ok(r)
    }

    fn ec_generate(&self, curve: Curve) -> Result<(Secret, Vec<u8>)> {
        let size = curve.size();
        let pair = (|| {
            let algorithm = Algorithm::open(names(curve).ecdh)?;
            let mut handle: BCRYPT_KEY_HANDLE = null_mut();
            // SAFETY: `handle` is a valid place for the key handle.
            let status =
                unsafe { BCryptGenerateKeyPair(algorithm.0, &mut handle, size as u32 * 8, 0) };
            if !ok(status) || handle.is_null() {
                return None;
            }
            let key = Key {
                handle,
                _algorithm: algorithm,
            };
            // SAFETY: the handle is the key pair just made. Finalizing it
            // is where CNG runs its pairwise consistency test.
            if !ok(unsafe { BCryptFinalizeKeyPair(key.handle, 0) }) {
                return None;
            }
            let mut blob = Secret::zeroed(8 + 3 * size);
            let mut written = 0u32;
            // SAFETY: the output is writable for its length.
            let status = unsafe {
                BCryptExportKey(
                    key.handle,
                    null_mut(),
                    BCRYPT_ECCPRIVATE_BLOB,
                    blob.bytes_mut().as_mut_ptr(),
                    len(blob.bytes())?,
                    &mut written,
                    0,
                )
            };
            if !ok(status) || written as usize != blob.bytes().len() {
                return None;
            }
            let mut public = vec![4u8];
            public.extend_from_slice(&blob.bytes()[8..8 + 2 * size]);
            let secret = Secret::new(blob.bytes()[8 + 2 * size..].to_vec());
            Some((secret, public))
        })();
        pair.ok_or(Error::Crypto("Windows CNG: making a key pair".into()))
    }

    fn ec_pair_matches(&self, curve: Curve, secret: &[u8], public: &[u8]) -> bool {
        // A signature made with the secret number verifies under the
        // public point only if they belong together.
        let Ok(digest) = self.digest(curve.hash(), b"pairwise consistency") else {
            return false;
        };
        let pair = KeyPair { secret, public };
        self.ecdsa_sign(curve, pair, &digest)
            .is_ok_and(|(r, s)| self.ecdsa_verify(curve, public, &digest, &r, &s))
    }

    fn ec_public_is_valid(&self, curve: Curve, public: &[u8]) -> bool {
        let names = names(curve);
        import(names.ecdh, names.ecdh_public, curve, public, None).is_some()
    }

    fn ecdh_kek(
        &self,
        curve: Curve,
        hash: Hash,
        ours: KeyPair<'_>,
        peer: &[u8],
        other_info: &[u8],
    ) -> Result<Secret> {
        let names = names(curve);
        let Some(peer) = import(names.ecdh, names.ecdh_public, curve, peer, None) else {
            return crate::malformed("a public key that is not a point on its curve");
        };
        let derived = (|| {
            let own = import(
                names.ecdh,
                names.ecdh_private,
                curve,
                ours.public,
                Some(ours.secret),
            )?;
            let mut handle: BCRYPT_SECRET_HANDLE = null_mut();
            // SAFETY: both keys are live handles on the same curve, and
            // `handle` is a valid place for the secret's handle.
            let status = unsafe { BCryptSecretAgreement(own.handle, peer.handle, &mut handle, 0) };
            if !ok(status) || handle.is_null() {
                return None;
            }
            let agreed = Agreed(handle);
            // The single-step derivation of SP 800-56A, inside CNG:
            // hash(00 00 00 01 || shared x || other_info).
            let name = hash_name(hash);
            let mut buffers = [
                BCryptBuffer {
                    cbBuffer: wide_size(name),
                    BufferType: KDF_HASH_ALGORITHM,
                    pvBuffer: name.cast_mut().cast(),
                },
                buffer(KDF_SECRET_PREPEND, &[0, 0, 0, 1])?,
                buffer(KDF_SECRET_APPEND, other_info)?,
            ];
            let parameters = BCryptBufferDesc {
                ulVersion: BCRYPTBUFFER_VERSION,
                cBuffers: buffers.len() as u32,
                pBuffers: buffers.as_mut_ptr(),
            };
            let mut out = Secret::zeroed(hash.size());
            let mut written = 0u32;
            // SAFETY: the parameter list points at buffers that outlive the
            // call and that CNG only reads; the output is writable for its
            // length.
            let status = unsafe {
                BCryptDeriveKey(
                    agreed.0,
                    BCRYPT_KDF_HASH,
                    &parameters,
                    out.bytes_mut().as_mut_ptr(),
                    hash.size() as u32,
                    &mut written,
                    0,
                )
            };
            (ok(status) && written as usize == hash.size()).then_some(out)
        })();
        derived.ok_or(Error::Crypto("Windows CNG: key agreement".into()))
    }

    fn ecdsa_sign(
        &self,
        curve: Curve,
        signer: KeyPair<'_>,
        digest: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let names = names(curve);
        let size = curve.size();
        let digest = &digest[..digest.len().min(size)];
        let signed = (|| {
            let key = import(
                names.ecdsa,
                names.ecdsa_private,
                curve,
                signer.public,
                Some(signer.secret),
            )?;
            let mut out = vec![0u8; 2 * size];
            let mut written = 0u32;
            // SAFETY: the hash is readable and the output writable for the
            // lengths given; ECDSA takes no padding information.
            let status = unsafe {
                BCryptSignHash(
                    key.handle,
                    null(),
                    digest.as_ptr(),
                    len(digest)?,
                    out.as_mut_ptr(),
                    len(&out)?,
                    &mut written,
                    0,
                )
            };
            (ok(status) && written as usize == out.len()).then(|| {
                let s = out.split_off(size);
                (out, s)
            })
        })();
        signed.ok_or(Error::Crypto("Windows CNG: signing".into()))
    }

    fn ecdsa_verify(&self, curve: Curve, public: &[u8], digest: &[u8], r: &[u8], s: &[u8]) -> bool {
        let names = names(curve);
        let size = curve.size();
        if r.len() > size || s.len() > size {
            return false;
        }
        let digest = &digest[..digest.len().min(size)];
        // r and s at full size, zeros in front.
        let mut signature = vec![0u8; 2 * size];
        signature[size - r.len()..size].copy_from_slice(r);
        signature[2 * size - s.len()..].copy_from_slice(s);
        let Some(key) = import(names.ecdsa, names.ecdsa_public, curve, public, None) else {
            return false;
        };
        let (Some(digest_len), Some(signature_len)) = (len(digest), len(&signature)) else {
            return false;
        };
        // SAFETY: the hash and signature are readable for the lengths
        // given.
        let status = unsafe {
            BCryptVerifySignature(
                key.handle,
                null(),
                digest.as_ptr(),
                digest_len,
                signature.as_ptr(),
                signature_len,
                0,
            )
        };
        status == 0
    }

    fn passphrase_key(&self, _: &[u8; 8], _: u32, _: &[u8]) -> Result<Secret> {
        Err(Error::NotApproved(
            "OpenPGP stretches passphrases in a way NIST does not approve, so this build \
             encrypts to public keys only"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SecretKey, Software, decrypt, encrypt};

    fn pair<'a>(secret: &'a Secret, public: &'a [u8]) -> KeyPair<'a> {
        KeyPair {
            secret: secret.bytes(),
            public,
        }
    }

    /// The validated module and Atelier's own code must agree to the
    /// byte, or files would not move between the two builds.
    #[test]
    fn the_validated_module_agrees_with_atelier_s_own_code() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        for hash in [Hash::Sha256, Hash::Sha384, Hash::Sha512] {
            assert_eq!(Cng.digest(hash, &data), Software.digest(hash, &data));
            assert_eq!(Cng.digest(hash, b""), Software.digest(hash, b""));
        }
        for (salt, n) in [(&b""[..], 16), (b"some salt", 36), (&[9u8; 32][..], 100)] {
            let (mut ours, mut theirs) = (vec![0u8; n], vec![0u8; n]);
            Software
                .hkdf_sha256(b"keying material", salt, b"info", &mut ours)
                .unwrap();
            Cng.hkdf_sha256(b"keying material", salt, b"info", &mut theirs)
                .unwrap();
            assert_eq!(ours, theirs, "{n} bytes");
        }
        let nonce = [5u8; 12];
        for key in [&[1u8; 16][..], &[2u8; 24], &[3u8; 32]] {
            for n in [0, 1, 16, 17, 1000] {
                for aad in [&b""[..], b"associated"] {
                    let sealed = Software.gcm_seal(key, &nonce, aad, &data[..n]).unwrap();
                    assert_eq!(Cng.gcm_seal(key, &nonce, aad, &data[..n]).unwrap(), sealed);
                    assert_eq!(
                        Cng.gcm_open(key, &nonce, aad, &sealed).unwrap().bytes(),
                        &data[..n]
                    );
                    let mut bent = sealed.clone();
                    *bent.last_mut().unwrap() ^= 1;
                    assert_eq!(Cng.gcm_open(key, &nonce, aad, &bent), Err(Error::Tampered));
                }
            }
            let wrapped = Software.key_wrap(key, &data[..40]).unwrap();
            assert_eq!(Cng.key_wrap(key, &data[..40]).unwrap(), wrapped);
            assert_eq!(Cng.key_unwrap(key, &wrapped).unwrap().bytes(), &data[..40]);
            let mut bent = wrapped.clone();
            bent[3] ^= 1;
            assert_eq!(Cng.key_unwrap(key, &bent), Err(Error::Tampered));
        }
    }

    #[test]
    fn keys_from_either_work_in_the_other() {
        for curve in [Curve::P256, Curve::P384] {
            let (a, public_a) = Cng.ec_generate(curve).unwrap();
            let (b, public_b) = Software.ec_generate(curve).unwrap();
            assert_eq!(a.bytes().len(), curve.size());
            assert!(Software.ec_pair_matches(curve, a.bytes(), &public_a));
            assert!(Cng.ec_pair_matches(curve, b.bytes(), &public_b));
            assert!(!Cng.ec_pair_matches(curve, a.bytes(), &public_b));
            assert!(Cng.ec_public_is_valid(curve, &public_b));
            let mut off = public_b.clone();
            *off.last_mut().unwrap() ^= 1;
            assert!(!Cng.ec_public_is_valid(curve, &off));
            assert!(!Cng.ec_public_is_valid(curve, &public_b[..10]));

            for hash in [Hash::Sha256, Hash::Sha384] {
                let ours = Software
                    .ecdh_kek(curve, hash, pair(&b, &public_b), &public_a, b"other info")
                    .unwrap();
                let theirs = Cng
                    .ecdh_kek(curve, hash, pair(&a, &public_a), &public_b, b"other info")
                    .unwrap();
                assert_eq!(ours, theirs);
            }
            assert!(
                Cng.ecdh_kek(curve, Hash::Sha256, pair(&a, &public_a), &off, b"")
                    .is_err()
            );

            // Each verifies the other's signatures, over hashes shorter
            // and longer than the curve.
            for hash in [Hash::Sha256, Hash::Sha384, Hash::Sha512] {
                let digest = hash.digest(b"signed");
                let (r, s) = Cng.ecdsa_sign(curve, pair(&a, &public_a), &digest).unwrap();
                assert!(Software.ecdsa_verify(curve, &public_a, &digest, &r, &s));
                assert!(Cng.ecdsa_verify(curve, &public_a, &digest, &r, &s));
                let (r, s) = Software
                    .ecdsa_sign(curve, pair(&b, &public_b), &digest)
                    .unwrap();
                assert!(Cng.ecdsa_verify(curve, &public_b, &digest, &r, &s));
                assert!(!Cng.ecdsa_verify(curve, &public_a, &digest, &r, &s));
                let other = hash.digest(b"not signed");
                assert!(!Cng.ecdsa_verify(curve, &public_b, &other, &r, &s));
            }
        }
    }

    #[test]
    fn whole_messages_cross_between_the_builds() {
        let now = 1_800_000_000;
        for curve in [Curve::P256, Curve::P384] {
            let theirs = SecretKey::generate(&Cng, curve, "Validated", now).unwrap();
            let ours = SecretKey::generate(&Software, curve, "Software", now).unwrap();
            // Each reads the other's key, checking its signatures.
            let theirs_read = SecretKey::parse(&Software, theirs.to_bytes()).unwrap();
            let ours_read = SecretKey::parse(&Cng, ours.to_bytes()).unwrap();
            let to = [theirs.certificate(), ours.certificate()];
            let sealed = encrypt(&Cng, b"from the validated build", &to, None, now).unwrap();
            assert_eq!(
                decrypt(&Software, &sealed, &[&ours], None).unwrap(),
                b"from the validated build"
            );
            assert_eq!(
                decrypt(&Software, &sealed, &[&theirs_read], None).unwrap(),
                b"from the validated build"
            );
            let sealed = encrypt(&Software, b"from the ordinary build", &to, None, now).unwrap();
            for key in [&theirs, &ours_read] {
                assert_eq!(
                    decrypt(&Cng, &sealed, &[key], None).unwrap(),
                    b"from the ordinary build"
                );
            }
        }
    }

    #[test]
    fn passphrases_are_refused() {
        assert!(matches!(
            encrypt(&Cng, b"x", &[], Some(b"passphrase"), 0),
            Err(Error::NotApproved(_))
        ));
    }
}
