//! PDF functions (ISO 32000-2, 7.10): sampled, exponential, stitching and
//! PostScript calculator functions. Shadings use them to map a parameter to
//! a colour, and Separation and DeviceN spaces to map tints to an
//! alternate colour space.

use atelier_syntax::{Document, Object};

/// Nested stitching functions deeper than this are rejected.
const MAX_DEPTH: usize = 8;
/// Sampled functions with more samples than this are rejected.
const MAX_SAMPLES: usize = 1 << 24;
/// The PostScript operand stack limit (PDF 2.0, annex C).
const MAX_STACK: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// `[min, max]` per input.
    domain: Vec<(f32, f32)>,
    /// `[min, max]` per output, when given.
    range: Option<Vec<(f32, f32)>>,
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Sampled {
        size: Vec<usize>,
        encode: Vec<(f32, f32)>,
        decode: Vec<(f32, f32)>,
        /// Samples scaled to 0..1, output-major within each sample point.
        samples: Vec<f32>,
        outputs: usize,
    },
    Exponential {
        c0: Vec<f32>,
        c1: Vec<f32>,
        n: f32,
    },
    Stitching {
        functions: Vec<Function>,
        bounds: Vec<f32>,
        encode: Vec<(f32, f32)>,
    },
    PostScript(Vec<Ps>),
}

fn floats(obj: Option<Object>) -> Option<Vec<f32>> {
    let obj = obj?;
    let items = obj.as_array()?;
    Some(
        items
            .iter()
            .map(|o| o.as_f64().unwrap_or(0.0) as f32)
            .collect(),
    )
}

fn pairs(v: &[f32]) -> Vec<(f32, f32)> {
    v.chunks_exact(2).map(|p| (p[0], p[1])).collect()
}

fn interpolate(x: f32, (x0, x1): (f32, f32), (y0, y1): (f32, f32)) -> f32 {
    if x1 == x0 {
        y0
    } else {
        y0 + (x - x0) * (y1 - y0) / (x1 - x0)
    }
}

impl Function {
    /// Parses a function dictionary or stream.
    pub fn parse(doc: &Document, obj: &Object) -> Option<Function> {
        Self::parse_depth(doc, obj, 0)
    }

    fn parse_depth(doc: &Document, obj: &Object, depth: usize) -> Option<Function> {
        if depth > MAX_DEPTH {
            return None;
        }
        let obj = doc.resolve(obj).ok()?;
        let dict = obj.as_dict()?;
        let get = |k: &[u8]| doc.get_key(dict, k).ok();
        let domain = pairs(&floats(get(b"Domain"))?);
        if domain.is_empty() {
            return None;
        }
        let range = floats(get(b"Range")).map(|r| pairs(&r));
        let kind = match get(b"FunctionType")?.as_i64()? {
            0 => {
                let Object::Stream(stream) = &obj else {
                    return None;
                };
                let range = range.clone()?;
                let size: Vec<usize> = floats(get(b"Size"))?
                    .iter()
                    .map(|&s| s.max(1.0) as usize)
                    .collect();
                if size.len() != domain.len() {
                    return None;
                }
                let bps = get(b"BitsPerSample")?.as_i64()?;
                if ![1, 2, 4, 8, 12, 16, 24, 32].contains(&bps) {
                    return None;
                }
                let outputs = range.len();
                let points = size.iter().try_fold(1usize, |acc, &s| acc.checked_mul(s))?;
                let count = points.checked_mul(outputs)?;
                if count > MAX_SAMPLES {
                    return None;
                }
                let data = doc.decode_stream(stream).ok()?.data;
                let max = ((1u64 << bps) - 1) as f32;
                let mut samples = Vec::with_capacity(count);
                let mut bit = 0usize;
                for _ in 0..count {
                    let mut v = 0u64;
                    for _ in 0..bps {
                        let byte = data.get(bit / 8).copied().unwrap_or(0);
                        v = (v << 1) | u64::from((byte >> (7 - bit % 8)) & 1);
                        bit += 1;
                    }
                    samples.push(v as f32 / max);
                }
                let encode = floats(get(b"Encode"))
                    .map(|e| pairs(&e))
                    .filter(|e| e.len() == size.len())
                    .unwrap_or_else(|| size.iter().map(|&s| (0.0, (s - 1) as f32)).collect());
                let decode = floats(get(b"Decode"))
                    .map(|d| pairs(&d))
                    .filter(|d| d.len() == outputs)
                    .unwrap_or_else(|| range.clone());
                Kind::Sampled {
                    size,
                    encode,
                    decode,
                    samples,
                    outputs,
                }
            }
            2 => {
                let c0 = floats(get(b"C0")).unwrap_or_else(|| vec![0.0]);
                let c1 = floats(get(b"C1")).unwrap_or_else(|| vec![1.0]);
                if c0.len() != c1.len() {
                    return None;
                }
                let n = get(b"N")?.as_f64()? as f32;
                Kind::Exponential { c0, c1, n }
            }
            3 => {
                let list = get(b"Functions")?;
                let functions: Vec<Function> = list
                    .as_array()?
                    .iter()
                    .map(|f| Self::parse_depth(doc, f, depth + 1))
                    .collect::<Option<_>>()?;
                if functions.is_empty() {
                    return None;
                }
                let bounds = floats(get(b"Bounds")).unwrap_or_default();
                if bounds.len() + 1 != functions.len() {
                    return None;
                }
                let encode = floats(get(b"Encode"))
                    .map(|e| pairs(&e))
                    .filter(|e| e.len() == functions.len())
                    .unwrap_or_else(|| vec![(0.0, 1.0); functions.len()]);
                Kind::Stitching {
                    functions,
                    bounds,
                    encode,
                }
            }
            4 => {
                let Object::Stream(stream) = &obj else {
                    return None;
                };
                range.as_ref()?;
                let data = doc.decode_stream(stream).ok()?.data;
                Kind::PostScript(parse_ps(&data)?)
            }
            _ => return None,
        };
        Some(Function {
            domain,
            range,
            kind,
        })
    }

