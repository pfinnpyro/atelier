//! Runs a stream's filter chain (ISO 32000-2, 7.4).

use atelier_filters as filters;

use crate::{Dict, Error, Name, Object, Result};

/// Image codecs that stay encoded here; the image decoders handle them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFilter {
    Dct,
    Jpx,
    Jbig2,
    CcittFax,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedStream {
    pub data: Vec<u8>,
    /// Set when the chain ends in an image codec: `data` is then still encoded
    /// with that codec, and the dictionary holds its /DecodeParms.
    pub image_filter: Option<(ImageFilter, Dict)>,
}

/// Decodes `data` according to the /Filter and /DecodeParms in `dict`.
/// `resolve` turns references into objects. Recoverable damage (a truncated Flate
/// stream, say) is reported through `warn` and the partial data is kept.
pub(crate) fn decode(
    data: &[u8],
    dict: &Dict,
    resolve: &dyn Fn(&Object) -> Object,
    warn: &mut dyn FnMut(String),
) -> Result<DecodedStream> {
    let filter_list = as_list(&resolve(dict.get(b"Filter").unwrap_or(&Object::Null)));
    let params_obj = dict.get(b"DecodeParms").or_else(|| dict.get(b"DP"));
    let param_list = as_list(&resolve(params_obj.unwrap_or(&Object::Null)));

    let mut current: Vec<u8> = data.to_vec();
    for (i, filter) in filter_list.iter().enumerate() {
        let Some(name) = resolve(filter).as_name().cloned() else {
            return Err(Error::Structure("filter is not a name".into()));
        };
        let params = param_list
            .get(i)
            .map(resolve)
            .and_then(|p| p.as_dict().cloned())
            .unwrap_or_default();
        let is_last = i + 1 == filter_list.len();

        let image = match name.as_bytes() {
            b"DCTDecode" | b"DCT" => Some(ImageFilter::Dct),
            b"JPXDecode" => Some(ImageFilter::Jpx),
            b"JBIG2Decode" => Some(ImageFilter::Jbig2),
            b"CCITTFaxDecode" | b"CCF" => Some(ImageFilter::CcittFax),
            _ => None,
        };
        if let Some(image) = image {
            if !is_last {
                return Err(Error::Unsupported(format!(
                    "{name} followed by another filter"
                )));
            }
            return Ok(DecodedStream {
                data: current,
                image_filter: Some((image, params)),
            });
        }

        let result = match name.as_bytes() {
            b"FlateDecode" | b"Fl" => filters::decode_flate(&current),
            b"LZWDecode" | b"LZW" => {
                let early = params.get(b"EarlyChange").and_then(Object::as_i64) != Some(0);
                filters::decode_lzw(&current, early)
            }
            b"ASCII85Decode" | b"A85" => filters::decode_ascii85(&current),
            b"ASCIIHexDecode" | b"AHx" => filters::decode_ascii_hex(&current),
            b"RunLengthDecode" | b"RL" => filters::decode_run_length(&current),
            b"Crypt" => {
                let identity = params
                    .get(b"Name")
                    .and_then(Object::as_name)
                    .is_none_or(|n| *n == b"Identity");
                if identity {
                    continue;
                }
                return Err(Error::Encrypted);
            }
            _ => return Err(Error::Unsupported(format!("filter {name}"))),
        };
        current = recover(result, &name, warn)?;

        if matches!(
            name.as_bytes(),
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW"
        ) {
            let p = predictor_params(&params);
            if p.predictor > 1 {
                current = recover(filters::apply_predictor(current, &p), &name, warn)?;
            }
        }
    }
    Ok(DecodedStream {
        data: current,
        image_filter: None,
    })
}

fn recover(
    result: filters::Result<Vec<u8>>,
    name: &Name,
    warn: &mut dyn FnMut(String),
) -> Result<Vec<u8>> {
    match result {
        Ok(data) => Ok(data),
        Err(e) if !e.partial.is_empty() => {
            warn(format!("{name}: {e}; using the partial data"));
            Ok(e.partial)
        }
        Err(e) => Err(e.into()),
    }
}

fn as_list(obj: &Object) -> Vec<Object> {
    match obj {
        Object::Null => Vec::new(),
        Object::Array(items) => items.clone(),
        other => vec![other.clone()],
    }
}

fn predictor_params(params: &Dict) -> filters::Predictor {
    let get =
        |key: &[u8], default: i64| params.get(key).and_then(Object::as_i64).unwrap_or(default);
    filters::Predictor {
        predictor: u8::try_from(get(b"Predictor", 1)).unwrap_or(1),
        colors: u8::try_from(get(b"Colors", 1)).unwrap_or(1),
        bits_per_component: u8::try_from(get(b"BitsPerComponent", 8)).unwrap_or(8),
        columns: u32::try_from(get(b"Columns", 1)).unwrap_or(1),
    }
}
