//! Re-export of [`libreofficekit_runtime`] with the `builder` feature, for build scripts.
//! A separate crate because Cargo does not allow one package to be both a normal and a
//! build dependency of `libreofficekit-rs` under different names.

pub use libreofficekit_runtime::*;
