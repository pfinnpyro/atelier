//! The standard security handler (ISO 32000-2, 7.6.4): checks a password
//! and decrypts strings and streams. RC4 (40 to 128 bit), AES-128 and
//! AES-256 are supported, revisions 2 to 6.

use atelier_crypto::{
    Aes, Md5, aes_cbc_decrypt, aes_cbc_encrypt, md5, rc4, sha256, sha384, sha512,
};

use crate::{Dict, Document, Error, ObjRef, Object, Result};

/// The 32-byte padding string passwords are extended with.
const PAD: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    /// Not encrypted (the Identity crypt filter).
    Identity,
    Rc4,
    Aes128,
    Aes256,
}

/// Which password opened the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// The user password (often empty): permissions apply.
    User,
    /// The owner password: full access.
    Owner,
}

/// What a successful password check gives: the file key and how strings
/// and streams are encrypted.
pub(crate) struct Security {
    key: Vec<u8>,
    strings: Method,
    streams: Method,
    pub encrypt_metadata: bool,
    /// The /Encrypt dictionary's own object, which is never encrypted.
    pub dict_ref: Option<ObjRef>,
    pub access: Access,
    /// The /P permission flags.
    pub permissions: i32,
}

/// The parts of the /Encrypt dictionary the password check needs.
struct Params {
    revision: i64,
    key_len: usize,
    o: Vec<u8>,
    u: Vec<u8>,
    oe: Vec<u8>,
    ue: Vec<u8>,
    p: i32,
    id: Vec<u8>,
    encrypt_metadata: bool,
    strings: Method,
    streams: Method,
}

impl Security {
    /// Reads the /Encrypt dictionary and checks `password` as the owner
    /// password, then as the user password.
    pub fn open(doc: &Document, password: &[u8]) -> Result<Security> {
        let entry = doc
            .trailer()
            .get(b"Encrypt")
            .cloned()
            .ok_or_else(|| Error::Structure("no /Encrypt".into()))?;
        let dict_ref = entry.as_reference();
        let dict = doc.resolve_dict(&entry)?;
        let params = Params::read(doc, &dict)?;
        let (key, access) = params
            .owner_key(password)
            .map(|k| (k, Access::Owner))
            .or_else(|| params.user_key(password).map(|k| (k, Access::User)))
            .ok_or(Error::Password)?;
        Ok(Security {
            key,
            strings: params.strings,
            streams: params.streams,
            encrypt_metadata: params.encrypt_metadata,
            dict_ref,
            access,
            permissions: params.p,
        })
    }

    /// The key for one object (algorithm 1), or the file key for AES-256.
    fn object_key(&self, r: ObjRef, method: Method) -> Vec<u8> {
        if method == Method::Aes256 {
            return self.key.clone();
        }
        let mut h = Md5::new();
        h.update(&self.key);
        h.update(&r.num.to_le_bytes()[..3]);
        h.update(&r.generation.to_le_bytes()[..2]);
        if method == Method::Aes128 {
            h.update(b"sAlT");
        }
        let n = (self.key.len() + 5).min(16);
        h.finish()[..n].to_vec()
    }

