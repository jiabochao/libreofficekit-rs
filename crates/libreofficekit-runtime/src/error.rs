use std::fmt;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Io(std::io::Error),
    /// A download failed (network error or non-success HTTP status).
    Http {
        url: String,
        message: String,
    },
    Checksum {
        expected: String,
        actual: String,
    },
    Cancelled,
    UnsupportedPlatform(String),
    /// A package or directory does not have the expected layout.
    Invalid(String),
    /// An external tool (hdiutil, msiexec, msiextract, 7z) failed or is missing.
    Tool(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => e.fmt(f),
            Error::Http { url, message } => write!(f, "download of {url} failed: {message}"),
            Error::Checksum { expected, actual } => {
                write!(f, "checksum mismatch: expected {expected}, got {actual}")
            }
            Error::Cancelled => f.write_str("cancelled"),
            Error::UnsupportedPlatform(p) => write!(f, "unsupported platform {p}"),
            Error::Invalid(m) | Error::Tool(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
