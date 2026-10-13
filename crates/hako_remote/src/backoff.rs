//! How long to wait before each attempt to reconnect.

use std::time::Duration;

const INITIAL_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);

/// Delays that start at one second and double up to thirty.
#[derive(Debug)]
pub struct Backoff {
    next: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            next: INITIAL_DELAY,
        }
    }

    /// The delay before the next attempt.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (delay * 2).min(MAX_DELAY);
        delay
    }

    /// Starts over, which a successful connection calls for.
    pub fn reset(&mut self) {
        self.next = INITIAL_DELAY;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "backoff_tests.rs"]
mod tests;
