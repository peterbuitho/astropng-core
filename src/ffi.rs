//! C ABI for non-Rust consumers (Go, Nim, Zig, Scala/JVM via a native-call
//! layer). Deliberately minimal: it mirrors the one high-level entry point
//! every existing CLI/GUI already uses ([`crate::batch::run`]), not the
//! individual xisf/fits/pixels/lookup steps, which stay Rust-internal.
//!
//! Every function here is `extern "C"` and panic-safe (`catch_unwind`): a
//! panic on the Rust side is turned into an error string rather than
//! unwinding across the FFI boundary, which is undefined behavior for
//! non-Rust callers.

use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::batch::{self, FileStatus, Options, Progress, Summary};

/// Opaque cancellation handle. Create one, pass it to [`astropng_run`], and
/// call [`astropng_cancel_token_cancel`] from another thread to stop the run
/// after the current file. Must be freed with
/// [`astropng_cancel_token_free`] once the run has returned.
pub struct AstropngCancelToken(Arc<AtomicBool>);

#[repr(C)]
pub struct AstropngOptions {
    pub input_dir: *const c_char,
    /// Nullable.
    pub output_dir: *const c_char,
    pub recursive: bool,
    pub overwrite: bool,
    pub resize4k: bool,
    pub png_only: bool,
    /// Nullable.
    pub font_path: *const c_char,
    pub lookup: bool,
    /// Nullable explicit file list override; when non-null, `files_len` must
    /// be its length.
    pub files: *const *const c_char,
    pub files_len: usize,
    /// `0` picks the same default as the Rust API (`min(available_parallelism, 8)`).
    pub concurrency: usize,
}

#[repr(C)]
pub enum AstropngFileStatus {
    Ok = 0,
    Skipped = 1,
    Failed = 2,
}

#[repr(C)]
pub struct AstropngProgress {
    pub index: usize,
    pub total: usize,
    pub rel_path: *const c_char,
    pub status: AstropngFileStatus,
    /// Failure detail; null unless `status == Failed`.
    pub status_message: *const c_char,
    /// Null unless the object was identified online and its title differs
    /// from the file name.
    pub label: *const c_char,
    /// Null unless there's something the caller should surface to the user.
    pub note: *const c_char,
}

#[repr(C)]
pub struct AstropngSummary {
    pub total: usize,
    pub converted: u32,
    pub skipped: u32,
    pub failed: u32,
    pub cancelled: bool,
    pub warnings: *mut *mut c_char,
    pub warnings_len: usize,
}

pub type AstropngProgressCallback =
    extern "C" fn(progress: *const AstropngProgress, user_data: *mut c_void);

/// Returns the crate version as a static, null-terminated string. Do not free.
#[no_mangle]
pub extern "C" fn astropng_version() -> *const c_char {
    static VERSION_CSTR: std::sync::OnceLock<CString> = std::sync::OnceLock::new();
    VERSION_CSTR
        .get_or_init(|| CString::new(crate::VERSION).unwrap())
        .as_ptr()
}

#[no_mangle]
pub extern "C" fn astropng_cancel_token_new() -> *mut AstropngCancelToken {
    Box::into_raw(Box::new(AstropngCancelToken(Arc::new(AtomicBool::new(
        false,
    )))))
}

/// # Safety
/// `token` must be a pointer returned by [`astropng_cancel_token_new`] and
/// not yet freed.
#[no_mangle]
pub unsafe extern "C" fn astropng_cancel_token_cancel(token: *const AstropngCancelToken) {
    if let Some(token) = token.as_ref() {
        token.0.store(true, Ordering::Relaxed);
    }
}

/// # Safety
/// `token` must be a pointer returned by [`astropng_cancel_token_new`],
/// not currently in use by a running [`astropng_run`] call, and not freed
/// twice.
#[no_mangle]
pub unsafe extern "C" fn astropng_cancel_token_free(token: *mut AstropngCancelToken) {
    if !token.is_null() {
        drop(Box::from_raw(token));
    }
}

/// Run the whole batch synchronously (blocks the calling thread until done,
/// cancelled, or a fatal setup error occurs). `callback` is invoked once per
/// file, possibly from a different native thread than the caller for each
/// invocation, but never concurrently with itself; callers that are not
/// thread-safe by default (e.g. most GUI toolkits) must marshal back to
/// their own main thread inside `callback`. Pointers inside the
/// [`AstropngProgress`] passed to `callback` are only valid for the duration
/// of that call.
///
/// Returns `0` on success (`*out_summary` is set; free it with
/// [`astropng_summary_free`]). Returns non-zero on a fatal setup error or an
/// internal panic (`*out_error` is set; free it with
/// [`astropng_string_free`]). Exactly one of `*out_summary` / `*out_error`
/// is set on return.
///
/// # Safety
/// `opts` must point to a valid, fully-initialized [`AstropngOptions`] whose
/// C-string fields are valid null-terminated UTF-8 (or null where the field
/// is documented nullable). `cancel` may be null (equivalent to a token that
/// is never cancelled). `out_summary` and `out_error` must be valid,
/// writable pointers.
#[no_mangle]
pub unsafe extern "C" fn astropng_run(
    opts: *const AstropngOptions,
    cancel: *const AstropngCancelToken,
    callback: AstropngProgressCallback,
    user_data: *mut c_void,
    out_summary: *mut *mut AstropngSummary,
    out_error: *mut *mut c_char,
) -> i32 {
    *out_summary = ptr::null_mut();
    *out_error = ptr::null_mut();

    let result = panic::catch_unwind(AssertUnwindSafe(|| -> Result<Summary, String> {
        let opts = match opts.as_ref() {
            Some(o) => o,
            None => return Err("opts must not be null".to_string()),
        };
        let options = options_from_ffi(opts)?;
        let default_cancel = AtomicBool::new(false);
        let cancel_flag: &AtomicBool = cancel
            .as_ref()
            .map(|t| t.0.as_ref())
            .unwrap_or(&default_cancel);

        // SendPtr wraps the raw user_data pointer so the closure below can be
        // Send: the caller's data races are the caller's problem, same as
        // any other C callback API.
        struct SendPtr(*mut c_void);
        unsafe impl Send for SendPtr {}
        let user_data = SendPtr(user_data);

        let mut report = |progress: &Progress| {
            report_progress(progress, callback, user_data.0);
        };
        batch::run(&options, cancel_flag, &mut report)
    }));

    match result {
        Ok(Ok(summary)) => {
            *out_summary = Box::into_raw(Box::new(summary_to_ffi(summary)));
            0
        }
        Ok(Err(e)) => {
            *out_error = string_to_ffi(e);
            1
        }
        Err(panic) => {
            let msg = panic_message(panic);
            *out_error = string_to_ffi(format!("internal error: {msg}"));
            2
        }
    }
}

