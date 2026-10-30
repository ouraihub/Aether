//! Wall-clock milliseconds for persisted timestamps, TTLs and id prefixes.
//!
//! The one place that reads the system clock. Code that needs time for decisions takes a
//! [`Clock`] so tests can fix or advance it instead of waiting on real time.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch. Cheap to clone and shareable across threads.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The real wall clock.
#[must_use]
pub fn system() -> Clock {
    Arc::new(now_millis)
}

/// Current wall-clock milliseconds since the Unix epoch; 0 before the epoch, saturating far after.
#[must_use]
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
