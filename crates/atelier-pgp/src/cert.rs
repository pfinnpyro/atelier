//! Version 6 keys and certificates (RFC 9580, sections 5.2, 5.5 and 10.1).
//!
//! A certificate is a primary key that signs, the names its owner goes
//! by, and subkeys that messages are encrypted to. Each part is tied to
//! the primary key by a signature the primary key made, and nothing is
//! taken from a certificate until that signature has been checked.

use atelier_crypto::{Curve, Hash};

use crate::packet::{Reader, packets, tag, write_mpi, write_packet};
use crate::provider::{KeyPair, Provider, Secret};
use crate::{
    Error, Result, armor, curve_oid, dearmor, ecdh_parameters, hash_from_id, hash_id, hex, id,
    malformed,
};

/// What names a version 6 key: the SHA-256 hash of its public part.
pub type Fingerprint = [u8; 32];

/// Signature type IDs.
mod kind {
    pub const CERTIFICATIONS: std::ops::RangeInclusive<u8> = 0x10..=0x13;
    pub const POSITIVE_CERTIFICATION: u8 = 0x13;
    pub const SUBKEY_BINDING: u8 = 0x18;
    pub const DIRECT_KEY: u8 = 0x1f;
    pub const KEY_REVOCATION: u8 = 0x20;
    pub const SUBKEY_REVOCATION: u8 = 0x28;
}

/// Signature subpacket type IDs.
mod sub {
    pub const CREATED: u8 = 2;
    pub const EXPIRES: u8 = 3;
    pub const KEY_EXPIRES: u8 = 9;
    pub const SYMMETRIC: u8 = 11;
    pub const HASHES: u8 = 21;
    pub const FLAGS: u8 = 27;
    pub const FEATURES: u8 = 30;
    pub const ISSUER_FINGERPRINT: u8 = 33;
    pub const AEAD_SUITES: u8 = 39;
    /// Those that are understood or safely passed over when marked
    /// critical. An unknown critical subpacket voids its signature.
    pub const KNOWN: [u8; 19] = [
        2, 3, 4, 7, 9, 11, 16, 21, 22, 23, 25, 27, 29, 30, 31, 33, 34, 35, 39,
    ];
}

/// Key flags.
const CERTIFY_AND_SIGN: u8 = 0x03;
const ENCRYPT: u8 = 0x0c;

/// The salt a version 6 signature carries, by its hash.
fn salt_size(hash: Hash) -> usize {
    match hash {
        Hash::Sha256 => 16,
        Hash::Sha384 => 24,
        Hash::Sha512 => 32,
    }
}

/// The public part of a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublicKey {
    pub created: u32,
    /// ECDSA for a primary key, ECDH for an encryption subkey.
    pub algorithm: u8,
    pub curve: Curve,
    /// `04 || x || y`.
    pub point: Vec<u8>,
    /// The body of the key's packet.
    body: Vec<u8>,
    pub fingerprint: Fingerprint,
}

impl PublicKey {
    fn new(
        provider: &dyn Provider,
        created: u32,
        algorithm: u8,
        curve: Curve,
        point: Vec<u8>,
    ) -> Result<Self> {
        let oid = curve_oid(curve);
        let mut material = vec![oid.len() as u8];
        material.extend_from_slice(oid);
        write_mpi(&mut material, &point);
        if algorithm == id::ECDH {
            let (hash, cipher) = ecdh_parameters(curve);
            material.extend_from_slice(&[3, 1, hash_id(hash), cipher]);
        }
        let mut body = vec![6];
        body.extend_from_slice(&created.to_be_bytes());
        body.push(algorithm);
        body.extend_from_slice(&(material.len() as u32).to_be_bytes());
        body.extend_from_slice(&material);
        Self::finish(provider, created, algorithm, curve, point, body)
    }

    fn finish(
        provider: &dyn Provider,
        created: u32,
        algorithm: u8,
        curve: Curve,
        point: Vec<u8>,
        body: Vec<u8>,
    ) -> Result<Self> {
        let mut key = Self {
            created,
            algorithm,
            curve,
            point,
            body,
            fingerprint: [0; 32],
        };
        let hashed = provider.digest(Hash::Sha256, &key.hashed_form())?;
        key.fingerprint.copy_from_slice(&hashed);
        Ok(key)
    }

