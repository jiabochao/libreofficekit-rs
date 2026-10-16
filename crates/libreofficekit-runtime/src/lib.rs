//! LibreOffice runtime packages for `libreofficekit-rs`.
//!
//! A *runtime* is a trimmed copy of an official LibreOffice release for one platform,
//! packaged as `libreoffice-<version>-r<revision>-<platform>.tar.zst` with a
//! `libreofficekit-runtime.json` manifest at its root.
//!
//! - feature `download` (default): [`Installer`] downloads, verifies and unpacks a prebuilt
//!   package, e.g. when a user enables document conversion in an app.
//! - feature `builder`: [`builder`] downloads the official release, extracts it, removes the
//!   parts document conversion does not need and packs the result.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod archive;
mod error;
#[cfg(feature = "download")]
mod install;

#[cfg(feature = "builder")]
pub mod build_support;
#[cfg(feature = "builder")]
pub mod builder;
#[cfg(feature = "builder")]
pub mod export;
#[cfg(feature = "builder")]
pub mod trim;

pub use archive::unpack;
pub use error::{Error, Result};
#[cfg(feature = "download")]
pub use install::{Installer, Progress};

/// The LibreOffice release the runtime packages are built from.
pub const LO_VERSION: &str = "26.2.6.3";
/// Packaging revision, bumped when trimming or layout changes for the same LibreOffice release.
pub const REVISION: u32 = 1;

/// Where prebuilt packages are published: `<base>/<release_tag()>/<asset_name()>`.
/// Override at runtime with [`Installer::base_urls`] or the `LIBREOFFICEKIT_RUNTIME_BASE_URL`
/// environment variable (comma separated mirrors).
pub const DEFAULT_BASE_URL: &str =
    "https://github.com/jiabochao/libreofficekit-rs/releases/download";

/// File name of the manifest at the root of every runtime.
pub const MANIFEST_FILE: &str = "libreofficekit-runtime.json";

/// SHA-256 of the published packages, pinned per platform once a release exists. Pinned
/// hashes take precedence over the `.sha256` file published next to each package.
pub const PINNED_SHA256: &[(Platform, &str)] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    #[serde(rename = "linux-x86_64")]
    LinuxX86_64,
    #[serde(rename = "linux-aarch64")]
    LinuxAarch64,
    #[serde(rename = "macos-x86_64")]
    MacosX86_64,
    #[serde(rename = "macos-aarch64")]
    MacosAarch64,
    #[serde(rename = "windows-x86_64")]
    WindowsX86_64,
    #[serde(rename = "windows-aarch64")]
    WindowsAarch64,
}

impl Platform {
    pub const ALL: [Platform; 6] = [
        Platform::LinuxX86_64,
        Platform::LinuxAarch64,
        Platform::MacosX86_64,
        Platform::MacosAarch64,
        Platform::WindowsX86_64,
        Platform::WindowsAarch64,
    ];

    /// The platform this code was compiled for.
    pub fn current() -> Option<Platform> {
        Self::from_os_arch(std::env::consts::OS, std::env::consts::ARCH)
    }

    pub fn from_os_arch(os: &str, arch: &str) -> Option<Platform> {
        Some(match (os, arch) {
            ("linux", "x86_64") => Platform::LinuxX86_64,
            ("linux", "aarch64") => Platform::LinuxAarch64,
            ("macos", "x86_64") => Platform::MacosX86_64,
            ("macos", "aarch64") => Platform::MacosAarch64,
            ("windows", "x86_64") => Platform::WindowsX86_64,
            ("windows", "aarch64") => Platform::WindowsAarch64,
            _ => return None,
        })
    }

    /// Maps a Rust target triple such as `aarch64-apple-darwin`.
    pub fn from_target_triple(triple: &str) -> Option<Platform> {
        let arch = triple.split('-').next()?;
        let os = if triple.contains("-linux-") {
            "linux"
        } else if triple.contains("-apple-darwin") {
            "macos"
        } else if triple.contains("-windows-") {
            "windows"
        } else {
            return None;
        };
        Self::from_os_arch(os, arch)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::LinuxX86_64 => "linux-x86_64",
            Platform::LinuxAarch64 => "linux-aarch64",
            Platform::MacosX86_64 => "macos-x86_64",
            Platform::MacosAarch64 => "macos-aarch64",
            Platform::WindowsX86_64 => "windows-x86_64",
            Platform::WindowsAarch64 => "windows-aarch64",
        }
    }

    pub fn is_macos(self) -> bool {
        matches!(self, Platform::MacosX86_64 | Platform::MacosAarch64)
    }

    pub fn is_windows(self) -> bool {
        matches!(self, Platform::WindowsX86_64 | Platform::WindowsAarch64)
    }

    /// The LibreOfficeKit program directory relative to the runtime root.
    pub fn program_dir(self) -> &'static str {
        if self.is_macos() {
            "Contents/Frameworks"
        } else {
            "program"
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Platform {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Platform::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .or_else(|| Platform::from_target_triple(s))
            .ok_or_else(|| Error::UnsupportedPlatform(s.to_string()))
    }
}

