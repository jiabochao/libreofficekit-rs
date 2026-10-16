use std::fmt;
use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// No LibreOffice program directory could be found at (or below) the given path.
    InstallationNotFound(PathBuf),
    /// The LibreOfficeKit shared library could not be loaded.
    LibraryLoad {
        path: PathBuf,
        source: libloading::Error,
    },
    /// The library does not export `libreofficekit_hook_2` / `libreofficekit_hook`.
    MissingHook(PathBuf),
    /// `libreofficekit_hook*` returned NULL.
    InitFailed(PathBuf),
    /// LibreOfficeKit can only be initialized once per process; it cannot be restarted after
    /// the [`Office`](crate::Office) has been dropped either.
    AlreadyInitialized,
    /// The loaded LibreOffice is too old to provide the given vtable entry.
    Unsupported(&'static str),
    /// The document is password protected and no password was supplied.
    PasswordRequired,
    /// The supplied password was rejected.
    WrongPassword,
    /// LibreOffice failed to load the document.
    Load {
        path: PathBuf,
        message: String,
    },
    /// LibreOffice failed to save/export the document.
    Save {
        path: PathBuf,
        message: String,
    },
    InvalidInput(String),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InstallationNotFound(p) => {
                write!(f, "no LibreOffice installation found at {}", p.display())
            }
            Error::LibraryLoad { path, source } => {
                write!(f, "failed to load {}: {source}", path.display())
            }
            Error::MissingHook(p) => {
                write!(f, "{} does not export libreofficekit_hook", p.display())
            }
            Error::InitFailed(p) => write!(
                f,
                "LibreOfficeKit failed to initialize from {}",
                p.display()
            ),
            Error::AlreadyInitialized => {
                f.write_str("LibreOfficeKit has already been initialized in this process")
            }
            Error::Unsupported(what) => {
                write!(f, "the loaded LibreOffice does not support {what}")
            }
            Error::PasswordRequired => f.write_str("the document is password protected"),
            Error::WrongPassword => f.write_str("the document password is wrong"),
            Error::Load { path, message } => {
                write!(f, "failed to load {}: {message}", path.display())
            }
            Error::Save { path, message } => {
                write!(f, "failed to save {}: {message}", path.display())
            }
            Error::InvalidInput(m) => f.write_str(m),
            Error::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::LibraryLoad { source, .. } => Some(source),
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
