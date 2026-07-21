//! Adaptive download concurrency: an AIMD controller plus the gate that
//! enforces its limit on a pool of scoped workers.
//!
//! A fixed connection count is either too timid on a fat pipe or an
//! accidental DoS against a throttling CDN. AUTO mode
//! (`FetchOptions::connections == 0`) instead probes upward like TCP:
//! additive increase while windows stay clean, multiplicative decrease the
//! moment the remote pushes back (a failed range request, a short read
//! needing a retry, an HTTP 429/503). The controller only moves a *limit*;
//! the [`Gate`] is what actually holds workers over that limit, so raising
//! and lowering it never spawns or kills threads — the pool is sized once
//! at [`MAX_CONCURRENCY`] and idles behind the gate.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Floor of the AIMD limit: even a hostile-looking remote gets two lanes,
/// so a single burst of pressure can never serialize the whole fetch.
pub(crate) const MIN_CONCURRENCY: usize = 2;

/// AUTO mode's starting limit — the crate's long-standing fixed default,
/// so a fetch too short to complete an evaluation window behaves exactly
/// like `connections: 8` did.
pub(crate) const INITIAL_CONCURRENCY: usize = 8;

/// Ceiling of the AIMD limit and the size of the worker pool in AUTO mode.
/// Beyond this, per-connection returns vanish while server pressure and
/// buffer memory (see the byte budget) keep growing.
pub(crate) const MAX_CONCURRENCY: usize = 64;

/// Evaluation window: one clean window (≥1 successful group, no pressure)
/// earns +1 concurrency.
const WINDOW: Duration = Duration::from_secs(1);

/// Minimum spacing between multiplicative decreases. Pressure events arrive
/// in bursts (every in-flight request against a throttling host fails at
/// once); one halving per cooldown answers the burst without collapsing
/// straight to the floor.
const DECREASE_COOLDOWN: Duration = Duration::from_secs(1);

/// Time-dependent state, under one lock so a window roll or a decrease is
/// atomic. The hot read — [`AimdController::limit`] — never takes it.
struct AimdWindow {
    /// Start of the current evaluation window.
    window_start: Instant,
    /// Successful groups recorded in the current window.
    successes: u64,
    /// Pressure events recorded in the current window (poisons the +1).
    pressure: u64,
    /// Last multiplicative decrease, for the cooldown.
    last_decrease: Option<Instant>,
}

/// AIMD (additive-increase / multiplicative-decrease) concurrency
/// controller. Thread-safe; every mutation has an `*_at(now)` variant so
/// tests drive the clock instead of sleeping.
pub(crate) struct AimdController {
    min: usize,
    max: usize,
    limit: AtomicUsize,
    decreases: AtomicUsize,
    window: Mutex<AimdWindow>,
}

impl AimdController {
    pub(crate) fn new(min: usize, initial: usize, max: usize) -> Self {
        let initial = initial.clamp(min, max);
        Self {
            min,
            max,
            limit: AtomicUsize::new(initial),
            decreases: AtomicUsize::new(0),
            window: Mutex::new(AimdWindow {
                window_start: Instant::now(),
                successes: 0,
                pressure: 0,
                last_decrease: None,
            }),
        }
    }

    /// Current concurrency limit. Lock-free: workers poll this on every
    /// gate check.
    pub(crate) fn limit(&self) -> usize {
        self.limit.load(Ordering::Relaxed)
    }

    /// Multiplicative decreases performed so far (for [`FetchStats`]).
    ///
    /// [`FetchStats`]: crate::FetchStats
    pub(crate) fn decreases(&self) -> u64 {
        self.decreases.load(Ordering::Relaxed) as u64
    }

    /// A group downloaded, decoded and cached cleanly.
    pub(crate) fn on_success(&self) {
        self.on_success_at(Instant::now());
    }

    pub(crate) fn on_success_at(&self, now: Instant) {
        let mut w = self.window.lock().unwrap();
        w.successes += 1;
        // Windows roll lazily on events rather than on a timer thread: a
        // fetch with no traffic has nothing to adapt anyway.
        if now.duration_since(w.window_start) >= WINDOW {
            if w.successes >= 1 && w.pressure == 0 {
                let cur = self.limit.load(Ordering::Relaxed);
                if cur < self.max {
                    self.limit.store(cur + 1, Ordering::Relaxed);
                }
            }
            w.window_start = now;
            w.successes = 0;
            w.pressure = 0;
        }
    }

    /// The remote pushed back: a failed range attempt, a short read, or an
    /// error propagated from a group fetch.
    pub(crate) fn on_pressure(&self) {
        self.on_pressure_at(Instant::now());
    }

    pub(crate) fn on_pressure_at(&self, now: Instant) {
        let mut w = self.window.lock().unwrap();
        w.pressure += 1; // poisons this window's +1 even if cooled down
        let cooled = w
            .last_decrease
            .is_none_or(|t| now.duration_since(t) >= DECREASE_COOLDOWN);
        if !cooled {
            return;
        }
        let cur = self.limit.load(Ordering::Relaxed);
        let next = (cur / 2).max(self.min);
        if next < cur {
            self.limit.store(next, Ordering::Relaxed);
            self.decreases.fetch_add(1, Ordering::Relaxed);
            w.last_decrease = Some(now);
        }
    }
}

/// Admission gate enforcing a *movable* limit on how many workers run the
/// download section at once. Workers over the limit park on the condvar;
/// the wait uses a short timeout so a limit *raise* — which nobody
/// `notify`s, since no permit was released — is noticed within ~50 ms.
pub(crate) struct Gate {
    active: Mutex<usize>,
    freed: Condvar,
    /// High-water mark of concurrently admitted workers (for
    /// `FetchStats::concurrency_peak`).
    peak: AtomicUsize,
}

