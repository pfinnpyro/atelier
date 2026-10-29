//! Shadings (ISO 32000-2, 8.7.4.5): smooth colour fills. Function-based,
//! axial and radial shadings are evaluated per pixel; the four mesh types
//! are broken into triangles and filled with interpolated colour.

use atelier_raster::{Color, Coverage, Mask, Pixmap, Point, Transform};
use atelier_syntax::{Dict, Document, Object};

use crate::colorspace::{self, ColorSpace};
use crate::function::Functions;

/// Colour lookup table size for axial, radial and function-driven meshes.
const LUT_SIZE: usize = 1024;
/// Meshes with more triangles than this are cut short.
const MAX_TRIANGLES: usize = 2_000_000;
/// Patches are split into at most this many rows and columns.
const MAX_PATCH_STEPS: usize = 64;

pub struct Shading {
    space: ColorSpace,
    function: Option<Functions>,
    /// The colour outside the shaded area, used when the shading paints a
    /// pattern.
    pub background: Option<Vec<f32>>,
    /// In shading space.
    pub bbox: Option<[f64; 4]>,
    kind: Kind,
}

impl Shading {
    /// The colour space the shading's colours are in.
    pub(crate) fn space(&self) -> &ColorSpace {
        &self.space
    }
}

enum Kind {
    Function {
        domain: [f32; 4],
        matrix: Transform,
    },
    Axial {
        coords: [f64; 4],
        domain: (f32, f32),
        extend: (bool, bool),
    },
    Radial {
        coords: [f64; 6],
        domain: (f32, f32),
        extend: (bool, bool),
    },
    Mesh {
        triangles: Vec<Triangle>,
        /// The range of the single parametric component, when a function
        /// maps it to colour.
        t_range: (f32, f32),
    },
}

type Triangle = [Vertex; 3];

#[derive(Debug, Clone)]
struct Vertex {
    p: Point,
    c: Vec<f32>,
}

fn floats(doc: &Document, dict: &Dict, key: &[u8]) -> Option<Vec<f64>> {
    let obj = doc.get_key(dict, key).ok()?;
    Some(obj.as_array()?.iter().filter_map(Object::as_f64).collect())
}

impl Shading {
    /// Parses a shading dictionary or stream. `warn` hears about anything
    /// skipped.
    pub fn parse(
        doc: &Document,
        obj: &Object,
        resources: &Dict,
        warn: &mut dyn FnMut(String),
    ) -> Option<Shading> {
        let obj = doc.resolve(obj).ok()?;
        let dict = obj.as_dict()?;
        let kind_num = doc.get_key(dict, b"ShadingType").ok()?.as_i64()?;
        let space = colorspace::parse(doc, &doc.get_key(dict, b"ColorSpace").ok()?, resources)?;
        let function = dict.get(b"Function").and_then(|f| Functions::parse(doc, f));
        let background =
            floats(doc, dict, b"Background").map(|v| v.iter().map(|&x| x as f32).collect());
        let bbox = floats(doc, dict, b"BBox")
            .filter(|v| v.len() == 4)
            .map(|v| [v[0], v[1], v[2], v[3]]);
        let domain = |default: [f64; 2]| {
            let d = floats(doc, dict, b"Domain").filter(|d| d.len() >= 2);
            let d = d.as_deref().unwrap_or(&default);
            (d[0] as f32, d[1] as f32)
        };
        let extend = || {
            let e = doc
                .get_key(dict, b"Extend")
                .ok()
                .and_then(|e| e.as_array().map(<[Object]>::to_vec))
                .unwrap_or_default();
            let b = |i: usize| matches!(e.get(i), Some(Object::Bool(true)));
            (b(0), b(1))
        };
        let kind = match kind_num {
            1 => {
                function.as_ref()?;
                let d = floats(doc, dict, b"Domain")
                    .filter(|d| d.len() == 4)
                    .unwrap_or_else(|| vec![0.0, 1.0, 0.0, 1.0]);
                let matrix = floats(doc, dict, b"Matrix")
                    .filter(|m| m.len() == 6)
                    .map_or(Transform::IDENTITY, |m| {
                        Transform::from_array([m[0], m[1], m[2], m[3], m[4], m[5]])
                    });
                Kind::Function {
                    domain: [d[0] as f32, d[1] as f32, d[2] as f32, d[3] as f32],
                    matrix,
                }
            }
            2 => {
                function.as_ref()?;
                let c = floats(doc, dict, b"Coords").filter(|c| c.len() == 4)?;
                Kind::Axial {
                    coords: [c[0], c[1], c[2], c[3]],
                    domain: domain([0.0, 1.0]),
                    extend: extend(),
                }
            }
            3 => {
                function.as_ref()?;
                let c = floats(doc, dict, b"Coords").filter(|c| c.len() == 6)?;
                Kind::Radial {
                    coords: [c[0], c[1], c[2], c[3], c[4], c[5]],
                    domain: domain([0.0, 1.0]),
                    extend: extend(),
                }
            }
            4..=7 => {
                let Object::Stream(stream) = &obj else {
                    return None;
                };
                let data = doc.decode_stream(stream).ok()?.data;
                let n = if function.is_some() {
                    1
                } else {
                    space.components()
                };
                let (triangles, t_range) = read_mesh(doc, dict, kind_num, &data, n, warn)?;
                Kind::Mesh { triangles, t_range }
            }
            _ => return None,
        };
        Some(Shading {
            space,
            function,
            background,
            bbox,
            kind,
        })
    }

