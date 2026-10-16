//! Converts documents to PDF.
//!
//! ```text
//! cargo run --example convert -- [--lo DIR] INPUT... OUTPUT_DIR
//! ```
//!
//! `--lo` is a LibreOffice installation root or program directory. Without it, the runtime
//! prepared by the `bundled` feature, `LOK_PROGRAM_DIR` or a system installation is used.

use std::path::PathBuf;
use std::time::Instant;

use libreofficekit::{Office, PdfOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut builder = Office::builder();
    if let Some(i) = args.iter().position(|a| a == "--lo") {
        args.remove(i);
        builder = builder.install_dir(args.remove(i));
    } else if let Some(dir) = libreofficekit::bundled_runtime_dir() {
        builder = builder.install_dir(dir);
    }
    if args.len() < 2 {
        eprintln!("usage: convert [--lo DIR] INPUT... OUTPUT_DIR");
        std::process::exit(2);
    }
    let out_dir = PathBuf::from(args.pop().unwrap());

    let start = Instant::now();
    let office = builder.build()?;
    println!(
        "started {} in {:.2?}",
        office.library_path().display(),
        start.elapsed()
    );
    if let Some(info) = office.version_info() {
        println!("version: {info}");
    }

    for input in &args {
        let input = PathBuf::from(input);
        let output = out_dir.join(format!(
            "{}.pdf",
            input.file_name().unwrap().to_string_lossy()
        ));
        let start = Instant::now();
        match office.convert_to_pdf(&input, &output, &PdfOptions::default()) {
            Ok(()) => println!(
                "{} -> {} ({:.2?})",
                input.display(),
                output.display(),
                start.elapsed()
            ),
            Err(e) => println!("{}: {e}", input.display()),
        }
    }
    Ok(())
}