/// How long a parked worker sleeps before re-reading the limit.
const GATE_POLL: Duration = Duration::from_millis(50);

impl Gate {
    pub(crate) fn new() -> Self {
        Self {
            active: Mutex::new(0),
            freed: Condvar::new(),
            peak: AtomicUsize::new(0),
        }
    }

    /// Block until a slot under `limit()` frees up, then take it. Returns
    /// `None` when `abort()` turns true while waiting (fetch failed or was
    /// cancelled — don't sit on the condvar holding nothing). The limit is
    /// re-read on every wakeup, so AIMD moves apply to already-parked
    /// workers, not only to future arrivals.
    pub(crate) fn enter(
        &self,
        limit: impl Fn() -> usize,
        abort: impl Fn() -> bool,
    ) -> Option<GatePermit<'_>> {
        let mut active = self.active.lock().unwrap();
        while *active >= limit().max(1) {
            if abort() {
                return None;
            }
            let (guard, _timed_out) = self.freed.wait_timeout(active, GATE_POLL).unwrap();
            active = guard;
        }
        *active += 1;
        self.peak.fetch_max(*active, Ordering::Relaxed);
        Some(GatePermit { gate: self })
    }

    pub(crate) fn peak(&self) -> u64 {
        self.peak.load(Ordering::Relaxed) as u64
    }
}

/// One admitted slot; releasing it (on drop, even on panic/error paths)
/// wakes the parked workers.
pub(crate) struct GatePermit<'a> {
    gate: &'a Gate,
}

impl Drop for GatePermit<'_> {
    fn drop(&mut self) {
        *self.gate.active.lock().unwrap() -= 1;
        self.gate.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn clean_window_earns_plus_one() {
        let c = AimdController::new(2, 8, 64);
        let t0 = Instant::now();
        c.on_success_at(t0);
        assert_eq!(c.limit(), 8, "no increase inside the window");
        c.on_success_at(t0 + ms(1100));
        assert_eq!(c.limit(), 9, "clean elapsed window earns +1");
        assert_eq!(c.decreases(), 0);
    }

    #[test]
    fn pressure_poisons_the_window_increase() {
        // At the floor, pressure can't decrease — but it must still block
        // the additive increase for its window.
        let c = AimdController::new(2, 2, 64);
        let t0 = Instant::now();
        c.on_pressure_at(t0);
        assert_eq!(c.limit(), 2, "already at the floor");
        c.on_success_at(t0 + ms(1100));
        assert_eq!(c.limit(), 2, "window with pressure earns nothing");
        c.on_success_at(t0 + ms(2200));
        assert_eq!(c.limit(), 3, "next clean window earns +1 again");
    }

    #[test]
    fn pressure_halves_and_floors_at_min() {
        let c = AimdController::new(2, 8, 64);
        let t0 = Instant::now();
        c.on_pressure_at(t0);
        assert_eq!(c.limit(), 4);
        c.on_pressure_at(t0 + ms(2000));
        assert_eq!(c.limit(), 2);
        c.on_pressure_at(t0 + ms(4000));
        assert_eq!(c.limit(), 2, "never below min");
        assert_eq!(c.decreases(), 2, "the no-op at the floor is not counted");
    }

    #[test]
    fn cooldown_prevents_double_halving_within_a_second() {
        let c = AimdController::new(2, 32, 64);
        let t0 = Instant::now();
        c.on_pressure_at(t0);
        assert_eq!(c.limit(), 16);
        c.on_pressure_at(t0 + ms(200)); // same burst
        c.on_pressure_at(t0 + ms(400));
        assert_eq!(c.limit(), 16, "burst within the cooldown halves once");
        assert_eq!(c.decreases(), 1);
        c.on_pressure_at(t0 + ms(1400)); // cooldown elapsed
        assert_eq!(c.limit(), 8);
        assert_eq!(c.decreases(), 2);
    }

    #[test]
    fn increase_ceils_at_max() {
        let c = AimdController::new(2, 63, 64);
        let t0 = Instant::now();
        c.on_success_at(t0);
        c.on_success_at(t0 + ms(1100));
        assert_eq!(c.limit(), 64);
        c.on_success_at(t0 + ms(2200));
        assert_eq!(c.limit(), 64, "never above max");
    }

    #[test]
    fn gate_admits_at_most_limit_workers() {
        // 8 workers, limit 2: a high-water mark measured inside a slow
        // critical section must never exceed the limit.
        let gate = Gate::new();
        let inside = AtomicUsize::new(0);
        let high = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..3 {
                        let _slot = gate.enter(|| 2, || false).unwrap();
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        high.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(ms(5)); // slow fake source
                        inside.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });
        let high = high.load(Ordering::SeqCst);
        assert!(high <= 2, "limit 2 breached: {high} concurrent workers");
        assert!(high >= 1);
        assert!(gate.peak() <= 2);
    }

    #[test]
    fn gate_abort_releases_waiters() {
        // A waiter whose fetch has failed must come back `None` instead of
        // parking forever on a slot that will never free.
        let gate = Gate::new();
        let holder = gate.enter(|| 1, || false).unwrap();
        std::thread::scope(|s| {
            let h = s.spawn(|| gate.enter(|| 1, || true).is_none());
            assert!(h.join().unwrap(), "aborted waiter must give up");
        });
        drop(holder);
    }
}