    /// Reads the public part off the front of a key packet's body and
    /// returns it with whatever follows (the secret part, if any).
    fn parse<'a>(provider: &dyn Provider, body: &'a [u8]) -> Result<(Self, &'a [u8])> {
        let mut r = Reader::new(body);
        match r.u8()? {
            6 => {}
            4 => {
                return Err(Error::Unsupported(
                    "version 4 keys (make a version 6 key on NIST P-256 or P-384)".into(),
                ));
            }
            v => return Err(Error::Unsupported(format!("version {v} keys"))),
        }
        let created = r.u32()?;
        let algorithm = r.u8()?;
        let len = r.u32()? as usize;
        let mut m = Reader::new(r.take(len)?);
        let public_len = body.len() - r.rest().len();
        if algorithm != id::ECDH && algorithm != id::ECDSA {
            let name = match algorithm {
                1..=3 => "RSA",
                22 | 27 => "Ed25519",
                25 => "X25519",
                26 => "X448",
                28 => "Ed448",
                _ => "this kind of",
            };
            return Err(Error::Unsupported(format!(
                "{name} keys (only keys on NIST P-256 and P-384 are used)"
            )));
        }
        let oid_len = usize::from(m.u8()?);
        let oid = m.take(oid_len)?;
        let Some(curve) = [Curve::P256, Curve::P384]
            .into_iter()
            .find(|c| curve_oid(*c) == oid)
        else {
            return Err(Error::Unsupported(
                "keys on this curve (only NIST P-256 and P-384 are used)".into(),
            ));
        };
        let point = m.mpi()?.to_vec();
        if !provider.ec_public_is_valid(curve, &point) {
            return malformed("a public key that is not a point on its curve");
        }
        if algorithm == id::ECDH {
            let (hash, cipher) = ecdh_parameters(curve);
            if m.rest() != [3, 1, hash_id(hash), cipher] {
                return Err(Error::Unsupported(
                    "an ECDH key with parameters other than those its curve must use".into(),
                ));
            }
        }
        let key = Self::finish(
            provider,
            created,
            algorithm,
            curve,
            point,
            body[..public_len].to_vec(),
        )?;
        Ok((key, &body[public_len..]))
    }

    /// The key as signatures and fingerprints hash it.
    fn hashed_form(&self) -> Vec<u8> {
        let mut out = vec![0x9b];
        out.extend_from_slice(&(self.body.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

/// Appends one signature subpacket.
fn subpacket(out: &mut Vec<u8>, kind: u8, critical: bool, data: &[u8]) {
    out.push(data.len() as u8 + 1);
    out.push(kind | if critical { 0x80 } else { 0 });
    out.extend_from_slice(data);
}

/// What a version 6 signature's hash ends with: its own head again in
/// summary (RFC 9580, 5.2.4).
fn trailer(head: &[u8]) -> Vec<u8> {
    let mut out = vec![6, 0xff];
    out.extend_from_slice(&(head.len() as u32).to_be_bytes());
    out
}

/// Makes a signature by `signer` over `data` and returns the body of its
/// packet. `subpackets` are the hashed ones, to which the time and the
/// signer's fingerprint are added.
fn sign(
    provider: &dyn Provider,
    signer: &PublicKey,
    secret: &[u8],
    kind: u8,
    created: u32,
    subpackets: &[u8],
    data: &[u8],
) -> Result<Vec<u8>> {
    let hash = signer.curve.hash();
    let mut hashed = Vec::new();
    subpacket(&mut hashed, sub::CREATED, true, &created.to_be_bytes());
    let mut issuer = vec![6];
    issuer.extend_from_slice(&signer.fingerprint);
    subpacket(&mut hashed, sub::ISSUER_FINGERPRINT, false, &issuer);
    hashed.extend_from_slice(subpackets);
    let mut head = vec![6, kind, id::ECDSA, hash_id(hash)];
    head.extend_from_slice(&(hashed.len() as u32).to_be_bytes());
    head.extend_from_slice(&hashed);
    let mut salt = vec![0u8; salt_size(hash)];
    provider.random(&mut salt)?;
    let mut input = salt.clone();
    input.extend_from_slice(data);
    input.extend_from_slice(&head);
    input.extend_from_slice(&trailer(&head));
    let digest = provider.digest(hash, &input)?;
    let pair = KeyPair {
        secret,
        public: &signer.point,
    };
    let (r, s) = provider.ecdsa_sign(signer.curve, pair, &digest)?;
    let mut body = head;
    body.extend_from_slice(&[0; 4]);
    body.extend_from_slice(&digest[..2]);
    body.push(salt.len() as u8);
    body.extend_from_slice(&salt);
    write_mpi(&mut body, &r);
    write_mpi(&mut body, &s);
    Ok(body)
}

/// A signature as read, with the subpackets that matter here.
struct Signature {
    kind: u8,
    hash: Hash,
    /// From the version through the hashed subpackets.
    head: Vec<u8>,
    left: [u8; 2],
    salt: Vec<u8>,
    r: Vec<u8>,
    s: Vec<u8>,
    created: u32,
    /// Seconds after `created` that the signature lapses.
    expires: Option<u32>,
    /// Seconds after the key's creation that the key lapses.
    key_expires: Option<u32>,
    flags: u8,
}

impl Signature {
    /// `None` for a signature that is to be passed over: another
    /// version or algorithm, or one that is malformed or has a critical
    /// subpacket that is not understood (RFC 9580, 5.2.5).
    fn parse(body: &[u8]) -> Option<Self> {
        let mut r = Reader::new(body);
        if r.u8().ok()? != 6 {
            return None;
        }
        let kind = r.u8().ok()?;
        if r.u8().ok()? != id::ECDSA {
            return None;
        }
        let hash = hash_from_id(r.u8().ok()?)?;
        let hashed_len = r.u32().ok()? as usize;
        let hashed = r.take(hashed_len).ok()?;
        let head = body[..8 + hashed_len].to_vec();
        let unhashed_len = r.u32().ok()? as usize;
        r.take(unhashed_len).ok()?;
        let left = r.take(2).ok()?;
        let salt_len = usize::from(r.u8().ok()?);
        if salt_len != salt_size(hash) {
            return None;
        }
        let salt = r.take(salt_len).ok()?.to_vec();
        let (sig_r, sig_s) = (r.mpi().ok()?.to_vec(), r.mpi().ok()?.to_vec());
        let mut sig = Self {
            kind,
            hash,
            head,
            left: [left[0], left[1]],
            salt,
            r: sig_r,
            s: sig_s,
            created: 0,
            expires: None,
            key_expires: None,
            flags: 0,
        };
        let mut have_created = false;
        let mut subs = Reader::new(hashed);
        while !subs.is_empty() {
            let len = match subs.u8().ok()? {
                b @ 0..192 => usize::from(b),
                b @ 192..255 => ((usize::from(b) - 192) << 8) + usize::from(subs.u8().ok()?) + 192,
                255 => subs.u32().ok()? as usize,
            };
            let mut data = Reader::new(subs.take(len).ok()?);
            let kind = data.u8().ok()?;
            let seconds = |d: &mut Reader| d.u32().ok().filter(|n| *n != 0);
            match kind & 0x7f {
                sub::CREATED => {
                    sig.created = data.u32().ok()?;
                    have_created = true;
                }
                sub::EXPIRES => sig.expires = seconds(&mut data),
                sub::KEY_EXPIRES => sig.key_expires = seconds(&mut data),
                sub::FLAGS => sig.flags = data.u8().unwrap_or(0),
                other if kind & 0x80 != 0 && !sub::KNOWN.contains(&other) => return None,
                _ => {}
            }
        }
        have_created.then_some(sig)
    }

    /// Whether `signer` made this signature over `data`.
    fn verify(&self, provider: &dyn Provider, signer: &PublicKey, data: &[u8]) -> bool {
        let mut input = self.salt.clone();
        input.extend_from_slice(data);
        input.extend_from_slice(&self.head);
        input.extend_from_slice(&trailer(&self.head));
        let Ok(digest) = provider.digest(self.hash, &input) else {
            return false;
        };
        digest[..2] == self.left
            && provider.ecdsa_verify(signer.curve, &signer.point, &digest, &self.r, &self.s)
    }
}

/// What one checked self-signature says about the key it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    created: u32,
    /// When the signature itself lapses.
    lapses: Option<u64>,
    /// When the key lapses.
    key_lapses: Option<u64>,
    flags: u8,
}

impl Binding {
    fn new(sig: &Signature, key: &PublicKey) -> Self {
        let after =
            |start: u32, seconds: Option<u32>| seconds.map(|s| u64::from(start) + u64::from(s));
        Self {
            created: sig.created,
            lapses: after(sig.created, sig.expires),
            key_lapses: after(key.created, sig.key_expires),
            flags: sig.flags,
        }
    }

    /// The newest of `bindings` that is in force at `now`.
    fn current(bindings: &[Binding], now: u64) -> Option<&Binding> {
        bindings
            .iter()
            .filter(|b| b.lapses.is_none_or(|t| now < t))
            .max_by_key(|b| b.created)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Subkey {
    key: PublicKey,
    bindings: Vec<Binding>,
    revoked: bool,
}

/// A certificate: someone's public keys, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    primary: PublicKey,
    /// What the primary key says about itself.
    direct: Vec<Binding>,
    revoked: bool,
    user_ids: Vec<String>,
    subkeys: Vec<Subkey>,
    /// The certificate as packets.
    bytes: Vec<u8>,
}

/// A certificate with the secret keys that go with it.
#[derive(Clone)]
pub struct SecretKey {
    certificate: Certificate,
    /// Secret numbers by the fingerprint of their key.
    secrets: Vec<(Fingerprint, Secret)>,
    /// The whole key as packets, secrets unprotected.
    bytes: Secret,
}

/// Reads a certificate or a secret key: both have the same shape.
fn read(provider: &dyn Provider, data: &[u8]) -> Result<SecretKey> {
    let data = dearmor(data)?;
    let mut cert: Option<Certificate> = None;
    let mut secrets = Vec::new();
    let mut public_bytes = Vec::new();
    // What signatures now apply to: the primary key, the user ID just
    // read, or the subkey just read. `None` after something skipped.
    enum After {
        Primary,
        UserId(Vec<u8>, bool),
        Subkey(usize),
        Skipped,
    }
    let mut after = After::Skipped;
    for packet in packets(&data)? {
        let body = packet.body.as_ref();
        match packet.tag {
            tag::PUBLIC_KEY | tag::SECRET_KEY | tag::PUBLIC_SUBKEY | tag::SECRET_SUBKEY => {
                let primary = matches!(packet.tag, tag::PUBLIC_KEY | tag::SECRET_KEY);
                let secret = matches!(packet.tag, tag::SECRET_KEY | tag::SECRET_SUBKEY);
                if primary && cert.is_some() {
                    return Err(Error::Unsupported(
                        "more than one certificate in a file".into(),
                    ));
                }
                let parsed = PublicKey::parse(provider, body);
                // A subkey of a kind not used here is passed over; the
                // primary key has to be one that can be checked.
                let (key, rest) = match parsed {
                    Ok(found) => found,
                    Err(Error::Unsupported(_)) if !primary => {
                        after = After::Skipped;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                if secret {
                    let mut r = Reader::new(rest);
                    if r.u8()? != 0 {
                        return Err(Error::Unsupported(
                            "secret keys locked with a passphrase inside the OpenPGP file".into(),
                        ));
                    }
                    let number = r.mpi()?;
                    if number.len() > key.curve.size() {
                        return malformed("a secret key too large for its curve");
                    }
                    let mut padded = Secret::zeroed(key.curve.size());
                    let at = key.curve.size() - number.len();
                    padded.bytes_mut()[at..].copy_from_slice(number);
                    if !provider.ec_pair_matches(key.curve, padded.bytes(), &key.point) {
                        return malformed("a secret key that does not match its public key");
                    }
                    secrets.push((key.fingerprint, padded));
                }
                let public_tag = if primary {
                    tag::PUBLIC_KEY
                } else {
                    tag::PUBLIC_SUBKEY
                };
                write_packet(&mut public_bytes, public_tag, &key.body);
                if primary {
                    if key.algorithm != id::ECDSA {
                        return malformed("a primary key that cannot sign");
                    }
                    cert = Some(Certificate {
                        primary: key,
                        direct: Vec::new(),
                        revoked: false,
                        user_ids: Vec::new(),
                        subkeys: Vec::new(),
                        bytes: Vec::new(),
                    });
                    after = After::Primary;
                } else {
                    let Some(cert) = cert.as_mut() else {
                        return malformed("a subkey before any primary key");
                    };
                    cert.subkeys.push(Subkey {
                        key,
                        bindings: Vec::new(),
                        revoked: false,
                    });
                    after = After::Subkey(cert.subkeys.len() - 1);
                }
            }
            tag::USER_ID => {
                write_packet(&mut public_bytes, tag::USER_ID, body);
                after = After::UserId(body.to_vec(), false);
            }
            tag::SIGNATURE => {
                let Some(cert) = cert.as_mut() else {
                    return malformed("a signature before any key");
                };
                if !matches!(after, After::Skipped) {
                    write_packet(&mut public_bytes, tag::SIGNATURE, body);
                }
                let Some(sig) = Signature::parse(body) else {
                    continue;
                };
                let primary = cert.primary.hashed_form();
                match (&mut after, sig.kind) {
                    (After::Primary, kind::DIRECT_KEY) => {
                        if sig.verify(provider, &cert.primary, &primary) {
                            cert.direct.push(Binding::new(&sig, &cert.primary));
                        }
                    }
                    (After::Primary, kind::KEY_REVOCATION) => {
                        cert.revoked |= sig.verify(provider, &cert.primary, &primary);
                    }
                    (After::UserId(name, listed), k) if kind::CERTIFICATIONS.contains(&k) => {
                        let mut data = primary;
                        data.push(0xb4);
                        data.extend_from_slice(&(name.len() as u32).to_be_bytes());
                        data.extend_from_slice(name);
                        if !*listed && sig.verify(provider, &cert.primary, &data) {
                            cert.user_ids
                                .push(String::from_utf8_lossy(name).into_owned());
                            *listed = true;
                        }
                    }
                    (After::Subkey(i), kind::SUBKEY_BINDING | kind::SUBKEY_REVOCATION) => {
                        let subkey = &mut cert.subkeys[*i];
                        let mut data = primary;
                        data.extend_from_slice(&subkey.key.hashed_form());
                        if sig.verify(provider, &cert.primary, &data) {
                            if sig.kind == kind::SUBKEY_BINDING {
                                subkey.bindings.push(Binding::new(&sig, &subkey.key));
                            } else {
                                subkey.revoked = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            tag::MARKER | tag::PADDING => {}
            // User attributes, trust packets and anything newer.
            _ => after = After::Skipped,
        }
    }
    let Some(mut certificate) = cert else {
        return malformed("no key found");
    };
    if certificate.direct.is_empty() {
        return malformed("a certificate whose key does not vouch for itself");
    }
    certificate.bytes = public_bytes;
    Ok(SecretKey {
        certificate,
        secrets,
        bytes: Secret::new(data),
    })
}

impl Certificate {
    /// Reads a certificate, armored or binary, and checks every
    /// signature that ties its parts to its primary key. Given a secret
    /// key, takes the public part.
    pub fn parse(provider: &dyn Provider, data: &[u8]) -> Result<Self> {
        Ok(read(provider, data)?.certificate)
    }

    /// The fingerprint of the primary key, which names the certificate.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.primary.fingerprint
    }

    /// The fingerprint as 64 hexadecimal digits.
    pub fn fingerprint_hex(&self) -> String {
        hex(&self.primary.fingerprint)
    }

    /// The names the owner has signed, such as `Ada <ada@example.org>`.
    pub fn user_ids(&self) -> &[String] {
        &self.user_ids
    }

    pub fn curve(&self) -> Curve {
        self.primary.curve
    }

    /// When the primary key was made, in seconds since 1970.
    pub fn created(&self) -> u32 {
        self.primary.created
    }

    /// The fingerprints of every key in the certificate, primary first.
    pub fn key_fingerprints(&self) -> Vec<Fingerprint> {
        std::iter::once(self.primary.fingerprint)
            .chain(self.subkeys.iter().map(|s| s.key.fingerprint))
            .collect()
    }

    /// The certificate as binary packets.
    pub fn to_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn to_armored(&self) -> String {
        armor("PUBLIC KEY BLOCK", &self.bytes)
    }

    /// Whether the certificate can be encrypted to at `now` (seconds
    /// since 1970); the error says why not.
    pub fn usable_at(&self, now: u64) -> Result<()> {
        self.encryption_key(now).map(|_| ())
    }

    /// The subkey to encrypt to at `now` (seconds since 1970): the
    /// newest that the primary key has bound for encryption and that is
    /// neither revoked nor expired.
    pub(crate) fn encryption_key(&self, now: u64) -> Result<&PublicKey> {
        let unusable = |why: &str| Err(Error::Unusable(why.into()));
        if self.revoked {
            return unusable("its owner has revoked it");
        }
        let Some(direct) = Binding::current(&self.direct, now) else {
            return unusable("its self-signature has expired");
        };
        if direct.key_lapses.is_some_and(|t| now >= t) {
            return unusable("it has expired");
        }
        let mut expired = false;
        let mut best: Option<&Subkey> = None;
        for subkey in &self.subkeys {
            let Some(binding) = Binding::current(&subkey.bindings, now) else {
                continue;
            };
            if subkey.revoked || subkey.key.algorithm != id::ECDH || binding.flags & ENCRYPT == 0 {
                continue;
            }
            if binding.key_lapses.is_some_and(|t| now >= t) {
                expired = true;
                continue;
            }
            if best.is_none_or(|b| b.key.created < subkey.key.created) {
                best = Some(subkey);
            }
        }
        match best {
            Some(subkey) => Ok(&subkey.key),
            None if expired => unusable("its encryption key has expired"),
            None => unusable("it has no encryption key on NIST P-256 or P-384"),
        }
    }
}

impl SecretKey {
    /// Makes a new key: a primary key that signs and a subkey that
    /// messages are encrypted to, both on `curve`, under the name
    /// `user_id` (by custom `Name <address>`; may be empty).
    pub fn generate(
        provider: &dyn Provider,
        curve: Curve,
        user_id: &str,
        now: u64,
    ) -> Result<Self> {
        let created = u32::try_from(now).unwrap_or(u32::MAX);
        let (primary_secret, point) = provider.ec_generate(curve)?;
        let primary = PublicKey::new(provider, created, id::ECDSA, curve, point)?;
        let (sub_secret, point) = provider.ec_generate(curve)?;
        let subkey = PublicKey::new(provider, created, id::ECDH, curve, point)?;
        let signed = |kind: u8, subpackets: &[u8], data: &[u8]| {
            sign(
                provider,
                &primary,
                primary_secret.bytes(),
                kind,
                created,
                subpackets,
                data,
            )
        };
        // Not locked: whoever stores the key seals the whole of it.
        let secret_packet = |key: &PublicKey, secret: &Secret| {
            let mut body = Vec::with_capacity(key.body.len() + 3 + secret.bytes().len());
            body.extend_from_slice(&key.body);
            body.push(0);
            write_mpi(&mut body, secret.bytes());
            Secret::new(body)
        };
        // Room for the whole key, so the secrets are not left behind in
        // buffers it outgrew.
        let mut bytes = Vec::with_capacity(4096);
        let out = |bytes: &mut Vec<u8>, tag: u8, body: &[u8]| write_packet(bytes, tag, body);
        out(
            &mut bytes,
            tag::SECRET_KEY,
            secret_packet(&primary, &primary_secret).bytes(),
        );
        // What the key says of itself and of how to write to its owner:
        // AES-256 or AES-128 in GCM, in the newer packet format only.
        let mut about = Vec::new();
        subpacket(&mut about, sub::FLAGS, true, &[CERTIFY_AND_SIGN]);
        subpacket(&mut about, sub::FEATURES, false, &[0x08]);
        subpacket(
            &mut about,
            sub::AEAD_SUITES,
            false,
            &[id::AES256, id::GCM, id::AES128, id::GCM],
        );
        subpacket(&mut about, sub::SYMMETRIC, false, &[id::AES256, id::AES128]);
        subpacket(
            &mut about,
            sub::HASHES,
            false,
            &[id::SHA512, id::SHA384, id::SHA256],
        );
        let primary_form = primary.hashed_form();
        out(
            &mut bytes,
            tag::SIGNATURE,
            &signed(kind::DIRECT_KEY, &about, &primary_form)?,
        );
        if !user_id.is_empty() {
            out(&mut bytes, tag::USER_ID, user_id.as_bytes());
            let mut data = primary_form.clone();
            data.push(0xb4);
            data.extend_from_slice(&(user_id.len() as u32).to_be_bytes());
            data.extend_from_slice(user_id.as_bytes());
            out(
                &mut bytes,
                tag::SIGNATURE,
                &signed(kind::POSITIVE_CERTIFICATION, &[], &data)?,
            );
        }
        out(
            &mut bytes,
            tag::SECRET_SUBKEY,
            secret_packet(&subkey, &sub_secret).bytes(),
        );
        let mut flags = Vec::new();
        subpacket(&mut flags, sub::FLAGS, true, &[ENCRYPT]);
        let mut data = primary_form;
        data.extend_from_slice(&subkey.hashed_form());
        out(
            &mut bytes,
            tag::SIGNATURE,
            &signed(kind::SUBKEY_BINDING, &flags, &data)?,
        );
        // Reading it back checks every signature just made.
        let bytes = Secret::new(bytes);
        read(provider, bytes.bytes())
    }

    /// Reads a secret key, armored or binary, whose secret numbers are
    /// not locked with a passphrase.
    pub fn parse(provider: &dyn Provider, data: &[u8]) -> Result<Self> {
        let key = read(provider, data)?;
        if key.secrets.is_empty() {
            return malformed("this is a certificate, without its secret key");
        }
        Ok(key)
    }

    /// The public part.
    pub fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    /// The whole key as binary packets. The secret numbers in it are
    /// not protected: seal it before it is stored.
    pub fn to_bytes(&self) -> &[u8] {
        self.bytes.bytes()
    }

    /// The whole key, armored and unprotected.
    pub fn to_armored(&self) -> String {
        armor("PRIVATE KEY BLOCK", self.bytes.bytes())
    }

    /// The encryption subkeys there are secrets for, to try on a message.
    pub(crate) fn decryption_keys(&self) -> impl Iterator<Item = (&PublicKey, &Secret)> {
        self.certificate.subkeys.iter().filter_map(|subkey| {
            let (_, secret) = self
                .secrets
                .iter()
                .find(|(fp, _)| *fp == subkey.key.fingerprint)?;
            // A key bound by its primary key, though it may since have
            // expired or been revoked: old messages still open.
            (subkey.key.algorithm == id::ECDH && !subkey.bindings.is_empty())
                .then_some((&subkey.key, secret))
        })
    }
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretKey({})", self.certificate.fingerprint_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Software;

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn a_new_key_reads_back_whole() {
        for curve in [Curve::P256, Curve::P384] {
            let key = SecretKey::generate(&Software, curve, "Ada <ada@example.org>", NOW).unwrap();
            let cert = key.certificate();
            assert_eq!(cert.user_ids(), ["Ada <ada@example.org>"]);
            assert_eq!(cert.curve(), curve);
            assert_eq!(cert.created(), NOW as u32);
            assert_eq!(cert.fingerprint_hex().len(), 64);
            assert_eq!(cert.key_fingerprints().len(), 2);
            let subkey = cert.encryption_key(NOW).unwrap();
            assert_eq!(subkey.algorithm, id::ECDH);
            assert_eq!(key.decryption_keys().count(), 1);
            // The public part alone, binary or armored, is the same
            // certificate and carries no secret.
            let public = Certificate::parse(&Software, cert.to_bytes()).unwrap();
            assert_eq!(&public, cert);
            let armored = cert.to_armored();
            assert!(armored.starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----"));
            assert_eq!(
                &Certificate::parse(&Software, armored.as_bytes()).unwrap(),
                cert
            );
            assert!(SecretKey::parse(&Software, cert.to_bytes()).is_err());
            let again = SecretKey::parse(&Software, key.to_armored().as_bytes()).unwrap();
            assert_eq!(again.certificate(), cert);
            assert_eq!(again.secrets, key.secrets);
        }
    }

    #[test]
    fn a_key_without_a_name_is_still_a_certificate() {
        let key = SecretKey::generate(&Software, Curve::P256, "", NOW).unwrap();
        assert!(key.certificate().user_ids().is_empty());
        assert!(key.certificate().encryption_key(NOW).is_ok());
    }

    /// Flips one bit in each byte of a certificate in turn: it must
    /// either be refused or lose the part that was changed, and never
    /// yield a different key to encrypt to.
    #[test]
    fn nothing_in_a_certificate_can_be_changed_unnoticed() {
        let key = SecretKey::generate(&Software, Curve::P256, "Ada", NOW).unwrap();
        let cert = key.certificate();
        let good = cert.encryption_key(NOW).unwrap().clone();
        let bytes = cert.to_bytes();
        let mut refused = 0;
        for i in 0..bytes.len() {
            let mut bent = bytes.to_vec();
            bent[i] ^= 0x04;
            match Certificate::parse(&Software, &bent) {
                Err(_) => refused += 1,
                Ok(changed) => {
                    assert_eq!(changed.fingerprint(), cert.fingerprint(), "byte {i}");
                    if let Ok(k) = changed.encryption_key(NOW) {
                        assert_eq!(k, &good, "byte {i}");
                    }
                }
            }
        }
        assert!(refused > bytes.len() / 4);
    }

    #[test]
    fn a_subkey_from_another_certificate_is_not_taken() {
        let ada = SecretKey::generate(&Software, Curve::P256, "Ada", NOW).unwrap();
        let eve = SecretKey::generate(&Software, Curve::P256, "Eve", NOW).unwrap();
        // Ada's primary key and name, then Eve's subkey and its binding.
        let all = packets(ada.certificate().to_bytes()).unwrap();
        let theirs = packets(eve.certificate().to_bytes()).unwrap();
        let mut forged = Vec::new();
        for p in all.iter().take(4).chain(theirs.iter().skip(4)) {
            write_packet(&mut forged, p.tag, &p.body);
        }
        let cert = Certificate::parse(&Software, &forged).unwrap();
        assert_eq!(cert.fingerprint(), ada.certificate().fingerprint());
        assert!(matches!(cert.encryption_key(NOW), Err(Error::Unusable(_))));
    }

    #[test]
    fn other_kinds_of_key_are_refused_by_name() {
        // A version 6 Ed25519 primary key.
        let mut body = vec![6, 0, 0, 0, 1, 27, 0, 0, 0, 32];
        body.extend_from_slice(&[7; 32]);
        let mut bytes = Vec::new();
        write_packet(&mut bytes, tag::PUBLIC_KEY, &body);
        let Err(Error::Unsupported(why)) = Certificate::parse(&Software, &bytes) else {
            panic!("an Ed25519 key was accepted");
        };
        assert!(why.contains("Ed25519"), "{why}");
        let mut bytes = Vec::new();
        write_packet(&mut bytes, tag::PUBLIC_KEY, &[4, 0, 0, 0, 1, 19]);
        assert!(matches!(
            Certificate::parse(&Software, &bytes),
            Err(Error::Unsupported(_))
        ));
        assert!(Certificate::parse(&Software, b"").is_err());
        assert!(Certificate::parse(&Software, b"%PDF-1.7").is_err());
    }
}
