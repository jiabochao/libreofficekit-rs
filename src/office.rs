use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::mem::offset_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::error::{Error, Result};
use crate::locate::{self, path_to_file_url};
use crate::pdf::PdfOptions;
use crate::sys::{
    self, LibreOfficeKit, LibreOfficeKitClass, LibreOfficeKitDocument, LibreOfficeKitDocumentClass,
};

/// LibreOfficeKit keeps process-global state and cannot be initialized twice, not even after
/// `destroy`. This flag enforces that.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Configures and starts LibreOfficeKit. See [`Office::builder`].
#[derive(Debug, Clone, Default)]
pub struct OfficeBuilder {
    install_dir: Option<PathBuf>,
    program_dir: Option<PathBuf>,
    library_path: Option<PathBuf>,
    user_profile_dir: Option<PathBuf>,
}

impl OfficeBuilder {
    /// An installation root, `.app` bundle or program directory; the program directory is
    /// resolved automatically (`<root>/program`, `<root>/Contents/Frameworks`, ...).
    pub fn install_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.install_dir = Some(path.into());
        self
    }

    /// The exact program directory LibreOfficeKit is initialized with (`program/` on
    /// Linux/Windows, `Contents/Frameworks` on macOS). Takes precedence over
    /// [`install_dir`](Self::install_dir).
    pub fn program_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.program_dir = Some(path.into());
        self
    }

    /// The exact shared library to load (`libmergedlo.so`, `mergedlo.dll`,
    /// `libsofficeapp.dylib`, ...). Defaults to the one found in the program directory; when
    /// only this is set, the program directory defaults to the library's directory.
    pub fn library_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.library_path = Some(path.into());
        self
    }

    /// Directory for LibreOffice's user profile. Defaults to
    /// `<temp dir>/libreofficekit-rs/profile`, so a desktop LibreOffice's profile is never
    /// shared. Use a directory inside your app's data/cache directory in sandboxed apps.
    pub fn user_profile_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_profile_dir = Some(path.into());
        self
    }

    pub fn build(self) -> Result<Office> {
        let (program_dir, library_path) = self.resolve()?;
        let profile_dir = self.user_profile_dir.unwrap_or_else(|| {
            std::env::temp_dir()
                .join("libreofficekit-rs")
                .join("profile")
        });
        std::fs::create_dir_all(&profile_dir)?;
        let profile_url = path_to_file_url(&profile_dir)?;

        if INITIALIZED
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(Error::AlreadyInitialized);
        }
        // Until the hook has been called, a failure leaves no LibreOffice state behind.
        let reset = |e: Error| {
            INITIALIZED.store(false, Ordering::SeqCst);
            e
        };

        let library = load_library(&program_dir, &library_path).map_err(reset)?;
        let program_dir_c = c_path(&program_dir).map_err(reset)?;
        let profile_url_c = CString::new(profile_url).map_err(|_| reset(nul_error()))?;

        let kit = unsafe {
            if let Ok(hook2) = library.get::<sys::Hook2Fn>(sys::HOOK_2_SYMBOL) {
                hook2(program_dir_c.as_ptr(), profile_url_c.as_ptr())
            } else if let Ok(hook) = library.get::<sys::HookFn>(sys::HOOK_SYMBOL) {
                hook(program_dir_c.as_ptr())
            } else {
                return Err(reset(Error::MissingHook(library_path)));
            }
        };
        // LibreOffice cannot be unloaded once initialized.
        std::mem::forget(library);

        if kit.is_null() || unsafe { (*kit).pClass.is_null() } {
            return Err(Error::InitFailed(program_dir));
        }

        let office = Office {
            kit,
            lock: Mutex::new(()),
            callback: Box::new(CallbackState {
                kit,
                password: Mutex::new(None),
                password_requests: AtomicU32::new(0),
            }),
            program_dir,
            library_path,
        };
        office.enable_password_callback();
        Ok(office)
    }

    fn resolve(&self) -> Result<(PathBuf, PathBuf)> {
        let program_dir = match (&self.program_dir, &self.install_dir, &self.library_path) {
            (Some(dir), _, _) => dir.clone(),
            (None, Some(dir), _) => locate::resolve_program_dir(dir)
                .ok_or_else(|| Error::InstallationNotFound(dir.clone()))?,
            (None, None, Some(lib)) => lib
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| Error::InstallationNotFound(lib.clone()))?,
            (None, None, None) => locate::find_installation()
                .ok_or_else(|| Error::InstallationNotFound(PathBuf::new()))?,
        };
        let program_dir = std::path::absolute(&program_dir)?;
        let library_path = match &self.library_path {
            Some(lib) => lib.clone(),
            None => locate::find_library(&program_dir)
                .ok_or_else(|| Error::InstallationNotFound(program_dir.clone()))?,
        };
        if !library_path.is_file() {
            return Err(Error::InstallationNotFound(library_path));
        }
        Ok((program_dir, library_path))
    }
}