    pub fn inputs(&self) -> usize {
        self.domain.len()
    }

    /// The number of outputs, when the function says.
    pub fn outputs(&self) -> Option<usize> {
        if let Some(r) = &self.range {
            return Some(r.len());
        }
        match &self.kind {
            Kind::Exponential { c0, .. } => Some(c0.len()),
            Kind::Stitching { functions, .. } => functions[0].outputs(),
            _ => None,
        }
    }

    pub fn domain(&self) -> &[(f32, f32)] {
        &self.domain
    }

    /// Evaluates the function. Inputs are clipped to the domain and outputs
    /// to the range; missing inputs count as 0.
    pub fn eval(&self, input: &[f32]) -> Vec<f32> {
        let x: Vec<f32> = self
            .domain
            .iter()
            .enumerate()
            .map(|(i, &(lo, hi))| {
                let v = input.get(i).copied().unwrap_or(0.0);
                let v = if v.is_nan() { lo } else { v };
                v.clamp(lo.min(hi), hi.max(lo))
            })
            .collect();
        let mut out = match &self.kind {
            Kind::Sampled {
                size,
                encode,
                decode,
                samples,
                outputs,
            } => sampled(&x, &self.domain, size, encode, decode, samples, *outputs),
            Kind::Exponential { c0, c1, n } => {
                let p = x[0].powf(*n);
                let p = if p.is_finite() { p } else { 0.0 };
                c0.iter().zip(c1).map(|(a, b)| a + p * (b - a)).collect()
            }
            Kind::Stitching {
                functions,
                bounds,
                encode,
            } => {
                let v = x[0];
                let k = bounds.iter().take_while(|&&b| v >= b).count();
                let lo = if k == 0 {
                    self.domain[0].0
                } else {
                    bounds[k - 1]
                };
                let hi = bounds.get(k).copied().unwrap_or(self.domain[0].1);
                functions[k].eval(&[interpolate(v, (lo, hi), encode[k])])
            }
            Kind::PostScript(program) => {
                let outputs = self.range.as_ref().map_or(0, Vec::len);
                run_ps(program, &x, outputs)
            }
        };
        if let Some(range) = &self.range {
            out.resize(range.len(), 0.0);
            for (v, &(lo, hi)) in out.iter_mut().zip(range) {
                *v = if v.is_nan() {
                    lo
                } else {
                    v.clamp(lo.min(hi), hi.max(lo))
                };
            }
        }
        out
    }
}

/// A set of functions used together: either one function with n outputs or
/// n one-output functions, as shadings allow.
#[derive(Debug, Clone, PartialEq)]
pub enum Functions {
    One(Function),
    Many(Vec<Function>),
}

