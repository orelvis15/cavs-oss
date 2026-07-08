//! # cavs-ffi
//!
//! A minimal, stable C ABI over [`cavs_sdk_core`]. The surface is coarse on
//! purpose (JSON in, JSON out, plus opaque handles for long-running jobs):
//! CAVS operations are file-system and compression heavy, so JSON overhead
//! at the boundary is negligible and the ABI stays stable as the Rust
//! internals evolve.
//!
//! ## Memory ownership
//!
//! - Every `*mut` handle returned here must be freed with its matching
//!   `*_free` function exactly once.
//! - Every `char*` returned here is heap-allocated by Rust and must be
//!   freed with [`cavs_string_free`], EXCEPT the `'static` strings returned
//!   by [`cavs_sdk_version`] and [`cavs_sdk_abi_version`], which must not be
//!   freed.
//! - Strings read from callers (`operation`, `request_json`, options) are
//!   borrowed for the duration of the call only.
//!
//! ## Threads
//!
//! `cavs_execute_json` runs on the calling thread. `cavs_start_json` runs
//! the operation on a background thread; a registered progress callback may
//! therefore be invoked from that thread. Callbacks must be thread-safe.

// Every function here is a C ABI entry point that receives raw pointers from
// foreign code; taking `*mut`/`*const` by value and validating them is the
// whole job, so clippy's "mark it unsafe" lint does not apply.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use cavs_sdk_core::{execute_envelope, ProgressEvent};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

// ---------------------------------------------------------------------------
// Static version strings (never freed by the caller)
// ---------------------------------------------------------------------------

const VERSION_CSTR: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
const ABI_CSTR: &str = "1.0.0\0";

/// SDK/engine semver. The returned pointer is `'static`; do NOT free it.
#[no_mangle]
pub extern "C" fn cavs_sdk_version() -> *const c_char {
    VERSION_CSTR.as_ptr() as *const c_char
}

/// C ABI contract version. The returned pointer is `'static`; do NOT free it.
#[no_mangle]
pub extern "C" fn cavs_sdk_abi_version() -> *const c_char {
    ABI_CSTR.as_ptr() as *const c_char
}

/// Capability descriptor as a JSON string. Free with [`cavs_string_free`].
#[no_mangle]
pub extern "C" fn cavs_sdk_capabilities_json() -> *mut c_char {
    into_c_string(cavs_sdk_core::capabilities_json())
}

// ---------------------------------------------------------------------------
// Progress callback plumbing
// ---------------------------------------------------------------------------

pub type CavsProgressCallback =
    Option<extern "C" fn(event_json: *const c_char, user_data: *mut c_void)>;

/// A callback plus its opaque user pointer. The pointer is owned by the
/// caller; we only pass it back. Wrapped so it can cross thread boundaries
/// for background jobs — the caller guarantees thread-safety (see module docs).
#[derive(Clone, Copy)]
struct ProgressHook {
    callback: CavsProgressCallback,
    user_data: *mut c_void,
}

unsafe impl Send for ProgressHook {}
unsafe impl Sync for ProgressHook {}