/// Options for [`Office::load`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Password for encrypted documents.
    pub password: Option<String>,
    /// BCP 47 language tag used for locale dependent formatting while loading, e.g. `"zh-CN"`.
    pub language: Option<String>,
}

/// The kind of document LibreOffice loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentType {
    Text,
    Spreadsheet,
    Presentation,
    Drawing,
    Other,
}

struct CallbackState {
    kit: *mut LibreOfficeKit,
    password: Mutex<Option<CString>>,
    password_requests: AtomicU32,
}

/// A running LibreOfficeKit instance.
///
/// Only one can exist per process, ever: keep it for the lifetime of the app (e.g. in an
/// `Arc` or a `OnceLock`). All calls are serialized internally, so it can be shared across
/// threads, but conversions run one at a time. Calls block; in async code run them on a
/// blocking thread (`tokio::task::spawn_blocking`).
pub struct Office {
    kit: *mut LibreOfficeKit,
    lock: Mutex<()>,
    callback: Box<CallbackState>,
    program_dir: PathBuf,
    library_path: PathBuf,
}

// SAFETY: every call into LibreOfficeKit goes through `lock`, and LibreOffice itself guards
// its internals with the SolarMutex, so the handle may be used from any thread.
unsafe impl Send for Office {}
unsafe impl Sync for Office {}

impl std::fmt::Debug for Office {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Office")
            .field("program_dir", &self.program_dir)
            .field("library_path", &self.library_path)
            .finish_non_exhaustive()
    }
}

macro_rules! kit_fn {
    ($office:expr, $name:ident) => {{
        #[allow(unused_unsafe)]
        let class = unsafe { &*(*$office.kit).pClass };
        if sys::has_member(offset_of!(LibreOfficeKitClass, $name), class.nSize) {
            class.$name
        } else {
            None
        }
    }};
}

macro_rules! doc_fn {
    ($doc:expr, $name:ident) => {{
        #[allow(unused_unsafe)]
        let class = unsafe { &*(*$doc).pClass };
        if sys::has_member(offset_of!(LibreOfficeKitDocumentClass, $name), class.nSize) {
            class.$name
        } else {
            None
        }
    }};
}

impl Office {
    pub fn builder() -> OfficeBuilder {
        OfficeBuilder::default()
    }

    /// Starts LibreOfficeKit from an installation root or program directory with defaults.
    pub fn new(install_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::builder().install_dir(install_dir).build()
    }

    /// Whether LibreOfficeKit has already been started in this process.
    pub fn is_initialized() -> bool {
        INITIALIZED.load(Ordering::SeqCst)
    }

    pub fn program_dir(&self) -> &Path {
        &self.program_dir
    }