impl Functions {
    pub fn parse(doc: &Document, obj: &Object) -> Option<Functions> {
        let resolved = doc.resolve(obj).ok()?;
        match &resolved {
            Object::Array(items) => Some(Functions::Many(
                items
                    .iter()
                    .map(|f| Function::parse(doc, f))
                    .collect::<Option<_>>()?,
            )),
            _ => Function::parse(doc, &resolved).map(Functions::One),
        }
    }

    pub fn eval(&self, input: &[f32]) -> Vec<f32> {
        match self {
            Functions::One(f) => f.eval(input),
            Functions::Many(list) => list
                .iter()
                .map(|f| f.eval(input).first().copied().unwrap_or(0.0))
                .collect(),
        }
    }
}

fn sampled(
    x: &[f32],
    domain: &[(f32, f32)],
    size: &[usize],
    encode: &[(f32, f32)],
    decode: &[(f32, f32)],
    samples: &[f32],
    outputs: usize,
) -> Vec<f32> {
    let m = x.len().min(size.len());
    // Each input's position in the sample grid, split into a cell and a
    // fraction for multilinear interpolation.
    let mut cell = Vec::with_capacity(m);
    let mut frac = Vec::with_capacity(m);
    for i in 0..m {
        let e = interpolate(x[i], domain[i], encode[i]).clamp(0.0, (size[i] - 1) as f32);
        let c = (e.floor() as usize).min(size[i].saturating_sub(2));
        cell.push(c);
        frac.push(if size[i] > 1 { e - c as f32 } else { 0.0 });
    }
    let mut out = vec![0.0f32; outputs];
    // Inputs beyond a few would need 2^m corners; interpolate only the first
    // eight and take the nearest sample for the rest.
    let interp = m.min(8);
    for corner in 0..(1usize << interp) {
        let mut weight = 1.0f32;
        let mut index = 0usize;
        let mut stride = 1usize;
        for i in 0..m {
            let up = i < interp && corner >> i & 1 == 1;
            let pos = if i < interp {
                weight *= if up { frac[i] } else { 1.0 - frac[i] };
                (cell[i] + usize::from(up)).min(size[i] - 1)
            } else {
                (cell[i] as f32 + frac[i]).round() as usize
            };
            index += pos * stride;
            stride *= size[i];
        }
        if weight == 0.0 {
            continue;
        }
        for (j, o) in out.iter_mut().enumerate() {
            *o += weight * samples.get(index * outputs + j).copied().unwrap_or(0.0);
        }
    }
    out.iter()
        .zip(decode)
        .map(|(&v, &(lo, hi))| lo + v * (hi - lo))
        .collect()
}

// ----- PostScript calculator functions ----------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Ps {
    Num(f64),
    Bool(bool),
    Op(Op),
    /// `{ then } if`
    If(Vec<Ps>),
    /// `{ then } { else } ifelse`
    IfElse(Vec<Ps>, Vec<Ps>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Abs,
    Add,
    Atan,
    Ceiling,
    Cos,
    Cvi,
    Cvr,
    Div,
    Exp,
    Floor,
    Idiv,
    Ln,
    Log,
    Mod,
    Mul,
    Neg,
    Round,
    Sin,
    Sqrt,
    Sub,
    Truncate,
    And,
    Bitshift,
    Eq,
    Ge,
    Gt,
    Le,
    Lt,
    Ne,
    Not,
    Or,
    Xor,
    Copy,
    Dup,
    Exch,
    Index,
    Pop,
    Roll,
}

fn op(name: &str) -> Option<Op> {
    Some(match name {
        "abs" => Op::Abs,
        "add" => Op::Add,
        "atan" => Op::Atan,
        "ceiling" => Op::Ceiling,
        "cos" => Op::Cos,
        "cvi" => Op::Cvi,
        "cvr" => Op::Cvr,
        "div" => Op::Div,
        "exp" => Op::Exp,
        "floor" => Op::Floor,
        "idiv" => Op::Idiv,
        "ln" => Op::Ln,
        "log" => Op::Log,
        "mod" => Op::Mod,
        "mul" => Op::Mul,
        "neg" => Op::Neg,
        "round" => Op::Round,
        "sin" => Op::Sin,
        "sqrt" => Op::Sqrt,
        "sub" => Op::Sub,
        "truncate" => Op::Truncate,
        "and" => Op::And,
        "bitshift" => Op::Bitshift,
        "eq" => Op::Eq,
        "ge" => Op::Ge,
        "gt" => Op::Gt,
        "le" => Op::Le,
        "lt" => Op::Lt,
        "ne" => Op::Ne,
        "not" => Op::Not,
        "or" => Op::Or,
        "xor" => Op::Xor,
        "copy" => Op::Copy,
        "dup" => Op::Dup,
        "exch" => Op::Exch,
        "index" => Op::Index,
        "pop" => Op::Pop,
        "roll" => Op::Roll,
        _ => return None,
    })
}