    /// The colour of a point given its shading components (or parameter).
    fn color(&self, input: &[f32]) -> Option<[u8; 4]> {
        let comps = match &self.function {
            Some(f) => f.eval(input),
            None => input.to_vec(),
        };
        self.space.to_rgb(&comps).map(Color::to_premultiplied)
    }

    /// The background as a solid colour.
    pub fn background_color(&self) -> Option<Color> {
        self.space.to_rgb(self.background.as_deref()?)
    }

    /// A table of colours across a parameter range.
    fn lut(&self, (t0, t1): (f32, f32)) -> Vec<Option<[u8; 4]>> {
        (0..LUT_SIZE)
            .map(|i| {
                let s = i as f32 / (LUT_SIZE - 1) as f32;
                self.color(&[t0 + s * (t1 - t0)])
            })
            .collect()
    }

    /// Paints the shading through `coverage` (and `clip`), with shading
    /// space mapped to device space by `to_device`.
    pub fn paint(
        &self,
        pixmap: &mut Pixmap,
        coverage: &Coverage,
        clip: Option<&Mask>,
        to_device: &Transform,
        alpha: f32,
    ) {
        let Some(inverse) = to_device.invert() else {
            return;
        };
        let at = |x: usize, y: usize| inverse.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
        match &self.kind {
            Kind::Function { domain, matrix } => {
                let Some(inv_m) = matrix.invert() else { return };
                pixmap.fill_coverage_with(coverage, clip, alpha, |x, y| {
                    let p = inv_m.apply(at(x, y));
                    let (u, v) = (p.x as f32, p.y as f32);
                    let inside = u >= domain[0].min(domain[1])
                        && u <= domain[0].max(domain[1])
                        && v >= domain[2].min(domain[3])
                        && v <= domain[2].max(domain[3]);
                    if inside { self.color(&[u, v]) } else { None }
                });
            }
            Kind::Axial {
                coords,
                domain,
                extend,
            } => {
                let lut = self.lut(*domain);
                let a = Point::new(coords[0], coords[1]);
                let d = Point::new(coords[2], coords[3]) - a;
                let len2 = d.dot(d);
                pixmap.fill_coverage_with(coverage, clip, alpha, |x, y| {
                    let s = if len2 == 0.0 {
                        0.0
                    } else {
                        (at(x, y) - a).dot(d) / len2
                    };
                    lookup(&lut, s, *extend)
                });
            }
            Kind::Radial {
                coords,
                domain,
                extend,
            } => {
                let lut = self.lut(*domain);
                let c0 = Point::new(coords[0], coords[1]);
                let r0 = coords[2];
                let dc = Point::new(coords[3], coords[4]) - c0;
                let dr = coords[5] - r0;
                pixmap.fill_coverage_with(coverage, clip, alpha, |x, y| {
                    let s = radial_param(at(x, y) - c0, dc, r0, dr, *extend)?;
                    lookup(&lut, s, *extend)
                });
            }
            Kind::Mesh { triangles, t_range } => {
                self.paint_mesh(
                    pixmap, coverage, clip, to_device, alpha, triangles, *t_range,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_mesh(
        &self,
        pixmap: &mut Pixmap,
        coverage: &Coverage,
        clip: Option<&Mask>,
        to_device: &Transform,
        alpha: f32,
        triangles: &[Triangle],
        t_range: (f32, f32),
    ) {
        // Fill a buffer over the area by pixel centres, so neighbouring
        // triangles meet without seams, then composite it through the
        // (anti-aliased) coverage.
        let (ax, ay, aw, ah) = (coverage.x, coverage.y, coverage.width, coverage.height);
        if aw == 0 || ah == 0 {
            return;
        }
        let mut buf: Vec<Option<[u8; 4]>> = vec![None; aw * ah];
        let lut = self.function.as_ref().map(|_| self.lut(t_range));
        for tri in triangles {
            let p: Vec<Point> = tri.iter().map(|v| to_device.apply(v.p)).collect();
            // Colours to interpolate: the parameter when a function maps it,
            // otherwise each vertex's colour converted up front.
            let vals: Vec<[f32; 4]> = tri
                .iter()
                .map(|v| match &lut {
                    Some(_) => [v.c.first().copied().unwrap_or(0.0), 0.0, 0.0, 0.0],
                    None => self.color(&v.c).map_or([0.0; 4], |c| c.map(f32::from)),
                })
                .collect();
            let min_x = p.iter().map(|q| q.x).fold(f64::INFINITY, f64::min);
            let max_x = p.iter().map(|q| q.x).fold(f64::NEG_INFINITY, f64::max);
            let min_y = p.iter().map(|q| q.y).fold(f64::INFINITY, f64::min);
            let max_y = p.iter().map(|q| q.y).fold(f64::NEG_INFINITY, f64::max);
            if !(min_x.is_finite() && max_x.is_finite() && min_y.is_finite() && max_y.is_finite()) {
                continue;
            }
            let x0 = ((min_x - 0.5).ceil().max(ax as f64)) as usize;
            let x1 = ((max_x - 0.5).floor().min((ax + aw) as f64 - 1.0)) as isize;
            let y0 = ((min_y - 0.5).ceil().max(ay as f64)) as usize;
            let y1 = ((max_y - 0.5).floor().min((ay + ah) as f64 - 1.0)) as isize;
            if (x1 as f64) < x0 as f64 || (y1 as f64) < y0 as f64 {
                continue;
            }
            let area = (p[1] - p[0]).cross(p[2] - p[0]);
            if area.abs() < 1e-12 {
                continue;
            }
            for y in y0..=y1 as usize {
                for x in x0..=x1 as usize {
                    let q = Point::new(x as f64 + 0.5, y as f64 + 0.5);
                    let w0 = (p[2] - p[1]).cross(q - p[1]) / area;
                    let w1 = (p[0] - p[2]).cross(q - p[2]) / area;
                    let w2 = 1.0 - w0 - w1;
                    let eps = -1e-9;
                    if w0 < eps || w1 < eps || w2 < eps {
                        continue;
                    }
                    let mix = |i: usize| {
                        (w0 * f64::from(vals[0][i])
                            + w1 * f64::from(vals[1][i])
                            + w2 * f64::from(vals[2][i])) as f32
                    };
                    let px = match &lut {
                        Some(lut) => {
                            let span = t_range.1 - t_range.0;
                            let s = if span == 0.0 {
                                0.0
                            } else {
                                (mix(0) - t_range.0) / span
                            };
                            lookup(lut, f64::from(s), (true, true))
                        }
                        None => Some([0, 1, 2, 3].map(|i| mix(i).round().clamp(0.0, 255.0) as u8)),
                    };
                    buf[(y - ay) * aw + (x - ax)] = px;
                }
            }
        }
        pixmap.fill_coverage_with(coverage, clip, alpha, |x, y| {
            buf.get((y - ay) * aw + (x - ax)).copied().flatten()
        });
    }
}

/// Looks up a colour for parameter `s` (0..1 across the shading), applying
/// the extend flags outside that range.
fn lookup(lut: &[Option<[u8; 4]>], s: f64, extend: (bool, bool)) -> Option<[u8; 4]> {
    if !s.is_finite() {
        return None;
    }
    let s = if s < 0.0 {
        if !extend.0 {
            return None;
        }
        0.0
    } else if s > 1.0 {
        if !extend.1 {
            return None;
        }
        1.0
    } else {
        s
    };
    lut[((s * (LUT_SIZE - 1) as f64).round() as usize).min(LUT_SIZE - 1)]
}

/// The radial shading parameter at offset `p` from the first centre: the
/// largest `s` whose circle passes through the point and has a
/// non-negative radius, within the extended range.
fn radial_param(p: Point, dc: Point, r0: f64, dr: f64, extend: (bool, bool)) -> Option<f64> {
    let a = dc.dot(dc) - dr * dr;
    let b = p.dot(dc) + r0 * dr;
    let c = p.dot(p) - r0 * r0;
    let ok = |s: f64| r0 + s * dr >= 0.0 && (s >= 0.0 || extend.0) && (s <= 1.0 || extend.1);
    if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            return None;
        }
        let s = c / (2.0 * b);
        return ok(s).then_some(s);
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    let (s1, s2) = ((b + root) / a, (b - root) / a);
    let (hi, lo) = if s1 > s2 { (s1, s2) } else { (s2, s1) };
    if ok(hi) {
        Some(hi)
    } else if ok(lo) {
        Some(lo)
    } else {
        None
    }
}

// ----- Mesh data ---------------------------------------------------------

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> Option<u64> {
        if self.pos + n as usize > self.data.len() * 8 {
            return None;
        }
        let mut v = 0u64;
        for _ in 0..n {
            let byte = self.data[self.pos / 8];
            v = (v << 1) | u64::from((byte >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        Some(v)
    }

    fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    fn done(&self) -> bool {
        self.pos + 8 > self.data.len() * 8
    }
}

struct MeshFormat {
    coord_bits: u32,
    comp_bits: u32,
    flag_bits: u32,
    /// Decode ranges: x, y, then each component.
    decode: Vec<(f64, f64)>,
    components: usize,
}

impl MeshFormat {
    fn value(&self, bits: &mut Bits, nbits: u32, range: (f64, f64)) -> Option<f64> {
        let raw = bits.read(nbits)?;
        let max = if nbits >= 64 {
            u64::MAX
        } else {
            (1u64 << nbits) - 1
        } as f64;
        Some(range.0 + raw as f64 * (range.1 - range.0) / max)
    }

    fn point(&self, bits: &mut Bits) -> Option<Point> {
        let x = self.value(bits, self.coord_bits, self.decode[0])?;
        let y = self.value(bits, self.coord_bits, self.decode[1])?;
        Some(Point::new(x, y))
    }

    fn color(&self, bits: &mut Bits) -> Option<Vec<f32>> {
        (0..self.components)
            .map(|i| {
                self.value(bits, self.comp_bits, self.decode[2 + i])
                    .map(|v| v as f32)
            })
            .collect()
    }
}

fn read_mesh(
    doc: &Document,
    dict: &Dict,
    kind: i64,
    data: &[u8],
    components: usize,
    warn: &mut dyn FnMut(String),
) -> Option<(Vec<Triangle>, (f32, f32))> {
    let int = |k: &[u8]| doc.get_key(dict, k).ok().and_then(|o| o.as_i64());
    let coord_bits = int(b"BitsPerCoordinate")? as u32;
    let comp_bits = int(b"BitsPerComponent")? as u32;
    let flag_bits = if matches!(kind, 4 | 6 | 7) {
        int(b"BitsPerFlag")? as u32
    } else {
        0
    };
    if !(1..=32).contains(&coord_bits) || !(1..=16).contains(&comp_bits) || flag_bits > 8 {
        return None;
    }
    let d = floats(doc, dict, b"Decode")?;
    if d.len() < 4 + 2 * components {
        return None;
    }
    let decode: Vec<(f64, f64)> = d.chunks_exact(2).map(|c| (c[0], c[1])).collect();
    let t_range = (decode[2].0 as f32, decode[2].1 as f32);
    let fmt = MeshFormat {
        coord_bits,
        comp_bits,
        flag_bits,
        decode,
        components,
    };
    let mut bits = Bits { data, pos: 0 };
    let mut tris: Vec<Triangle> = Vec::new();
    match kind {
        4 => {
            // The first vertex with flag 0 and the two after it make a
            // triangle; flags 1 and 2 add a triangle on an edge of the last.
            let mut pending: Vec<Vertex> = Vec::new();
            let mut last: Option<Triangle> = None;
            while !bits.done() && tris.len() < MAX_TRIANGLES {
                let Some(flag) = bits.read(fmt.flag_bits) else {
                    break;
                };
                let (Some(p), Some(c)) = (fmt.point(&mut bits), fmt.color(&mut bits)) else {
                    break;
                };
                bits.align();
                let v = Vertex { p, c };
                if flag == 0 || !pending.is_empty() {
                    pending.push(v);
                    if pending.len() == 3 {
                        let t: Triangle =
                            [pending[0].clone(), pending[1].clone(), pending[2].clone()];
                        pending.clear();
                        tris.push(t.clone());
                        last = Some(t);
                    }
                    continue;
                }
                let t = match (flag, &last) {
                    (1, Some(l)) => [l[1].clone(), l[2].clone(), v],
                    (2, Some(l)) => [l[0].clone(), l[2].clone(), v],
                    _ => {
                        warn("a mesh shading has a bad edge flag".into());
                        break;
                    }
                };
                tris.push(t.clone());
                last = Some(t);
            }
        }
        5 => {
            let per_row = int(b"VerticesPerRow")?;
            if per_row < 2 {
                return None;
            }
            let per_row = per_row as usize;
            let mut rows: Vec<Vec<Vertex>> = Vec::new();
            let mut row = Vec::new();
            while !bits.done() {
                let (Some(p), Some(c)) = (fmt.point(&mut bits), fmt.color(&mut bits)) else {
                    break;
                };
                row.push(Vertex { p, c });
                if row.len() == per_row {
                    rows.push(std::mem::take(&mut row));
                }
                if rows.len() * per_row * 2 > MAX_TRIANGLES {
                    break;
                }
            }
            for pair in rows.windows(2) {
                for i in 0..per_row - 1 {
                    let (a, b, c, e) = (&pair[0][i], &pair[0][i + 1], &pair[1][i], &pair[1][i + 1]);
                    tris.push([a.clone(), b.clone(), c.clone()]);
                    tris.push([b.clone(), e.clone(), c.clone()]);
                }
            }
        }
        _ => {
            // Coons (6) and tensor-product (7) patches.
            let npoints = if kind == 6 { 12 } else { 16 };
            let mut prev: Option<(Vec<Point>, Vec<Vec<f32>>)> = None;
            while !bits.done() && tris.len() < MAX_TRIANGLES {
                let Some(flag) = bits.read(fmt.flag_bits) else {
                    break;
                };
                let (keep_pts, keep_cols) = if flag == 0 { (0, 0) } else { (4, 2) };
                let mut pts: Vec<Point> = Vec::with_capacity(npoints);
                let mut cols: Vec<Vec<f32>> = Vec::with_capacity(4);
                if flag != 0 {
                    let Some((pp, pc)) = &prev else {
                        warn("a patch mesh continues a patch that is not there".into());
                        break;
                    };
                    // Data-order indices of the shared edge in the previous
                    // patch, and of its corner colours.
                    let (edge, colours): ([usize; 4], [usize; 2]) = match flag {
                        1 => ([3, 4, 5, 6], [1, 2]),
                        2 => ([6, 7, 8, 9], [2, 3]),
                        3 => ([9, 10, 11, 0], [3, 0]),
                        _ => {
                            warn("a patch mesh has a bad edge flag".into());
                            break;
                        }
                    };
                    pts.extend(edge.iter().map(|&i| pp[i]));
                    cols.extend(colours.iter().map(|&i| pc[i].clone()));
                }
                let mut ok = true;
                for _ in keep_pts..npoints {
                    match fmt.point(&mut bits) {
                        Some(p) => pts.push(p),
                        None => ok = false,
                    }
                }
                for _ in keep_cols..4 {
                    match fmt.color(&mut bits) {
                        Some(c) => cols.push(c),
                        None => ok = false,
                    }
                }
                bits.align();
                if !ok {
                    break;
                }
                tessellate_patch(&pts, &cols, &mut tris);
                prev = Some((pts, cols));
            }
        }
    }
    Some((tris, t_range))
}

/// The 4 x 4 control points of a patch, indexed `[u][v]`, from points in
/// data order (12 for a Coons patch, 16 for a tensor-product patch).
fn control_grid(d: &[Point]) -> [[Point; 4]; 4] {
    let mut g = [[Point::new(0.0, 0.0); 4]; 4];
    // Data order around the edge: p00 p01 p02 p03 p13 p23 p33 p32 p31 p30 p20 p10.
    let edge = [
        (0, 0),
        (0, 1),
        (0, 2),
        (0, 3),
        (1, 3),
        (2, 3),
        (3, 3),
        (3, 2),
        (3, 1),
        (3, 0),
        (2, 0),
        (1, 0),
    ];
    for (k, &(i, j)) in edge.iter().enumerate() {
        g[i][j] = d[k];
    }
    if d.len() >= 16 {
        g[1][1] = d[12];
        g[1][2] = d[13];
        g[2][2] = d[14];
        g[2][1] = d[15];
    } else {
        // Interior points that make the tensor surface equal the Coons one.
        let edges = g;
        let p = |i: usize, j: usize| edges[i][j];
        let mix = |terms: &[(f64, Point)]| {
            let (mut x, mut y) = (0.0, 0.0);
            for (w, q) in terms {
                x += w * q.x;
                y += w * q.y;
            }
            Point::new(x / 9.0, y / 9.0)
        };
        let inner = |a: (usize, usize),
                     b: [(usize, usize); 2],
                     c: [(usize, usize); 2],
                     e: [(usize, usize); 2],
                     f: (usize, usize)| {
            mix(&[
                (-4.0, p(a.0, a.1)),
                (6.0, p(b[0].0, b[0].1)),
                (6.0, p(b[1].0, b[1].1)),
                (-2.0, p(c[0].0, c[0].1)),
                (-2.0, p(c[1].0, c[1].1)),
                (3.0, p(e[0].0, e[0].1)),
                (3.0, p(e[1].0, e[1].1)),
                (-1.0, p(f.0, f.1)),
            ])
        };
        g[1][1] = inner(
            (0, 0),
            [(0, 1), (1, 0)],
            [(0, 3), (3, 0)],
            [(3, 1), (1, 3)],
            (3, 3),
        );
        g[1][2] = inner(
            (0, 3),
            [(0, 2), (1, 3)],
            [(0, 0), (3, 3)],
            [(3, 2), (1, 0)],
            (3, 0),
        );
        g[2][1] = inner(
            (3, 0),
            [(3, 1), (2, 0)],
            [(3, 3), (0, 0)],
            [(0, 1), (2, 3)],
            (0, 3),
        );
        g[2][2] = inner(
            (3, 3),
            [(3, 2), (2, 3)],
            [(3, 0), (0, 3)],
            [(0, 2), (2, 0)],
            (0, 0),
        );
    }
    g
}

fn bernstein(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
}

/// Splits a patch into triangles with bilinearly interpolated colours.
fn tessellate_patch(pts: &[Point], cols: &[Vec<f32>], out: &mut Vec<Triangle>) {
    if pts.len() < 12 || cols.len() < 4 {
        return;
    }
    let g = control_grid(pts);
    // Enough steps that each piece is small relative to the patch's size in
    // its own space; the device size is unknown here, so be generous.
    let extent = pts.iter().fold(0.0f64, |m, p| {
        m.max((p.x - pts[0].x).abs()).max((p.y - pts[0].y).abs())
    });
    let steps = ((extent / 4.0).ceil() as usize).clamp(4, MAX_PATCH_STEPS);
    let surface = |u: f64, v: f64| {
        let (bu, bv) = (bernstein(u), bernstein(v));
        let (mut x, mut y) = (0.0, 0.0);
        for i in 0..4 {
            for j in 0..4 {
                x += g[i][j].x * bu[i] * bv[j];
                y += g[i][j].y * bu[i] * bv[j];
            }
        }
        Point::new(x, y)
    };
    // Corner colours: c[0] at (0,0), c[1] at (0,1), c[2] at (1,1), c[3] at (1,0).
    let colour = |u: f64, v: f64| -> Vec<f32> {
        let (u, v) = (u as f32, v as f32);
        (0..cols[0].len())
            .map(|k| {
                let c = |i: usize| cols[i].get(k).copied().unwrap_or(0.0);
                (1.0 - u) * (1.0 - v) * c(0)
                    + (1.0 - u) * v * c(1)
                    + u * v * c(2)
                    + u * (1.0 - v) * c(3)
            })
            .collect()
    };
    let n = steps as f64;
    let grid: Vec<Vec<Vertex>> = (0..=steps)
        .map(|i| {
            (0..=steps)
                .map(|j| {
                    let (u, v) = (i as f64 / n, j as f64 / n);
                    Vertex {
                        p: surface(u, v),
                        c: colour(u, v),
                    }
                })
                .collect()
        })
        .collect();
    for i in 0..steps {
        for j in 0..steps {
            let (a, b, c, d) = (
                &grid[i][j],
                &grid[i + 1][j],
                &grid[i][j + 1],
                &grid[i + 1][j + 1],
            );
            out.push([a.clone(), b.clone(), c.clone()]);
            out.push([b.clone(), d.clone(), c.clone()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radial_parameter() {
        let none = (false, false);
        // Concentric circles from radius 0 to 10.
        let s = |x: f64| radial_param(Point::new(x, 0.0), Point::new(0.0, 0.0), 0.0, 10.0, none);
        assert!((s(5.0).unwrap() - 0.5).abs() < 1e-9);
        assert!((s(10.0).unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(s(11.0), None);
        let ext = radial_param(
            Point::new(20.0, 0.0),
            Point::new(0.0, 0.0),
            0.0,
            10.0,
            (false, true),
        );
        assert!((ext.unwrap() - 2.0).abs() < 1e-9);
        // Two circles side by side: points between them pick the larger s.
        let s = radial_param(Point::new(5.0, 0.0), Point::new(10.0, 0.0), 2.0, 0.0, none).unwrap();
        assert!((0.0..=1.0).contains(&s));
    }

    #[test]
    fn coons_patch_corners() {
        // A unit square as a Coons patch: straight edges.
        let p = |x: f64, y: f64| Point::new(x, y);
        let third = 1.0 / 3.0;
        let pts = vec![
            p(0.0, 0.0),
            p(0.0, third),
            p(0.0, 2.0 * third),
            p(0.0, 1.0),
            p(third, 1.0),
            p(2.0 * third, 1.0),
            p(1.0, 1.0),
            p(1.0, 2.0 * third),
            p(1.0, third),
            p(1.0, 0.0),
            p(2.0 * third, 0.0),
            p(third, 0.0),
        ];
        let g = control_grid(&pts);
        // A flat patch's interior points sit on the thirds grid.
        assert!((g[1][1].x - third).abs() < 1e-9 && (g[1][1].y - third).abs() < 1e-9);
        assert!((g[2][2].x - 2.0 * third).abs() < 1e-9);
        let cols = vec![vec![0.0], vec![1.0], vec![2.0], vec![3.0]];
        let mut tris = Vec::new();
        tessellate_patch(&pts, &cols, &mut tris);
        assert!(!tris.is_empty());
        // The vertex at (1, 1) carries colour 2.
        let corner = tris
            .iter()
            .flatten()
            .find(|v| (v.p.x - 1.0).abs() < 1e-9 && (v.p.y - 1.0).abs() < 1e-9)
            .unwrap();
        assert!((corner.c[0] - 2.0).abs() < 1e-6);
    }
}
