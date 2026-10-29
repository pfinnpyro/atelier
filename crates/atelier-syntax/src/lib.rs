//! PDF file structure (ISO 32000-2, section 7): lexing, objects, cross-reference
//! data, object streams, broken-file repair and the page tree.
//!
//! Objects are loaded lazily: opening a file reads only the cross-reference data,
//! and each object is parsed the first time something asks for it.

mod content;
mod crypt;
mod decode;
mod document;
mod error;
mod lexer;
mod object;
mod page;
mod parser;
mod write;
mod xref;

pub use content::{ContentParser, Operation};
pub use crypt::Access;
pub use decode::{DecodedStream, ImageFilter};
pub use document::Document;
pub use error::{Error, Result};
pub use lexer::{Lexer, Token};
pub use object::{Dict, Name, ObjRef, Object, PdfString, Stream, text_from_bytes};
pub use page::{Page, Rect};
pub use parser::Parser;
pub use write::{Update, serialize};
pub use xref::XrefEntry;