    fn decrypt(&self, r: ObjRef, method: Method, data: &[u8]) -> Vec<u8> {
        match method {
            Method::Identity => data.to_vec(),
            Method::Rc4 => rc4(&self.object_key(r, method), data),
            Method::Aes128 | Method::Aes256 => {
                let Some(aes) = Aes::new(&self.object_key(r, method)) else {
                    return data.to_vec();
                };
                if data.len() < 16 {
                    return Vec::new();
                }
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&data[..16]);
                let mut out = aes_cbc_decrypt(&aes, &iv, &data[16..]);
                // Strip PKCS#7 padding when it is well formed.
                if let Some(&n) = out.last() {
                    let n = n as usize;
                    if (1..=16).contains(&n)
                        && n <= out.len()
                        && out[out.len() - n..].iter().all(|&b| b as usize == n)
                    {
                        out.truncate(out.len() - n);
                    }
                }
                out
            }
        }
    }

    fn encrypt(&self, r: ObjRef, method: Method, data: &[u8]) -> Vec<u8> {
        match method {
            Method::Identity => data.to_vec(),
            Method::Rc4 => rc4(&self.object_key(r, method), data),
            Method::Aes128 | Method::Aes256 => {
                let key = self.object_key(r, method);
                let Some(aes) = Aes::new(&key) else {
                    return data.to_vec();
                };
                // A fresh IV per object and content, without a random
                // source: the hash of the key, the object and the data.
                let mut seed = key.clone();
                seed.extend_from_slice(&r.num.to_le_bytes());
                seed.extend_from_slice(&r.generation.to_le_bytes());
                seed.extend_from_slice(data);
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&sha256(&seed)[..16]);
                let pad = 16 - data.len() % 16;
                let mut padded = data.to_vec();
                padded.extend(std::iter::repeat_n(pad as u8, pad));
                let mut out = iv.to_vec();
                out.extend(aes_cbc_encrypt(&aes, &iv, &padded));
                out
            }
        }
    }

    /// Encrypts every string inside `obj` and, for a stream, its data, to
    /// be written as object `r`. The inverse of reading.
    pub fn encrypt_object(&self, r: ObjRef, obj: &mut Object) {
        if self.dict_ref == Some(r) {
            return;
        }
        self.encrypt_strings(r, obj);
        if let Object::Stream(stream) = obj {
            let skip = stream.dict.is_type(b"XRef")
                || (stream.dict.is_type(b"Metadata") && !self.encrypt_metadata)
                || crate::document::has_identity_crypt_filter(&stream.dict);
            if !skip {
                stream.data = self.encrypt(r, self.streams, &stream.data).into();
            }
        }
    }

    fn encrypt_strings(&self, r: ObjRef, obj: &mut Object) {
        match obj {
            Object::String(s) => s.bytes = self.encrypt(r, self.strings, &s.bytes),
            Object::Array(a) => a.iter_mut().for_each(|o| self.encrypt_strings(r, o)),
            Object::Dict(d) => d.0.iter_mut().for_each(|(_, o)| self.encrypt_strings(r, o)),
            Object::Stream(s) => s
                .dict
                .0
                .iter_mut()
                .for_each(|(_, o)| self.encrypt_strings(r, o)),
            _ => {}
        }
    }

    pub fn decrypt_string(&self, r: ObjRef, data: &[u8]) -> Vec<u8> {
        self.decrypt(r, self.strings, data)
    }

    pub fn decrypt_stream(&self, r: ObjRef, data: &[u8]) -> Vec<u8> {
        self.decrypt(r, self.streams, data)
    }

    /// Decrypts every string inside `obj`, which was read as object `r`.
    pub fn decrypt_strings(&self, r: ObjRef, obj: &mut Object) {
        match obj {
            Object::String(s) => s.bytes = self.decrypt_string(r, &s.bytes),
            Object::Array(a) => a.iter_mut().for_each(|o| self.decrypt_strings(r, o)),
            Object::Dict(d) => d.0.iter_mut().for_each(|(_, o)| self.decrypt_strings(r, o)),
            Object::Stream(s) => s
                .dict
                .0
                .iter_mut()
                .for_each(|(_, o)| self.decrypt_strings(r, o)),
            _ => {}
        }
    }
}

