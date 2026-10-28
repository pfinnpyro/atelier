//! Stream filters for PDF (ISO 32000-2, section 7.4).
//!
//! Every decoder here is written from scratch. Decoders are lenient: PDF files in
//! the wild often carry truncated or slightly corrupt streams, so on failure we
//! return the bytes decoded so far inside [`FilterError`] and let the caller decide
//! whether to use them.

mod ascii85;
mod asciihex;
mod flate;
mod lzw;
mod predictor;
mod runlength;

pub use ascii85::decode_ascii85;
pub use asciihex::decode_ascii_hex;
pub use flate::{decode_flate, inflate};
pub use lzw::decode_lzw;
pub use predictor::{Predictor, apply_predictor};
pub use runlength::decode_run_length;

use std::fmt;

/// A decode failure. `partial` holds whatever was decoded before the failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError {
    pub filter: &'static str,
    pub reason: String,
    pub partial: Vec<u8>,
}

impl FilterError {
    pub(crate) fn new(filter: &'static str, reason: impl Into<String>, partial: Vec<u8>) -> Self {
        Self {
            filter,
            reason: reason.into(),
            partial,
        }
    }
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} ({} bytes decoded before failure)",
            self.filter,
            self.reason,
            self.partial.len()
        )
    }
}

impl std::error::Error for FilterError {}

pub type Result<T> = std::result::Result<T, FilterError>;

/// PDF whitespace characters (ISO 32000-2, table 1).
#[inline]
pub(crate) fn is_pdf_whitespace(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}
