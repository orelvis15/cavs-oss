//! Exercises the C ABI the way a language binding would: raw pointers,
//! CStrings, manual frees, async jobs and cancellation. Also checks the
//! shipped header declares exactly the exported symbols.

use cavs_sdk::*;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicUsize, Ordering};

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn result_json(result: *const CavsResult) -> String {
    let ptr = cavs_result_json(result);
    assert!(!ptr.is_null());
    unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string()
}

#[test]
fn version_and_abi_are_semver() {
    let v = unsafe { CStr::from_ptr(cavs_sdk_version()) }
        .to_str()
        .unwrap();
    let abi = unsafe { CStr::from_ptr(cavs_sdk_abi_version()) }
        .to_str()
        .unwrap();
    assert_eq!(v.split('.').count(), 3, "version not semver: {v}");
    assert_eq!(abi, "1.0.0");
}

#[test]
fn capabilities_json_roundtrips_and_frees() {
    let ptr = cavs_sdk_capabilities_json();
    assert!(!ptr.is_null());
    let json = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string();
    cavs_string_free(ptr);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["abiVersion"], "1.0.0");
    assert!(v["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f == "createPlan"));
}

#[test]
fn execute_estimate_savings_ok() {
    let ctx = cavs_context_new(std::ptr::null());
    let op = cstr("estimateSavings");
    let req = cstr(
        r#"{"data":{"pricePerGb":0.08,"monthlyDownloads":500000,
            "averageFullDownloadBytes":65011712,"averageCavsDownloadBytes":2631921}}"#,
    );
    let result = cavs_execute_json(ctx, op.as_ptr(), req.as_ptr());
    assert!(!result.is_null());
    assert_eq!(cavs_result_ok(result), 1);
    assert!(cavs_result_error_code(result).is_null());
    let v: serde_json::Value = serde_json::from_str(&result_json(result)).unwrap();
    assert!(v["data"]["savingsPercent"].as_f64().unwrap() > 90.0);
    cavs_result_free(result);
    cavs_context_free(ctx);
}

#[test]
fn unknown_operation_is_structured_error() {
    let op = cstr("nope");
    let result = cavs_execute_json(std::ptr::null_mut(), op.as_ptr(), std::ptr::null());
    assert_eq!(cavs_result_ok(result), 0);
    let code = unsafe { CStr::from_ptr(cavs_result_error_code(result)) }
        .to_str()
        .unwrap();
    assert_eq!(code, "CAVS-E-UNKNOWN-OPERATION");
    let msg = cavs_result_error_message(result);
    assert!(!msg.is_null());
    cavs_result_free(result);
}

#[test]
fn null_context_is_tolerated() {
    let op = cstr("estimateSavings");
    let req = cstr(
        r#"{"pricePerGb":0.08,"monthlyDownloads":1,"averageFullDownloadBytes":1000,"averageCavsDownloadBytes":100}"#,
    );
    let result = cavs_execute_json(std::ptr::null_mut(), op.as_ptr(), req.as_ptr());
    assert_eq!(cavs_result_ok(result), 1);
    cavs_result_free(result);
}

static PROGRESS_HITS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn count_progress(event_json: *const c_char, user_data: *mut c_void) {
    assert!(!event_json.is_null());
    let s = unsafe { CStr::from_ptr(event_json) }.to_str().unwrap();
    // Events are valid JSON carrying a "type".
    let v: serde_json::Value = serde_json::from_str(s).unwrap();
    assert!(v["type"].is_string());
    let counter = unsafe { &*(user_data as *const AtomicUsize) };
    counter.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn progress_callback_receives_events() {
    PROGRESS_HITS.store(0, Ordering::SeqCst);
    let ctx = cavs_context_new(std::ptr::null());
    let rc = cavs_context_set_progress_callback(
        ctx,
        Some(count_progress),
        &PROGRESS_HITS as *const AtomicUsize as *mut c_void,
    );
    assert_eq!(rc, 0);
    // A cheap op still emits started/completed at minimum.
    let op = cstr("estimateSavings");
    let req = cstr(
        r#"{"pricePerGb":0.01,"monthlyDownloads":1,"averageFullDownloadBytes":1000,"averageCavsDownloadBytes":10}"#,
    );
    let result = cavs_execute_json(ctx, op.as_ptr(), req.as_ptr());
    cavs_result_free(result);
    cavs_context_free(ctx);
    assert!(PROGRESS_HITS.load(Ordering::SeqCst) >= 2);
}

#[test]
fn async_job_starts_polls_completes() {
    let ctx = cavs_context_new(std::ptr::null());
    let op = cstr("estimateSavings");
    let req = cstr(
        r#"{"pricePerGb":0.08,"monthlyDownloads":100,"averageFullDownloadBytes":1000000,"averageCavsDownloadBytes":50000}"#,
    );
    let job = cavs_start_json(ctx, op.as_ptr(), req.as_ptr());
    assert!(!job.is_null());
    // Poll until finished (bounded so a hang fails rather than loops forever).
    let mut result = std::ptr::null_mut();
    for _ in 0..10_000 {
        result = cavs_job_poll(job);
        if !result.is_null() {
            break;
        }
        std::thread::yield_now();
    }
    assert!(!result.is_null(), "job never completed");
    assert_eq!(cavs_result_ok(result), 1);
    cavs_result_free(result);
    cavs_job_free(job);
    cavs_context_free(ctx);
}

#[test]
fn async_job_can_be_cancelled_and_freed() {
    let ctx = cavs_context_new(std::ptr::null());
    let op = cstr("estimateSavings");
    let req = cstr(
        r#"{"pricePerGb":0.08,"monthlyDownloads":1,"averageFullDownloadBytes":1,"averageCavsDownloadBytes":1}"#,
    );
    let job = cavs_start_json(ctx, op.as_ptr(), req.as_ptr());
    assert!(!job.is_null());
    // Cancelling is always safe, whether or not the op already finished.
    assert_eq!(cavs_job_cancel(job), 0);
    cavs_job_free(job); // joins the worker
    cavs_context_free(ctx);
    // Cancelling a NULL job is a no-op error.
    assert_eq!(cavs_job_cancel(std::ptr::null_mut()), -1);
}

#[test]
fn header_declares_exactly_the_exported_symbols() {
    let header = include_str!("../include/cavs_sdk.h");
    let exported = [
        "cavs_sdk_version",
        "cavs_sdk_abi_version",
        "cavs_sdk_capabilities_json",
        "cavs_context_new",
        "cavs_context_free",
        "cavs_context_set_progress_callback",
        "cavs_execute_json",
        "cavs_start_json",
        "cavs_job_poll",
        "cavs_job_cancel",
        "cavs_job_free",
        "cavs_result_json",
        "cavs_result_ok",
        "cavs_result_error_code",
        "cavs_result_error_message",
        "cavs_result_free",
        "cavs_string_free",
    ];
    for sym in exported {
        assert!(header.contains(sym), "header is missing {sym}");
    }
}