impl Params {
    fn read(doc: &Document, dict: &Dict) -> Result<Params> {
        let get = |k: &[u8]| doc.get_key(dict, k).unwrap_or(Object::Null);
        let bytes = |k: &[u8]| {
            get(k)
                .as_string()
                .map(|s| s.bytes.clone())
                .unwrap_or_default()
        };
        let filter = get(b"Filter");
        if !filter.as_name().is_some_and(|n| *n == b"Standard") {
            return Err(Error::Unsupported(format!(
                "the {} security handler",
                filter.as_name().map_or("unnamed".into(), |n| n.to_string())
            )));
        }
        let version = get(b"V").as_i64().unwrap_or(0);
        let revision = get(b"R").as_i64().unwrap_or(0);
        let mut key_bits = get(b"Length").as_i64().unwrap_or(40);
        let (strings, streams) = match version {
            1..=3 => (Method::Rc4, Method::Rc4),
            4 | 5 => {
                let filters = get(b"CF");
                let method = |name: Object| -> Result<Method> {
                    let name = name.as_name().map(|n| n.as_bytes().to_vec());
                    let Some(name) = name.filter(|n| n != b"Identity") else {
                        return Ok(Method::Identity);
                    };
                    let cf = filters
                        .as_dict()
                        .and_then(|d| d.get(&name))
                        .map(|o| doc.resolve(o).unwrap_or(Object::Null))
                        .unwrap_or(Object::Null);
                    let cfm = cf
                        .as_dict()
                        .and_then(|d| d.get(b"CFM"))
                        .and_then(Object::as_name)
                        .map(|n| n.as_bytes().to_vec());
                    match cfm.as_deref() {
                        Some(b"V2") => Ok(Method::Rc4),
                        Some(b"AESV2") => Ok(Method::Aes128),
                        Some(b"AESV3") => Ok(Method::Aes256),
                        Some(b"None") | None => Ok(Method::Identity),
                        Some(other) => Err(Error::Unsupported(format!(
                            "the {} crypt method",
                            String::from_utf8_lossy(other)
                        ))),
                    }
                };
                let strings = method(get(b"StrF"))?;
                let streams = method(get(b"StmF"))?;
                if version == 4 && dict.get(b"Length").is_none() {
                    key_bits = 128;
                }
                (strings, streams)
            }
            _ => {
                return Err(Error::Unsupported(format!(
                    "encryption algorithm version {version}"
                )));
            }
        };
        if !(2..=6).contains(&revision) {
            return Err(Error::Unsupported(format!(
                "security handler revision {revision}"
            )));
        }
        // /Length is in bits, but a few writers give bytes.
        if key_bits <= 16 {
            key_bits *= 8;
        }
        let key_len = if revision >= 5 {
            32
        } else if revision == 2 {
            5
        } else {
            (key_bits.clamp(40, 128) / 8) as usize
        };
        let id = doc
            .trailer()
            .get(b"ID")
            .and_then(|o| doc.resolve(o).ok())
            .and_then(|o| {
                o.as_array()
                    .and_then(|a| a.first())
                    .and_then(|s| doc.resolve(s).ok())
                    .and_then(|s| s.as_string().map(|s| s.bytes.clone()))
            })
            .unwrap_or_default();
        Ok(Params {
            revision,
            key_len,
            o: bytes(b"O"),
            u: bytes(b"U"),
            oe: bytes(b"OE"),
            ue: bytes(b"UE"),
            p: get(b"P").as_i64().unwrap_or(-1) as i32,
            id,
            encrypt_metadata: get(b"EncryptMetadata").as_bool().unwrap_or(true),
            strings,
            streams,
        })
    }

    /// The password padded or cut to 32 bytes.
    fn padded(password: &[u8]) -> [u8; 32] {
        let mut out = PAD;
        let n = password.len().min(32);
        out[..n].copy_from_slice(&password[..n]);
        out[n..].copy_from_slice(&PAD[..32 - n]);
        out
    }

    /// Algorithm 2: the file key from a user password (revisions 2 to 4).
    fn rc4_file_key(&self, password: &[u8]) -> Vec<u8> {
        let mut h = Md5::new();
        h.update(&Self::padded(password));
        h.update(&self.o[..self.o.len().min(32)]);
        h.update(&self.p.to_le_bytes());
        h.update(&self.id);
        if self.revision >= 4 && !self.encrypt_metadata {
            h.update(&[0xff; 4]);
        }
        let mut key = h.finish();
        if self.revision >= 3 {
            for _ in 0..50 {
                key = md5(&key[..self.key_len]);
            }
        }
        key[..self.key_len].to_vec()
    }

