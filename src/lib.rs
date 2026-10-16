//! Rust bindings for [LibreOfficeKit](https://docs.libreoffice.org/libreofficekit.html),
//! focused on converting documents to PDF.
//!
//! LibreOffice is loaded at runtime from a directory you choose: a trimmed runtime shipped
//! inside your application (see `lok-pack export` and [`locate::app_program_dir`]), one
//! prepared at build time (feature `bundled`), or a system installation.
//!
//! ```no_run
//! use libreofficekit::{Office, PdfOptions};
//!
//! let office = Office::builder()
//!     .install_dir("/path/to/libreoffice")      // root, .app bundle or program dir
//!     .user_profile_dir("/path/to/app-cache/lo-profile")
//!     .build()?;
//! office.convert_to_pdf("report.docx", "report.pdf", &PdfOptions::default())?;
//! # Ok::<(), libreofficekit::Error>(())
//! ```

mod error;
pub mod locate;
mod office;
mod pdf;
pub mod sys;

pub use error::{Error, Result};
pub use office::{Document, DocumentType, LoadOptions, Office, OfficeBuilder};
pub use pdf::{FilterValue, PdfOptions, PdfVersion};

/// The runtime directory prepared by the `bundled` feature at build time, if any.
///
/// This is an absolute path on the build machine, useful for development and tests. To ship
/// it, copy the directory into your app (a dependent build script can read it from the
/// `DEP_LIBREOFFICEKIT_RUNTIME_DIR` environment variable) and point
/// [`OfficeBuilder::install_dir`] at the copy.
pub fn bundled_runtime_dir() -> Option<&'static std::path::Path> {
    option_env!("LIBREOFFICEKIT_BUNDLED_DIR").map(std::path::Path::new)
}
