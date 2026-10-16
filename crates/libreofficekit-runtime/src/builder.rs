//! Building runtime packages from official LibreOffice releases.
//!
//! | platform | official package | extraction |
//! |----------|------------------|------------|
//! | Linux    | `*_deb.tar.gz`   | in-process: only the needed `.deb`s are unpacked |
//! | Windows  | `.msi`           | `msiexec /a` on Windows, `msiextract` (msitools) elsewhere |
//! | macOS    | `.dmg`           | `hdiutil` on macOS, `7z` elsewhere |

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use crate::install::{download, sha256_file};
use crate::trim::{self, TrimOptions};
use crate::{Error, LO_VERSION, Manifest, Platform, Progress, Result};

/// Base URL of the LibreOffice download archive, which keeps every release. Override with
/// `LIBREOFFICEKIT_OFFICIAL_MIRROR` (same directory layout).
pub const OFFICIAL_ARCHIVE: &str = "https://downloadarchive.documentfoundation.org/libreoffice/old";

/// Debian packages of the official Linux release that are not unpacked. Matched against the
/// component name, e.g. `pyuno` for `libobasis26.2-pyuno_26.2.6.3-3_amd64.deb`.
pub const SKIPPED_DEB_COMPONENTS: &[&str] = &[
    "base",
    "pyuno",
    "python-script-provider",
    "postgresql-sdbc",
    "firebird",
    "librelogo",
    "kde-integration",
    "gnome-integration",
    "onlineupdate",
    "ogltrans",
    "debian-menus",
    "libreofficekit-data",
];

/// Component prefixes of skipped Debian packages (bundled extensions and dictionaries).
pub const SKIPPED_DEB_PREFIXES: &[&str] = &["extension-", "dict-"];

/// The official package for `platform`.
pub fn official_url(platform: Platform, version: &str) -> String {
    let base = std::env::var("LIBREOFFICEKIT_OFFICIAL_MIRROR")
        .unwrap_or_else(|_| OFFICIAL_ARCHIVE.to_string());
    let path = match platform {
        Platform::LinuxX86_64 => {
            format!("deb/x86_64/LibreOffice_{version}_Linux_x86-64_deb.tar.gz")
        }
        Platform::LinuxAarch64 => {
            format!("deb/aarch64/LibreOffice_{version}_Linux_aarch64_deb.tar.gz")
        }
        Platform::MacosX86_64 => format!("mac/x86_64/LibreOffice_{version}_MacOS_x86-64.dmg"),
        Platform::MacosAarch64 => format!("mac/aarch64/LibreOffice_{version}_MacOS_aarch64.dmg"),
        Platform::WindowsX86_64 => format!("win/x86_64/LibreOffice_{version}_Win_x86-64.msi"),
        Platform::WindowsAarch64 => format!("win/aarch64/LibreOffice_{version}_Win_aarch64.msi"),
    };
    format!("{}/{version}/{path}", base.trim_end_matches('/'))
}