    pub fn library_path(&self) -> &Path {
        &self.library_path
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Version information as the JSON string LibreOffice returns, e.g.
    /// `{"ProductName":"LibreOffice","ProductVersion":"26.2","ProductExtension":".6.3",...}`.
    pub fn version_info(&self) -> Option<String> {
        let f = kit_fn!(self, getVersionInfo)?;
        let _g = self.guard();
        unsafe { self.take_string(f(self.kit)) }
    }

    /// All import/export filter types LibreOffice knows, as a JSON string.
    pub fn filter_types(&self) -> Option<String> {
        let f = kit_fn!(self, getFilterTypes)?;
        let _g = self.guard();
        unsafe { self.take_string(f(self.kit)) }
    }

    /// Asks LibreOffice to release cached memory.
    pub fn trim_memory(&self) {
        if let Some(f) = kit_fn!(self, trimMemory) {
            let _g = self.guard();
            unsafe { f(self.kit, 4096) };
        }
    }

    /// Loads a document. The format is detected from the content.
    pub fn load(&self, path: impl AsRef<Path>, options: &LoadOptions) -> Result<Document<'_>> {
        let path = path.as_ref();
        if !path.is_file() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} not found", path.display()),
            )));
        }
        let url = CString::new(path_to_file_url(path)?).map_err(|_| nul_error())?;

        let mut extra = String::new();
        if let Some(lang) = &options.language {
            if lang.contains(',') {
                return Err(Error::InvalidInput(format!(
                    "invalid language tag {lang:?}"
                )));
            }
            extra.push_str(",Language=");
            extra.push_str(lang);
        }
        // `Batch=true` makes LibreOffice cancel every dialog (process wide, from then on) and
        // load silently, so e.g. CSV import uses defaults instead of blocking. A silent load
        // never asks for a password though, so a failed load is retried without `Batch`: dialogs
        // stay cancelled, but password requests reach `kit_callback`.
        let batch_opts = CString::new(format!("Batch=true{extra}")).map_err(|_| nul_error())?;
        let interactive_opts =
            CString::new(extra.trim_start_matches(',')).map_err(|_| nul_error())?;
        let password = options
            .password
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| nul_error())?;
        let has_password = password.is_some();

        let _g = self.guard();
        let mut doc = unsafe { self.document_load(&url, &batch_opts)? };
        if doc.is_null() {
            *self
                .callback
                .password
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = password;
            self.callback.password_requests.store(0, Ordering::SeqCst);
            doc = unsafe { self.document_load(&url, &interactive_opts)? };
            *self
                .callback
                .password
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            let password_requests = self.callback.password_requests.load(Ordering::SeqCst);
            if doc.is_null() && password_requests > 0 {
                return Err(if has_password {
                    Error::WrongPassword
                } else {
                    Error::PasswordRequired
                });
            }
        }
        if doc.is_null() {
            return Err(Error::Load {
                path: path.to_path_buf(),
                message: self.last_error().unwrap_or_else(|| "unknown error".into()),
            });
        }
        Ok(Document { office: self, doc })
    }

    /// Converts `input` to `output` in the given format (a file extension LibreOffice knows,
    /// such as `"pdf"`, `"docx"` or `"png"`) with optional raw filter options.
    pub fn convert(
        &self,
        input: impl AsRef<Path>,
        output: impl AsRef<Path>,
        format: &str,
        filter_options: Option<&str>,
    ) -> Result<()> {
        let doc = self.load(input, &LoadOptions::default())?;
        doc.save_as(output, Some(format), filter_options)
    }

    /// Converts any document LibreOffice can open (Word, Excel, PowerPoint, OpenDocument, RTF,
    /// ...) to PDF.
    pub fn convert_to_pdf(
        &self,
        input: impl AsRef<Path>,
        output: impl AsRef<Path>,
        options: &PdfOptions,
    ) -> Result<()> {
        self.convert_to_pdf_with(input, output, &LoadOptions::default(), options)
    }

    /// Like [`convert_to_pdf`](Self::convert_to_pdf), with load options such as a password.
    pub fn convert_to_pdf_with(
        &self,
        input: impl AsRef<Path>,
        output: impl AsRef<Path>,
        load: &LoadOptions,
        options: &PdfOptions,
    ) -> Result<()> {
        let doc = self.load(input, load)?;
        doc.save_as_pdf(output, options)
    }

    /// Must be called with `lock` held.
    unsafe fn document_load(
        &self,
        url: &CStr,
        options: &CStr,
    ) -> Result<*mut LibreOfficeKitDocument> {
        unsafe {
            Ok(
                match (
                    kit_fn!(self, documentLoadWithOptions),
                    kit_fn!(self, documentLoad),
                ) {
                    (Some(f), _) => f(self.kit, url.as_ptr(), options.as_ptr()),
                    (None, Some(f)) => f(self.kit, url.as_ptr()),
                    (None, None) => return Err(Error::Unsupported("documentLoad")),
                },
            )
        }
    }

    /// Must be called with `lock` held.
    fn last_error(&self) -> Option<String> {
        let f = kit_fn!(self, getError)?;
        unsafe { self.take_string(f(self.kit)) }.filter(|s| !s.is_empty())
    }

    /// Copies and frees a string allocated by LibreOffice.
    unsafe fn take_string(&self, p: *mut c_char) -> Option<String> {
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        // freeError is plain free(); without it (LibreOffice < 5.2) the string is leaked.
        if let Some(free) = kit_fn!(self, freeError) {
            unsafe { free(p) };
        }
        Some(s)
    }

    fn enable_password_callback(&self) {
        let (Some(set_features), Some(register)) = (
            kit_fn!(self, setOptionalFeatures),
            kit_fn!(self, registerCallback),
        ) else {
            return;
        };
        let data = &*self.callback as *const CallbackState as *mut c_void;
        unsafe {
            register(self.kit, Some(kit_callback), data);
            set_features(self.kit, sys::LOK_FEATURE_DOCUMENT_PASSWORD);
        }
    }
}

impl Drop for Office {
    fn drop(&mut self) {
        if let Some(destroy) = kit_fn!(self, destroy) {
            let _g = self.guard();
            unsafe { destroy(self.kit) };
        }
    }
}

