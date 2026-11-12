//! AES (FIPS 197) with 128- and 256-bit keys, and CBC mode.

const fn make_sbox() -> [u8; 256] {
    let mut sbox = [0u8; 256];
    let (mut p, mut q) = (1u8, 1u8);
    loop {
        // p times 3 in GF(2^8).
        p ^= (p << 1) ^ if p & 0x80 != 0 { 0x1b } else { 0 };
        // q divided by 3.
        q ^= q << 1;
        q ^= q << 2;
        q ^= q << 4;
        if q & 0x80 != 0 {
            q ^= 0x09;
        }
        let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
        sbox[p as usize] = x ^ 0x63;
        if p == 1 {
            break;
        }
    }
    sbox[0] = 0x63;
    sbox
}

const fn invert(sbox: &[u8; 256]) -> [u8; 256] {
    let mut inv = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        inv[sbox[i] as usize] = i as u8;
        i += 1;
    }
    inv
}

const SBOX: [u8; 256] = make_sbox();
const INV_SBOX: [u8; 256] = invert(&SBOX);

#[inline]
fn xtime(b: u8) -> u8 {
    (b << 1) ^ if b & 0x80 != 0 { 0x1b } else { 0 }
}

#[inline]
fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    r
}

/// An expanded AES key.
#[derive(Clone)]
pub struct Aes {
    round_keys: Vec<[u8; 16]>,
}

impl Drop for Aes {
    /// The round keys give the key away, so they do not outlive it.
    fn drop(&mut self) {
        for k in &mut self.round_keys {
            k.fill(0);
        }
        std::hint::black_box(&self.round_keys);
    }
}

impl Aes {
    /// Expands a 16-, 24- or 32-byte key; `None` for any other length.
    pub fn new(key: &[u8]) -> Option<Self> {
        let nk = match key.len() {
            16 | 24 | 32 => key.len() / 4,
            _ => return None,
        };
        let rounds = nk + 6;
        let total = 4 * (rounds + 1);
        let mut w: Vec<[u8; 4]> = key
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        let mut rcon = 1u8;
        for i in nk..total {
            let mut t = w[i - 1];
            if i % nk == 0 {
                t = [
                    SBOX[t[1] as usize],
                    SBOX[t[2] as usize],
                    SBOX[t[3] as usize],
                    SBOX[t[0] as usize],
                ];
                t[0] ^= rcon;
                rcon = xtime(rcon);
            } else if nk > 6 && i % nk == 4 {
                t = t.map(|b| SBOX[b as usize]);
            }
            let prev = w[i - nk];
            w.push([
                prev[0] ^ t[0],
                prev[1] ^ t[1],
                prev[2] ^ t[2],
                prev[3] ^ t[3],
            ]);
        }
        let round_keys = w
            .chunks_exact(4)
            .map(|c| {
                let mut k = [0u8; 16];
                for (j, word) in c.iter().enumerate() {
                    k[j * 4..j * 4 + 4].copy_from_slice(word);
                }
                k
            })
            .collect();
        Some(Self { round_keys })
    }

    fn rounds(&self) -> usize {
        self.round_keys.len() - 1
    }

    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        add(block, &self.round_keys[0]);
        for r in 1..=self.rounds() {
            for b in block.iter_mut() {
                *b = SBOX[*b as usize];
            }
            shift_rows(block);
            if r != self.rounds() {
                for c in 0..4 {
                    let col = [
                        block[c * 4],
                        block[c * 4 + 1],
                        block[c * 4 + 2],
                        block[c * 4 + 3],
                    ];
                    let all = col[0] ^ col[1] ^ col[2] ^ col[3];
                    for i in 0..4 {
                        block[c * 4 + i] = col[i] ^ all ^ xtime(col[i] ^ col[(i + 1) % 4]);
                    }
                }
            }
            add(block, &self.round_keys[r]);
        }
    }

    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        let rounds = self.rounds();
        add(block, &self.round_keys[rounds]);
        for r in (0..rounds).rev() {
            inv_shift_rows(block);
            for b in block.iter_mut() {
                *b = INV_SBOX[*b as usize];
            }
            add(block, &self.round_keys[r]);
            if r != 0 {
                for c in 0..4 {
                    let col = [
                        block[c * 4],
                        block[c * 4 + 1],
                        block[c * 4 + 2],
                        block[c * 4 + 3],
                    ];
                    for i in 0..4 {
                        block[c * 4 + i] = mul(col[i], 14)
                            ^ mul(col[(i + 1) % 4], 11)
                            ^ mul(col[(i + 2) % 4], 13)
                            ^ mul(col[(i + 3) % 4], 9);
                    }
                }
            }
        }
    }
}

fn add(block: &mut [u8; 16], key: &[u8; 16]) {
    for (b, k) in block.iter_mut().zip(key) {
        *b ^= k;
    }
}

/// Bytes are column-major: byte `c * 4 + r` is row r of column c.
fn shift_rows(b: &mut [u8; 16]) {
    let s = *b;
    for c in 0..4 {
        for r in 0..4 {
            b[c * 4 + r] = s[((c + r) % 4) * 4 + r];
        }
    }
}

fn inv_shift_rows(b: &mut [u8; 16]) {
    let s = *b;
    for c in 0..4 {
        for r in 0..4 {
            b[((c + r) % 4) * 4 + r] = s[c * 4 + r];
        }
    }
}

/// CBC decryption of whole blocks; a trailing partial block is dropped.
/// No padding is removed.
pub fn aes_cbc_decrypt(key: &Aes, iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut prev = *iv;
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        let cipher = block;
        key.decrypt_block(&mut block);
        for (b, p) in block.iter_mut().zip(prev) {
            *b ^= p;
        }
        out.extend_from_slice(&block);
        prev = cipher;
    }
    out
}

/// CBC encryption of whole blocks; the caller pads. A trailing partial
/// block is dropped.
pub fn aes_cbc_encrypt(key: &Aes, iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut prev = *iv;
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(16) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = chunk[i] ^ prev[i];
        }
        key.encrypt_block(&mut block);
        out.extend_from_slice(&block);
        prev = block;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn sbox_is_right() {
        assert_eq!(SBOX[0x00], 0x63);
        assert_eq!(SBOX[0x01], 0x7c);
        assert_eq!(SBOX[0x53], 0xed);
        assert_eq!(SBOX[0xff], 0x16);
        assert_eq!(INV_SBOX[0x63], 0x00);
    }

    #[test]
    fn fips_197_appendix_c() {
        let plain = unhex("00112233445566778899aabbccddeeff");
        for (key, want) in [
            (
                "000102030405060708090a0b0c0d0e0f",
                "69c4e0d86a7b0430d8cdb78070b4c55a",
            ),
            (
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                "8ea2b7ca516745bfeafc49904b496089",
            ),
        ] {
            let aes = Aes::new(&unhex(key)).unwrap();
            let mut block: [u8; 16] = plain.clone().try_into().unwrap();
            aes.encrypt_block(&mut block);
            assert_eq!(hex(&block), want);
            aes.decrypt_block(&mut block);
            assert_eq!(block.to_vec(), plain);
        }
    }

    #[test]
    fn cbc_round_trip() {
        let aes = Aes::new(&[7; 16]).unwrap();
        let iv = [9; 16];
        let data: Vec<u8> = (0..64).collect();
        let c = aes_cbc_encrypt(&aes, &iv, &data);
        assert_ne!(c, data);
        assert_eq!(aes_cbc_decrypt(&aes, &iv, &c), data);
    }

    #[test]
    fn bad_key_length() {
        assert!(Aes::new(&[0; 15]).is_none());
    }
}
