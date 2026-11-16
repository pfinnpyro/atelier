//! The NIST curves P-256 and P-384 (FIPS 186-5, SP 800-186): key pairs,
//! Diffie-Hellman key agreement (SP 800-56A) and ECDSA signatures with
//! the deterministic nonces of RFC 6979, which FIPS 186-5 approves.
//!
//! Numbers are kept in Montgomery form in 64-bit limbs. Everything done
//! with a secret takes the same steps whatever the secret is: field
//! arithmetic picks its results with masks, points are added with the
//! complete formulas of Renes, Costello and Batina (which have no special
//! cases), and a scalar multiplication adds at every bit and keeps or
//! drops the sum with a mask. Only public values (the fixed exponents
//! used for inversion, and a signature being checked) steer branches.

use std::sync::LazyLock;

use crate::sha2::Hash;

/// The limbs of the wider curve; scratch space is sized for it.
const MAX: usize = 6;

/// Which curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    P256,
    P384,
}

impl Curve {
    /// The size in bytes of a coordinate, a secret key and each half of a
    /// signature.
    pub fn size(self) -> usize {
        match self {
            Curve::P256 => 32,
            Curve::P384 => 48,
        }
    }

    /// The hash of the same strength, used for this curve's nonces.
    pub fn hash(self) -> Hash {
        match self {
            Curve::P256 => Hash::Sha256,
            Curve::P384 => Hash::Sha384,
        }
    }
}

fn adc(a: u64, b: u64, carry: u64) -> (u64, u64) {
    let t = u128::from(a) + u128::from(b) + u128::from(carry);
    (t as u64, (t >> 64) as u64)
}

fn sbb(a: u64, b: u64, borrow: u64) -> (u64, u64) {
    let t = u128::from(a).wrapping_sub(u128::from(b) + u128::from(borrow));
    (t as u64, ((t >> 64) as u64) & 1)
}

/// `acc + a * b + carry`, which always fits in two limbs.
fn mac(acc: u64, a: u64, b: u64, carry: u64) -> (u64, u64) {
    let t = u128::from(acc) + u128::from(a) * u128::from(b) + u128::from(carry);
    (t as u64, (t >> 64) as u64)
}

/// `a` where `mask` is all ones, `b` where it is zero.
fn select<const N: usize>(mask: u64, a: &[u64; N], b: &[u64; N]) -> [u64; N] {
    std::array::from_fn(|i| (a[i] & mask) | (b[i] & !mask))
}

fn is_zero<const N: usize>(a: &[u64; N]) -> bool {
    std::hint::black_box(a.iter().fold(0, |acc, x| acc | x)) == 0
}

fn equal<const N: usize>(a: &[u64; N], b: &[u64; N]) -> bool {
    std::hint::black_box(a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y))) == 0
}

/// Whether `a < b`.
fn less<const N: usize>(a: &[u64; N], b: &[u64; N]) -> bool {
    let mut borrow = 0;
    for i in 0..N {
        (_, borrow) = sbb(a[i], b[i], borrow);
    }
    borrow == 1
}

/// Big-endian bytes as limbs, lowest first. Short input is taken as
/// having zeros in front; `None` when it is too long.
fn limbs<const N: usize>(bytes: &[u8]) -> Option<[u64; N]> {
    if bytes.len() > N * 8 {
        return None;
    }
    let mut out = [0u64; N];
    for (i, b) in bytes.iter().rev().enumerate() {
        out[i / 8] |= u64::from(*b) << (8 * (i % 8));
    }
    Some(out)
}

fn bytes<const N: usize>(a: &[u64; N]) -> Vec<u8> {
    a.iter().rev().flat_map(|l| l.to_be_bytes()).collect()
}

fn clear<const N: usize>(a: &mut [u64; N]) {
    a.fill(0);
    std::hint::black_box(a);
}