unsafe extern "C" fn kit_callback(ty: c_int, payload: *const c_char, data: *mut c_void) {
    if ty != sys::LOK_CALLBACK_DOCUMENT_PASSWORD || data.is_null() || payload.is_null() {
        return;
    }
    let state = unsafe { &*(data as *const CallbackState) };
    // LibreOffice asks again after a wrong password; answer NULL then to abort the load.
    let attempt = state.password_requests.fetch_add(1, Ordering::SeqCst);
    let password = if attempt == 0 {
        state
            .password
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    } else {
        None
    };
    let class = unsafe { &*(*state.kit).pClass };
    if !sys::has_member(
        offset_of!(LibreOfficeKitClass, setDocumentPassword),
        class.nSize,
    ) {
        return;
    }
    if let Some(set_password) = class.setDocumentPassword {
        let ptr = password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
        unsafe { set_password(state.kit, payload, ptr) };
    }
}

/// A loaded document. Closed when dropped.
pub struct Document<'a> {
    office: &'a Office,
    doc: *mut LibreOfficeKitDocument,
}

// SAFETY: see `Office`; every call is serialized through the office lock.
unsafe impl Send for Document<'_> {}

impl Document<'_> {
    pub fn document_type(&self) -> DocumentType {
        let Some(f) = doc_fn!(self.doc, getDocumentType) else {
            return DocumentType::Other;
        };
        let _g = self.office.guard();
        match unsafe { f(self.doc) } {
            sys::LOK_DOCTYPE_TEXT => DocumentType::Text,
            sys::LOK_DOCTYPE_SPREADSHEET => DocumentType::Spreadsheet,
            sys::LOK_DOCTYPE_PRESENTATION => DocumentType::Presentation,
            sys::LOK_DOCTYPE_DRAWING => DocumentType::Drawing,
            _ => DocumentType::Other,
        }
    }

    /// Saves a copy of the document. `format` is a file extension (`"pdf"`, `"docx"`, ...);
    /// when `None` it is taken from `path`. Missing parent directories are created.
    pub fn save_as(
        &self,
        path: impl AsRef<Path>,
        format: Option<&str>,
        filter_options: Option<&str>,
    ) -> Result<()> {
        let path = path.as_ref();
        let save = doc_fn!(self.doc, saveAs).ok_or(Error::Unsupported("saveAs"))?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let url = CString::new(path_to_file_url(path)?).map_err(|_| nul_error())?;
        let format = format
            .map(CString::new)
            .transpose()
            .map_err(|_| nul_error())?;
        let filter_options = filter_options
            .map(CString::new)
            .transpose()
            .map_err(|_| nul_error())?;

        let _g = self.office.guard();
        let ok = unsafe {
            save(
                self.doc,
                url.as_ptr(),
                format.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                filter_options
                    .as_ref()
                    .map_or(std::ptr::null(), |s| s.as_ptr()),
            )
        };
        if ok == 0 {
            return Err(Error::Save {
                path: path.to_path_buf(),
                message: self
                    .office
                    .last_error()
                    .unwrap_or_else(|| "unknown error".into()),
            });
        }
        Ok(())
    }

    pub fn save_as_pdf(&self, path: impl AsRef<Path>, options: &PdfOptions) -> Result<()> {
        self.save_as(path, Some("pdf"), options.to_filter_options().as_deref())
    }
}

impl Drop for Document<'_> {
    fn drop(&mut self) {
        if let Some(destroy) = doc_fn!(self.doc, destroy) {
            let _g = self.office.guard();
            unsafe { destroy(self.doc) };
        }
    }
}

fn load_library(program_dir: &Path, library_path: &Path) -> Result<libloading::Library> {
    let map_err = |source| Error::LibraryLoad {
        path: library_path.to_path_buf(),
        source,
    };

    #[cfg(windows)]
    {
        use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library};
        // Same as extendUnoPath() in LibreOfficeKitInit.h: dependent DLLs live next to the
        // library and in the URE directory of older layouts.
        let mut paths = vec![
            program_dir.to_path_buf(),
            program_dir.join("..").join("URE").join("bin"),
        ];
        if let Some(old) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&old));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            // SAFETY: Windows' environment block is internally synchronized.
            unsafe { std::env::set_var("PATH", joined) };
        }
        unsafe { Library::load_with_flags(library_path, LOAD_WITH_ALTERED_SEARCH_PATH) }
            .map(Into::into)
            .map_err(map_err)
    }
    #[cfg(not(windows))]
    {
        let _ = program_dir;
        unsafe { libloading::Library::new(library_path) }.map_err(map_err)
    }
}

fn c_path(path: &Path) -> Result<CString> {
    let s = path
        .to_str()
        .ok_or_else(|| Error::InvalidInput(format!("{} is not valid UTF-8", path.display())))?;
    CString::new(s).map_err(|_| nul_error())
}

fn nul_error() -> Error {
    Error::InvalidInput("string contains a NUL byte".into())
}