    fn user_key(&self, password: &[u8]) -> Option<Vec<u8>> {
        if self.revision >= 5 {
            return self.aes256_key(password, false);
        }
        let key = self.rc4_file_key(password);
        let ok = if self.revision == 2 {
            // Algorithm 4.
            self.u.len() >= 32 && rc4(&key, &PAD) == self.u[..32]
        } else {
            // Algorithm 5.
            let mut h = Md5::new();
            h.update(&PAD);
            h.update(&self.id);
            let mut value = rc4(&key, &h.finish());
            for i in 1..=19u8 {
                let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
                value = rc4(&k, &value);
            }
            self.u.len() >= 16 && value[..16] == self.u[..16]
        };
        ok.then_some(key)
    }

    fn owner_key(&self, password: &[u8]) -> Option<Vec<u8>> {
        if self.revision >= 5 {
            return self.aes256_key(password, true);
        }
        // Algorithm 7: the owner password decrypts /O to the user password.
        let mut key = md5(&Self::padded(password));
        if self.revision >= 3 {
            for _ in 0..50 {
                key = md5(&key);
            }
        }
        let key = &key[..self.key_len];
        let o = &self.o[..self.o.len().min(32)];
        let user = if self.revision == 2 {
            rc4(key, o)
        } else {
            let mut value = o.to_vec();
            for i in (0..=19u8).rev() {
                let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
                value = rc4(&k, &value);
            }
            value
        };
        self.user_key(&user)
    }

    /// Algorithms 2.A and 11/12 for revisions 5 and 6.
    fn aes256_key(&self, password: &[u8], owner: bool) -> Option<Vec<u8>> {
        // The password is UTF-8, at most 127 bytes.
        let password = &password[..password.len().min(127)];
        let (hash, encrypted, udata) = if owner {
            (&self.o, &self.oe, self.u.get(..48)?)
        } else {
            (&self.u, &self.ue, &[][..])
        };
        if hash.len() < 48 || encrypted.len() < 32 {
            return None;
        }
        let check = self.hash(password, &hash[32..40], udata);
        if check[..] != hash[..32] {
            return None;
        }
        let k = self.hash(password, &hash[40..48], udata);
        let aes = Aes::new(&k)?;
        Some(aes_cbc_decrypt(&aes, &[0; 16], &encrypted[..32]))
    }

    /// The revision 5 hash (SHA-256) or the revision 6 one (algorithm 2.B).
    fn hash(&self, password: &[u8], salt: &[u8], udata: &[u8]) -> [u8; 32] {
        let mut input = Vec::with_capacity(password.len() + salt.len() + udata.len());
        input.extend_from_slice(password);
        input.extend_from_slice(salt);
        input.extend_from_slice(udata);
        let mut k = sha256(&input).to_vec();
        if self.revision >= 6 {
            let mut round = 0usize;
            loop {
                let mut k1 = Vec::with_capacity(64 * (password.len() + k.len() + udata.len()));
                for _ in 0..64 {
                    k1.extend_from_slice(password);
                    k1.extend_from_slice(&k);
                    k1.extend_from_slice(udata);
                }
                let Some(aes) = Aes::new(&k[..16]) else { break };
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&k[16..32]);
                let e = aes_cbc_encrypt(&aes, &iv, &k1);
                let sum: u32 = e[..16].iter().map(|&b| u32::from(b)).sum();
                k = match sum % 3 {
                    0 => sha256(&e).to_vec(),
                    1 => sha384(&e).to_vec(),
                    _ => sha512(&e).to_vec(),
                };
                round += 1;
                let last = usize::from(e.last().copied().unwrap_or(0));
                if round >= 64 && last + 32 <= round {
                    break;
                }
            }
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&k[..32]);
        out
    }
}