/// Arithmetic modulo an odd number of exactly `N` limbs.
struct Modulus<const N: usize> {
    m: [u64; N],
    /// `-1 / m` modulo 2^64.
    inv: u64,
    /// 1 in Montgomery form.
    one: [u64; N],
    /// What a number is multiplied by to put it in Montgomery form.
    r2: [u64; N],
}

impl<const N: usize> Modulus<N> {
    fn new(m: [u64; N]) -> Self {
        // Newton's iteration doubles the correct low bits each time.
        let mut inv = 1u64;
        for _ in 0..6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(m[0].wrapping_mul(inv)));
        }
        let mut this = Self {
            m,
            inv: inv.wrapping_neg(),
            one: [0; N],
            r2: [0; N],
        };
        // Doubling 1 gives 2^(64N) and then 2^(128N), each modulo m.
        let mut x = [0u64; N];
        x[0] = 1;
        for i in 0..128 * N {
            x = this.add(&x, &x);
            if i + 1 == 64 * N {
                this.one = x;
            }
        }
        this.r2 = x;
        this
    }

    fn add(&self, a: &[u64; N], b: &[u64; N]) -> [u64; N] {
        let (mut sum, mut carry) = ([0u64; N], 0);
        for i in 0..N {
            (sum[i], carry) = adc(a[i], b[i], carry);
        }
        let (mut less_m, mut borrow) = ([0u64; N], 0);
        for i in 0..N {
            (less_m[i], borrow) = sbb(sum[i], self.m[i], borrow);
        }
        select((carry | (borrow ^ 1)).wrapping_neg(), &less_m, &sum)
    }

    fn sub(&self, a: &[u64; N], b: &[u64; N]) -> [u64; N] {
        let (mut diff, mut borrow) = ([0u64; N], 0);
        for i in 0..N {
            (diff[i], borrow) = sbb(a[i], b[i], borrow);
        }
        let mask = borrow.wrapping_neg();
        let mut carry = 0;
        for (d, m) in diff.iter_mut().zip(self.m) {
            (*d, carry) = adc(*d, m & mask, carry);
        }
        diff
    }

    /// Montgomery multiplication: `a * b / 2^(64N)` modulo m.
    fn mul(&self, a: &[u64; N], b: &[u64; N]) -> [u64; N] {
        let mut t = [0u64; MAX + 2];
        for &bi in b {
            let mut carry = 0;
            for j in 0..N {
                (t[j], carry) = mac(t[j], a[j], bi, carry);
            }
            (t[N], t[N + 1]) = adc(t[N], carry, 0);
            let q = t[0].wrapping_mul(self.inv);
            let (_, mut carry) = mac(t[0], q, self.m[0], 0);
            for j in 1..N {
                (t[j - 1], carry) = mac(t[j], q, self.m[j], carry);
            }
            let (low, high) = adc(t[N], carry, 0);
            t[N - 1] = low;
            t[N] = t[N + 1] + high;
        }
        let (mut less_m, mut borrow) = ([0u64; N], 0);
        for i in 0..N {
            (less_m[i], borrow) = sbb(t[i], self.m[i], borrow);
        }
        (_, borrow) = sbb(t[N], 0, borrow);
        let mut out = [0u64; N];
        out.copy_from_slice(&t[..N]);
        let out = select((borrow ^ 1).wrapping_neg(), &less_m, &out);
        t.fill(0);
        std::hint::black_box(&t);
        out
    }

    fn mont(&self, a: &[u64; N]) -> [u64; N] {
        self.mul(a, &self.r2)
    }

    fn plain(&self, a: &[u64; N]) -> [u64; N] {
        let mut one = [0u64; N];
        one[0] = 1;
        self.mul(a, &one)
    }

    /// `1 / a` for `a` in Montgomery form, by Fermat's little theorem.
    /// The exponent, m - 2, is public.
    fn invert(&self, a: &[u64; N]) -> [u64; N] {
        let mut e = self.m;
        e[0] -= 2;
        let mut r = self.one;
        for i in (0..64 * N).rev() {
            r = self.mul(&r, &r);
            if (e[i / 64] >> (i % 64)) & 1 == 1 {
                r = self.mul(&r, a);
            }
        }
        r
    }

    /// A number below 2m brought below m.
    fn reduce(&self, a: &[u64; N]) -> [u64; N] {
        self.add(a, &[0; N])
    }
}

