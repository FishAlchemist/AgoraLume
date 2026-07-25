//! A tiny sliding-window rate limiter for outgoing LLM requests.
//!
//! Dependency-free (a hand-rolled window, in the spirit of the loop's own
//! SplitMix64 RNG) and shared across every group's coordinator, so the limit is
//! *server-wide*: it caps the total request rate against a provider's per-minute
//! quota rather than any single group's. The motivating case is a free tier such
//! as Gemini's `gemini-2.5-flash-lite` (15 requests/minute).

use std::collections::VecDeque;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Admits at most `max_per_window` request starts within any rolling `window`.
pub struct RateLimiter {
    max_per_window: usize,
    window: Duration,
    /// Start instants of the requests admitted and still inside the window,
    /// oldest at the front. Bounded by `max_per_window`.
    recent: Mutex<VecDeque<Instant>>,
}

impl RateLimiter {
    /// A limiter of `max_per_minute` request starts per rolling 60 seconds.
    pub fn per_minute(max_per_minute: usize) -> Self {
        Self {
            max_per_window: max_per_minute.max(1),
            window: Duration::from_secs(60),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    /// Waits until a request may start without exceeding the window, then records
    /// its start and returns. Admission is serialised through the mutex, so
    /// concurrent callers (agents across different groups) queue rather than all
    /// slip through at once. The lock is never held across the sleep.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut recent = self.recent.lock().await;
                let now = Instant::now();
                // Drop everything that has aged out of the window.
                while recent.front().is_some_and(|&t| now.duration_since(t) >= self.window) {
                    recent.pop_front();
                }
                if recent.len() < self.max_per_window {
                    recent.push_back(now);
                    return;
                }
                // Full: wait until the oldest admitted request leaves the window.
                let oldest = *recent.front().expect("a full window has a front entry");
                self.window - now.duration_since(oldest)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn admits_up_to_the_limit_without_waiting() {
        let limiter = RateLimiter::per_minute(3);
        let start = Instant::now();
        for _ in 0..3 {
            limiter.acquire().await;
        }
        // The first three are immediate — no time has to pass.
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn the_next_request_waits_out_the_window() {
        let limiter = RateLimiter::per_minute(2);
        limiter.acquire().await;
        limiter.acquire().await;
        // The third can't start until the first ages out — a full minute later,
        // since all three would otherwise fall in the same window.
        let before = Instant::now();
        limiter.acquire().await;
        assert_eq!(before.elapsed(), Duration::from_secs(60));
    }
}
