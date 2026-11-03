//! Clock abstraction for the reporting loop.
//!
//! Tests can supply a clock with tiny delays; production uses [`TokioClock`].
//! The trait deliberately only exposes `sleep`, which is all the loop needs.
//! Collection itself (the one second CPU window) is controlled by
//! `agent-monitor`, not by this clock.

use std::time::Duration;

use async_trait::async_trait;

/// Injectable delay source.
#[async_trait]
pub trait Clock: Send + Sync {
    async fn sleep(&self, duration: Duration);
}

/// Production clock backed by [`tokio::time::sleep`].
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioClock;

#[async_trait]
impl Clock for TokioClock {
    async fn sleep(&self, duration: Duration) {
        if !duration.is_zero() {
            tokio::time::sleep(duration).await;
        }
    }
}
