//! `lok-pack`: build, trim, pack and install LibreOffice runtimes.
//!
//! ```text
//! lok-pack build   [--platform P] [--out DIR] [--cache DIR] [--keep-lang zh-CN,ja] [--level N] [--no-pack]
//! lok-pack trim    DIR [--platform P] [--keep-lang ...] [--dry-run]
//! lok-pack pack    DIR [--out DIR] [--level N]
//! lok-pack export  OUT_DIR [--platform P] [--from RUNTIME_DIR] [--bundle-id ID] [--local]
//! lok-pack sign    LibreOffice.bundle --identity ID [--no-timestamp]
//! lok-pack install DIR [--platform P] [--base-url URL[,URL...]] [--archive FILE]
//! lok-pack info
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use libreofficekit_runtime::export::{self, ExportOptions};
use libreofficekit_runtime::trim::{self, TrimOptions};
use libreofficekit_runtime::{Installer, LO_VERSION, Platform, Progress, REVISION, builder};

struct Args {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse() -> Args {
        let mut positional = Vec::new();
        let mut flags = Vec::new();
        let mut it = std::env::args().skip(1).peekable();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if let Some((k, v)) = name.split_once('=') {
                    flags.push((k.to_string(), Some(v.to_string())));
                } else if it.peek().is_some_and(|n| !n.starts_with("--"))
                    && !matches!(name, "dry-run" | "no-pack" | "local" | "no-timestamp")
                {
                    flags.push((name.to_string(), it.next()));
                } else {
                    flags.push((name.to_string(), None));
                }
            } else {
                positional.push(a);
            }
        }
        Args { positional, flags }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(k, _)| k == name)
    }

    fn platform(&self) -> Result<Platform, String> {
        match self.get("platform") {
            Some(p) => p.parse().map_err(|e| format!("{e}")),
            None => Platform::current()
                .ok_or_else(|| "unsupported host platform, pass --platform".into()),
        }
    }

    fn trim_options(&self) -> TrimOptions {
        TrimOptions {
            keep_languages: self
                .get("keep-lang")
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            ..Default::default()
        }
    }

    fn level(&self) -> Result<i32, String> {
        self.get("level").map_or(Ok(19), |l| {
            l.parse().map_err(|_| format!("bad --level {l}"))
        })
    }
}

fn progress() -> impl FnMut(Progress) {
    let mut last = 0u64;
    move |p| match p {
        Progress::Downloading { downloaded, total } => {
            if downloaded - last >= 20 << 20 || Some(downloaded) == total {
                last = downloaded;
                match total {
                    Some(t) => eprintln!("  downloaded {} / {} MiB", downloaded >> 20, t >> 20),
                    None => eprintln!("  downloaded {} MiB", downloaded >> 20),
                }
            }
        }
        Progress::Verifying => eprintln!("  verifying checksum"),
        Progress::Extracting { entries: 0 } => eprintln!("  extracting"),
        _ => {}
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1 << 20) as f64)
}