/// A point in projective coordinates, each in Montgomery form. The point
/// at infinity has `z` zero.
#[derive(Clone, Copy)]
struct Point<const N: usize> {
    x: [u64; N],
    y: [u64; N],
    z: [u64; N],
}

struct Params<const N: usize> {
    curve: Curve,
    /// The field the coordinates are in.
    p: Modulus<N>,
    /// The number of points, which scalars are taken modulo.
    n: Modulus<N>,
    /// The curve is y^2 = x^3 - 3x + b.
    b: [u64; N],
    g: Point<N>,
}

impl<const N: usize> Params<N> {
    fn new(curve: Curve, p: &str, n: &str, b: &str, gx: &str, gy: &str) -> Self {
        let read = |s: &str| -> [u64; N] {
            let raw: Vec<u8> = (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex constant"))
                .collect();
            limbs(&raw).expect("constant fits")
        };
        let p = Modulus::new(read(p));
        let g = Point {
            x: p.mont(&read(gx)),
            y: p.mont(&read(gy)),
            z: p.one,
        };
        Self {
            curve,
            b: p.mont(&read(b)),
            g,
            n: Modulus::new(read(n)),
            p,
        }
    }

    fn infinity(&self) -> Point<N> {
        Point {
            x: [0; N],
            y: self.p.one,
            z: [0; N],
        }
    }

    /// The sum of any two points, the same point twice included
    /// (Renes, Costello and Batina 2016, algorithm 4, for a = -3).
    fn add(&self, p: &Point<N>, q: &Point<N>) -> Point<N> {
        let f = &self.p;
        let (x1, y1, z1) = (&p.x, &p.y, &p.z);
        let (x2, y2, z2) = (&q.x, &q.y, &q.z);
        let mut t0 = f.mul(x1, x2);
        let mut t1 = f.mul(y1, y2);
        let mut t2 = f.mul(z1, z2);
        let mut t3 = f.add(x1, y1);
        let mut t4 = f.add(x2, y2);
        t3 = f.mul(&t3, &t4);
        t4 = f.add(&t0, &t1);
        t3 = f.sub(&t3, &t4);
        t4 = f.add(y1, z1);
        let mut x3 = f.add(y2, z2);
        t4 = f.mul(&t4, &x3);
        x3 = f.add(&t1, &t2);
        t4 = f.sub(&t4, &x3);
        x3 = f.add(x1, z1);
        let mut y3 = f.add(x2, z2);
        x3 = f.mul(&x3, &y3);
        y3 = f.add(&t0, &t2);
        y3 = f.sub(&x3, &y3);
        let mut z3 = f.mul(&self.b, &t2);
        x3 = f.sub(&y3, &z3);
        z3 = f.add(&x3, &x3);
        x3 = f.add(&x3, &z3);
        z3 = f.sub(&t1, &x3);
        x3 = f.add(&t1, &x3);
        y3 = f.mul(&self.b, &y3);
        t1 = f.add(&t2, &t2);
        t2 = f.add(&t1, &t2);
        y3 = f.sub(&y3, &t2);
        y3 = f.sub(&y3, &t0);
        t1 = f.add(&y3, &y3);
        y3 = f.add(&t1, &y3);
        t1 = f.add(&t0, &t0);
        t0 = f.add(&t1, &t0);
        t0 = f.sub(&t0, &t2);
        t1 = f.mul(&t4, &y3);
        t2 = f.mul(&t0, &y3);
        y3 = f.mul(&x3, &z3);
        y3 = f.add(&y3, &t2);
        x3 = f.mul(&t3, &x3);
        x3 = f.sub(&x3, &t1);
        z3 = f.mul(&t4, &z3);
        t1 = f.mul(&t3, &t0);
        z3 = f.add(&z3, &t1);
        Point {
            x: x3,
            y: y3,
            z: z3,
        }
    }

    /// `k` times `p`: at each bit the running point is doubled, `p` is
    /// added, and a mask says whether the sum is kept.
    fn mul(&self, k: &[u64; N], p: &Point<N>) -> Point<N> {
        let mut r = self.infinity();
        for i in (0..64 * N).rev() {
            r = self.add(&r, &r);
            let sum = self.add(&r, p);
            let mask = ((k[i / 64] >> (i % 64)) & 1).wrapping_neg();
            r = Point {
                x: select(mask, &sum.x, &r.x),
                y: select(mask, &sum.y, &r.y),
                z: select(mask, &sum.z, &r.z),
            };
        }
        r
    }

    /// The ordinary coordinates of a point, as plain numbers; `None` for
    /// the point at infinity.
    fn affine(&self, p: &Point<N>) -> Option<([u64; N], [u64; N])> {
        if is_zero(&p.z) {
            return None;
        }
        let f = &self.p;
        let mut inv = f.invert(&p.z);
        let out = (f.plain(&f.mul(&p.x, &inv)), f.plain(&f.mul(&p.y, &inv)));
        clear(&mut inv);
        Some(out)
    }

    /// Reads `04 || x || y`, refusing anything that is not a point on
    /// the curve (SP 800-56A, 5.6.2.3.3; the curves have cofactor 1, so a
    /// point on the curve is in the right group).
    fn point(&self, sec1: &[u8]) -> Option<Point<N>> {
        let size = N * 8;
        if sec1.len() != 1 + 2 * size || sec1[0] != 4 {
            return None;
        }
        let x: [u64; N] = limbs(&sec1[1..1 + size])?;
        let y: [u64; N] = limbs(&sec1[1 + size..])?;
        let f = &self.p;
        if !less(&x, &f.m) || !less(&y, &f.m) {
            return None;
        }
        let (x, y) = (f.mont(&x), f.mont(&y));
        // y^2 = x^3 - 3x + b
        let x2 = f.mul(&x, &x);
        let mut right = f.mul(&x2, &x);
        for _ in 0..3 {
            right = f.sub(&right, &x);
        }
        right = f.add(&right, &self.b);
        equal(&f.mul(&y, &y), &right).then_some(Point { x, y, z: f.one })
    }

    fn sec1(&self, p: &Point<N>) -> Option<Vec<u8>> {
        let (x, y) = self.affine(p)?;
        let mut out = vec![4u8];
        out.extend(bytes(&x));
        out.extend(bytes(&y));
        Some(out)
    }

    /// A secret key or nonce: exactly the curve's size and from 1 to
    /// n - 1.
    fn scalar(&self, secret: &[u8]) -> Option<[u64; N]> {
        if secret.len() != N * 8 {
            return None;
        }
        let k: [u64; N] = limbs(secret)?;
        (!is_zero(&k) && less(&k, &self.n.m)).then_some(k)
    }

    /// The number a signature is made over: the leftmost bits of the
    /// hash, as many as the curve has, modulo n.
    fn hashed(&self, digest: &[u8]) -> [u64; N] {
        let take = digest.len().min(N * 8);
        let z: [u64; N] = limbs(&digest[..take]).expect("cut to fit");
        self.n.reduce(&z)
    }

    fn public_key(&self, secret: &[u8]) -> Option<Vec<u8>> {
        let mut d = self.scalar(secret)?;
        let out = self.sec1(&self.mul(&d, &self.g));
        clear(&mut d);
        out
    }

    fn ecdh(&self, secret: &[u8], peer: &[u8]) -> Option<Vec<u8>> {
        let mut d = self.scalar(secret)?;
        let shared = self.mul(&d, &self.point(peer)?);
        clear(&mut d);
        let (mut x, mut y) = self.affine(&shared)?;
        let out = bytes(&x);
        clear(&mut x);
        clear(&mut y);
        Some(out)
    }

    fn sign(&self, secret: &[u8], digest: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let n = &self.n;
        let mut d = self.scalar(secret)?;
        let z = self.hashed(digest);
        let mut d_mont = n.mont(&d);
        let z_mont = n.mont(&z);
        let mut nonces = Nonces::new(self.curve.hash(), secret, &bytes(&z));
        let signature = loop {
            let mut k_bytes = nonces.next(N * 8);
            let k = self.scalar(&k_bytes);
            k_bytes.fill(0);
            let Some(mut k) = k else { continue };
            let (x, _) = self.affine(&self.mul(&k, &self.g))?;
            let r = n.reduce(&x);
            let mut k_inv = n.invert(&n.mont(&k));
            clear(&mut k);
            let rd = n.mul(&n.mont(&r), &d_mont);
            let s = n.plain(&n.mul(&k_inv, &n.add(&z_mont, &rd)));
            clear(&mut k_inv);
            if !is_zero(&r) && !is_zero(&s) {
                break (bytes(&r), bytes(&s));
            }
        };
        clear(&mut d);
        clear(&mut d_mont);
        Some(signature)
    }

    fn verify(&self, public: &[u8], digest: &[u8], r: &[u8], s: &[u8]) -> bool {
        let n = &self.n;
        let (Some(q), Some(r), Some(s)) = (self.point(public), limbs::<N>(r), limbs::<N>(s)) else {
            return false;
        };
        if is_zero(&r) || is_zero(&s) || !less(&r, &n.m) || !less(&s, &n.m) {
            return false;
        }
        let w = n.invert(&n.mont(&s));
        let u1 = n.plain(&n.mul(&n.mont(&self.hashed(digest)), &w));
        let u2 = n.plain(&n.mul(&n.mont(&r), &w));
        let sum = self.add(&self.mul(&u1, &self.g), &self.mul(&u2, &q));
        match self.affine(&sum) {
            Some((x, _)) => equal(&n.reduce(&x), &r),
            None => false,
        }
    }
}

/// The nonces RFC 6979 (section 3.2) works out from the key and the
/// hash being signed, so that no signature depends on a random number
/// generator. For both curves the size of the numbers is a whole number
/// of bytes, which keeps the conversions simple.
struct Nonces {
    hash: Hash,
    k: Vec<u8>,
    v: Vec<u8>,
    first: bool,
}

impl Nonces {
    /// `hashed` is the hash already reduced modulo n, at full size.
    fn new(hash: Hash, secret: &[u8], hashed: &[u8]) -> Self {
        let mut k = vec![0u8; hash.size()];
        let mut v = vec![1u8; hash.size()];
        for step in [0u8, 1] {
            let mut m = v.clone();
            m.push(step);
            m.extend_from_slice(secret);
            m.extend_from_slice(hashed);
            k = hash.hmac(&k, &m);
            m.fill(0);
            std::hint::black_box(&m);
            v = hash.hmac(&k, &v);
        }
        Self {
            hash,
            k,
            v,
            first: true,
        }
    }

