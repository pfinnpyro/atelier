use super::{add_box, add_point, line, nums, read_operation, unsupported};
use crate::font::PdfFont;
use atelier_raster::{Point, Transform};
use atelier_syntax::{Document, Object, Parser, Rect, Result, read_inline_image};

pub(super) struct Procedure {
    pub matrix: Transform,
    pub bounds: Option<Rect>,
    pub content: String,
}

#[derive(Clone, Copy)]
struct State {
    matrix: Transform,
    // Stroke geometry must be independent of the page's inherited settings.
    width: Option<f64>,
    miter: Option<f64>,
}

pub(super) fn procedure(
    doc: &Document,
    font: &PdfFont,
    code: u32,
    width: f64,
) -> Result<Procedure> {
    let Some((matrix, Some(proc), _)) = font.type3(code) else {
        return Err(unsupported("A Type 3 glyph has no verifiable procedure"));
    };
    if matrix.a <= 0.0 || matrix.d <= 0.0 || matrix.b != 0.0 || matrix.c != 0.0 {
        return Err(unsupported("Unsupported Type 3 font matrix"));
    }
    let Object::Stream(proc) = doc.resolve(proc)? else {
        return Err(unsupported("Invalid Type 3 glyph stream"));
    };
    let decoded = doc.decode_stream(&proc)?;
    if decoded.image_filter.is_some() || decoded.data.len() > 256 * 1024 {
        return Err(unsupported("Invalid or oversized Type 3 glyph stream"));
    }
    let mut parser = Parser::new(&decoded.data);
    let Some((header, kind)) = read_operation(&mut parser)? else {
        return Err(unsupported("Type 3 glyph metrics are missing"));
    };
    let (shape_width, y) = match kind {
        b"d0" => {
            let [x, y] = nums(&header)?;
            (x, y)
        }
        b"d1" => {
            let [x, y, ..] = nums::<6>(&header)?;
            (x, y)
        }
        _ => return Err(unsupported("Type 3 glyph metrics are missing")),
    };
    if y != 0.0 || (shape_width * matrix.a - width).abs() > 1e-8 {
        return Err(unsupported("Type 3 glyph metrics do not match its font"));
    }
    let mut state = State {
        matrix,
        width: None,
        miter: None,
    };
    let mut stack = Vec::new();
    let mut content = String::new();
    let mut bounds = None;
    let mut path = None;
    while let Some((args, op)) = read_operation(&mut parser)? {
        let values = match op {
            b"q" if stack.len() < 32 && path.is_none() => {
                stack.push(state);
                nums::<0>(&args)?.to_vec()
            }
            b"Q" if path.is_none() => {
                state = stack
                    .pop()
                    .ok_or_else(|| unsupported("Unbalanced Type 3 graphics state"))?;
                nums::<0>(&args)?.to_vec()
            }
            b"cm" if path.is_none() => {
                let values = nums(&args)?;
                state.matrix = Transform::from_array(values).then(&state.matrix);
                if state.matrix.invert().is_none() || !state.matrix.is_finite() {
                    return Err(unsupported("Invalid Type 3 glyph transform"));
                }
                values.to_vec()
            }
            b"g" | b"G" if kind == b"d0" => nums::<1>(&args)?.to_vec(),
            b"rg" | b"RG" if kind == b"d0" => nums::<3>(&args)?.to_vec(),
            b"k" | b"K" if kind == b"d0" => nums::<4>(&args)?.to_vec(),
            b"w" => {
                let [width] = nums(&args)?;
                state.width = Some(width);
                vec![width]
            }
            b"M" => {
                let [miter] = nums(&args)?;
                state.miter = Some(miter);
                vec![miter]
            }
            b"J" | b"j" => {
                let [style] = nums(&args)?;
                if ![0.0, 1.0, 2.0].contains(&style) {
                    return Err(unsupported("Invalid Type 3 stroke style"));
                }
                vec![style]
            }
            b"m" => points::<2>(&args, &mut path, state.matrix)?,
            b"l" if path.is_some() => points::<2>(&args, &mut path, state.matrix)?,
            b"c" if path.is_some() => points::<6>(&args, &mut path, state.matrix)?,
            b"v" | b"y" if path.is_some() => points::<4>(&args, &mut path, state.matrix)?,
            b"re" => {
                let [x, y, w, h] = nums(&args)?;
                add_box(
                    &mut path,
                    state.matrix,
                    Rect {
                        x0: x,
                        y0: y,
                        x1: x + w,
                        y1: y + h,
                    },
                )?;
                vec![x, y, w, h]
            }
            b"h" | b"W" | b"W*" if path.is_some() => nums::<0>(&args)?.to_vec(),
            b"n" => {
                path = None;
                nums::<0>(&args)?.to_vec()
            }
            b"f" | b"F" | b"f*" | b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                let Some(mut painted) = path.take() else {
                    return Err(unsupported("Type 3 paint has no path"));
                };
                if !matches!(op, b"f" | b"F" | b"f*") {
                    let (Some(width), Some(miter)) = (state.width, state.miter) else {
                        return Err(unsupported(
                            "Type 3 stroke needs explicit width and miter limit",
                        ));
                    };
                    if width <= 0.0 || miter < 1.0 {
                        return Err(unsupported("Invalid Type 3 stroke geometry"));
                    }
                    let reach = width * miter.max(2.0) * state.matrix.max_scale();
                    painted.x0 -= reach;
                    painted.y0 -= reach;
                    painted.x1 += reach;
                    painted.y1 += reach;
                }
                include(&mut bounds, painted)?;
                nums::<0>(&args)?.to_vec()
            }
            b"BI" if args.is_empty() && path.is_none() => {
                let (dict, data) = read_inline_image(&mut parser)?;
                let stencil = stencil(doc, &dict, data)?;
                if !stencil.is_empty() {
                    add_box(
                        &mut bounds,
                        state.matrix,
                        Rect {
                            x0: 0.0,
                            y0: 0.0,
                            x1: 1.0,
                            y1: 1.0,
                        },
                    )?;
                    content.push_str(&stencil);
                }
                continue;
            }
            _ => {
                return Err(unsupported(format!(
                    "Unsupported operator in Type 3 glyph: {}",
                    String::from_utf8_lossy(op)
                )));
            }
        };
        content.push_str(&line(op, &values));
        if content.len() > 2 * 1024 * 1024 {
            return Err(unsupported("Type 3 glyph exceeds the export limit"));
        }
    }
    if path.is_some() || !stack.is_empty() {
        return Err(unsupported("Unbalanced Type 3 glyph content"));
    }
    Ok(Procedure {
        matrix,
        bounds,
        content,
    })
}

