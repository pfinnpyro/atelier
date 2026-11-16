//! Encrypted messages (RFC 9580, sections 5.1, 5.3, 5.9 and 5.13).
//!
//! A message is a random session key, wrapped once for each recipient
//! and once under the passphrase if there is one, followed by the data
//! sealed under that key with AES-256-GCM in chunks. Every chunk carries
//! its own tag and a last tag covers the length, so a message that was
//! changed, reordered or cut short gives an error and never any data.

use crate::cert::{Certificate, PublicKey, SecretKey};
use crate::packet::{Reader, packets, tag, write_mpi, write_packet};
use crate::provider::{KeyPair, Provider, Secret};
use crate::{
    Error, Result, aes_key_size, curve_oid, dearmor, ecdh_parameters, hash_id, id, malformed,
};

/// Each chunk is 2^(6 + this) bytes: 256 KiB.
const CHUNK: u8 = 12;

/// The coded count for passphrases: 65,011,712 bytes hashed, the most
/// the format allows.
const S2K_COUNT: u8 = 0xff;

/// What a version 6 symmetric-key packet's key and tag are bound to.
fn skesk_info(cipher: u8) -> [u8; 4] {
    [0xc0 | tag::SKESK, 6, cipher, id::GCM]
}

/// What a version 2 data packet's keys and tags are bound to.
fn seipd_info(cipher: u8, chunk: u8) -> [u8; 5] {
    [0xc0 | tag::SEIPD, 2, cipher, id::GCM, chunk]
}

fn nonce(prefix: &[u8], index: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(prefix);
    nonce[4..].copy_from_slice(&index.to_be_bytes());
    nonce
}

/// The session key with its checksum, padded to a multiple of eight
/// bytes for the key wrap (RFC 9580, 11.5).
fn padded(session: &[u8]) -> Secret {
    let sum = session
        .iter()
        .fold(0u16, |s, b| s.wrapping_add(u16::from(*b)));
    let mut out = Vec::with_capacity(session.len() + 10);
    out.extend_from_slice(session);
    out.extend_from_slice(&sum.to_be_bytes());
    let pad = 8 - out.len() % 8;
    out.resize(out.len() + pad, pad as u8);
    Secret::new(out)
}

/// The inverse; `None` when the padding or checksum is wrong.
fn unpadded(wrapped: &Secret) -> Option<Secret> {
    let bytes = wrapped.bytes();
    let pad = usize::from(*bytes.last()?);
    if !(1..=8).contains(&pad) || bytes.len() < pad + 2 {
        return None;
    }
    let (body, padding) = bytes.split_at(bytes.len() - pad);
    let (session, sum) = body.split_at(body.len() - 2);
    let want = session
        .iter()
        .fold(0u16, |s, b| s.wrapping_add(u16::from(*b)));
    (padding.iter().all(|b| usize::from(*b) == pad) && sum == want.to_be_bytes())
        .then(|| Secret::new(session.to_vec()))
}

/// What the key that wraps a session key is bound to: the curve, the
/// algorithms and the recipient (RFC 9580, 11.5).
fn kdf_parameters(key: &PublicKey) -> Vec<u8> {
    let oid = curve_oid(key.curve);
    let (hash, cipher) = ecdh_parameters(key.curve);
    let mut out = vec![oid.len() as u8];
    out.extend_from_slice(oid);
    out.extend_from_slice(&[id::ECDH, 3, 1, hash_id(hash), cipher]);
    out.extend_from_slice(b"Anonymous Sender    ");
    out.extend_from_slice(&key.fingerprint);
    out
}

/// A version 6 public-key encrypted session key packet for `key`.
fn pkesk(provider: &dyn Provider, key: &PublicKey, session: &[u8]) -> Result<Vec<u8>> {
    let (hash, cipher) = ecdh_parameters(key.curve);
    let (ephemeral, point) = provider.ec_generate(key.curve)?;
    let ours = KeyPair {
        secret: ephemeral.bytes(),
        public: &point,
    };
    let kek = provider.ecdh_kek(key.curve, hash, ours, &key.point, &kdf_parameters(key))?;
    let kek = &kek.bytes()[..aes_key_size(cipher)?];
    let wrapped = provider.key_wrap(kek, padded(session).bytes())?;
    let mut body = vec![6, 33, 6];
    body.extend_from_slice(&key.fingerprint);
    body.push(id::ECDH);
    write_mpi(&mut body, &point);
    body.push(wrapped.len() as u8);
    body.extend_from_slice(&wrapped);
    Ok(body)
}

