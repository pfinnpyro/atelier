use crate::{FilterError, Result, is_pdf_whitespace};

/// ASCII85Decode. Whitespace is ignored, `z` stands for four zero bytes and `~>`
/// ends the data. A leading `<~` (Adobe's framing) is tolerated.
pub fn decode_ascii85(data: &[u8]) -> Result<Vec<u8>> {
    let data = data.strip_prefix(b"<~").unwrap_or(data);
    let mut out = Vec::with_capacity(data.len() * 4 / 5);
    let mut group = [0u8; 5];
    let mut n = 0usize;

    for &b in data {
        match b {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = b - b'!';
                n += 1;
                if n == 5 {
                    let Some(value) = group_value(&group) else {
                        return Err(FilterError::new(
                            "ASCII85Decode",
                            "group overflows 32 bits",
                            out,
                        ));
                    };
                    out.extend_from_slice(&value.to_be_bytes());
                    n = 0;
                }
            }
            _ if is_pdf_whitespace(b) => {}
            _ => {
                return Err(FilterError::new(
                    "ASCII85Decode",
                    format!("invalid character 0x{b:02x}"),
                    out,
                ));
            }
        }
    }

    match n {
        0 => {}
        1 => {
            return Err(FilterError::new(
                "ASCII85Decode",
                "final group has a single character",
                out,
            ));
        }
        _ => {
            // Pad with 'u' (84) and keep n - 1 bytes.
            for slot in group.iter_mut().skip(n) {
                *slot = 84;
            }
            let Some(value) = group_value(&group) else {
                return Err(FilterError::new(
                    "ASCII85Decode",
                    "final group overflows 32 bits",
                    out,
                ));
            };
            out.extend_from_slice(&value.to_be_bytes()[..n - 1]);
        }
    }
    Ok(out)
}

fn group_value(group: &[u8; 5]) -> Option<u32> {
    let v = group.iter().fold(0u64, |acc, &d| acc * 85 + u64::from(d));
    u32::try_from(v).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_text() {
        // "Man is distinguished" from the classic Wikipedia example.
        let encoded = b"9jqo^BlbD-BleB1DJ+*+F(f,q~>";
        assert_eq!(decode_ascii85(encoded).unwrap(), b"Man is distinguished");
    }

    #[test]
    fn handles_z_and_partial_group() {
        assert_eq!(decode_ascii85(b"z~>").unwrap(), vec![0; 4]);
        // "A" encodes to "5l".
        assert_eq!(decode_ascii85(b"<~5l~>").unwrap(), b"A");
    }

    #[test]
    fn ignores_whitespace() {
        assert_eq!(decode_ascii85(b"9jqo^\n Bl bD-~>").unwrap(), b"Man is d");
    }
}
