//! Build-time preparation used by the `bundled` feature of `libreofficekit-rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::trim::TrimOptions;
use crate::{Installer, Platform, Progress, Result, Runtime, package_stem};

/// Returns a ready runtime directory for `platform` below `cache_dir`, preparing it if needed:
///
/// 1. reuse `<cache>/runtimes/<package stem>` if it is complete;
/// 2. otherwise download the prebuilt package (unless `force_local`);
/// 3. otherwise download the official LibreOffice release and trim it locally.
pub fn prepare(cache_dir: &Path, platform: Platform, force_local: bool) -> Result<PathBuf> {
    let runtimes = cache_dir.join("runtimes");
    let installer = Installer::new(&runtimes).platform(platform);
    if let Some(rt) = installer.installed() {
        return Ok(rt.dir);
    }

    let mut progress = |p: Progress| {
        if let Progress::Downloading {
            downloaded,
            total: Some(total),
        } = p
        {
            if downloaded == total {
                eprintln!("libreofficekit: downloaded {downloaded} bytes");
            }
        }
    };
    if !force_local {
        match installer.install(&mut progress, &AtomicBool::new(false)) {
            Ok(rt) => return Ok(rt.dir),
            Err(e) => println!(
                "cargo:warning=libreofficekit: no prebuilt runtime ({e}); building it locally from the official LibreOffice release"
            ),
        }
    }

    let out = runtimes.join(package_stem(platform));
    let staging = runtimes.join(format!(".local-{}", package_stem(platform)));
    crate::builder::build_runtime(
        platform,
        &cache_dir.join("official"),
        &staging,
        &TrimOptions::default(),
        &mut progress,
    )?;
    if out.exists() {
        std::fs::remove_dir_all(&out)?;
    }
    std::fs::rename(&staging, &out)?;
    Ok(Runtime::open(out)?.dir)
}