/// Splits the program text into tokens: braces and words.
fn tokens(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut comment = false;
    for &b in data {
        if comment {
            comment = b != b'\n' && b != b'\r';
            continue;
        }
        match b {
            b'%' => {
                comment = true;
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            b'{' | b'}' => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                out.push((b as char).to_string());
            }
            b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\0' => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(b as char),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

fn parse_ps(data: &[u8]) -> Option<Vec<Ps>> {
    let toks = tokens(data);
    let mut pos = 0;
    if toks.first().map(String::as_str) != Some("{") {
        return None;
    }
    pos += 1;
    parse_block(&toks, &mut pos, 0)
}

/// Parses tokens up to the matching `}`.
fn parse_block(toks: &[String], pos: &mut usize, depth: usize) -> Option<Vec<Ps>> {
    if depth > 64 {
        return None;
    }
    let mut out = Vec::new();
    let mut pending: Vec<Vec<Ps>> = Vec::new();
    while let Some(t) = toks.get(*pos) {
        *pos += 1;
        match t.as_str() {
            "}" => {
                return pending.is_empty().then_some(out);
            }
            "{" => pending.push(parse_block(toks, pos, depth + 1)?),
            "if" => {
                let then = pending.pop()?;
                if !pending.is_empty() {
                    return None;
                }
                out.push(Ps::If(then));
            }
            "ifelse" => {
                let otherwise = pending.pop()?;
                let then = pending.pop()?;
                if !pending.is_empty() {
                    return None;
                }
                out.push(Ps::IfElse(then, otherwise));
            }
            "true" => out.push(Ps::Bool(true)),
            "false" => out.push(Ps::Bool(false)),
            word => {
                if !pending.is_empty() {
                    return None;
                }
                if let Some(o) = op(word) {
                    out.push(Ps::Op(o));
                } else {
                    out.push(Ps::Num(word.parse::<f64>().ok()?));
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy)]
enum Val {
    Num(f64),
    Bool(bool),
}

impl Val {
    fn num(self) -> f64 {
        match self {
            Val::Num(v) => v,
            Val::Bool(b) => f64::from(u8::from(b)),
        }
    }
}

fn run_ps(program: &[Ps], input: &[f32], outputs: usize) -> Vec<f32> {
    let mut stack: Vec<Val> = input.iter().map(|&v| Val::Num(f64::from(v))).collect();
    // A malformed program just yields what is left on the stack.
    let _ = exec(program, &mut stack);
    let n = stack.len();
    let start = n.saturating_sub(outputs);
    let mut out: Vec<f32> = stack[start..].iter().map(|v| v.num() as f32).collect();
    while out.len() < outputs {
        out.insert(0, 0.0);
    }
    out
}

fn exec(program: &[Ps], s: &mut Vec<Val>) -> Option<()> {
    for item in program {
        match item {
            Ps::Num(v) => s.push(Val::Num(*v)),
            Ps::Bool(b) => s.push(Val::Bool(*b)),
            Ps::If(then) => {
                if let Val::Bool(true) = s.pop()? {
                    exec(then, s)?;
                }
            }
            Ps::IfElse(then, otherwise) => match s.pop()? {
                Val::Bool(true) => exec(then, s)?,
                _ => exec(otherwise, s)?,
            },
            Ps::Op(o) => apply(*o, s)?,
        }
        if s.len() > MAX_STACK {
            return None;
        }
    }
    Some(())
}

fn apply(o: Op, s: &mut Vec<Val>) -> Option<()> {
    let num = |s: &mut Vec<Val>| s.pop().map(Val::num);
    let int = |v: f64| v as i64;
    match o {
        Op::Abs => {
            let a = num(s)?;
            s.push(Val::Num(a.abs()));
        }
        Op::Neg => {
            let a = num(s)?;
            s.push(Val::Num(-a));
        }
        Op::Ceiling => {
            let a = num(s)?;
            s.push(Val::Num(a.ceil()));
        }
        Op::Floor => {
            let a = num(s)?;
            s.push(Val::Num(a.floor()));
        }
        Op::Round => {
            let a = num(s)?;
            s.push(Val::Num((a + 0.5).floor()));
        }
        Op::Truncate | Op::Cvi => {
            let a = num(s)?;
            s.push(Val::Num(a.trunc()));
        }
        Op::Cvr => {
            let a = num(s)?;
            s.push(Val::Num(a));
        }
        Op::Sqrt => {
            let a = num(s)?;
            s.push(Val::Num(a.max(0.0).sqrt()));
        }
        Op::Sin => {
            let a = num(s)?;
            s.push(Val::Num(a.to_radians().sin()));
        }
        Op::Cos => {
            let a = num(s)?;
            s.push(Val::Num(a.to_radians().cos()));
        }
        Op::Ln => {
            let a = num(s)?;
            s.push(Val::Num(if a > 0.0 { a.ln() } else { 0.0 }));
        }
        Op::Log => {
            let a = num(s)?;
            s.push(Val::Num(if a > 0.0 { a.log10() } else { 0.0 }));
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Exp | Op::Atan => {
            let b = num(s)?;
            let a = num(s)?;
            let v = match o {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                Op::Div => {
                    if b == 0.0 {
                        0.0
                    } else {
                        a / b
                    }
                }
                Op::Exp => a.powf(b),
                _ => {
                    // atan num den: angle in degrees, 0..360.
                    let d = a.atan2(b).to_degrees();
                    if d < 0.0 { d + 360.0 } else { d }
                }
            };
            s.push(Val::Num(if v.is_finite() { v } else { 0.0 }));
        }
        Op::Idiv | Op::Mod => {
            let b = int(num(s)?);
            let a = int(num(s)?);
            let v = if b == 0 {
                0
            } else if o == Op::Idiv {
                a.wrapping_div(b)
            } else {
                a.wrapping_rem(b)
            };
            s.push(Val::Num(v as f64));
        }
        Op::Bitshift => {
            let shift = int(num(s)?);
            let a = int(num(s)?);
            let v = if shift >= 0 {
                a.checked_shl(shift.min(63) as u32).unwrap_or(0)
            } else {
                a >> (-shift).min(63)
            };
            s.push(Val::Num(v as f64));
        }
        Op::Eq | Op::Ne | Op::Ge | Op::Gt | Op::Le | Op::Lt => {
            let b = num(s)?;
            let a = num(s)?;
            let v = match o {
                Op::Eq => a == b,
                Op::Ne => a != b,
                Op::Ge => a >= b,
                Op::Gt => a > b,
                Op::Le => a <= b,
                _ => a < b,
            };
            s.push(Val::Bool(v));
        }
        Op::And | Op::Or | Op::Xor => {
            let b = s.pop()?;
            let a = s.pop()?;
            s.push(match (a, b) {
                (Val::Bool(a), Val::Bool(b)) => Val::Bool(match o {
                    Op::And => a & b,
                    Op::Or => a | b,
                    _ => a ^ b,
                }),
                (a, b) => {
                    let (a, b) = (int(a.num()), int(b.num()));
                    Val::Num(match o {
                        Op::And => a & b,
                        Op::Or => a | b,
                        _ => a ^ b,
                    } as f64)
                }
            });
        }
        Op::Not => {
            let a = s.pop()?;
            s.push(match a {
                Val::Bool(b) => Val::Bool(!b),
                Val::Num(v) => Val::Num(!int(v) as f64),
            });
        }
        Op::Dup => {
            let a = *s.last()?;
            s.push(a);
        }
        Op::Pop => {
            s.pop()?;
        }
        Op::Exch => {
            let n = s.len();
            if n < 2 {
                return None;
            }
            s.swap(n - 1, n - 2);
        }
        Op::Copy => {
            let k = int(num(s)?);
            if k < 0 || k as usize > s.len() {
                return None;
            }
            let start = s.len() - k as usize;
            let copy: Vec<Val> = s[start..].to_vec();
            s.extend(copy);
        }
        Op::Index => {
            let k = int(num(s)?);
            if k < 0 || k as usize >= s.len() {
                return None;
            }
            let v = s[s.len() - 1 - k as usize];
            s.push(v);
        }
        Op::Roll => {
            let j = int(num(s)?);
            let n = int(num(s)?);
            if n < 0 || n as usize > s.len() {
                return None;
            }
            if n > 0 {
                let start = s.len() - n as usize;
                let shift = j.rem_euclid(n) as usize;
                s[start..].rotate_right(shift);
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        // Functions are parsed from dictionaries; a document is only needed
        // to resolve references, so an empty-but-valid file will do.
        let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
                    2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\n\
                    trailer << /Root 1 0 R >>\n%%EOF\n";
        Document::open(pdf.to_vec()).unwrap()
    }

    fn parse(doc: &Document, src: &str) -> Function {
        let obj = atelier_syntax::Parser::new(src.as_bytes())
            .parse_object()
            .unwrap();
        Function::parse(doc, &obj).unwrap()
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn exponential() {
        let d = doc();
        let f = parse(
            &d,
            "<< /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >>",
        );
        assert!(close(&f.eval(&[0.25]), &[0.75, 0.0, 0.25]));
        // Inputs are clipped to the domain.
        assert!(close(&f.eval(&[2.0]), &[0.0, 0.0, 1.0]));
        let sq = parse(&d, "<< /FunctionType 2 /Domain [0 1] /N 2 >>");
        assert!(close(&sq.eval(&[0.5]), &[0.25]));
    }

    #[test]
    fn stitching() {
        let d = doc();
        let f = parse(
            &d,
            "<< /FunctionType 3 /Domain [0 1] /Bounds [0.5] /Encode [0 1 1 0] \
             /Functions [ << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> \
                          << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> ] >>",
        );
        assert!(close(&f.eval(&[0.0]), &[0.0]));
        assert!(close(&f.eval(&[0.25]), &[0.5]));
        assert!(close(&f.eval(&[0.5]), &[1.0]));
        assert!(close(&f.eval(&[0.75]), &[0.5]));
        assert!(close(&f.eval(&[1.0]), &[0.0]));
    }

    #[test]
    fn postscript() {
        let ps = |src: &str, input: &[f32], outputs: usize| {
            let prog = parse_ps(src.as_bytes()).unwrap();
            run_ps(&prog, input, outputs)
        };
        // A typical tint transform: tint to CMYK.
        assert!(close(
            &ps("{ dup 0.5 mul exch 0 exch 0 }", &[0.8], 4),
            &[0.4, 0.0, 0.8, 0.0]
        ));
        assert!(close(
            &ps("{ dup 0.5 gt { pop 1 } { pop 0 } ifelse }", &[0.7], 1),
            &[1.0]
        ));
        assert!(close(&ps("{ 0.3 lt { 5 } if }", &[0.1], 1), &[5.0]));
        assert!(close(&ps("{ 1 2 3 3 1 roll }", &[], 3), &[3.0, 1.0, 2.0]));
        assert!(close(&ps("{ 7 2 idiv 7 2 mod }", &[], 2), &[3.0, 1.0]));
        assert!(close(&ps("{ 0 1 atan 90 sin }", &[], 2), &[0.0, 1.0]));
        assert!(close(
            &ps("{ 1 1 2 index }", &[5.0], 4),
            &[5.0, 1.0, 1.0, 5.0]
        ));
        // Malformed programs do not panic.
        assert!(parse_ps(b"{ { 1 } }").is_none());
        assert!(parse_ps(b"{ 1 2 ").is_none());
        let _ = ps("{ pop pop pop add exch roll }", &[1.0], 3);
        let _ = ps("{ 1 -5 roll 99 copy 99 index }", &[1.0], 1);
    }

    #[test]
    fn sampled_interpolates() {
        // Two inputs, one output: a 2 x 2 grid of 8-bit samples.
        let data = [0u8, 255, 255, 0];
        let f = Function {
            domain: vec![(0.0, 1.0), (0.0, 1.0)],
            range: Some(vec![(0.0, 1.0)]),
            kind: Kind::Sampled {
                size: vec![2, 2],
                encode: vec![(0.0, 1.0), (0.0, 1.0)],
                decode: vec![(0.0, 1.0)],
                samples: data.iter().map(|&b| f32::from(b) / 255.0).collect(),
                outputs: 1,
            },
        };
        assert!(close(&f.eval(&[0.0, 0.0]), &[0.0]));
        assert!(close(&f.eval(&[1.0, 0.0]), &[1.0]));
        assert!(close(&f.eval(&[0.5, 0.5]), &[0.5]));
        assert!(close(&f.eval(&[0.5, 0.0]), &[0.5]));
    }
}
