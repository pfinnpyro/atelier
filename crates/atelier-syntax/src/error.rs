use std::fmt;

use atelier_filters::FilterError;

use crate::ObjRef;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Malformed syntax at a byte offset in the file (or in a decoded stream).
    Syntax { offset: usize, message: String },
    /// An object the document refers to could not be found.
    MissingObject(ObjRef),
    /// The object exists but has the wrong type for how it is used.
    WrongType {
        expected: &'static str,
        found: &'static str,
    },
    /// No usable cross-reference data or trailer, even after repair.
    Structure(String),
    /// A stream filter failed and produced no usable output.
    Filter(FilterError),
    /// The file is encrypted and the password given does not open it.
    Password,
    /// A feature this build does not handle yet.
    Unsupported(String),
    /// Object references nest deeper than we allow (usually a reference cycle).
    TooDeep,
}

impl Error {
    pub(crate) fn syntax(offset: usize, message: impl Into<String>) -> Self {
        Self::Syntax {
            offset,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { offset, message } => {
                write!(f, "syntax error at byte {offset}: {message}")
            }
            Self::MissingObject(r) => write!(f, "object {r} not found"),
            Self::WrongType { expected, found } => write!(f, "expected {expected}, found {found}"),
            Self::Structure(m) => write!(f, "broken file structure: {m}"),
            Self::Filter(e) => write!(f, "{e}"),
            Self::Password => write!(f, "this PDF is protected by a password"),
            Self::Unsupported(m) => write!(f, "not supported yet: {m}"),
            Self::TooDeep => write!(f, "object references nest too deeply"),
        }
    }
}

impl std::error::Error for Error {}

impl From<FilterError> for Error {
    fn from(e: FilterError) -> Self {
        Self::Filter(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