    fn next(&mut self, size: usize) -> Vec<u8> {
        if !self.first {
            let mut m = self.v.clone();
            m.push(0);
            self.k = self.hash.hmac(&self.k, &m);
            self.v = self.hash.hmac(&self.k, &self.v);
        }
        self.first = false;
        let mut t = Vec::with_capacity(size + self.v.len());
        while t.len() < size {
            self.v = self.hash.hmac(&self.k, &self.v);
            t.extend_from_slice(&self.v);
        }
        t.truncate(size);
        t
    }
}

impl Drop for Nonces {
    fn drop(&mut self) {
        self.k.fill(0);
        self.v.fill(0);
        std::hint::black_box((&self.k, &self.v));
    }
}

static P256: LazyLock<Params<4>> = LazyLock::new(|| {
    Params::new(
        Curve::P256,
        "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
        "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551",
        "5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b",
        "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
        "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
    )
});

static P384: LazyLock<Params<6>> = LazyLock::new(|| {
    Params::new(
        Curve::P384,
        "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe\
         ffffffff0000000000000000ffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf\
         581a0db248b0a77aecec196accc52973",
        "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875a\
         c656398d8a2ed19d2a85c8edd3ec2aef",
        "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a38\
         5502f25dbf55296c3a545e3872760ab7",
        "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c0\
         0a60b1ce1d7e819d7a431d7c90ea0e5f",
    )
});

/// Runs `$call` on whichever curve's parameters `$curve` names.
macro_rules! on {
    ($curve:expr, $params:ident => $call:expr) => {
        match $curve {
            Curve::P256 => {
                let $params = &*P256;
                $call
            }
            Curve::P384 => {
                let $params = &*P384;
                $call
            }
        }
    };
}

/// Whether `secret` can be a secret key: the curve's size in bytes and a
/// number from 1 to n - 1. A new key is made by filling that many bytes
/// from an approved random generator and trying again on `false`
/// (FIPS 186-5, A.2.2), which for these curves all but never happens.
pub fn ec_secret_is_valid(curve: Curve, secret: &[u8]) -> bool {
    on!(curve, c => c.scalar(secret).is_some())
}

/// The public key of a secret key, as `04 || x || y`.
pub fn ec_public_key(curve: Curve, secret: &[u8]) -> Option<Vec<u8>> {
    on!(curve, c => c.public_key(secret))
}

/// Whether `public` is `04 || x || y` for a point on the curve.
pub fn ec_public_is_valid(curve: Curve, public: &[u8]) -> bool {
    on!(curve, c => c.point(public).is_some())
}

/// The secret two parties share: the x coordinate of our secret key
/// times their public key. `None` when their key is not a point on the
/// curve. It is to be put through a key derivation function, not used
/// as a key.
pub fn ecdh(curve: Curve, secret: &[u8], peer: &[u8]) -> Option<Vec<u8>> {
    on!(curve, c => c.ecdh(secret, peer))
}

/// Signs a hash. Returns `r` and `s`, each the curve's size.
pub fn ecdsa_sign(curve: Curve, secret: &[u8], digest: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    on!(curve, c => c.sign(secret, digest))
}

/// Whether `r` and `s` are a signature of the hash by `public`. Leading
/// zeros on `r` and `s` may be missing.
pub fn ecdsa_verify(curve: Curve, public: &[u8], digest: &[u8], r: &[u8], s: &[u8]) -> bool {
    on!(curve, c => c.verify(public, digest, r, s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hex, unhex};

    /// Key pairs, their shared secret and a signature of "sample", from
    /// OpenSSL.
    struct Known {
        curve: Curve,
        a: &'static str,
        b: &'static str,
        public_a: &'static str,
        public_b: &'static str,
        shared: &'static str,
        r: &'static str,
        s: &'static str,
    }

    const KNOWN: [Known; 2] = [
        Known {
            curve: Curve::P256,
            a: "61eeb30419b47be66e4e867a2c381dd8cbefd1bf5c9707d627cb48dc7cdebfe9",
            b: "55af895fe7bbcda415c02ae7fee1df2d69375c34c7652e4b5ed470ababc4fa19",
            public_a: "495cae78a6abce49a25168065891dc7eb19167d426f77afb26e2f6bb03157d47\
                       a993ea8138d5d01b45212295d6233af12fb6f7cf4b4839becb736271f4461eb3",
            public_b: "76c445e2555c3fecad9cfa134ce7ecdb5ef97fc3c63e98e4e3d0187a6bf6c802\
                       2e2efbccc9c53ac0a46431c447a778444c69be9e9972906113a5f3a79cc88043",
            shared: "f23dcde5a31708aad5b40d8a4e092f302f73d7da058803666617b9971f6e0875",
            r: "4dd9fdcc01fe19816f66d3f1d39beb39d5354f575f3b4a066422d6aace576edf",
            s: "bdafb0369e318d6fbaf1bbdddbf9b85563d0096ad0ea44c642bfa086230ee5ea",
        },
        Known {
            curve: Curve::P384,
            a: "1cf81d33f4f9a8465f6b44d587d70f4bfdb535045512bd844caf811acdc3ea27\
                ca53931390e4a030e223a137034a17ab",
            b: "3cf27b6ed03a2341123d67a34c57c542580cd90f6c4d212eda361602765182e3\
                f6e8426379c2086d83c376ece4b1c268",
            public_a: "479b4e1f1d4842d37063f846e3fd302b6fc9d9e0c4f8d960f86984e3204ca110\
                       70b6b7466d136b09f79ff7579af451136c050683fe84342b983a6cba7fd15a1b\
                       80b7616250d94d7b6fe6b28e0615ffdcdfbcaa923c9a07c2b77c2bc9f88e84f6",
            public_b: "de96bbb8f151941283a1adba36a36ed8ebfd267f66e4b2462ed670f81bbdcff6\
                       c87c39c55d761387448297e424cca9de598b94caa96a7563b15f5c4f1dbacfe0\
                       89a68361136314e9a27968fba7b3012c1f5bf68478e162624fa803516a74edbd",
            shared: "3c714954b5da1c56cae2ee50595b3cda7b32a440e1eb9f25a0262d4e9f2d757b\
                     ba190372d4b499c35f5badfc30673070",
            r: "ca0c7e25a65d0a1cf14bfc929ada1646c726f4aad686df2f8c26f46ab3af1fa5\
                bbf04ebf48c6097df12774f33d0a33d3",
            s: "e900608d4d1cd5623c6854b2f7eb87d02bf8f4c0f619e85f4c1aea9844f981c1\
                db620355da01fb5a8cb15e608b171d6b",
        },
    ];

    fn point(s: &str) -> Vec<u8> {
        let mut p = vec![4u8];
        p.extend(unhex(s));
        p
    }

    #[test]
    fn public_keys_and_shared_secrets_agree_with_openssl() {
        for k in &KNOWN {
            let (a, b) = (unhex(k.a), unhex(k.b));
            assert_eq!(ec_public_key(k.curve, &a), Some(point(k.public_a)));
            assert_eq!(ec_public_key(k.curve, &b), Some(point(k.public_b)));
            assert_eq!(
                ecdh(k.curve, &a, &point(k.public_b)).map(|z| hex(&z)),
                Some(k.shared.to_string())
            );
            assert_eq!(
                ecdh(k.curve, &b, &point(k.public_a)).map(|z| hex(&z)),
                Some(k.shared.to_string())
            );
        }
    }

    #[test]
    fn signatures_agree_with_openssl() {
        for k in &KNOWN {
            let digest = k.curve.hash().digest(b"sample");
            let (r, s) = ecdsa_sign(k.curve, &unhex(k.a), &digest).unwrap();
            assert_eq!((hex(&r), hex(&s)), (k.r.to_string(), k.s.to_string()));
            let public = point(k.public_a);
            assert!(ecdsa_verify(k.curve, &public, &digest, &r, &s));
            // Another hash, key or signature does not pass.
            let other = k.curve.hash().digest(b"simple");
            assert!(!ecdsa_verify(k.curve, &public, &other, &r, &s));
            assert!(!ecdsa_verify(k.curve, &point(k.public_b), &digest, &r, &s));
            assert!(!ecdsa_verify(k.curve, &public, &digest, &s, &r));
            let mut bent = s.clone();
            bent[5] ^= 4;
            assert!(!ecdsa_verify(k.curve, &public, &digest, &r, &bent));
            let zero = vec![0u8; k.curve.size()];
            assert!(!ecdsa_verify(k.curve, &public, &digest, &zero, &s));
            assert!(!ecdsa_verify(k.curve, &public, &digest, &r, &zero));
        }
    }

    #[test]
    fn rfc_6979_p256_sample() {
        // Appendix A.2.5, SHA-256, message "sample".
        let x = unhex("c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721");
        let digest = Hash::Sha256.digest(b"sample");
        let (r, s) = ecdsa_sign(Curve::P256, &x, &digest).unwrap();
        assert_eq!(
            hex(&r),
            "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716"
        );
        assert_eq!(
            hex(&s),
            "f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8"
        );
    }

    #[test]
    fn a_hash_of_another_size_still_signs() {
        for k in &KNOWN {
            for hash in [Hash::Sha256, Hash::Sha384, Hash::Sha512] {
                let digest = hash.digest(b"another size");
                let (r, s) = ecdsa_sign(k.curve, &unhex(k.a), &digest).unwrap();
                assert!(ecdsa_verify(k.curve, &point(k.public_a), &digest, &r, &s));
            }
        }
    }

    #[test]
    fn the_group_has_the_order_it_should() {
        for (curve, n) in [
            (Curve::P256, bytes(&P256.n.m)),
            (Curve::P384, bytes(&P384.n.m)),
        ] {
            // n is not a secret key; n - 1 is, and its public key is the
            // base point mirrored.
            assert!(!ec_secret_is_valid(curve, &n));
            assert!(!ec_secret_is_valid(curve, &vec![0; curve.size()]));
            assert!(!ec_secret_is_valid(curve, &n[1..]));
            let mut last = n.clone();
            *last.last_mut().unwrap() -= 1;
            let mut one = vec![0u8; curve.size()];
            *one.last_mut().unwrap() = 1;
            let g = ec_public_key(curve, &one).unwrap();
            let mirrored = ec_public_key(curve, &last).unwrap();
            assert_eq!(g[..=curve.size()], mirrored[..=curve.size()]);
            assert_ne!(g, mirrored);
        }
        // n times the base point is the point at infinity.
        assert!(P256.affine(&P256.mul(&P256.n.m, &P256.g)).is_none());
        assert!(P384.affine(&P384.mul(&P384.n.m, &P384.g)).is_none());
    }

    #[test]
    fn points_off_the_curve_are_refused() {
        for k in &KNOWN {
            let good = point(k.public_b);
            assert!(ec_public_is_valid(k.curve, &good));
            let mut bent = good.clone();
            *bent.last_mut().unwrap() ^= 1;
            assert!(!ec_public_is_valid(k.curve, &bent));
            assert_eq!(ecdh(k.curve, &unhex(k.a), &bent), None);
            // Compressed, short and oversized encodings.
            let mut compressed = good[..=k.curve.size()].to_vec();
            compressed[0] = 2;
            assert!(!ec_public_is_valid(k.curve, &compressed));
            assert!(!ec_public_is_valid(k.curve, &good[..good.len() - 1]));
            let mut high = vec![4u8];
            high.extend(vec![0xff; 2 * k.curve.size()]);
            assert!(!ec_public_is_valid(k.curve, &high));
            let mut origin = vec![4u8];
            origin.extend(vec![0; 2 * k.curve.size()]);
            assert!(!ec_public_is_valid(k.curve, &origin));
        }
    }

    #[test]
    fn field_arithmetic_holds_together() {
        let f = &P256.p;
        let a = f.mont(&limbs(&unhex(KNOWN[0].a)).unwrap());
        let b = f.mont(&limbs(&unhex(KNOWN[0].b)).unwrap());
        assert!(equal(&f.mul(&a, &f.invert(&a)), &f.one));
        assert!(equal(&f.sub(&f.add(&a, &b), &b), &a));
        assert!(equal(&f.add(&f.sub(&a, &b), &b), &a));
        assert!(equal(
            &f.mul(&a, &f.add(&b, &f.one)),
            &f.add(&f.mul(&a, &b), &a)
        ));
        // The largest value survives the round trip.
        let mut top = f.m;
        top[0] -= 1;
        assert!(equal(&f.plain(&f.mont(&top)), &top));
        assert!(is_zero(&f.add(&top, &f.plain(&f.one))));
    }
}
