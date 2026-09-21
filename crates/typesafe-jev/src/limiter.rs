use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Uniform in [0, 1). Only used for retry and pause jitter, so a tiny xorshift is plenty.
pub(crate) fn jitter() -> f64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        x = nanos ^ (u64::from(std::process::id()) << 32) | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    (x >> 11) as f64 / (1u64 << 53) as f64
}

struct Gate {
    limit: f64,
    in_flight: usize,
    paused_until: Option<Instant>,
}

/// Shared concurrency gate with additive-increase, multiplicative-decrease backoff.
///
/// TypeSafe's rate limit is a sustained token budget with no quota headers, and several
/// processes may share one key. So on a throttled reply every thread pauses briefly and the
/// allowed concurrency halves; each success grows it back by roughly one slot per round.
///
/// A [`Client`](crate::Client) owns one and wraps every attempt in [`acquire`](Self::acquire)
/// and [`release`](Self::release). The type is public so that the gate can be inspected, and so
/// that other transports can share the policy.
pub struct AdaptiveLimiter {
    max: f64,
    pause: Duration,
    gate: Mutex<Gate>,
    cv: Condvar,
}

impl AdaptiveLimiter {
    /// `max_concurrency` is the ceiling the limit grows back to (at least one). `pause` is how
    /// long every thread waits after a throttled reply, scaled by a random factor in [1, 2).
    pub fn new(max_concurrency: usize, pause: Duration) -> Self {
        let max = max_concurrency.max(1) as f64;
        AdaptiveLimiter { max, pause, gate: Mutex::new(Gate { limit: max, in_flight: 0, paused_until: None }), cv: Condvar::new() }
    }

    /// The current concurrency limit. Starts at the ceiling, halves on throttling, grows back on
    /// success; never below one.
    pub fn limit(&self) -> f64 {
        self.gate.lock().unwrap().limit
    }

    /// Attempts between `acquire` and `release` right now.
    pub fn in_flight(&self) -> usize {
        self.gate.lock().unwrap().in_flight
    }

    /// Blocks until there is a free slot under the current limit and no pause is in effect.
    /// Every `acquire` must be matched by one [`release`](Self::release).
    pub fn acquire(&self) {
        let mut gate = self.gate.lock().unwrap();
        loop {
            let wait = gate.paused_until.and_then(|t| t.checked_duration_since(Instant::now())).filter(|d| !d.is_zero());
            match wait {
                None if gate.in_flight < (gate.limit as usize).max(1) => {
                    gate.in_flight += 1;
                    return;
                }
                None => gate = self.cv.wait(gate).unwrap(),
                Some(d) => gate = self.cv.wait_timeout(gate, d.max(Duration::from_millis(50))).unwrap().0,
            }
        }
    }

    /// Frees the slot. `throttled` reports a rate-limited reply (HTTP 429 or 529): the limit
    /// halves and every thread pauses, once per throttling episode rather than once per reply.
    /// Any other outcome grows the limit by about one slot per round of requests.
    pub fn release(&self, throttled: bool) {
        let mut gate = self.gate.lock().unwrap();
        gate.in_flight -= 1;
        let now = Instant::now();
        if throttled {
            if gate.paused_until.is_none_or(|t| now >= t) {
                gate.limit = (gate.limit / 2.0).max(1.0);
                gate.paused_until = Some(now + self.pause.mul_f64(1.0 + jitter()));
            }
        } else {
            gate.limit = self.max.min(gate.limit + 1.0 / gate.limit.max(1.0));
        }
        self.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    #[test]
    fn jitter_is_in_the_unit_interval_and_varies() {
        let draws: Vec<f64> = (0..64).map(|_| jitter()).collect();
        assert!(draws.iter().all(|&x| (0.0..1.0).contains(&x)));
        assert!(draws.windows(2).any(|w| w[0] != w[1]));
    }

    #[test]
    fn throttling_halves_the_limit_once_per_episode_and_success_grows_it_back() {
        let limiter = AdaptiveLimiter::new(8, Duration::ZERO);
        assert_eq!(limiter.limit(), 8.0);
        for _ in 0..3 {
            limiter.acquire();
        }
        assert_eq!(limiter.in_flight(), 3);
        limiter.release(true);
        limiter.release(true); // same episode: no second cut while the pause is not over
        assert!(limiter.limit() <= 4.0 && limiter.limit() >= 2.0, "{}", limiter.limit());
        limiter.release(false);
        assert_eq!(limiter.in_flight(), 0);
        for _ in 0..200 {
            limiter.acquire();
            limiter.release(false);
        }
        assert_eq!(limiter.limit(), 8.0);
    }

    #[test]
    fn the_limit_never_drops_below_one() {
        let limiter = AdaptiveLimiter::new(0, Duration::ZERO);
        assert_eq!(limiter.limit(), 1.0);
        for _ in 0..5 {
            limiter.acquire();
            std::thread::sleep(Duration::from_millis(1));
            limiter.release(true);
        }
        assert_eq!(limiter.limit(), 1.0);
    }

    #[test]
    fn acquire_blocks_while_the_gate_is_full() {
        let limiter = Arc::new(AdaptiveLimiter::new(2, Duration::ZERO));
        let peak = Arc::new(AtomicUsize::new(0));
        let running = Arc::new(AtomicUsize::new(0));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let (limiter, peak, running) = (Arc::clone(&limiter), Arc::clone(&peak), Arc::clone(&running));
                std::thread::spawn(move || {
                    limiter.acquire();
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(5));
                    running.fetch_sub(1, Ordering::SeqCst);
                    limiter.release(false);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 2, "{}", peak.load(Ordering::SeqCst));
        assert_eq!(limiter.in_flight(), 0);
    }
}