/// A version 6 symmetric-key encrypted session key packet.
fn skesk(provider: &dyn Provider, passphrase: &[u8], session: &[u8]) -> Result<Vec<u8>> {
    let mut salt = [0u8; 8];
    provider.random(&mut salt)?;
    let mut iv = [0u8; 12];
    provider.random(&mut iv)?;
    let stretched = provider.passphrase_key(&salt, s2k_count(S2K_COUNT), passphrase)?;
    let info = skesk_info(id::AES256);
    let mut kek = Secret::zeroed(32);
    provider.hkdf_sha256(stretched.bytes(), b"", &info, kek.bytes_mut())?;
    let sealed = provider.gcm_seal(kek.bytes(), &iv, &info, session)?;
    let mut s2k = vec![3, id::SHA256];
    s2k.extend_from_slice(&salt);
    s2k.push(S2K_COUNT);
    let mut body = vec![6, (3 + s2k.len() + iv.len()) as u8, id::AES256, id::GCM];
    body.push(s2k.len() as u8);
    body.extend_from_slice(&s2k);
    body.extend_from_slice(&iv);
    body.extend_from_slice(&sealed);
    Ok(body)
}

fn s2k_count(coded: u8) -> u32 {
    (16 + u32::from(coded & 15)) << ((coded >> 4) + 6)
}

/// Encrypts `data` so that the holder of any of `recipients`' secret
/// keys, or anyone who knows `passphrase`, can read it. `now` is the
/// time in seconds since 1970, by which certificates are judged.
/// Returns a binary OpenPGP message.
pub fn encrypt(
    provider: &dyn Provider,
    data: &[u8],
    recipients: &[&Certificate],
    passphrase: Option<&[u8]>,
    now: u64,
) -> Result<Vec<u8>> {
    if recipients.is_empty() && passphrase.is_none() {
        return malformed("nobody to encrypt to");
    }
    if passphrase.is_some_and(<[u8]>::is_empty) {
        return malformed("an empty passphrase");
    }
    let mut session = Secret::zeroed(32);
    provider.random(session.bytes_mut())?;
    let mut out = Vec::with_capacity(data.len() + data.len() / 4096 + 1024);
    for cert in recipients {
        let key = cert.encryption_key(now)?;
        write_packet(
            &mut out,
            tag::PKESK,
            &pkesk(provider, key, session.bytes())?,
        );
    }
    if let Some(passphrase) = passphrase {
        write_packet(
            &mut out,
            tag::SKESK,
            &skesk(provider, passphrase, session.bytes())?,
        );
    }
    seal(provider, session.bytes(), data, &mut out)?;
    Ok(out)
}

/// Appends the data packet: `data` sealed under a key worked out from
/// the session key and a fresh salt.
fn seal(provider: &dyn Provider, session: &[u8], data: &[u8], out: &mut Vec<u8>) -> Result<()> {
    // What is sealed: one literal data packet, binary, with no file name
    // or date to give away.
    if data.len() as u64 + 6 > u64::from(u32::MAX) {
        return Err(Error::Unsupported("files of 4 GB and more".into()));
    }
    let mut plain = Secret::with_capacity(data.len() + 12);
    plain.extend(&[0xc0 | tag::LITERAL, 0xff]);
    plain.extend(&(data.len() as u32 + 6).to_be_bytes());
    plain.extend(&[b'b', 0, 0, 0, 0, 0]);
    plain.extend(data);

    let mut salt = [0u8; 32];
    provider.random(&mut salt)?;
    let info = seipd_info(id::AES256, CHUNK);
    let mut keys = Secret::zeroed(36);
    provider.hkdf_sha256(session, &salt, &info, keys.bytes_mut())?;
    let (key, prefix) = keys.bytes().split_at(32);
    let mut body = Vec::with_capacity(plain.bytes().len() + plain.bytes().len() / 4096 + 128);
    body.extend_from_slice(&[2, id::AES256, id::GCM, CHUNK]);
    body.extend_from_slice(&salt);
    let mut chunks = 0u64;
    for chunk in plain.bytes().chunks(1 << (CHUNK + 6)) {
        body.extend(provider.gcm_seal(key, &nonce(prefix, chunks), &info, chunk)?);
        chunks += 1;
    }
    let mut last = info.to_vec();
    last.extend_from_slice(&(plain.bytes().len() as u64).to_be_bytes());
    body.extend(provider.gcm_seal(key, &nonce(prefix, chunks), &last, b"")?);
    write_packet(out, tag::SEIPD, &body);
    Ok(())
}

