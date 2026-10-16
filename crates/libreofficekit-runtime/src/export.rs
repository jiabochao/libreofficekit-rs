//! Exporting a runtime into the layout an application ships it in.
//!
//! LibreOffice finds its configuration and data relative to the program directory, so a
//! runtime is shipped as one self-contained directory, not as loose libraries:
//!
//! - macOS: `LibreOffice.bundle` (`Contents/Frameworks` = dylibs, `Contents/Resources` = data),
//!   placed at `<App>.app/Contents/Frameworks/LibreOffice.bundle`. Executables are removed,
//!   since App Store builds may only contain sandboxed executables, and every Mach-O file has
//!   to be signed with the app's identity ([`sign_macos`]).
//! - Windows / Linux: a `libreoffice/` directory (`program/`, `share/`, ...) placed next to the
//!   application executable.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::builder::{copy_dir, run};
use crate::{Error, LO_VERSION, MANIFEST_FILE, Manifest, Platform, Result};

/// Directory name of an exported Windows / Linux runtime.
pub const DIR_NAME: &str = "libreoffice";
/// Bundle name of an exported macOS runtime.
pub const MACOS_BUNDLE_NAME: &str = "LibreOffice.bundle";
/// Default `CFBundleIdentifier` of the exported macOS bundle. Prefer one below your app's
/// identifier, e.g. `com.example.app.libreoffice`.
pub const DEFAULT_BUNDLE_ID: &str = "org.libreoffice.libreofficekit-rs.runtime";

/// Options for [`export`].
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// `CFBundleIdentifier` of the macOS bundle.
    pub bundle_id: String,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            bundle_id: DEFAULT_BUNDLE_ID.to_string(),
        }
    }
}

/// Exports the runtime at `runtime_dir` into `out_dir`, replacing a previous export. Returns
/// the exported directory (`<out>/LibreOffice.bundle` or `<out>/libreoffice`).
pub fn export(runtime_dir: &Path, out_dir: &Path, options: &ExportOptions) -> Result<PathBuf> {
    let manifest = Manifest::read(runtime_dir)?;
    fs::create_dir_all(out_dir)?;
    if !manifest.platform.is_macos() {
        let dest = out_dir.join(DIR_NAME);
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        copy_dir(runtime_dir, &dest)?;
        return Ok(dest);
    }

    let dest = out_dir.join(MACOS_BUNDLE_NAME);
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    let contents = dest.join("Contents");
    copy_dir(&runtime_dir.join("Contents"), &contents)?;
    // Executables, the old signature and app-only metadata.
    for name in [
        "MacOS",
        "_CodeSignature",
        "CodeResources",
        "PkgInfo",
        "Library",
        "PlugIns",
    ] {
        let p = contents.join(name);
        if p.is_dir() {
            fs::remove_dir_all(&p)?;
        } else if p.exists() {
            fs::remove_file(&p)?;
        }
    }
    for file in macho_files(&contents)? {
        if macho_kind(&file)? == Some(MachO::Executable) {
            fs::remove_file(&file)?;
        }
    }
    fs::write(contents.join("Info.plist"), info_plist(&options.bundle_id))?;
    // Inside Contents so the bundle root stays clean for code signing.
    let resources = contents.join("Resources");
    let mut m = manifest.clone();
    m.program_dir = "../Frameworks".to_string();
    let data = serde_json::to_vec_pretty(&m).expect("manifest serializes");
    fs::write(resources.join(MANIFEST_FILE), data)?;
    Ok(dest)
}

fn info_plist(bundle_id: &str) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>{id}</string>
    <key>CFBundleName</key>
    <string>LibreOffice</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>{ver}</string>
    <key>CFBundleVersion</key>
    <string>{ver}</string>
</dict>
</plist>
"#,
        id = esc(bundle_id),
        ver = esc(LO_VERSION)
    )
}

/// Kind of a Mach-O file (for fat files, of its first slice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachO {
    Executable,
    Dylib,
    Bundle,
    Other,
}