/// Downloads (or reuses from `cache_dir`) the official package and verifies it against the
/// `.sha256` file published with it.
pub fn fetch_official(
    platform: Platform,
    version: &str,
    cache_dir: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf> {
    let url = official_url(platform, version);
    let name = url.rsplit('/').next().expect("url has a file name");
    fs::create_dir_all(cache_dir)?;
    let dest = cache_dir.join(name);

    let expected = {
        let mut resp = ureq::get(&format!("{url}.sha256"))
            .call()
            .map_err(|e| Error::Http {
                url: format!("{url}.sha256"),
                message: e.to_string(),
            })?;
        let text = resp.body_mut().read_to_string().map_err(|e| Error::Http {
            url: format!("{url}.sha256"),
            message: e.to_string(),
        })?;
        crate::parse_sha256(&text)
            .ok_or_else(|| Error::Invalid(format!("malformed checksum at {url}.sha256")))?
    };
    if dest.is_file() && sha256_file(&dest)? == expected {
        return Ok(dest);
    }
    download(&url, &dest, progress, &AtomicBool::new(false))?;
    progress(Progress::Verifying);
    let actual = sha256_file(&dest)?;
    if actual != expected {
        let _ = fs::remove_file(&dest);
        return Err(Error::Checksum { expected, actual });
    }
    Ok(dest)
}

/// Extracts an official package into `dest`, which then is a runtime root without manifest:
/// `program/` + `share/` for Linux and Windows, `Contents/` for macOS.
pub fn extract_official(platform: Platform, package: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest)?;
    let work = dest.with_file_name(format!(
        "{}.extract",
        dest.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let res = match platform {
        Platform::LinuxX86_64 | Platform::LinuxAarch64 => extract_debs(package, dest),
        Platform::WindowsX86_64 | Platform::WindowsAarch64 => extract_msi(package, &work, dest),
        Platform::MacosX86_64 | Platform::MacosAarch64 => extract_dmg(package, &work, dest),
    };
    let _ = fs::remove_dir_all(&work);
    res
}

/// Downloads, extracts and trims the official release into `out_dir` (a runtime root with a
/// manifest), using `cache_dir` for the official package.
pub fn build_runtime(
    platform: Platform,
    cache_dir: &Path,
    out_dir: &Path,
    options: &TrimOptions,
    progress: &mut dyn FnMut(Progress),
) -> Result<trim::TrimReport> {
    let package = fetch_official(platform, LO_VERSION, cache_dir, progress)?;
    extract_official(platform, &package, out_dir)?;
    let report = trim::trim(out_dir, platform, options, false)?;
    let program_dir = out_dir.join(platform.program_dir());
    if !program_dir.is_dir() {
        return Err(Error::Invalid(format!(
            "{} missing after extraction",
            program_dir.display()
        )));
    }
    Manifest::new(platform, official_url(platform, LO_VERSION)).write(out_dir)?;
    Ok(report)
}

/// Packs a runtime directory into `<out_dir>/<asset name>` and writes `<asset>.sha256` next to
/// it. Returns the archive path.
pub fn pack_runtime(runtime_dir: &Path, out_dir: &Path, level: i32) -> Result<PathBuf> {
    let manifest = Manifest::read(runtime_dir)?;
    fs::create_dir_all(out_dir)?;
    let name = crate::asset_name(manifest.platform);
    let archive = out_dir.join(&name);
    crate::archive::pack(runtime_dir, &archive, level)?;
    let hash = sha256_file(&archive)?;
    fs::write(
        out_dir.join(format!("{name}.sha256")),
        format!("{hash}  {name}\n"),
    )?;
    Ok(archive)
}

/// The component of an official LibreOffice `.deb` file name (`core`, `pyuno`, `ure`, ...);
/// empty for the brand package.
fn deb_component(file_name: &str) -> &str {
    let package = file_name.split('_').next().unwrap_or(file_name);
    match package.split_once('-') {
        Some((_, component)) => component,
        None => "",
    }
}

fn deb_wanted(file_name: &str) -> bool {
    let c = deb_component(file_name);
    !SKIPPED_DEB_COMPONENTS.contains(&c) && !SKIPPED_DEB_PREFIXES.iter().any(|p| c.starts_with(p))
}

/// Unpacks the wanted `.deb`s inside an official `*_deb.tar.gz` into `dest`, stripping the
/// `opt/libreofficeX.Y/` prefix.
fn extract_debs(package: &Path, dest: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(BufReader::new(File::open(package)?));
    let mut outer = tar::Archive::new(gz);
    let mut found = 0;
    for entry in outer.entries()? {
        let entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !name.ends_with(".deb") || !deb_wanted(&name) {
            continue;
        }
        found += 1;
        extract_deb(entry, dest).map_err(|e| Error::Invalid(format!("{name}: {e}")))?;
    }
    if found == 0 {
        return Err(Error::Invalid(format!(
            "no .deb packages in {}",
            package.display()
        )));
    }
    Ok(())
}

fn extract_deb(deb: impl Read, dest: &Path) -> Result<()> {
    let mut ar = ar::Archive::new(deb);
    while let Some(member) = ar.next_entry() {
        let member = member?;
        let id = String::from_utf8_lossy(member.header().identifier()).into_owned();
        let data: Box<dyn Read> = match id.as_str() {
            "data.tar.xz" => Box::new(xz2::read::XzDecoder::new(member)),
            "data.tar.gz" => Box::new(flate2::read::GzDecoder::new(member)),
            "data.tar.zst" => Box::new(zstd::Decoder::new(member)?),
            "data.tar" => Box::new(member),
            _ => continue,
        };
        let mut tar = tar::Archive::new(data);
        tar.set_preserve_permissions(true);
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            // ./opt/libreoffice26.2/program/... -> program/...
            let mut comps = path
                .components()
                .filter(|c| !matches!(c, std::path::Component::CurDir));
            let (Some(opt), Some(brand)) = (comps.next(), comps.next()) else {
                continue;
            };
            if opt.as_os_str() != "opt"
                || !brand
                    .as_os_str()
                    .to_string_lossy()
                    .starts_with("libreoffice")
            {
                continue;
            }
            let rel: PathBuf = comps.collect();
            if rel.as_os_str().is_empty() {
                continue;
            }
            if rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err(Error::Invalid(format!("unsafe entry {}", path.display())));
            }
            let target = dest.join(&rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let header = entry.header();
            let mode = header.mode().unwrap_or(0o644);
            match header.entry_type() {
                tar::EntryType::Directory => fs::create_dir_all(&target)?,
                tar::EntryType::Regular | tar::EntryType::Continuous => {
                    let mut file = File::create(&target)?;
                    std::io::copy(&mut entry, &mut file)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        file.set_permissions(fs::Permissions::from_mode(mode & 0o777))?;
                    }
                    let _ = mode;
                }
                tar::EntryType::Symlink => {
                    let link = entry
                        .link_name()?
                        .ok_or_else(|| {
                            Error::Invalid(format!("symlink without target: {}", path.display()))
                        })?
                        .into_owned();
                    let _ = fs::remove_file(&target);
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&link, &target)?;
                    #[cfg(not(unix))]
                    {
                        // Resolve the link inside the package and copy its target instead.
                        let resolved = target.parent().expect("has parent").join(&link);
                        if resolved.is_file() {
                            fs::copy(&resolved, &target)?;
                        }
                    }
                }
                _ => {}
            }
        }
        return Ok(());
    }
    Err(Error::Invalid("no data member".into()))
}

