use crate::{FilterError, Result, is_pdf_whitespace};

/// ASCIIHexDecode: pairs of hex digits, whitespace ignored, `>` ends the data.
/// An odd final digit is treated as if followed by `0`.
pub fn decode_ascii_hex(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut high: Option<u8> = None;
    for &b in data {
        let nibble = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            b'>' => break,
            _ if is_pdf_whitespace(b) => continue,
            _ => {
                return Err(FilterError::new(
                    "ASCIIHexDecode",
                    format!("invalid character 0x{b:02x}"),
                    out,
                ));
            }
        };
        match high.take() {
            Some(h) => out.push(h << 4 | nibble),
            None => high = Some(nibble),
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_pairs_and_whitespace() {
        assert_eq!(decode_ascii_hex(b"48 65\n6c6C6f>").unwrap(), b"Hello");
    }

    #[test]
    fn odd_digit_is_padded() {
        assert_eq!(decode_ascii_hex(b"414>").unwrap(), vec![0x41, 0x40]);
    }

    #[test]
    fn rejects_garbage() {
        let err = decode_ascii_hex(b"41zz").unwrap_err();
        assert_eq!(err.partial, b"A");
    }
}
