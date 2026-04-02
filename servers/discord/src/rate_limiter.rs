//! Modular rate limiter for Discord API calls.
//!
//! Uses a token-bucket algorithm with per-route and global buckets.
//! Designed to be shared across all Discord API operations.
//!
//! Discord rate limits:
//!   - Per channel (messages/edits): 5 requests / 5 seconds
//!   - Per channel (reactions): 1 request / 0.25 seconds
//!   - Global: 50 requests / second

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Route categories with distinct rate limit buckets.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Route {
    /// Channel message operations (send, edit, delete): 5/5s per channel.
    ChannelMessage(u64),
    /// Reaction operations: 1/0.25s per channel.
    ChannelReaction(u64),
    /// Global bucket: 50/s across all routes.
    Global,
}

/// Configuration for a token bucket.
#[derive(Debug, Clone, Copy)]
struct BucketConfig {
    /// Maximum tokens (burst capacity).
    capacity: u32,
    /// Refill interval: one token is added every `refill_interval`.
    refill_interval: Duration,
}

impl BucketConfig {
    fn for_route(route: &Route) -> Self {
        match route {
            Route::ChannelMessage(_) => Self {
                capacity: 5,
                refill_interval: Duration::from_secs(1), // 5 tokens / 5s = 1/s refill
            },
            Route::ChannelReaction(_) => Self {
                capacity: 1,
                refill_interval: Duration::from_millis(250),
            },
            Route::Global => Self {
                capacity: 50,
                refill_interval: Duration::from_millis(20), // 50/s = 1/20ms
            },
        }
    }
}

/// A single token bucket.
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    capacity: u32,
    refill_interval: Duration,
    last_refill: Instant,
    /// If set, we're in a 429 retry-after window — block until this time.
    retry_after: Option<Instant>,
}

impl TokenBucket {
    fn new(config: BucketConfig) -> Self {
        Self {
            tokens: config.capacity as f64,
            capacity: config.capacity,
            refill_interval: config.refill_interval,
            last_refill: Instant::now(),
            retry_after: None,
        }
    }

    /// Refill tokens based on elapsed time.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill);
        let tokens_to_add = elapsed.as_secs_f64() / self.refill_interval.as_secs_f64();
        self.tokens = (self.tokens + tokens_to_add).min(self.capacity as f64);
        self.last_refill = now;
    }

    /// Try to consume one token. Returns wait duration if unavailable.
    fn try_acquire(&mut self) -> Result<(), Duration> {
        let now = Instant::now();

        // Check 429 retry-after window
        if let Some(retry_until) = self.retry_after {
            if now < retry_until {
                return Err(retry_until - now);
            }
            self.retry_after = None;
            // Reset tokens after retry-after expires
            self.tokens = 1.0;
            self.last_refill = now;
        }

        self.refill();

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            // Calculate wait time until next token
            let deficit = 1.0 - self.tokens;
            let wait = Duration::from_secs_f64(deficit * self.refill_interval.as_secs_f64());
            Err(wait)
        }
    }

    /// Record a 429 Retry-After response.
    #[allow(dead_code)]
    fn set_retry_after(&mut self, duration: Duration) {
        self.retry_after = Some(Instant::now() + duration);
        self.tokens = 0.0;
    }
}

/// Thread-safe rate limiter managing multiple buckets.
#[derive(Clone)]
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<Route, TokenBucket>>>,
    global: Arc<Mutex<TokenBucket>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        let global_config = BucketConfig::for_route(&Route::Global);
        Self {
            buckets: Arc::new(Mutex::new(HashMap::new())),
            global: Arc::new(Mutex::new(TokenBucket::new(global_config))),
        }
    }

    /// Acquire a permit for the given route. Waits if rate limited.
    /// Returns immediately if tokens are available.
    pub async fn acquire(&self, route: Route) {
        // Check route-specific bucket
        loop {
            let wait = {
                let mut buckets = self.buckets.lock().await;
                let bucket = buckets
                    .entry(route.clone())
                    .or_insert_with(|| TokenBucket::new(BucketConfig::for_route(&route)));
                bucket.try_acquire()
            };
            match wait {
                Ok(()) => break,
                Err(duration) => {
                    tracing::debug!(?route, ?duration, "Rate limited (route), waiting");
                    tokio::time::sleep(duration).await;
                }
            }
        }

        // Check global bucket
        loop {
            let wait = {
                let mut global = self.global.lock().await;
                global.try_acquire()
            };
            match wait {
                Ok(()) => break,
                Err(duration) => {
                    tracing::debug!(?duration, "Rate limited (global), waiting");
                    tokio::time::sleep(duration).await;
                }
            }
        }
    }

    /// Record a 429 Retry-After for a specific route.
    #[allow(dead_code)]
    pub async fn record_retry_after(&self, route: Route, retry_after_secs: f64) {
        let duration = Duration::from_secs_f64(retry_after_secs);
        tracing::warn!(?route, ?duration, "429 Retry-After received");

        let mut buckets = self.buckets.lock().await;
        let bucket = buckets
            .entry(route.clone())
            .or_insert_with(|| TokenBucket::new(BucketConfig::for_route(&route)));
        bucket.set_retry_after(duration);
    }

    /// Record a global 429.
    #[allow(dead_code)]
    pub async fn record_global_retry_after(&self, retry_after_secs: f64) {
        let duration = Duration::from_secs_f64(retry_after_secs);
        tracing::warn!(?duration, "Global 429 Retry-After received");

        let mut global = self.global.lock().await;
        global.set_retry_after(duration);
    }

    /// Clean up stale buckets (channels not accessed for > 5 minutes).
    pub async fn cleanup(&self) {
        let mut buckets = self.buckets.lock().await;
        let stale_threshold = Instant::now() - Duration::from_secs(300);
        buckets.retain(|_, bucket| bucket.last_refill > stale_threshold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_acquire() {
        let limiter = RateLimiter::new();
        // Should not block for first few requests
        for _ in 0..5 {
            limiter.acquire(Route::ChannelMessage(123)).await;
        }
    }

    #[tokio::test]
    async fn test_rate_limit_blocks() {
        let limiter = RateLimiter::new();
        let route = Route::ChannelMessage(456);

        // Exhaust the bucket (5 tokens)
        for _ in 0..5 {
            limiter.acquire(route.clone()).await;
        }

        // 6th request should take time
        let start = Instant::now();
        limiter.acquire(route).await;
        let elapsed = start.elapsed();
        assert!(elapsed.as_millis() > 500, "Should have waited for refill");
    }

    #[tokio::test]
    async fn test_retry_after() {
        let limiter = RateLimiter::new();
        let route = Route::ChannelMessage(789);

        limiter.record_retry_after(route.clone(), 0.5).await;

        let start = Instant::now();
        limiter.acquire(route).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() >= 400,
            "Should have waited for retry-after"
        );
    }

    #[tokio::test]
    async fn test_reaction_rate_limit() {
        let limiter = RateLimiter::new();
        let route = Route::ChannelReaction(123);

        // First request should be instant
        limiter.acquire(route.clone()).await;

        // Second should wait ~250ms
        let start = Instant::now();
        limiter.acquire(route).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() >= 200,
            "Reaction should be rate limited"
        );
    }
}
