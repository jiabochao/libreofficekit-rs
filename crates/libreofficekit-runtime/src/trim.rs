//! Removing the parts of a LibreOffice installation that document conversion does not need.
//!
//! Rules are `/`-separated glob patterns relative to the runtime root, where `*` matches
//! within one path segment and `**` matches any number of segments. Linux and Windows
//! installations use `program/` + `share/`; in a macOS bundle the same data lives in
//! `Contents/Frameworks`, `Contents/MacOS` and `Contents/Resources`.

use std::fs;
use std::path::Path;

use crate::{Platform, Result};

/// What to remove. [`TrimOptions::default`] is the configuration used for published packages.
#[derive(Debug, Clone)]
pub struct TrimOptions {
    /// UI translations to keep besides the built-in English, e.g. `["zh-CN", "ja"]`.
    /// Translations affect strings LibreOffice generates into documents (sheet names in
    /// headers, captions, ...), not just the UI.
    pub keep_languages: Vec<String>,
    /// Remove every icon theme. Headless conversion does not draw icons.
    pub remove_icon_themes: bool,
    /// Additional patterns to remove.
    pub extra_remove: Vec<String>,
    /// Patterns that are never removed, even when matched by a remove rule.
    pub keep: Vec<String>,
}

impl Default for TrimOptions {
    fn default() -> Self {
        TrimOptions {
            keep_languages: Vec::new(),
            remove_icon_themes: true,
            extra_remove: Vec::new(),
            keep: Vec::new(),
        }
    }
}

/// Data directories (`share/` equivalents) that conversion never reads.
const SHARE_DIRS: &[&str] = &[
    "gallery",
    "template",
    "wizards",
    "Scripts",
    "autocorr",
    "autotext",
    "wordbook",
    "xpdfimport",
    "firebird",
    "tipoftheday",
    "extensions",
    "labels",
    "config/wizard",
];

/// Program files (executables, interpreters) LibreOfficeKit does not load.
const PROGRAM_FILES: &[&str] = &[
    "python*",
    "pyuno*",
    "pythonloader*",
    "libpyuno*",
    "libpythonloader*",
    "uno.py",
    "unohelper.py",
    "officehelper.py",
    "msgbox.py",
    "mailmerge.py",
    "wizards",
    "classes",
    "xpdfimport*",
    "opencltest*",
    "gengal*",
    "senddoc*",
    "ui-previewer*",
    "unopkg*",
    "sbase*",
    "scalc*",
    "sdraw*",
    "simpress*",
    "smath*",
    "swriter*",
    "quickstart*",
    "soffice_safe*",
    "update*",
    "shell",
    "twain32shim*",
    "minidump_upload*",
    "regmerge*",
    "regview*",
    "uri-encode*",
];

/// The default remove patterns for `platform`.
pub fn remove_patterns(platform: Platform, options: &TrimOptions) -> Vec<String> {
    let (share, programs): (&[&str], &[&str]) = if platform.is_macos() {
        (
            &["Contents/Resources"],
            &[
                "Contents/Frameworks",
                "Contents/MacOS",
                "Contents/Resources",
            ],
        )
    } else {
        (&["share"], &["program"])
    };
    let resources = if platform.is_macos() {
        "Contents/Resources"
    } else {
        "program"
    };

    let mut p: Vec<String> = vec![
        "help".into(),
        "readmes".into(),
        "*.msi".into(),
        "Contents/Resources/help".into(),
        "Contents/Frameworks/LibreOfficePython.framework".into(),
        "Contents/Library/Spotlight".into(),
        "Contents/Library/QuickLook".into(),
    ];
    for base in share {
        p.extend(SHARE_DIRS.iter().map(|d| format!("{base}/{d}")));
        if options.remove_icon_themes {
            p.push(format!("{base}/config/images_*.zip"));
        }
        // Language specific configuration layers.
        p.push(format!("{base}/registry/res/registry_*.xcd"));
        p.push(format!("{base}/registry/res/fcfg_langpack_*.xcd"));
    }
    for base in programs {
        p.extend(PROGRAM_FILES.iter().map(|f| format!("{base}/{f}")));
    }
    // UI translations (English is built in).
    p.push(format!("{resources}/resource/*"));
    p.extend(options.extra_remove.iter().cloned());
    p
}

