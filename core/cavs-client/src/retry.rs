//! Retry with exponential backoff and jitter (v0.5.0 hardening).
//!
//! Only transient failures are retried: transport errors (connection
//! reset, timeout, DNS) and 429/5xx responses. Anything the server meant
//! (404, 400, …) and anything that failed *verification* is never retried
//! unchanged — a hash mismatch on identical bytes stays a mismatch.

use anyhow::{anyhow, Result};
use cavs_proto::errors::ErrorCode;
use std::time::Duration;

pub const MAX_ATTEMPTS: u32 = 5;
const INITIAL_BACKOFF_MS: u64 = 250;
const MAX_BACKOFF_MS: u64 = 8_000;

/// Whether an HTTP failure is worth retrying unchanged.
pub fn is_retryable(err: &ureq::Error) -> bool {
    match err {
        ureq::Error::Status(code, _) => matches!(code, 429 | 500 | 502 | 503 | 504),
        ureq::Error::Transport(_) => true,
    }
}

/// Run `op` with up to [`MAX_ATTEMPTS`] tries and exponential backoff
/// (250 ms → 8 s, ±25% jitter). Exhausted retryable failures surface as
/// `CAVS-E-NETWORK`; non-retryable failures return immediately untouched.
pub fn with_retry<T>(
    what: &str,
    mut op: impl FnMut() -> std::result::Result<T, ureq::Error>,
) -> Result<T> {
    let mut backoff = INITIAL_BACKOFF_MS;
    let mut last: Option<ureq::Error> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if is_retryable(&e) && attempt < MAX_ATTEMPTS => {
                let wait = jittered(backoff);
                eprintln!(
                    "[retry] {what}: {e} (attempt {attempt}/{MAX_ATTEMPTS}, retrying in {wait} ms)"
                );
                std::thread::sleep(Duration::from_millis(wait));
                backoff = (backoff * 2).min(MAX_BACKOFF_MS);
                last = Some(e);
            }
            Err(e) => {
                last = Some(e);
                break;
            }
        }
    }
    let e = last.expect("loop always records an error before breaking");
    if is_retryable(&e) {
        Err(anyhow!(ErrorCode::Network.msg(format!(
            "{what}: {e} (gave up after {MAX_ATTEMPTS} attempts)"
        ))))
    } else {
        Err(anyhow!("{what}: {e}"))
    }
}

/// ±25% jitter without a rand dependency: the clock's sub-second nanos are
/// plenty to de-synchronize a fleet of retrying clients.
fn jittered(base: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let spread = (base / 2).max(1);
    base - base / 4 + nanos % spread
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_bounds() {
        for _ in 0..100 {
            let w = jittered(1000);
            assert!((750..1250).contains(&w), "{w}");
        }
    }

    #[test]
    fn non_retryable_fails_immediately() {
        let mut calls = 0;
        let r: Result<()> = with_retry("op", || {
            calls += 1;
            Err(ureq::Error::Status(
                404,
                ureq::Response::new(404, "Not Found", "nope").unwrap(),
            ))
        });
        assert!(r.is_err());
        assert_eq!(calls, 1);
        // A 404 is not a network failure: no CAVS-E-NETWORK tag.
        assert_eq!(
            cavs_proto::errors::error_code_of(&format!("{:#}", r.unwrap_err())),
            None
        );
    }

    #[test]
    fn retryable_retries_then_tags_network() {
        let mut calls = 0;
        let started = std::time::Instant::now();
        let r: Result<()> = with_retry("op", || {
            calls += 1;
            Err(ureq::Error::Status(
                503,
                ureq::Response::new(503, "Service Unavailable", "busy").unwrap(),
            ))
        });
        assert_eq!(calls, MAX_ATTEMPTS);
        // Four backoffs of 250/500/1000/2000 ms, each jittered down to 75%.
        assert!(started.elapsed() >= Duration::from_millis(2812));
        assert_eq!(
            cavs_proto::errors::error_code_of(&format!("{:#}", r.unwrap_err())),
            Some(cavs_proto::errors::ErrorCode::Network)
        );
    }

    #[test]
    fn success_after_transient_failures() {
        let mut calls = 0;
        let r = with_retry("op", || {
            calls += 1;
            if calls < 3 {
                Err(ureq::Error::Status(
                    502,
                    ureq::Response::new(502, "Bad Gateway", "eh").unwrap(),
                ))
            } else {
                Ok(42)
            }
        });
        assert_eq!(r.unwrap(), 42);
        assert_eq!(calls, 3);
    }
}
