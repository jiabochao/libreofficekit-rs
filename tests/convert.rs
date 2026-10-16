//! Integration tests against a real LibreOffice. They are skipped (with a message) when none
//! is found; point `LOK_PROGRAM_DIR` at an installation or runtime to run them.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libreofficekit::{DocumentType, Error, LoadOptions, Office, PdfOptions, PdfVersion};

fn office() -> Option<&'static Office> {
    static OFFICE: OnceLock<Option<Office>> = OnceLock::new();
    OFFICE
        .get_or_init(|| {
            let dir = std::env::var_os("LOK_PROGRAM_DIR")
                .map(PathBuf::from)
                .or_else(|| libreofficekit::bundled_runtime_dir().map(Path::to_path_buf))
                .or_else(libreofficekit::locate::find_installation)?;
            let profile =
                std::env::temp_dir().join(format!("lok-test-profile-{}", std::process::id()));
            Some(
                Office::builder()
                    .install_dir(dir)
                    .user_profile_dir(profile)
                    .build()
                    .expect("LibreOfficeKit starts"),
            )
        })
        .as_ref()
}

macro_rules! office_or_skip {
    () => {
        match office() {
            Some(o) => o,
            None => {
                eprintln!("skipped: no LibreOffice installation found");
                return;
            }
        }
    };
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn assert_pdf(path: &Path) -> Vec<u8> {
    let data = std::fs::read(path).unwrap();
    assert!(
        data.starts_with(b"%PDF-"),
        "{} is not a PDF",
        path.display()
    );
    data
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn converts_office_formats() {
    let office = office_or_skip!();
    let out = tempfile::tempdir().unwrap();
    for name in [
        "sample.docx",
        "sample.xlsx",
        "sample.pptx",
        "sample.odt",
        "sample.rtf",
        "sample.csv",
    ] {
        let pdf = out.path().join(format!("{name}.pdf"));
        office
            .convert_to_pdf(fixture(name), &pdf, &PdfOptions::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_pdf(&pdf);
    }
}

#[test]
fn reports_document_type() {
    let office = office_or_skip!();
    let types = [
        ("sample.docx", DocumentType::Text),
        ("sample.xlsx", DocumentType::Spreadsheet),
        ("sample.pptx", DocumentType::Presentation),
    ];
    for (name, ty) in types {
        assert_eq!(
            office
                .load(fixture(name), &LoadOptions::default())
                .unwrap()
                .document_type(),
            ty
        );
    }
}

#[test]
fn applies_pdf_options() {
    let office = office_or_skip!();
    let out = tempfile::tempdir().unwrap();

    let pdf = out.path().join("a2b.pdf");
    let opts = PdfOptions {
        version: Some(PdfVersion::PdfA2b),
        ..Default::default()
    };
    office
        .convert_to_pdf(fixture("sample.docx"), &pdf, &opts)
        .unwrap();
    assert!(
        contains(&assert_pdf(&pdf), b"pdfaid:part"),
        "missing PDF/A metadata"
    );

    let pdf = out.path().join("encrypted.pdf");
    let opts = PdfOptions {
        open_password: Some("s3cret, with comma".into()),
        ..Default::default()
    };
    office
        .convert_to_pdf(fixture("sample.docx"), &pdf, &opts)
        .unwrap();
    assert!(
        contains(&assert_pdf(&pdf), b"/Encrypt"),
        "PDF is not encrypted"
    );

    let pdf = out.path().join("range.pdf");
    let opts = PdfOptions {
        page_range: Some("2".into()),
        ..Default::default()
    };
    office
        .convert_to_pdf(fixture("sample.pptx"), &pdf, &opts)
        .unwrap();
    let data = assert_pdf(&pdf);
    let pages = data
        .windows(11)
        .filter(|w| w == b"/Type/Page/" || w == b"/Type /Page")
        .count();
    assert!(pages <= 1, "page range not applied ({pages} pages)");
}

#[test]
fn password_protected_documents() {
    let office = office_or_skip!();
    let out = tempfile::tempdir().unwrap();
    let encrypted = out.path().join("encrypted.odt");
    office
        .load(fixture("sample.odt"), &LoadOptions::default())
        .unwrap()
        .save_as(&encrypted, Some("odt"), Some(",Password=pa55PASSWORDEND"))
        .unwrap();

    let err = office
        .load(&encrypted, &LoadOptions::default())
        .err()
        .expect("load without password fails");
    assert!(matches!(err, Error::PasswordRequired), "{err:?}");

    let wrong = LoadOptions {
        password: Some("nope".into()),
        ..Default::default()
    };
    let err = office
        .load(&encrypted, &wrong)
        .err()
        .expect("wrong password fails");
    assert!(matches!(err, Error::WrongPassword), "{err:?}");

    let right = LoadOptions {
        password: Some("pa55".into()),
        ..Default::default()
    };
    let pdf = out.path().join("decrypted.pdf");
    office
        .convert_to_pdf_with(&encrypted, &pdf, &right, &PdfOptions::default())
        .unwrap();
    assert_pdf(&pdf);
}

#[test]
fn errors() {
    let office = office_or_skip!();
    let out = tempfile::tempdir().unwrap();
    assert!(matches!(
        office.convert_to_pdf(
            out.path().join("missing.docx"),
            out.path().join("x.pdf"),
            &PdfOptions::default()
        ),
        Err(Error::Io(_))
    ));
    // A directory is not a document.
    assert!(
        office
            .convert_to_pdf(out.path(), out.path().join("d.pdf"), &PdfOptions::default())
            .is_err()
    );

    assert!(Office::is_initialized());
    assert!(matches!(
        Office::builder().program_dir(office.program_dir()).build(),
        Err(Error::AlreadyInitialized)
    ));
}

#[test]
fn unicode_paths() {
    let office = office_or_skip!();
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().join("文档 #1 %20");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("报告 (final).docx");
    std::fs::copy(fixture("sample.docx"), &input).unwrap();
    let pdf = dir.join("输出.pdf");
    office
        .convert_to_pdf(&input, &pdf, &PdfOptions::default())
        .unwrap();
    assert_pdf(&pdf);
}
