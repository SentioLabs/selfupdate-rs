use crate::{Error, Result};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Clones share a one-way, thread-safe cancellation flag.
///
/// Requests and prompt reads already in progress are not interrupted.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Create a token that has not been cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation of this token and all its clones.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Test whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Return `Error::Cancelled` if cancelled.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
