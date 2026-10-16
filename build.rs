fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "bundled")]
    bundled::run();
}

#[cfg(feature = "bundled")]
mod bundled {
    use libreofficekit_build as rt;
    use std::path::PathBuf;

    pub fn run() {
        for var in [
            "LIBREOFFICEKIT_RUNTIME_DIR",
            "LIBREOFFICEKIT_CACHE_DIR",
            "LIBREOFFICEKIT_RUNTIME_BASE_URL",
            "LIBREOFFICEKIT_FORCE_LOCAL_BUILD",
        ] {
            println!("cargo:rerun-if-env-changed={var}");
        }
        if std::env::var_os("DOCS_RS").is_some() {
            return;
        }

        // A runtime prepared elsewhere (e.g. by CI) short-circuits everything.
        let dir = if let Some(dir) = std::env::var_os("LIBREOFFICEKIT_RUNTIME_DIR") {
            PathBuf::from(dir)
        } else {
            let target = std::env::var("TARGET").unwrap();
            let platform = rt::Platform::from_target_triple(&target).unwrap_or_else(|| {
                panic!("libreofficekit: no LibreOffice runtime for target {target}")
            });
            let cache = std::env::var_os("LIBREOFFICEKIT_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(rt::default_cache_dir);
            match rt::build_support::prepare(
                &cache,
                platform,
                std::env::var_os("LIBREOFFICEKIT_FORCE_LOCAL_BUILD").is_some(),
            ) {
                Ok(dir) => dir,
                Err(e) => {
                    panic!("libreofficekit: failed to prepare the LibreOffice runtime: {e:#}")
                }
            }
        };
        println!(
            "cargo:rustc-env=LIBREOFFICEKIT_BUNDLED_DIR={}",
            dir.display()
        );
        println!("cargo:runtime_dir={}", dir.display());
    }
}