/// # Safety
/// `summary` must be a pointer produced by [`astropng_run`] and not freed
/// twice.
#[no_mangle]
pub unsafe extern "C" fn astropng_summary_free(summary: *mut AstropngSummary) {
    if summary.is_null() {
        return;
    }
    let summary = Box::from_raw(summary);
    if !summary.warnings.is_null() {
        let warnings = Vec::from_raw_parts(summary.warnings, summary.warnings_len, summary.warnings_len);
        for w in warnings {
            drop(CString::from_raw(w));
        }
    }
}

/// # Safety
/// `s` must be a pointer produced by this crate (e.g. via `out_error`) and
/// not freed twice.
#[no_mangle]
pub unsafe extern "C" fn astropng_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

unsafe fn cstr_opt(ptr: *const c_char) -> Result<Option<String>, String> {
    if ptr.is_null() {
        Ok(None)
    } else {
        CStr::from_ptr(ptr)
            .to_str()
            .map(|s| Some(s.to_string()))
            .map_err(|e| format!("invalid UTF-8: {e}"))
    }
}

unsafe fn options_from_ffi(opts: &AstropngOptions) -> Result<Options, String> {
    let input_dir = cstr_opt(opts.input_dir)?
        .ok_or_else(|| "input_dir must not be null".to_string())?;
    let output_dir = cstr_opt(opts.output_dir)?.map(PathBuf::from);
    let font = cstr_opt(opts.font_path)?.map(PathBuf::from);

    let mut files = Vec::new();
    if !opts.files.is_null() && opts.files_len > 0 {
        let slice = std::slice::from_raw_parts(opts.files, opts.files_len);
        for &p in slice {
            let s = cstr_opt(p)?.ok_or_else(|| "files[] entries must not be null".to_string())?;
            files.push(PathBuf::from(s));
        }
    }

    Ok(Options {
        input_dir: PathBuf::from(input_dir),
        output_dir,
        recursive: opts.recursive,
        overwrite: opts.overwrite,
        resize4k: opts.resize4k,
        png_only: opts.png_only,
        font,
        lookup: opts.lookup,
        files,
        concurrency: opts.concurrency,
    })
}

fn string_to_ffi(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).unwrap().into_raw()
}

fn opt_string_to_ffi(s: Option<String>) -> *mut c_char {
    match s {
        Some(s) => string_to_ffi(s),
        None => ptr::null_mut(),
    }
}

fn report_progress(progress: &Progress, callback: AstropngProgressCallback, user_data: *mut c_void) {
    let rel_path = string_to_ffi(progress.rel.to_string_lossy().into_owned());
    let (status, status_message) = match &progress.status {
        FileStatus::Ok => (AstropngFileStatus::Ok, ptr::null_mut()),
        FileStatus::Skipped => (AstropngFileStatus::Skipped, ptr::null_mut()),
        FileStatus::Failed(msg) => (AstropngFileStatus::Failed, string_to_ffi(msg.clone())),
    };
    let label = opt_string_to_ffi(progress.label.clone());
    let note = opt_string_to_ffi(progress.note.clone());

    let ffi_progress = AstropngProgress {
        index: progress.index,
        total: progress.total,
        rel_path,
        status,
        status_message,
        label,
        note,
    };
    callback(&ffi_progress, user_data);

    unsafe {
        drop(CString::from_raw(rel_path));
        if !status_message.is_null() {
            drop(CString::from_raw(status_message));
        }
        if !label.is_null() {
            drop(CString::from_raw(label));
        }
        if !note.is_null() {
            drop(CString::from_raw(note));
        }
    }
}

fn summary_to_ffi(summary: Summary) -> AstropngSummary {
    let mut warnings: Vec<*mut c_char> = summary
        .warnings
        .into_iter()
        .map(string_to_ffi)
        .collect();
    warnings.shrink_to_fit();
    let warnings_len = warnings.len();
    let warnings_ptr = if warnings_len == 0 {
        ptr::null_mut()
    } else {
        let ptr = warnings.as_mut_ptr();
        std::mem::forget(warnings);
        ptr
    };

    AstropngSummary {
        total: summary.total,
        converted: summary.converted,
        skipped: summary.skipped,
        failed: summary.failed,
        cancelled: summary.cancelled,
        warnings: warnings_ptr,
        warnings_len,
    }
}