impl ProgressHook {
    fn fire(&self, event: &ProgressEvent) {
        let Some(cb) = self.callback else { return };
        if let Ok(json) = serde_json::to_string(event) {
            if let Ok(c) = CString::new(json) {
                cb(c.as_ptr(), self.user_data);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Opaque execution context. Holds an optional progress hook. Cheap to
/// create; create one per logical client and reuse across calls.
pub struct CavsContext {
    progress: Mutex<Option<ProgressHook>>,
}

/// Create a context. `options_json` is currently reserved (pass NULL or
/// `"{}"`); it exists so options can be added without an ABI break.
#[no_mangle]
pub extern "C" fn cavs_context_new(_options_json: *const c_char) -> *mut CavsContext {
    Box::into_raw(Box::new(CavsContext {
        progress: Mutex::new(None),
    }))
}

/// Free a context created by [`cavs_context_new`].
#[no_mangle]
pub extern "C" fn cavs_context_free(ctx: *mut CavsContext) {
    if !ctx.is_null() {
        unsafe { drop(Box::from_raw(ctx)) };
    }
}

/// Register (or clear, with a NULL callback) the progress callback. Returns
/// 0 on success, -1 if `ctx` is NULL.
#[no_mangle]
pub extern "C" fn cavs_context_set_progress_callback(
    ctx: *mut CavsContext,
    callback: CavsProgressCallback,
    user_data: *mut c_void,
) -> c_int {
    let Some(ctx) = (unsafe { ctx.as_ref() }) else {
        return -1;
    };
    let hook = callback.map(|_| ProgressHook {
        callback,
        user_data,
    });
    *ctx.progress.lock().unwrap() = hook;
    0
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Opaque operation result. Wraps the full response envelope plus decoded
/// `ok`/`error` fields for cheap access.
pub struct CavsResult {
    json: CString,
    ok: bool,
    error_code: Option<CString>,
    error_message: Option<CString>,
}

impl CavsResult {
    fn from_envelope(envelope: String) -> Self {
        let parsed: serde_json::Value =
            serde_json::from_str(&envelope).unwrap_or_else(|_| serde_json::json!({"ok": false}));
        let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let error_code = parsed
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(|v| v.as_str())
            .and_then(|s| CString::new(s).ok());
        let error_message = parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            .and_then(|s| CString::new(s).ok());
        CavsResult {
            json: CString::new(envelope).unwrap_or_default(),
            ok,
            error_code,
            error_message,
        }
    }

    fn into_raw(self) -> *mut CavsResult {
        Box::into_raw(Box::new(self))
    }
}

/// The full response envelope JSON. Borrowed from the result; valid until
/// [`cavs_result_free`]. NULL if `result` is NULL.
#[no_mangle]
pub extern "C" fn cavs_result_json(result: *const CavsResult) -> *const c_char {
    match unsafe { result.as_ref() } {
        Some(r) => r.json.as_ptr(),
        None => std::ptr::null(),
    }
}

/// 1 if the operation succeeded, 0 otherwise.
#[no_mangle]
pub extern "C" fn cavs_result_ok(result: *const CavsResult) -> c_int {
    match unsafe { result.as_ref() } {
        Some(r) if r.ok => 1,
        _ => 0,
    }
}

/// Stable `CAVS-E-*` error code, or NULL when the result is OK / NULL.
#[no_mangle]
pub extern "C" fn cavs_result_error_code(result: *const CavsResult) -> *const c_char {
    match unsafe { result.as_ref() } {
        Some(r) => r
            .error_code
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null()),
        None => std::ptr::null(),
    }
}

/// Human-readable error message, or NULL when the result is OK / NULL.
#[no_mangle]
pub extern "C" fn cavs_result_error_message(result: *const CavsResult) -> *const c_char {
    match unsafe { result.as_ref() } {
        Some(r) => r
            .error_message
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null()),
        None => std::ptr::null(),
    }
}

/// Free a result returned by [`cavs_execute_json`] or [`cavs_job_poll`].
#[no_mangle]
pub extern "C" fn cavs_result_free(result: *mut CavsResult) {
    if !result.is_null() {
        unsafe { drop(Box::from_raw(result)) };
    }
}

// ---------------------------------------------------------------------------
// Synchronous execution
// ---------------------------------------------------------------------------

/// Execute `operation` with `request_json` on the calling thread and return
/// a [`CavsResult`] (never NULL unless inputs are unreadable). Free the
/// result with [`cavs_result_free`].
#[no_mangle]
pub extern "C" fn cavs_execute_json(
    ctx: *mut CavsContext,
    operation: *const c_char,
    request_json: *const c_char,
) -> *mut CavsResult {
    let (operation, request) = match read_call_args(operation, request_json) {
        Ok(pair) => pair,
        Err(envelope) => return CavsResult::from_envelope(envelope).into_raw(),
    };
    let hook = context_hook(ctx);
    let envelope = run_with_hook(&operation, &request, hook, None);
    CavsResult::from_envelope(envelope).into_raw()
}

// ---------------------------------------------------------------------------
// Asynchronous jobs
// ---------------------------------------------------------------------------

/// Opaque long-running job handle.
pub struct CavsJob {
    handle: Option<JoinHandle<String>>,
    cancel: Arc<AtomicBool>,
}

/// Start `operation` on a background thread. Poll it with [`cavs_job_poll`],
/// cancel with [`cavs_job_cancel`], and always free with [`cavs_job_free`].
/// Returns NULL only if `operation`/`request_json` are unreadable.
#[no_mangle]
pub extern "C" fn cavs_start_json(
    ctx: *mut CavsContext,
    operation: *const c_char,
    request_json: *const c_char,
) -> *mut CavsJob {
    let (operation, request) = match read_call_args(operation, request_json) {
        Ok(pair) => pair,
        Err(_) => return std::ptr::null_mut(),
    };
    let hook = context_hook(ctx);
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);
    let handle =
        std::thread::spawn(move || run_with_hook(&operation, &request, hook, Some(&cancel_thread)));
    Box::into_raw(Box::new(CavsJob {
        handle: Some(handle),
        cancel,
    }))
}

/// Return the result if the job has finished, else NULL (still running).
/// After a non-NULL return the job is drained; free it with
/// [`cavs_job_free`] and free the returned result with [`cavs_result_free`].
#[no_mangle]
pub extern "C" fn cavs_job_poll(job: *mut CavsJob) -> *mut CavsResult {
    let Some(job) = (unsafe { job.as_mut() }) else {
        return std::ptr::null_mut();
    };
    match &job.handle {
        Some(h) if h.is_finished() => {
            let handle = job.handle.take().unwrap();
            let envelope = handle
                .join()
                .unwrap_or_else(|_| r#"{"ok":false,"error":{"code":"CAVS-E-INTERNAL","message":"worker thread panicked","recoverable":false}}"#.to_string());
            CavsResult::from_envelope(envelope).into_raw()
        }
        _ => std::ptr::null_mut(),
    }
}

/// Request cooperative cancellation. Returns 0 on success, -1 if NULL.
#[no_mangle]
pub extern "C" fn cavs_job_cancel(job: *mut CavsJob) -> c_int {
    match unsafe { job.as_ref() } {
        Some(job) => {
            job.cancel.store(true, Ordering::Relaxed);
            0
        }
        None => -1,
    }
}

/// Free a job. If it is still running, cancellation is requested and the
/// worker is joined first so no thread outlives the handle.
#[no_mangle]
pub extern "C" fn cavs_job_free(job: *mut CavsJob) {
    if job.is_null() {
        return;
    }
    let mut job = unsafe { Box::from_raw(job) };
    job.cancel.store(true, Ordering::Relaxed);
    if let Some(handle) = job.handle.take() {
        let _ = handle.join();
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Free a string returned by this library (capabilities, result JSON is
/// owned by the result and must NOT be freed here).
#[no_mangle]
pub extern "C" fn cavs_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe { drop(CString::from_raw(ptr)) };
    }
}

fn into_c_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Read the `operation` and `request_json` arguments. On failure, returns a
/// ready-made error envelope (as `Err`) so callers can wrap it in a result.
fn read_call_args(
    operation: *const c_char,
    request_json: *const c_char,
) -> std::result::Result<(String, String), String> {
    let operation = read_str(operation).ok_or_else(|| {
        r#"{"schemaVersion":"1.0","ok":false,"operation":"","error":{"code":"CAVS-E-INVALID-REQUEST","message":"operation pointer was null or not UTF-8","recoverable":false,"details":{}}}"#
            .to_string()
    })?;
    // A NULL request is treated as an empty object (some ops need no fields).
    let request = if request_json.is_null() {
        "{}".to_string()
    } else {
        read_str(request_json).ok_or_else(|| {
            r#"{"schemaVersion":"1.0","ok":false,"operation":"","error":{"code":"CAVS-E-INVALID-JSON","message":"request_json was not UTF-8","recoverable":false,"details":{}}}"#
                .to_string()
        })?
    };
    Ok((operation, request))
}

/// Snapshot the context's progress hook, tolerating a NULL context.
fn context_hook(ctx: *mut CavsContext) -> Option<ProgressHook> {
    let ctx = unsafe { ctx.as_ref() }?;
    *ctx.progress.lock().unwrap()
}

fn read_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(str::to_string)
}

fn run_with_hook(
    operation: &str,
    request: &str,
    hook: Option<ProgressHook>,
    cancel: Option<&AtomicBool>,
) -> String {
    match hook {
        Some(hook) => {
            let sink = move |event: &ProgressEvent| hook.fire(event);
            execute_envelope(operation, request, Some(&sink), cancel)
        }
        None => execute_envelope(operation, request, None, cancel),
    }
}
