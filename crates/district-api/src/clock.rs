//! Where the time is read: Unix epoch milliseconds, the unit the service states
//! every expiry in. One trait for the sign-in crate and the live updates alike,
//! so a test moves both by hand and the app wires one clock.

use std::time::{SystemTime, UNIX_EPOCH};

/// Where the time is read, in Unix epoch milliseconds. A trait so tests can
/// move time by hand.
pub trait Clock: Send + Sync + 'static {
    /// The current time, in epoch milliseconds.
    fn now_ms(&self) -> i64;
}

/// The system's wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_reads_epoch_milliseconds() {
        // Later than 2026-01-01 and earlier than 2100-01-01, in milliseconds.
        let now = SystemClock.now_ms();
        assert!(
            (1_767_225_600_000..4_102_444_800_000).contains(&now),
            "{now}"
        );
    }
}