/// Contents of [`MANIFEST_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub lo_version: String,
    pub revision: u32,
    pub platform: Platform,
    /// Program directory relative to the runtime root, `/` separated.
    pub program_dir: String,
    /// The official LibreOffice download the runtime was made from.
    #[serde(default)]
    pub source: String,
}

impl Manifest {
    pub fn new(platform: Platform, source: impl Into<String>) -> Self {
        Manifest {
            lo_version: LO_VERSION.to_string(),
            revision: REVISION,
            platform,
            program_dir: platform.program_dir().to_string(),
            source: source.into(),
        }
    }

    pub fn read(runtime_dir: &Path) -> Result<Manifest> {
        let data = std::fs::read(runtime_dir.join(MANIFEST_FILE))?;
        serde_json::from_slice(&data)
            .map_err(|e| Error::Invalid(format!("bad runtime manifest: {e}")))
    }

    pub fn write(&self, runtime_dir: &Path) -> Result<()> {
        let data = serde_json::to_vec_pretty(self).expect("manifest serializes");
        std::fs::write(runtime_dir.join(MANIFEST_FILE), data)?;
        Ok(())
    }
}

/// An unpacked runtime on disk.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Runtime {
    /// Opens a runtime directory, checking its manifest and that the program directory exists.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Runtime> {
        let dir = dir.into();
        let manifest = Manifest::read(&dir)?;
        let rt = Runtime { dir, manifest };
        if !rt.program_dir().is_dir() {
            return Err(Error::Invalid(format!(
                "{} is missing",
                rt.program_dir().display()
            )));
        }
        Ok(rt)
    }

    /// The directory to hand to `OfficeBuilder::program_dir`.
    pub fn program_dir(&self) -> PathBuf {
        self.dir.join(&self.manifest.program_dir)
    }

    /// Whether this runtime is the release and revision this crate version expects.
    pub fn is_current(&self) -> bool {
        self.manifest.lo_version == LO_VERSION && self.manifest.revision == REVISION
    }
}

/// `libreoffice-<version>-r<revision>-<platform>`, the package and install directory name.
pub fn package_stem(platform: Platform) -> String {
    format!("libreoffice-{LO_VERSION}-r{REVISION}-{platform}")
}

pub fn asset_name(platform: Platform) -> String {
    format!("{}.tar.zst", package_stem(platform))
}

/// The release tag the packages for this crate version are published under.
pub fn release_tag() -> String {
    format!("runtime-{LO_VERSION}-r{REVISION}")
}

pub fn asset_url(base_url: &str, platform: Platform) -> String {
    format!(
        "{}/{}/{}",
        base_url.trim_end_matches('/'),
        release_tag(),
        asset_name(platform)
    )
}

pub fn pinned_sha256(platform: Platform) -> Option<&'static str> {
    PINNED_SHA256
        .iter()
        .find(|(p, _)| *p == platform)
        .map(|(_, h)| *h)
}

/// Per-user cache directory used by build-time preparation and `lok-pack`:
/// `LIBREOFFICEKIT_CACHE_DIR`, else the platform cache dir + `libreofficekit-rs`.
pub fn default_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LIBREOFFICEKIT_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches"))
    } else {
        env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("libreofficekit-rs")
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses a `sha256sum` style line (`<hex>  <file>`) or a bare hash.
pub fn parse_sha256(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?.to_ascii_lowercase();
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triples() {
        assert_eq!(
            Platform::from_target_triple("aarch64-apple-darwin"),
            Some(Platform::MacosAarch64)
        );
        assert_eq!(
            Platform::from_target_triple("x86_64-pc-windows-msvc"),
            Some(Platform::WindowsX86_64)
        );
        assert_eq!(
            Platform::from_target_triple("x86_64-unknown-linux-gnu"),
            Some(Platform::LinuxX86_64)
        );
        assert_eq!(Platform::from_target_triple("wasm32-unknown-unknown"), None);
    }

    #[test]
    fn names() {
        assert_eq!(
            asset_url("https://x/y/", Platform::LinuxX86_64),
            format!(
                "https://x/y/runtime-{LO_VERSION}-r{REVISION}/libreoffice-{LO_VERSION}-r{REVISION}-linux-x86_64.tar.zst"
            )
        );
        assert_eq!(
            parse_sha256(&format!("{}  f.tar.zst\n", "A".repeat(64))),
            Some("a".repeat(64))
        );
    }
}