fn points<const N: usize>(
    args: &[Object],
    bounds: &mut Option<Rect>,
    matrix: Transform,
) -> Result<Vec<f64>> {
    let values = nums::<N>(args)?;
    for xy in values.chunks_exact(2) {
        add_point(bounds, matrix.apply(Point::new(xy[0], xy[1])))?;
    }
    Ok(values.to_vec())
}

fn include(bounds: &mut Option<Rect>, rect: Rect) -> Result<()> {
    add_box(bounds, Transform::IDENTITY, rect)
}

// Bitmap Type 3 fonts frequently paint a 1-bit inline stencil. Rebuild it
// as scanline runs of rectangles so the font carries no source image stream.
fn stencil(doc: &Document, dict: &atelier_syntax::Dict, data: Vec<u8>) -> Result<String> {
    let get = |a, b| dict.get(a).or_else(|| dict.get(b));
    if get(b"IM", b"ImageMask").and_then(Object::as_bool) != Some(true)
        || get(b"I", b"Interpolate").and_then(Object::as_bool) == Some(true)
    {
        return Err(unsupported(
            "Only non-interpolated inline stencils are supported inside Type 3 glyphs",
        ));
    }
    let dimension = |a, b| {
        get(a, b)
            .and_then(Object::as_i64)
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| (1..=512).contains(v))
            .ok_or_else(|| unsupported("Unsupported Type 3 stencil dimensions"))
    };
    let (w, h) = (dimension(b"W", b"Width")?, dimension(b"H", b"Height")?);
    let invert = match get(b"D", b"Decode") {
        None => false,
        Some(value) => match nums::<2>(value.as_array().unwrap_or_default())? {
            [0.0, 1.0] => false,
            [1.0, 0.0] => true,
            _ => return Err(unsupported("Invalid Type 3 stencil decode array")),
        },
    };
    let mut full = dict.clone();
    for (short, long) in [
        (b"F".as_slice(), "Filter"),
        (b"DP".as_slice(), "DecodeParms"),
    ] {
        if let Some(value) = dict.get(short) {
            if dict.contains_key(long.as_bytes()) {
                return Err(unsupported("Ambiguous stencil filter dictionary"));
            }
            full.insert(long, value.clone());
        }
    }
    let decoded = doc.decode_stream(&atelier_syntax::Stream {
        dict: full,
        data: data.into(),
    })?;
    let stride = w.div_ceil(8);
    if decoded.image_filter.is_some() || decoded.data.len() != h * stride {
        return Err(unsupported("Invalid Type 3 stencil image"));
    }
    let mut content = String::new();
    for (y, row) in decoded.data.chunks_exact(stride).enumerate() {
        let paints = |x: usize| (row[x / 8] & (0x80_u8 >> (x % 8)) != 0) == invert;
        let mut x = 0;
        while x < w {
            if !paints(x) {
                x += 1;
                continue;
            }
            let first = x;
            x += 1;
            while x < w && paints(x) {
                x += 1;
            }
            content.push_str(&line(
                b"re",
                &[
                    first as f64 / w as f64,
                    (h - y - 1) as f64 / h as f64,
                    (x - first) as f64 / w as f64,
                    1.0 / h as f64,
                ],
            ));
            if content.len() > 2 * 1024 * 1024 {
                return Err(unsupported("Type 3 stencil exceeds the export limit"));
            }
        }
    }
    if !content.is_empty() {
        content.push_str("f\n");
    }
    Ok(content)
}