/// Detects Mach-O files by their header; `None` for anything else.
pub fn macho_kind(path: &Path) -> Result<Option<MachO>> {
    let mut f = fs::File::open(path)?;
    let mut head = [0u8; 4096];
    let mut n = 0;
    while n < head.len() {
        let r = f.read(&mut head[n..])?;
        if r == 0 {
            break;
        }
        n += r;
    }
    let head = &head[..n];
    if head.len() < 16 {
        return Ok(None);
    }
    let be = |o: usize| u32::from_be_bytes(head[o..o + 4].try_into().unwrap());
    let le = |o: usize| u32::from_le_bytes(head[o..o + 4].try_into().unwrap());
    let filetype = match be(0) {
        // Thin, little endian (arm64 / x86_64).
        0xcffa_edfe | 0xcefa_edfe => le(12),
        // Fat: nfat_arch is small; Java class files share the magic but have a large version.
        0xcafe_babe if be(4) < 16 && head.len() >= 20 => {
            let offset = be(8 + 8) as usize;
            if offset + 16 > head.len() {
                let mut slice = [0u8; 16];
                use std::io::{Seek, SeekFrom};
                f.seek(SeekFrom::Start(offset as u64))?;
                f.read_exact(&mut slice)?;
                u32::from_le_bytes(slice[12..16].try_into().unwrap())
            } else {
                le(offset + 12)
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(match filetype {
        2 => MachO::Executable,
        6 => MachO::Dylib,
        8 => MachO::Bundle,
        _ => MachO::Other,
    }))
}

/// All Mach-O files below `dir` (symlinks not followed), deepest first.
pub fn macho_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_macho(dir, &mut out)?;
    out.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    Ok(out)
}

fn collect_macho(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let path = entry.path();
        if ty.is_dir() {
            collect_macho(&path, out)?;
        } else if ty.is_file() && macho_kind(&path)?.is_some() {
            out.push(path);
        }
    }
    Ok(())
}

/// Signs an exported `LibreOffice.bundle` with `codesign` (macOS only): every Mach-O file,
/// then nested frameworks, then the bundle. Use the identity the app is signed with
/// (`Apple Distribution: ...` for the App Store, `-` for ad-hoc). The dylibs are loaded
/// in-process and need no entitlements.
pub fn sign_macos(bundle: &Path, identity: &str, timestamp: bool) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(Error::Tool("code signing requires macOS (codesign)".into()));
    }
    let codesign = |path: &Path| {
        let mut cmd = Command::new("codesign");
        cmd.args(["--force", "--options", "runtime", "--sign", identity]);
        cmd.arg(if timestamp {
            "--timestamp"
        } else {
            "--timestamp=none"
        });
        run(cmd.arg(path))
    };
    for file in macho_files(bundle)? {
        codesign(&file)?;
    }
    let mut frameworks = Vec::new();
    find_dirs_with_ext(bundle, "framework", &mut frameworks)?;
    frameworks.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for fw in frameworks {
        codesign(&fw)?;
    }
    codesign(bundle)
}

fn find_dirs_with_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == ext) {
                out.push(path.clone());
            }
            find_dirs_with_ext(&path, ext, out)?;
        }
    }
    Ok(())
}

/// Returns the platform of an exported runtime directory.
pub fn exported_platform(dir: &Path) -> Option<Platform> {
    Manifest::read(dir)
        .or_else(|_| Manifest::read(&dir.join("Contents/Resources")))
        .ok()
        .map(|m| m.platform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_macho() {
        let dir = std::env::temp_dir().join(format!("lok-macho-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let class = dir.join("A.class");
        // Java class magic + version 52.
        fs::write(
            &class,
            [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52, 0, 0, 0, 0, 0, 0, 0, 0],
        )
        .unwrap();
        assert_eq!(macho_kind(&class).unwrap(), None);
        let dylib = dir.join("x.dylib");
        let mut h = vec![
            0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 0x01, 0, 0, 0, 0, 6, 0, 0, 0,
        ];
        h.resize(64, 0);
        fs::write(&dylib, &h).unwrap();
        assert_eq!(macho_kind(&dylib).unwrap(), Some(MachO::Dylib));
        fs::remove_dir_all(&dir).unwrap();
    }
}