/// What it takes to seal new data for exactly the readers of a message
/// that was opened: the message's session key and its key packets, as
/// they were. RFC 9580 (13.8) provides for this: a fresh salt gives the
/// new data its own key, and whoever could open the old message, by key
/// or passphrase, can open the new one, with no need for their
/// certificates or the passphrase.
///
/// That is the whole of what it can do. It cannot add or drop a reader,
/// and anyone who could ever read the original can read what is sealed
/// with this; to change who can read, encrypt afresh.
pub struct Readers {
    session: Secret,
    /// The key packets, serialized.
    locks: Vec<u8>,
    summary: Summary,
}

impl Readers {
    /// Who the readers are, as the message named them.
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// Seals `data` for the same readers. Returns a binary message.
    pub fn encrypt(&self, provider: &dyn Provider, data: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.locks.len() + data.len() + data.len() / 4096 + 256);
        out.extend_from_slice(&self.locks);
        seal(provider, self.session.bytes(), data, &mut out)?;
        Ok(out)
    }
}

impl std::fmt::Debug for Readers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Readers({:?})", self.summary)
    }
}

/// What a message says of itself without being opened, to know what to
/// ask its reader for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Summary {
    /// The fingerprints of the keys it is encrypted to (those of the
    /// encryption subkeys). A recipient may also be left unnamed.
    pub recipients: Vec<Vec<u8>>,
    /// Whether a passphrase opens it.
    pub passphrase: bool,
}

/// One way into the session key, as read from the message.
enum Lock<'a> {
    Key {
        /// Empty for a recipient left unnamed.
        fingerprint: &'a [u8],
        point: &'a [u8],
        wrapped: &'a [u8],
    },
    Passphrase {
        cipher: u8,
        salt: [u8; 8],
        count: u32,
        iv: [u8; 12],
        sealed: &'a [u8],
    },
}

struct Parsed<'a> {
    locks: Vec<Lock<'a>>,
    /// Locks of kinds this crate does not open, named for the error.
    skipped: Vec<&'static str>,
    data: &'a [u8],
}

