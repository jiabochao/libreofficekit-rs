//! Finding the LibreOffice program directory and the LibreOfficeKit shared library.
//!
//! The *program directory* is what LibreOfficeKit calls the install path: the directory
//! holding `sofficeapp`/`mergedlo`. It is `<root>/program` on Linux and Windows and
//! `<root>/Contents/Frameworks` inside a macOS app bundle.

use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
const LIB_NAMES: [&str; 2] = ["sofficeapp.dll", "mergedlo.dll"];
#[cfg(target_os = "macos")]
const LIB_NAMES: [&str; 2] = ["libsofficeapp.dylib", "libmergedlo.dylib"];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const LIB_NAMES: [&str; 2] = ["libsofficeapp.so", "libmergedlo.so"];

/// Relative locations of the program directory below an installation root, in lookup order.
const PROGRAM_SUBDIRS: [&str; 7] = [
    "",
    "program",
    "Contents/Frameworks",
    "LibreOffice.app/Contents/Frameworks",
    // Layouts written by `lok-pack export`, given their parent directory.
    "libreoffice/program",
    "LibreOffice.bundle/Contents/Frameworks",
    "Frameworks/LibreOffice.bundle/Contents/Frameworks",
];

/// Name of the runtime directory shipped next to the executable (Windows, Linux).
pub const APP_DIR_NAME: &str = "libreoffice";
/// Name of the runtime bundle shipped in `<App>.app/Contents/Frameworks` (macOS).
pub const APP_BUNDLE_NAME: &str = "LibreOffice.bundle";

/// Returns the LibreOfficeKit library inside `program_dir`, mirroring `lok_dlopen()` from
/// `LibreOfficeKitInit.h`: prefer `sofficeapp`, unless it is the small text stub that
/// `--enable-mergelibs` builds ship, then fall back to `mergedlo`.
pub fn find_library(program_dir: &Path) -> Option<PathBuf> {
    let [app, merged] = LIB_NAMES;
    let app = program_dir.join(app);
    if app.metadata().is_ok_and(|m| m.is_file() && m.len() > 100) {
        return Some(app);
    }
    let merged = program_dir.join(merged);
    merged.is_file().then_some(merged)
}

/// Resolves `path` (an installation root, a `.app` bundle, or the program directory itself)
/// to the program directory.
pub fn resolve_program_dir(path: &Path) -> Option<PathBuf> {
    PROGRAM_SUBDIRS
        .iter()
        .map(|sub| {
            if sub.is_empty() {
                path.to_path_buf()
            } else {
                path.join(sub)
            }
        })
        .find(|dir| find_library(dir).is_some())
}

/// Well known system-wide installation locations for the current platform.
pub fn system_program_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(base) = std::env::var_os(var) {
                dirs.push(PathBuf::from(base).join("LibreOffice").join("program"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from(
            "/Applications/LibreOffice.app/Contents/Frameworks",
        ));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Applications/LibreOffice.app/Contents/Frameworks"));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/lib/libreoffice/program"));
        dirs.push(PathBuf::from("/usr/lib64/libreoffice/program"));
        dirs.push(PathBuf::from("/usr/local/lib/libreoffice/program"));
        dirs.push(PathBuf::from(
            "/snap/libreoffice/current/lib/libreoffice/program",
        ));
        if let Ok(entries) = std::fs::read_dir("/opt") {
            let mut opt: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("libreoffice"))
                })
                .map(|p| p.join("program"))
                .collect();
            // Newest version first.
            opt.sort();
            opt.reverse();
            dirs.extend(opt);
        }
    }
    dirs
}

/// The program directory of a runtime shipped with the running application, as written by
/// `lok-pack export`:
///
/// - macOS: `<App>.app/Contents/Frameworks/LibreOffice.bundle/Contents/Frameworks`
/// - Windows / Linux: `<executable dir>/libreoffice/program`
pub fn app_program_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let dir = if cfg!(target_os = "macos") {
        exe_dir
            .parent()?
            .join("Frameworks")
            .join(APP_BUNDLE_NAME)
            .join("Contents/Frameworks")
    } else {
        exe_dir.join(APP_DIR_NAME).join("program")
    };
    find_library(&dir).is_some().then_some(dir)
}

/// Finds a usable program directory: `LOK_PROGRAM_DIR`, then a runtime shipped with the
/// application ([`app_program_dir`]), then system installations.
pub fn find_installation() -> Option<PathBuf> {
    std::env::var_os("LOK_PROGRAM_DIR")
        .map(PathBuf::from)
        .and_then(|p| resolve_program_dir(&p))
        .or_else(app_program_dir)
        .or_else(|| {
            system_program_dirs()
                .iter()
                .find_map(|p| resolve_program_dir(p))
        })
}

/// Converts a filesystem path to an absolute, percent-encoded `file://` URL.
pub fn path_to_file_url(path: &Path) -> std::io::Result<String> {
    let abs = absolute(path)?;
    let s = abs.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path is not valid UTF-8")
    })?;

    #[cfg(windows)]
    let (prefix, s) = {
        let s = s.replace('\\', "/");
        if let Some(rest) = s.strip_prefix("//?/UNC/") {
            ("file://".to_string(), rest.to_string())
        } else if let Some(rest) = s.strip_prefix("//?/") {
            ("file:///".to_string(), rest.to_string())
        } else if let Some(rest) = s.strip_prefix("//") {
            ("file://".to_string(), rest.to_string())
        } else {
            ("file:///".to_string(), s)
        }
    };
    #[cfg(not(windows))]
    let prefix = "file://".to_string();

    let mut url = prefix;
    for (i, b) in s.bytes().enumerate() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
            // Keep the drive letter colon (`C:`) on Windows.
            || (cfg!(windows) && b == b':' && i == 1);
        if keep {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(url)
}

fn absolute(path: &Path) -> std::io::Result<PathBuf> {
    // Canonicalize the deepest existing ancestor so symlinks (e.g. /tmp -> /private/tmp on
    // macOS) are resolved while still allowing a not-yet-existing output file.
    if let Ok(p) = path.canonicalize() {
        return Ok(p);
    }
    let abs = std::path::absolute(path)?;
    match (abs.parent(), abs.file_name()) {
        (Some(parent), Some(name)) => Ok(parent
            .canonicalize()
            .unwrap_or(parent.to_path_buf())
            .join(name)),
        _ => Ok(abs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn file_url_escapes() {
        let url = path_to_file_url(Path::new("/nonexistent dir/文档 #1.docx")).unwrap();
        assert_eq!(
            url,
            "file:///nonexistent%20dir/%E6%96%87%E6%A1%A3%20%231.docx"
        );
    }

    #[cfg(windows)]
    #[test]
    fn file_url_escapes() {
        let url = path_to_file_url(Path::new(r"Q:\nonexistent dir\文档 #1.docx")).unwrap();
        assert_eq!(
            url,
            "file:///Q:/nonexistent%20dir/%E6%96%87%E6%A1%A3%20%231.docx"
        );
        let url = path_to_file_url(Path::new(r"\\server\share\a b.docx")).unwrap();
        assert_eq!(url, "file://server/share/a%20b.docx");
    }
}
