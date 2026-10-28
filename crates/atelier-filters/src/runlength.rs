use crate::{FilterError, Result};

/// RunLengthDecode: a length byte 0..=127 copies the next `len + 1` bytes,
/// 129..=255 repeats the next byte `257 - len` times, 128 ends the data.
pub fn decode_run_length(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 2);
    let mut i = 0;
    while i < data.len() {
        let len = data[i];
        i += 1;
        match len {
            128 => break,
            0..=127 => {
                let count = usize::from(len) + 1;
                let Some(run) = data.get(i..i + count) else {
                    out.extend_from_slice(&data[i..]);
                    return Err(FilterError::new(
                        "RunLengthDecode",
                        "literal run is truncated",
                        out,
                    ));
                };
                out.extend_from_slice(run);
                i += count;
            }
            _ => {
                let Some(&b) = data.get(i) else {
                    return Err(FilterError::new(
                        "RunLengthDecode",
                        "repeat run is truncated",
                        out,
                    ));
                };
                out.resize(out.len() + 257 - usize::from(len), b);
                i += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_and_repeat_runs() {
        let data = [2, b'a', b'b', b'c', 254, b'x', 128, 0, b'q'];
        assert_eq!(decode_run_length(&data).unwrap(), b"abcxxx");
    }

    #[test]
    fn truncated_literal_keeps_partial() {
        let err = decode_run_length(&[4, b'a', b'b']).unwrap_err();
        assert_eq!(err.partial, b"ab");
    }
}