pub(crate) fn run(cmd: &mut Command) -> Result<()> {
    let desc = format!("{cmd:?}");
    let status = cmd
        .status()
        .map_err(|e| Error::Tool(format!("failed to run {desc}: {e}")))?;
    if !status.success() {
        return Err(Error::Tool(format!("{desc} exited with {status}")));
    }
    Ok(())
}

fn extract_msi(package: &Path, work: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(work)?;
    let package = std::path::absolute(package)?;
    let work = std::path::absolute(work)?;
    if cfg!(windows) {
        // Administrative install: unpacks the files without installing anything.
        let mut target = std::ffi::OsString::from("TARGETDIR=");
        target.push(&work);
        run(Command::new("msiexec")
            .arg("/a")
            .arg(&package)
            .arg("/qn")
            .arg(target))?;
    } else {
        run(Command::new("msiextract")
            .arg("-C")
            .arg(&work)
            .arg(&package))
        .map_err(|e| {
            Error::Tool(format!(
                "{e} (install msitools to build Windows runtimes on this host)"
            ))
        })?;
    }
    let lib = find_file(&work, &["mergedlo.dll", "sofficeapp.dll"], 6)
        .ok_or_else(|| Error::Invalid("program/mergedlo.dll not found in the MSI".into()))?;
    let root = lib
        .parent()
        .and_then(Path::parent)
        .expect("program dir has a parent");
    move_children(root, dest)
}

fn extract_dmg(package: &Path, work: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(work)?;
    let contents = if cfg!(target_os = "macos") {
        let mount = work.join("mnt");
        fs::create_dir_all(&mount)?;
        run(Command::new("hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
            ])
            .arg(&mount)
            .arg(package))?;
        let copy = work.join("Contents");
        // ditto keeps symlinks, extended attributes and code signatures intact.
        let res = run(Command::new("ditto")
            .arg(mount.join("LibreOffice.app/Contents"))
            .arg(&copy));
        let _ = run(Command::new("hdiutil")
            .args(["detach", "-force"])
            .arg(&mount));
        res?;
        copy
    } else {
        run(Command::new("7z")
            .arg("x")
            .arg("-y")
            .arg(format!("-o{}", work.display()))
            .arg(package))
        .map_err(|e| Error::Tool(format!("{e} (macOS runtimes are best built on macOS)")))?;
        let lib =
            find_file(work, &["libmergedlo.dylib", "libsofficeapp.dylib"], 6).ok_or_else(|| {
                Error::Invalid("Contents/Frameworks/libmergedlo.dylib not found in the DMG".into())
            })?;
        lib.parent()
            .and_then(Path::parent)
            .expect("Frameworks has a parent")
            .to_path_buf()
    };
    let target = dest.join("Contents");
    fs::rename(&contents, &target).or_else(|_| copy_dir(&contents, &target))
}

fn find_file(dir: &Path, names: &[&str], depth: usize) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ty = entry.file_type().ok()?;
        if ty.is_file()
            && names
                .iter()
                .any(|n| entry.file_name().eq_ignore_ascii_case(n))
        {
            return Some(path);
        }
        if ty.is_dir() {
            dirs.push(path);
        }
    }
    if depth == 0 {
        return None;
    }
    dirs.into_iter()
        .find_map(|d| find_file(&d, names, depth - 1))
}

fn move_children(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if fs::rename(entry.path(), &target).is_err() {
            copy_dir(&entry.path(), &target)?;
        }
    }
    Ok(())
}

pub(crate) fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_dir(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
        #[cfg(not(unix))]
        fs::copy(from, to).map(drop)?;
    } else {
        fs::copy(from, to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deb_selection() {
        assert!(deb_wanted("libobasis26.2-core_26.2.6.3-3_amd64.deb"));
        assert!(deb_wanted("libreoffice26.2_26.2.6.3-3_amd64.deb"));
        assert!(deb_wanted("libreoffice26.2-ure_26.2.6.3-3_amd64.deb"));
        assert!(!deb_wanted("libobasis26.2-pyuno_26.2.6.3-3_amd64.deb"));
        assert!(!deb_wanted(
            "libobasis26.2-extension-nlpsolver_26.2.6.3-3_amd64.deb"
        ));
        assert!(!deb_wanted("libreoffice26.2-dict-en_26.2.6.3-3_amd64.deb"));
        assert!(!deb_wanted("libobasis26.2-base_26.2.6.3-3_amd64.deb"));
    }
}
