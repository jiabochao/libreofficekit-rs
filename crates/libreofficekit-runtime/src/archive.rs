//! `.tar.zst` packing and unpacking.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, Result};

/// zstd window used when packing; the decoder must allow at least this much.
pub(crate) const WINDOW_LOG: u32 = 27;

/// Unpacks a `.tar.zst` archive into `dest` (created if missing), calling `on_entry` with the
/// number of entries unpacked so far. Entries escaping `dest` are rejected.
pub fn unpack(
    archive: &Path,
    dest: &Path,
    cancel: &AtomicBool,
    on_entry: &mut dyn FnMut(u64),
) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let mut decoder = zstd::Decoder::new(BufReader::new(File::open(archive)?))?;
    decoder.window_log_max(WINDOW_LOG)?;
    let mut tar = tar::Archive::new(decoder);
    tar.set_preserve_permissions(true);
    tar.set_overwrite(true);
    let mut count = 0;
    for entry in tar.entries()? {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut entry = entry?;
        if !entry.unpack_in(dest)? {
            return Err(Error::Invalid(format!(
                "archive entry escapes the destination: {}",
                entry.path()?.display()
            )));
        }
        count += 1;
        on_entry(count);
    }
    Ok(())
}

/// Packs the contents of `src` into a `.tar.zst` file. Entries are sorted and use
/// deterministic metadata, symlinks are stored as links.
#[cfg(feature = "builder")]
pub fn pack(src: &Path, out: &Path, level: i32) -> Result<()> {
    let file = File::create(out)?;
    let mut encoder = zstd::Encoder::new(std::io::BufWriter::new(file), level)?;
    encoder.long_distance_matching(true)?;
    encoder.window_log(WINDOW_LOG)?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    encoder.multithread(threads)?;

    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    builder.mode(tar::HeaderMode::Deterministic);
    let mut entries = Vec::new();
    collect(src, src, &mut entries)?;
    entries.sort();
    for rel in entries {
        builder.append_path_with_name(src.join(&rel), &rel)?;
    }
    builder.into_inner()?.finish()?;
    Ok(())
}

#[cfg(feature = "builder")]
fn collect(root: &Path, dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        out.push(path.strip_prefix(root).expect("below root").to_path_buf());
        if entry.file_type()?.is_dir() {
            collect(root, &path, out)?;
        }
    }
    Ok(())
}