/// Patterns protected from removal: licenses, the kept languages and `options.keep`.
pub fn keep_patterns(platform: Platform, options: &TrimOptions) -> Vec<String> {
    let share = if platform.is_macos() {
        "Contents/Resources"
    } else {
        "share"
    };
    let resources = if platform.is_macos() {
        "Contents/Resources"
    } else {
        "program"
    };
    let mut k: Vec<String> = vec![
        "LICENSE*".into(),
        "NOTICE*".into(),
        "CREDITS*".into(),
        "license*".into(),
    ];
    for lang in std::iter::once("en-US").chain(options.keep_languages.iter().map(String::as_str)) {
        k.push(format!("{share}/registry/res/registry_{lang}.xcd"));
        k.push(format!("{share}/registry/res/fcfg_langpack_{lang}.xcd"));
        k.push(format!("{resources}/resource/{lang}"));
        // Translations are also stored under the bare language (`zh-CN` -> `zh_CN`).
        k.push(format!("{resources}/resource/{}", lang.replace('-', "_")));
    }
    k.extend(options.keep.iter().cloned());
    k
}

/// Statistics of a [`trim`] run.
#[derive(Debug, Default, Clone, Copy)]
pub struct TrimReport {
    pub removed_entries: u64,
    pub removed_bytes: u64,
}

/// Removes everything matched by the remove patterns (and not by a keep pattern) below `root`.
/// With `dry_run`, only reports what would be removed.
pub fn trim(
    root: &Path,
    platform: Platform,
    options: &TrimOptions,
    dry_run: bool,
) -> Result<TrimReport> {
    let remove = remove_patterns(platform, options);
    let keep = keep_patterns(platform, options);
    let mut report = TrimReport::default();
    walk(root, root, &remove, &keep, dry_run, &mut report)?;
    Ok(report)
}

fn walk(
    root: &Path,
    dir: &Path,
    remove: &[String],
    keep: &[String],
    dry_run: bool,
    report: &mut TrimReport,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .expect("below root")
            .to_string_lossy()
            .replace('\\', "/");
        let ty = entry.file_type()?;
        let kept = keep.iter().any(|k| glob_match(k, &rel));
        let protects_child = ty.is_dir() && keep.iter().any(|k| k.starts_with(&format!("{rel}/")));
        if !kept && !protects_child && remove.iter().any(|r| glob_match(r, &rel)) {
            let bytes = size_of(&path)?;
            report.removed_entries += 1;
            report.removed_bytes += bytes;
            if !dry_run {
                if ty.is_dir() {
                    fs::remove_dir_all(&path)?;
                } else {
                    fs::remove_file(&path)?;
                }
            }
        } else if ty.is_dir() && !kept {
            walk(root, &path, remove, keep, dry_run, report)?;
        }
    }
    Ok(())
}

fn size_of(path: &Path) -> Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        total += size_of(&entry?.path())?;
    }
    Ok(total)
}

/// Total size of a directory tree in bytes (symlinks are not followed).
pub fn dir_size(path: &Path) -> Result<u64> {
    size_of(path)
}

/// Matches a `/`-separated path against a pattern where `*` matches within a segment and a
/// `**` segment matches any number of segments.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let segs: Vec<&str> = path.split('/').collect();
    match_segments(&pat, &segs)
}

fn match_segments(pat: &[&str], segs: &[&str]) -> bool {
    match pat.split_first() {
        None => segs.is_empty(),
        Some((&"**", rest)) => (0..=segs.len()).any(|i| match_segments(rest, &segs[i..])),
        Some((p, rest)) => match segs.split_first() {
            Some((s, srest)) => wildcard(p.as_bytes(), s.as_bytes()) && match_segments(rest, srest),
            None => false,
        },
    }
}

fn wildcard(p: &[u8], s: &[u8]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| wildcard(rest, &s[i..])),
        Some((c, rest)) => s.first() == Some(c) && wildcard(rest, &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("program/python*", "program/python-core-3.11"));
        assert!(glob_match(
            "share/config/images_*.zip",
            "share/config/images_colibre.zip"
        ));
        assert!(!glob_match(
            "share/config/images_*.zip",
            "share/config/soffice.cfg"
        ));
        assert!(glob_match("**/*.pyc", "a/b/c.pyc"));
        assert!(!glob_match("help", "share/help"));
    }

    #[test]
    fn keeps_languages() {
        let opts = TrimOptions {
            keep_languages: vec!["zh-CN".into()],
            ..Default::default()
        };
        let keep = keep_patterns(Platform::LinuxX86_64, &opts);
        assert!(keep.iter().any(|k| glob_match(k, "program/resource/zh-CN")));
        assert!(
            keep.iter()
                .any(|k| glob_match(k, "share/registry/res/registry_zh-CN.xcd"))
        );
        let remove = remove_patterns(Platform::LinuxX86_64, &opts);
        assert!(remove.iter().any(|r| glob_match(r, "program/resource/de")));
    }
}
