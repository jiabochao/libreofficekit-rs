//! Downloading and installing prebuilt runtime packages.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::{
    DEFAULT_BASE_URL, Error, Platform, Result, Runtime, asset_name, asset_url, hex, package_stem,
};

/// Installation progress, reported to the callback passed to [`Installer::install`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// `total` is `None` when the server sends no length.
    Downloading {
        downloaded: u64,
        total: Option<u64>,
    },
    Verifying,
    Extracting {
        entries: u64,
    },
    Done,
}

/// Downloads and installs the runtime for one platform into `<root>/<package stem>/`.
///
/// ```no_run
/// use std::sync::atomic::AtomicBool;
/// use libreofficekit_runtime::{Installer, Progress};
///
/// let installer = Installer::new("/path/to/app-data/libreoffice");
/// let runtime = installer.install(&mut |p: Progress| println!("{p:?}"), &AtomicBool::new(false))?;
/// println!("program dir: {}", runtime.program_dir().display());
/// # Ok::<(), libreofficekit_runtime::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Installer {
    root: PathBuf,
    platform: Option<Platform>,
    base_urls: Vec<String>,
    sha256: Option<String>,
}

impl Installer {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let base_urls = std::env::var("LIBREOFFICEKIT_RUNTIME_BASE_URL")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec![DEFAULT_BASE_URL.to_string()]);
        Installer {
            root: root.into(),
            platform: Platform::current(),
            base_urls,
            sha256: None,
        }
    }

    /// Install the runtime for another platform (defaults to the current one).
    pub fn platform(mut self, platform: Platform) -> Self {
        self.platform = Some(platform);
        self
    }

    /// Release download base URLs, tried in order (e.g. a CDN mirror first, then GitHub).
    /// The package is fetched from `<base>/<release tag>/<asset name>`.
    pub fn base_urls<I, S>(mut self, urls: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.base_urls = urls.into_iter().map(Into::into).collect();
        self
    }

    /// Expected SHA-256 of the package. Defaults to the pinned hash, then to the `.sha256`
    /// file published next to the package.
    pub fn sha256(mut self, hash: impl Into<String>) -> Self {
        self.sha256 = Some(hash.into().to_ascii_lowercase());
        self
    }

    fn require_platform(&self) -> Result<Platform> {
        self.platform.ok_or_else(|| {
            Error::UnsupportedPlatform(format!(
                "{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
        })
    }

    /// The directory the runtime is (or will be) installed in.
    pub fn install_dir(&self) -> Result<PathBuf> {
        Ok(self.root.join(package_stem(self.require_platform()?)))
    }

    /// The installed runtime, if it is complete.
    pub fn installed(&self) -> Option<Runtime> {
        Runtime::open(self.install_dir().ok()?)
            .ok()
            .filter(|rt| rt.is_current())
    }

    /// Downloads and installs the runtime unless it is already installed. A partial download
    /// is resumed. Blocks; run it on a background thread. Set `cancel` to abort.
    pub fn install(
        &self,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<Runtime> {
        if let Some(rt) = self.installed() {
            progress(Progress::Done);
            return Ok(rt);
        }
        let platform = self.require_platform()?;
        let downloads = self.root.join(".downloads");
        fs::create_dir_all(&downloads)?;
        let archive = downloads.join(asset_name(platform));

        let mut last_err = None;
        let mut downloaded = false;
        for base in &self.base_urls {
            match self.download_from(base, platform, &archive, progress, cancel) {
                Ok(()) => {
                    downloaded = true;
                    break;
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => last_err = Some(e),
            }
        }
        if !downloaded {
            return Err(
                last_err.unwrap_or_else(|| Error::Invalid("no download URL configured".into()))
            );
        }

        let rt = self.install_archive(&archive, progress, cancel)?;
        let _ = fs::remove_file(&archive);
        Ok(rt)
    }

    /// Installs from a package file that is already on disk (e.g. shipped with the app or
    /// downloaded by other means). Its checksum is not verified here.
    pub fn install_archive(
        &self,
        archive: &Path,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<Runtime> {
        let dir = self.install_dir()?;
        let staging = self.root.join(format!(
            ".staging-{}",
            package_stem(self.require_platform()?)
        ));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        progress(Progress::Extracting { entries: 0 });
        let res = crate::unpack(archive, &staging, cancel, &mut |entries| {
            if entries % 200 == 0 {
                progress(Progress::Extracting { entries });
            }
        });
        if let Err(e) = res {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
        let rt = match Runtime::open(&staging) {
            Ok(rt) => rt,
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        if Some(rt.manifest.platform) != self.platform || !rt.is_current() {
            let _ = fs::remove_dir_all(&staging);
            return Err(Error::Invalid(format!(
                "package is LibreOffice {} r{} for {}, expected {} r{} for {}",
                rt.manifest.lo_version,
                rt.manifest.revision,
                rt.manifest.platform,
                crate::LO_VERSION,
                crate::REVISION,
                self.require_platform()?
            )));
        }
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::rename(&staging, &dir)?;
        progress(Progress::Done);
        Runtime::open(dir)
    }

    /// Removes runtimes of other versions and leftover temporary files below the root.
    pub fn remove_stale(&self) -> Result<()> {
        let keep = self.install_dir()?;
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let stale = name.starts_with(".staging-")
                || (name.starts_with("libreoffice-")
                    && path != keep
                    && path.join(crate::MANIFEST_FILE).exists());
            if stale {
                fs::remove_dir_all(&path)?;
            }
        }
        Ok(())
    }

    /// Removes the installed runtime of this version.
    pub fn uninstall(&self) -> Result<()> {
        let dir = self.install_dir()?;
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    fn download_from(
        &self,
        base: &str,
        platform: Platform,
        archive: &Path,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<()> {
        let url = asset_url(base, platform);
        let expected = match self
            .sha256
            .clone()
            .or_else(|| crate::pinned_sha256(platform).map(str::to_string))
        {
            Some(h) => h,
            None => {
                let text = get_text(&format!("{url}.sha256"))?;
                crate::parse_sha256(&text).ok_or_else(|| {
                    Error::Invalid(format!("malformed checksum file at {url}.sha256"))
                })?
            }
        };
        download(&url, archive, progress, cancel)?;
        progress(Progress::Verifying);
        let actual = sha256_file(archive)?;
        if actual != expected {
            let _ = fs::remove_file(archive);
            return Err(Error::Checksum { expected, actual });
        }
        Ok(())
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_body(None)
        .user_agent(concat!(
            "libreofficekit-runtime/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .into()
}

fn http_err(url: &str, e: impl std::fmt::Display) -> Error {
    Error::Http {
        url: url.to_string(),
        message: e.to_string(),
    }
}

fn get_text(url: &str) -> Result<String> {
    let mut resp = agent().get(url).call().map_err(|e| http_err(url, e))?;
    resp.body_mut()
        .read_to_string()
        .map_err(|e| http_err(url, e))
}

/// Downloads `url` to `dest`, resuming from `dest.part` if a previous attempt was interrupted.
pub(crate) fn download(
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<()> {
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let mut offset = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

    let agent = agent();
    let mut req = agent.get(url);
    if offset > 0 {
        req = req.header("Range", &format!("bytes={offset}-"));
    }
    let resp = match req.call() {
        // The part file is already complete (or bigger than the resource): start over.
        Err(ureq::Error::StatusCode(416)) => {
            fs::remove_file(&part)?;
            offset = 0;
            agent.get(url).call().map_err(|e| http_err(url, e))?
        }
        r => r.map_err(|e| http_err(url, e))?,
    };

    let resumed = offset > 0 && resp.status().as_u16() == 206;
    if !resumed {
        offset = 0;
    }
    let length = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let total = length.map(|l| l + offset);

    let mut file = if resumed {
        OpenOptions::new().append(true).open(&part)?
    } else {
        File::create(&part)?
    };
    let mut reader = resp.into_body().into_reader();
    let mut buf = vec![0u8; 256 * 1024];
    let mut downloaded = offset;
    let mut last_report = 0;
    progress(Progress::Downloading { downloaded, total });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = reader.read(&mut buf).map_err(|e| http_err(url, e))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        if downloaded - last_report >= 1 << 20 {
            last_report = downloaded;
            progress(Progress::Downloading { downloaded, total });
        }
    }
    file.sync_all()?;
    drop(file);
    if let Some(total) = total {
        if downloaded != total {
            return Err(http_err(
                url,
                format!("connection closed after {downloaded} of {total} bytes"),
            ));
        }
    }
    progress(Progress::Downloading {
        downloaded,
        total: Some(downloaded),
    });
    fs::rename(&part, dest)?;
    Ok(())
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}