fn run(args: &Args) -> Result<(), String> {
    let cmd = args
        .positional
        .first()
        .map(String::as_str)
        .unwrap_or("help");
    let e = |e: libreofficekit_runtime::Error| e.to_string();
    match cmd {
        "build" => {
            let platform = args.platform()?;
            let out = PathBuf::from(args.get("out").unwrap_or("dist"));
            let cache = args
                .get("cache")
                .map(PathBuf::from)
                .unwrap_or_else(|| libreofficekit_runtime::default_cache_dir().join("official"));
            let work = out.join(libreofficekit_runtime::package_stem(platform));
            eprintln!("building LibreOffice {LO_VERSION} r{REVISION} for {platform}");
            eprintln!("  source {}", builder::official_url(platform, LO_VERSION));
            let report = builder::build_runtime(
                platform,
                &cache,
                &work,
                &args.trim_options(),
                &mut progress(),
            )
            .map_err(e)?;
            let size = trim::dir_size(&work).map_err(e)?;
            eprintln!(
                "  trimmed {} ({} entries), runtime is {}",
                mib(report.removed_bytes),
                report.removed_entries,
                mib(size)
            );
            if !args.has("no-pack") {
                eprintln!("  packing (zstd level {})", args.level()?);
                let archive = builder::pack_runtime(&work, &out, args.level()?).map_err(e)?;
                let size = std::fs::metadata(&archive)
                    .map_err(|e| e.to_string())?
                    .len();
                println!("{} ({})", archive.display(), mib(size));
            } else {
                println!("{}", work.display());
            }
            Ok(())
        }
        "trim" => {
            let dir = PathBuf::from(args.positional.get(1).ok_or("trim needs a directory")?);
            let report = trim::trim(
                &dir,
                args.platform()?,
                &args.trim_options(),
                args.has("dry-run"),
            )
            .map_err(e)?;
            println!(
                "{} {} in {} entries",
                if args.has("dry-run") {
                    "would remove"
                } else {
                    "removed"
                },
                mib(report.removed_bytes),
                report.removed_entries
            );
            Ok(())
        }
        "pack" => {
            let dir = PathBuf::from(
                args.positional
                    .get(1)
                    .ok_or("pack needs a runtime directory")?,
            );
            let out = PathBuf::from(args.get("out").unwrap_or("dist"));
            let archive = builder::pack_runtime(&dir, &out, args.level()?).map_err(e)?;
            println!("{}", archive.display());
            Ok(())
        }
        "export" => {
            let out = PathBuf::from(
                args.positional
                    .get(1)
                    .ok_or("export needs an output directory")?,
            );
            let runtime = match args.get("from") {
                Some(dir) => PathBuf::from(dir),
                None => libreofficekit_runtime::build_support::prepare(
                    &libreofficekit_runtime::default_cache_dir(),
                    args.platform()?,
                    args.has("local"),
                )
                .map_err(e)?,
            };
            let mut options = ExportOptions::default();
            if let Some(id) = args.get("bundle-id") {
                options.bundle_id = id.to_string();
            }
            let dir = export::export(&runtime, &out, &options).map_err(e)?;
            println!("{}", dir.display());
            Ok(())
        }
        "sign" => {
            let bundle = PathBuf::from(
                args.positional
                    .get(1)
                    .ok_or("sign needs a LibreOffice.bundle")?,
            );
            let identity = args
                .get("identity")
                .ok_or("sign needs --identity (\"-\" for ad-hoc)")?;
            export::sign_macos(&bundle, identity, !args.has("no-timestamp")).map_err(e)?;
            println!("signed {}", bundle.display());
            Ok(())
        }
        "install" => {
            let dir = PathBuf::from(
                args.positional
                    .get(1)
                    .ok_or("install needs a target directory")?,
            );
            let mut installer = Installer::new(dir).platform(args.platform()?);
            if let Some(urls) = args.get("base-url") {
                installer = installer.base_urls(urls.split(','));
            }
            let cancel = AtomicBool::new(false);
            let rt = match args.get("archive") {
                Some(a) => installer.install_archive(a.as_ref(), &mut progress(), &cancel),
                None => installer.install(&mut progress(), &cancel),
            }
            .map_err(e)?;
            println!("{}", rt.dir.display());
            Ok(())
        }
        "info" => {
            println!("LibreOffice {LO_VERSION}, package revision {REVISION}");
            println!("release tag: {}", libreofficekit_runtime::release_tag());
            for p in Platform::ALL {
                println!(
                    "{:16} {}",
                    p.as_str(),
                    libreofficekit_runtime::asset_name(p)
                );
                println!("{:16} <- {}", "", builder::official_url(p, LO_VERSION));
            }
            Ok(())
        }
        _ => {
            println!(
                "{}",
                include_str!("lok-pack.rs")
                    .lines()
                    .skip(2)
                    .take(7)
                    .map(|l| l.trim_start_matches("//! "))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    match run(&Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
