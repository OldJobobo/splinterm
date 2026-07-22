use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

/// Maximum inbound newline-delimited MCP frame, including its newline.
pub const MAXIMUM_LINE_BYTES: usize = 256 * 1024;
/// Maximum complete tool response across structured and compatibility content.
pub const MAXIMUM_TOOL_RESPONSE_BYTES: usize = 1024 * 1024;
/// Maximum number of requests actively executing in one adapter process.
pub const MAXIMUM_ACTIVE_REQUESTS: usize = 4;
/// Maximum number of admitted active requests and cancellable waiters combined.
pub const MAXIMUM_ADMITTED_REQUESTS: usize = 32;

#[derive(Debug, Clone)]
pub(crate) struct AdmissionGate {
    admitted: Arc<Semaphore>,
    active: Arc<Semaphore>,
}

impl AdmissionGate {
    pub(crate) fn new() -> Self {
        Self {
            admitted: Arc::new(Semaphore::new(MAXIMUM_ADMITTED_REQUESTS)),
            active: Arc::new(Semaphore::new(MAXIMUM_ACTIVE_REQUESTS)),
        }
    }

    pub(crate) async fn acquire(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<AdmissionPermit, AdmissionError> {
        let admitted = Arc::clone(&self.admitted)
            .try_acquire_owned()
            .map_err(|_| AdmissionError::Full)?;
        let active = tokio::select! {
            permit = Arc::clone(&self.active).acquire_owned() => {
                permit.map_err(|_| AdmissionError::Closed)?
            }
            () = cancellation.cancelled() => return Err(AdmissionError::Cancelled),
        };
        Ok(AdmissionPermit {
            _admitted: admitted,
            _active: active,
        })
    }
}

#[derive(Debug)]
pub(crate) struct AdmissionPermit {
    _admitted: OwnedSemaphorePermit,
    _active: OwnedSemaphorePermit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionError {
    Full,
    Cancelled,
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admission_bounds_active_requests_and_cancellable_waiters() {
        let gate = AdmissionGate::new();
        let never_cancelled = CancellationToken::new();
        let mut active = Vec::new();
        for _ in 0..MAXIMUM_ACTIVE_REQUESTS {
            active.push(gate.acquire(&never_cancelled).await.unwrap());
        }

        let mut waiters = Vec::new();
        for _ in MAXIMUM_ACTIVE_REQUESTS..MAXIMUM_ADMITTED_REQUESTS {
            let waiter_gate = gate.clone();
            let cancellation = CancellationToken::new();
            waiters.push((
                cancellation.clone(),
                tokio::spawn(async move { waiter_gate.acquire(&cancellation).await }),
            ));
        }
        tokio::task::yield_now().await;
        assert!(waiters.iter().all(|(_, waiter)| !waiter.is_finished()));
        assert_eq!(
            gate.acquire(&never_cancelled).await.unwrap_err(),
            AdmissionError::Full
        );

        for (cancellation, _) in &waiters {
            cancellation.cancel();
        }
        for (_, waiter) in waiters {
            assert_eq!(
                waiter.await.unwrap().unwrap_err(),
                AdmissionError::Cancelled
            );
        }

        let cancelled_probe = CancellationToken::new();
        cancelled_probe.cancel();
        assert_eq!(
            gate.acquire(&cancelled_probe).await.unwrap_err(),
            AdmissionError::Cancelled,
            "cancelled waiters must release their admitted permits"
        );
        assert_eq!(active.len(), MAXIMUM_ACTIVE_REQUESTS);
    }
}