fn parse<'a>(found: &'a [crate::packet::Packet<'a>]) -> Result<Parsed<'a>> {
    let mut parsed = Parsed {
        locks: Vec::new(),
        skipped: Vec::new(),
        data: &[],
    };
    for packet in found {
        let mut r = Reader::new(&packet.body);
        match packet.tag {
            tag::PKESK => {
                if r.u8()? != 6 {
                    parsed
                        .skipped
                        .push("a session key in the older packet format");
                    continue;
                }
                let len = usize::from(r.u8()?);
                let mut named = Reader::new(r.take(len)?);
                let fingerprint = if named.is_empty() {
                    &[][..]
                } else {
                    named.u8()?;
                    named.rest()
                };
                if r.u8()? != id::ECDH {
                    parsed
                        .skipped
                        .push("a key that is not on NIST P-256 or P-384");
                    continue;
                }
                let point = r.mpi()?;
                let len = usize::from(r.u8()?);
                parsed.locks.push(Lock::Key {
                    fingerprint,
                    point,
                    wrapped: r.take(len)?,
                });
            }
            tag::SKESK => {
                if r.u8()? != 6 {
                    parsed
                        .skipped
                        .push("a passphrase in the older packet format");
                    continue;
                }
                let len = usize::from(r.u8()?);
                let mut fields = Reader::new(r.take(len)?);
                let (cipher, aead) = (fields.u8()?, fields.u8()?);
                let s2k_len = usize::from(fields.u8()?);
                let mut s2k = Reader::new(fields.take(s2k_len)?);
                let iv = fields.rest();
                let sealed = r.rest();
                if aead != id::GCM {
                    parsed
                        .skipped
                        .push("a passphrase with OCB or EAX in place of GCM");
                    continue;
                }
                match (s2k.u8()?, s2k.u8()?) {
                    (3, id::SHA256) => {}
                    (4, _) => {
                        parsed.skipped.push("a passphrase stretched with Argon2");
                        continue;
                    }
                    _ => {
                        parsed
                            .skipped
                            .push("a passphrase stretched in a way not used here");
                        continue;
                    }
                }
                let (Ok(salt), Ok(iv)) =
                    (<[u8; 8]>::try_from(s2k.take(8)?), <[u8; 12]>::try_from(iv))
                else {
                    return malformed("a passphrase packet of the wrong shape");
                };
                parsed.locks.push(Lock::Passphrase {
                    cipher,
                    salt,
                    count: s2k_count(s2k.u8()?),
                    iv,
                    sealed,
                });
            }
            tag::SEIPD => {
                parsed.data = &packet.body;
                return Ok(parsed);
            }
            tag::MARKER | tag::PADDING => {}
            9 | 20 => {
                return Err(Error::Unsupported(
                    "messages without the newer integrity protection".into(),
                ));
            }
            _ => return malformed("this is not an encrypted message"),
        }
    }
    malformed("an encrypted message with no data in it")
}

/// Reads who a message is for. The message may be armored.
pub fn summary(message: &[u8]) -> Result<Summary> {
    let message = dearmor(message)?;
    let found = packets(&message)?;
    Ok(summarize(&parse(&found)?))
}

fn summarize(parsed: &Parsed) -> Summary {
    let mut summary = Summary::default();
    for lock in &parsed.locks {
        match lock {
            Lock::Key { fingerprint, .. } if !fingerprint.is_empty() => {
                summary.recipients.push(fingerprint.to_vec());
            }
            Lock::Key { .. } => {}
            Lock::Passphrase { .. } => summary.passphrase = true,
        }
    }
    summary
}

/// Tries one lock; `None` when this key or passphrase does not fit it.
fn unlock(
    provider: &dyn Provider,
    lock: &Lock,
    keys: &[&SecretKey],
    passphrase: Option<&[u8]>,
    refused: &mut Option<Error>,
) -> Option<Secret> {
    match lock {
        Lock::Key {
            fingerprint,
            point,
            wrapped,
        } => {
            for (key, secret) in keys.iter().flat_map(|k| k.decryption_keys()) {
                if !fingerprint.is_empty() && *fingerprint != key.fingerprint {
                    continue;
                }
                let (hash, cipher) = ecdh_parameters(key.curve);
                let size = aes_key_size(cipher).ok()?;
                let ours = KeyPair {
                    secret: secret.bytes(),
                    public: &key.point,
                };
                let session = provider
                    .ecdh_kek(key.curve, hash, ours, point, &kdf_parameters(key))
                    .and_then(|kek| provider.key_unwrap(&kek.bytes()[..size], wrapped))
                    .ok()
                    .and_then(|plain| unpadded(&plain));
                if session.is_some() {
                    return session;
                }
            }
            None
        }
        Lock::Passphrase {
            cipher,
            salt,
            count,
            iv,
            sealed,
        } => {
            let passphrase = passphrase?;
            let size = aes_key_size(*cipher).ok()?;
            let stretched = match provider.passphrase_key(salt, *count, passphrase) {
                Ok(key) => key,
                Err(e) => {
                    *refused = Some(e);
                    return None;
                }
            };
            let info = skesk_info(*cipher);
            let mut kek = Secret::zeroed(size);
            provider
                .hkdf_sha256(&stretched.bytes()[..size], b"", &info, kek.bytes_mut())
                .ok()?;
            provider.gcm_open(kek.bytes(), iv, &info, sealed).ok()
        }
    }
}

/// Opens the sealed data with a session key.
fn open(provider: &dyn Provider, body: &[u8], session: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    if r.u8()? != 2 {
        return Err(Error::Unsupported(
            "messages without the newer integrity protection".into(),
        ));
    }
    let (cipher, aead, chunk) = (r.u8()?, r.u8()?, r.u8()?);
    if aead != id::GCM {
        return Err(Error::Unsupported(
            "messages sealed with OCB or EAX in place of GCM".into(),
        ));
    }
    if chunk > 16 {
        return malformed("chunks larger than the format allows");
    }
    let size = aes_key_size(cipher)?;
    if session.len() != size {
        return Err(Error::NoKey);
    }
    let salt = r.take(32)?;
    let sealed = r.rest();
    let Some(at) = sealed.len().checked_sub(16) else {
        return Err(Error::Tampered);
    };
    let (sealed, last_tag) = sealed.split_at(at);
    let info = seipd_info(cipher, chunk);
    let mut keys = Secret::zeroed(size + 4);
    provider.hkdf_sha256(session, salt, &info, keys.bytes_mut())?;
    let (key, prefix) = keys.bytes().split_at(size);
    let mut plain = Secret::with_capacity(sealed.len());
    let mut chunks = 0u64;
    for piece in sealed.chunks((1usize << (chunk + 6)) + 16) {
        plain.extend(
            provider
                .gcm_open(key, &nonce(prefix, chunks), &info, piece)?
                .bytes(),
        );
        chunks += 1;
    }
    let mut last = info.to_vec();
    last.extend_from_slice(&(plain.bytes().len() as u64).to_be_bytes());
    provider.gcm_open(key, &nonce(prefix, chunks), &last, last_tag)?;

    // Inside: a literal data packet, perhaps with padding or signature
    // packets around it, which are passed over.
    for packet in packets(plain.bytes())? {
        match packet.tag {
            tag::LITERAL => {
                let mut r = Reader::new(&packet.body);
                r.u8()?;
                let name = usize::from(r.u8()?);
                r.take(name)?;
                r.u32()?;
                return Ok(r.rest().to_vec());
            }
            tag::COMPRESSED => {
                return Err(Error::Unsupported("compressed messages".into()));
            }
            tag::SIGNATURE | tag::ONE_PASS | tag::MARKER | tag::PADDING => {}
            _ => return malformed("something other than data inside the message"),
        }
    }
    malformed("no data inside the message")
}

/// Decrypts a message, armored or binary, with whichever of `keys` or
/// the `passphrase` opens it. Nothing is returned unless every tag in
/// the message is right.
pub fn decrypt(
    provider: &dyn Provider,
    message: &[u8],
    keys: &[&SecretKey],
    passphrase: Option<&[u8]>,
) -> Result<Vec<u8>> {
    Ok(decrypt_keeping_readers(provider, message, keys, passphrase)?.0)
}

/// Decrypts as [`decrypt`] does, and also returns what is needed to
/// seal a changed copy for the same readers.
pub fn decrypt_keeping_readers(
    provider: &dyn Provider,
    message: &[u8],
    keys: &[&SecretKey],
    passphrase: Option<&[u8]>,
) -> Result<(Vec<u8>, Readers)> {
    let message = dearmor(message)?;
    let found = packets(&message)?;
    let parsed = parse(&found)?;
    let mut refused = None;
    for lock in &parsed.locks {
        let Some(session) = unlock(provider, lock, keys, passphrase, &mut refused) else {
            continue;
        };
        let data = match open(provider, parsed.data, session.bytes()) {
            // A session key of the wrong size came from the wrong lock.
            Err(Error::NoKey) => continue,
            other => other?,
        };
        // The key packets in the newer format, which are the ones that
        // may come before data sealed this way.
        let mut locks = Vec::new();
        for packet in &found {
            let is_lock = matches!(packet.tag, tag::PKESK | tag::SKESK);
            if is_lock && packet.body.first() == Some(&6) {
                write_packet(&mut locks, packet.tag, &packet.body);
            }
        }
        let readers = Readers {
            session,
            locks,
            summary: summarize(&parsed),
        };
        return Ok((data, readers));
    }
    if let Some(e) = refused {
        return Err(e);
    }
    match parsed.skipped.first() {
        Some(what) if parsed.locks.is_empty() => Err(Error::Unsupported((*what).into())),
        _ => Err(Error::NoKey),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Curve, Software};

    const NOW: u64 = 1_800_000_000;

    /// RFC 9580, A.11.5: "Hello, world!" under the passphrase "password"
    /// with AES-128 in GCM.
    const SAMPLE: &str = "-----BEGIN PGP MESSAGE-----

wzwGGgcDCwMI6dOXhbIHAAj/tC58SD70iERXyzcmubPbn/d25fTZpAlS4kRymIUa
v/91Jt8t1VRBdXmneZ/SaQIHAwb8uUSQvLmLvcnRBsYJAmaUD3LontwhtVlrFXax
Ae0Pn/xvxtZbv9JNzQeQlm5tHoWjAFN4TLHYtqBpnvEhVaeyrWJYUxtXZR/Xd3kS
+pXjXZtAIW9ppMJI2yj/QzHxYykHOZ5v+Q==
-----END PGP MESSAGE-----
";

    #[test]
    fn the_rfc_sample_message_opens() {
        let plain = decrypt(&Software, SAMPLE.as_bytes(), &[], Some(b"password")).unwrap();
        assert_eq!(plain, b"Hello, world!");
        assert_eq!(
            decrypt(&Software, SAMPLE.as_bytes(), &[], Some(b"Password")),
            Err(Error::NoKey)
        );
        assert_eq!(
            summary(SAMPLE.as_bytes()).unwrap(),
            Summary {
                recipients: vec![],
                passphrase: true
            }
        );
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 131 + i / 7) as u8).collect()
    }

    #[test]
    fn a_passphrase_opens_what_it_sealed() {
        let plain = data(1000);
        let sealed = encrypt(&Software, &plain, &[], Some(b"open sesame"), NOW).unwrap();
        assert!(!sealed.windows(8).any(|w| plain.windows(8).any(|p| p == w)));
        assert_eq!(
            decrypt(&Software, &sealed, &[], Some(b"open sesame")).unwrap(),
            plain
        );
        assert_eq!(
            decrypt(&Software, &sealed, &[], Some(b"open sesame!")),
            Err(Error::NoKey)
        );
        assert_eq!(decrypt(&Software, &sealed, &[], None), Err(Error::NoKey));
        // Armored, it is the same message.
        let text = crate::armor("MESSAGE", &sealed);
        assert_eq!(
            decrypt(&Software, text.as_bytes(), &[], Some(b"open sesame")).unwrap(),
            plain
        );
    }

    #[test]
    fn each_recipient_and_the_passphrase_open_it_and_no_one_else() {
        let ada = SecretKey::generate(&Software, Curve::P256, "Ada", NOW).unwrap();
        let bob = SecretKey::generate(&Software, Curve::P384, "Bob", NOW).unwrap();
        let eve = SecretKey::generate(&Software, Curve::P256, "Eve", NOW).unwrap();
        // Sizes around the 256 KiB chunk, and nothing at all.
        for n in [0, 1, 262_137, 262_138, 262_139, 600_000] {
            let plain = data(n);
            let sealed = encrypt(
                &Software,
                &plain,
                &[ada.certificate(), bob.certificate()],
                Some(b"or a passphrase"),
                NOW,
            )
            .unwrap();
            for key in [&ada, &bob] {
                assert_eq!(decrypt(&Software, &sealed, &[key], None).unwrap(), plain);
            }
            assert_eq!(
                decrypt(&Software, &sealed, &[&eve, &bob], None).unwrap(),
                plain
            );
            assert_eq!(
                decrypt(&Software, &sealed, &[&eve], Some(b"or a passphrase")).unwrap(),
                plain
            );
            assert_eq!(
                decrypt(&Software, &sealed, &[&eve], None),
                Err(Error::NoKey)
            );
            let said = summary(&sealed).unwrap();
            assert!(said.passphrase);
            assert_eq!(said.recipients.len(), 2);
            assert!(ada.certificate().key_fingerprints()[1][..] == said.recipients[0][..]);
        }
    }

    #[test]
    fn a_changed_message_gives_nothing() {
        let ada = SecretKey::generate(&Software, Curve::P256, "Ada", NOW).unwrap();
        let plain = data(3000);
        let sealed = encrypt(&Software, &plain, &[ada.certificate()], None, NOW).unwrap();
        // A bit in every seventh byte, and in each of the last forty.
        let tail = sealed.len() - 40..sealed.len();
        for i in (0..sealed.len()).step_by(7).chain(tail) {
            let mut bent = sealed.clone();
            bent[i] ^= 0x20;
            assert!(
                decrypt(&Software, &bent, &[&ada], None).is_err(),
                "byte {i}"
            );
        }
        // Cut short.
        assert!(decrypt(&Software, &sealed[..sealed.len() - 1], &[&ada], None).is_err());
    }

    #[test]
    fn chunks_cannot_be_dropped_or_reordered() {
        let ada = SecretKey::generate(&Software, Curve::P256, "Ada", NOW).unwrap();
        // The literal packet's header makes this a little over three
        // chunks: three whole ones and a short fourth.
        let plain = data(3 << 18);
        let sealed = encrypt(&Software, &plain, &[ada.certificate()], None, NOW).unwrap();
        let found = packets(&sealed).unwrap();
        let body = found[1].body.to_vec();
        let piece = (1 << 18) + 16;
        let rebuilt = |body: &[u8]| {
            let mut out = Vec::new();
            write_packet(&mut out, tag::PKESK, &found[0].body);
            write_packet(&mut out, tag::SEIPD, body);
            out
        };
        assert_eq!(
            decrypt(&Software, &rebuilt(&body), &[&ada], None).unwrap(),
            plain
        );
        // The short last chunk removed, keeping the final tag.
        let mut dropped = body[..36 + 3 * piece].to_vec();
        dropped.extend_from_slice(&body[body.len() - 16..]);
        assert_eq!(
            decrypt(&Software, &rebuilt(&dropped), &[&ada], None),
            Err(Error::Tampered)
        );
        // The first two chunks swapped.
        let mut swapped = body[..36].to_vec();
        swapped.extend_from_slice(&body[36 + piece..36 + 2 * piece]);
        swapped.extend_from_slice(&body[36..36 + piece]);
        swapped.extend_from_slice(&body[36 + 2 * piece..]);
        assert_eq!(
            decrypt(&Software, &rebuilt(&swapped), &[&ada], None),
            Err(Error::Tampered)
        );
    }

    #[test]
    fn a_changed_copy_goes_to_the_same_readers() {
        let ada = SecretKey::generate(&Software, Curve::P256, "Ada", NOW).unwrap();
        let bob = SecretKey::generate(&Software, Curve::P384, "Bob", NOW).unwrap();
        let eve = SecretKey::generate(&Software, Curve::P256, "Eve", NOW).unwrap();
        let first = encrypt(
            &Software,
            b"first draft",
            &[ada.certificate(), bob.certificate()],
            Some(b"shared passphrase"),
            NOW,
        )
        .unwrap();
        // Ada opens it with her key, knowing neither Bob's certificate
        // nor the passphrase, and saves a change.
        let (plain, readers) = decrypt_keeping_readers(&Software, &first, &[&ada], None).unwrap();
        assert_eq!(plain, b"first draft");
        assert_eq!(readers.summary(), &summary(&first).unwrap());
        let second = readers.encrypt(&Software, b"second draft").unwrap();
        assert_eq!(summary(&second).unwrap(), summary(&first).unwrap());
        assert_eq!(
            decrypt(&Software, &second, &[&bob], None).unwrap(),
            b"second draft"
        );
        assert_eq!(
            decrypt(&Software, &second, &[], Some(b"shared passphrase")).unwrap(),
            b"second draft"
        );
        assert_eq!(
            decrypt(&Software, &second, &[&eve], None),
            Err(Error::NoKey)
        );
        // The same data sealed twice shares no ciphertext: each has its
        // own salt, and so its own key.
        let third = readers.encrypt(&Software, b"second draft").unwrap();
        assert_ne!(second[second.len() - 40..], third[third.len() - 40..]);
        assert_eq!(second[..readers.locks.len()], third[..readers.locks.len()]);
    }

    #[test]
    fn there_must_be_someone_to_encrypt_to() {
        let ada = SecretKey::generate(&Software, Curve::P256, "Ada", NOW).unwrap();
        assert!(encrypt(&Software, b"x", &[], None, NOW).is_err());
        assert!(encrypt(&Software, b"x", &[], Some(b""), NOW).is_err());
        assert!(encrypt(&Software, b"x", &[ada.certificate()], None, NOW).is_ok());
    }

    #[test]
    fn the_session_key_padding_is_checked() {
        let padded = padded(&[7; 32]);
        assert_eq!(padded.bytes().len(), 40);
        assert_eq!(padded.bytes()[34..], [6; 6]);
        assert_eq!(unpadded(&padded).unwrap().bytes(), [7; 32]);
        for i in [0, 32, 33, 34, 39] {
            let mut bent = padded.clone();
            bent.bytes_mut()[i] ^= 1;
            assert!(unpadded(&bent).is_none(), "byte {i}");
        }
        assert!(unpadded(&Secret::new(vec![])).is_none());
        assert!(unpadded(&Secret::new(vec![2, 2])).is_none());
    }

    /// A provider held to approved algorithms: everything but the
    /// passphrase.
    struct Approved;

    impl Provider for Approved {
        fn name(&self) -> &'static str {
            "test"
        }
        fn random(&self, out: &mut [u8]) -> Result<()> {
            Software.random(out)
        }
        fn digest(&self, hash: atelier_crypto::Hash, data: &[u8]) -> Result<Vec<u8>> {
            Software.digest(hash, data)
        }
        fn hkdf_sha256(&self, ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Result<()> {
            Software.hkdf_sha256(ikm, salt, info, out)
        }
        fn gcm_seal(&self, k: &[u8], n: &[u8; 12], a: &[u8], p: &[u8]) -> Result<Vec<u8>> {
            Software.gcm_seal(k, n, a, p)
        }
        fn gcm_open(&self, k: &[u8], n: &[u8; 12], a: &[u8], s: &[u8]) -> Result<Secret> {
            Software.gcm_open(k, n, a, s)
        }
        fn key_wrap(&self, kek: &[u8], key: &[u8]) -> Result<Vec<u8>> {
            Software.key_wrap(kek, key)
        }
        fn key_unwrap(&self, kek: &[u8], wrapped: &[u8]) -> Result<Secret> {
            Software.key_unwrap(kek, wrapped)
        }
        fn ec_generate(&self, curve: Curve) -> Result<(Secret, Vec<u8>)> {
            Software.ec_generate(curve)
        }
        fn ec_pair_matches(&self, curve: Curve, secret: &[u8], public: &[u8]) -> bool {
            Software.ec_pair_matches(curve, secret, public)
        }
        fn ec_public_is_valid(&self, curve: Curve, public: &[u8]) -> bool {
            Software.ec_public_is_valid(curve, public)
        }
        fn ecdh_kek(
            &self,
            curve: Curve,
            hash: atelier_crypto::Hash,
            ours: KeyPair<'_>,
            peer: &[u8],
            other_info: &[u8],
        ) -> Result<Secret> {
            Software.ecdh_kek(curve, hash, ours, peer, other_info)
        }
        fn ecdsa_sign(&self, c: Curve, k: KeyPair<'_>, d: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
            Software.ecdsa_sign(c, k, d)
        }
        fn ecdsa_verify(&self, c: Curve, p: &[u8], d: &[u8], r: &[u8], s: &[u8]) -> bool {
            Software.ecdsa_verify(c, p, d, r, s)
        }
        fn passphrase_key(&self, _: &[u8; 8], _: u32, _: &[u8]) -> Result<Secret> {
            Err(Error::NotApproved("passphrases".into()))
        }
    }

    #[test]
    fn a_provider_may_refuse_passphrases_and_keep_public_keys() {
        let ada = SecretKey::generate(&Approved, Curve::P384, "Ada", NOW).unwrap();
        assert!(matches!(
            encrypt(&Approved, b"x", &[], Some(b"passphrase"), NOW),
            Err(Error::NotApproved(_))
        ));
        let sealed = encrypt(&Approved, b"for Ada", &[ada.certificate()], None, NOW).unwrap();
        assert_eq!(
            decrypt(&Approved, &sealed, &[&ada], None).unwrap(),
            b"for Ada"
        );
        // A message that only a passphrase opens says why it cannot be.
        let other = encrypt(&Software, b"x", &[], Some(b"passphrase"), NOW).unwrap();
        assert!(matches!(
            decrypt(&Approved, &other, &[&ada], Some(b"passphrase")),
            Err(Error::NotApproved(_))
        ));
    }
}
