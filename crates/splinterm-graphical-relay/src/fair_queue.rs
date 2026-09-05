use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use crate::{MAX_CHANNEL_QUEUED_BYTES, MAX_DATA_BYTES, MAX_SESSION_QUEUED_BYTES};

/// One scheduled frame retaining byte permits through physical write completion.
#[derive(Debug)]
pub struct FairData {
    channel_id: u32,
    bytes: Vec<u8>,
    _channel_bytes: OwnedSemaphorePermit,
    _session_bytes: OwnedSemaphorePermit,
}

impl FairData {
    /// Returns the originating logical channel.
    #[must_use]
    pub const fn channel_id(&self) -> u32 {
        self.channel_id
    }

    /// Returns the opaque bytes to write while this value retains its permits.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Default)]
struct QueueState {
    channels: HashMap<u32, VecDeque<FairData>>,
    ready: VecDeque<u32>,
    receiver_closed: bool,
    sender_count: usize,
}

#[derive(Debug)]
struct SharedQueue {
    state: Mutex<QueueState>,
    ready: Notify,
    session_bytes: Arc<Semaphore>,
}

/// Producer factory for bounded fair per-channel data.
#[derive(Debug)]
pub struct FairDataSender {
    shared: Arc<SharedQueue>,
}

impl Clone for FairDataSender {
    fn clone(&self) -> Self {
        if let Ok(mut state) = self.shared.state.lock() {
            state.sender_count = state.sender_count.saturating_add(1);
        }
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for FairDataSender {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.sender_count = state.sender_count.saturating_sub(1);
        }
        self.shared.ready.notify_waiters();
    }
}

impl FairDataSender {
    /// Creates one independently bounded channel producer.
    #[must_use]
    pub fn channel(&self, channel_id: u32) -> FairDataChannel {
        FairDataChannel {
            channel_id,
            shared: self.shared.clone(),
            channel_bytes: Arc::new(Semaphore::new(MAX_CHANNEL_QUEUED_BYTES)),
        }
    }
}

/// One channel's reservation authority.
#[derive(Clone, Debug)]
pub struct FairDataChannel {
    channel_id: u32,
    shared: Arc<SharedQueue>,
    channel_bytes: Arc<Semaphore>,
}

impl FairDataChannel {
    /// Closes this producer and discards only its queued frames on full close.
    /// An in-flight physical frame retains its permits until its write completes.
    pub fn discard(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            self.channel_bytes.close();
            state.channels.remove(&self.channel_id);
            state.ready.retain(|id| *id != self.channel_id);
        }
    }

    /// Waits until every prior frame for this channel has completed its physical write.
    ///
    /// # Errors
    ///
    /// Returns an error when the channel semaphore has closed.
    pub async fn drain(&self) -> Result<()> {
        let amount =
            u32::try_from(MAX_CHANNEL_QUEUED_BYTES).context("channel byte bound exceeds u32")?;
        let permit = self
            .channel_bytes
            .clone()
            .acquire_many_owned(amount)
            .await
            .context("fair data channel byte budget closed")?;
        drop(permit);
        Ok(())
    }

    /// Reserves one maximum frame against both channel and session bounds before
    /// the producer reads bytes into that frame.
    ///
    /// # Errors
    ///
    /// Returns an error when the receiver has closed or semaphore state is invalid.
    pub async fn reserve(&self) -> Result<FairDataPermit> {
        let amount = u32::try_from(MAX_DATA_BYTES).context("data frame bound exceeds u32")?;
        let channel_bytes = self
            .channel_bytes
            .clone()
            .acquire_many_owned(amount)
            .await
            .context("fair data channel byte budget closed")?;
        let session_bytes = self
            .shared
            .session_bytes
            .clone()
            .acquire_many_owned(amount)
            .await
            .context("fair data session byte budget closed")?;
        if self
            .shared
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("fair data queue is poisoned"))?
            .receiver_closed
        {
            bail!("fair data receiver is closed");
        }
        Ok(FairDataPermit {
            channel_id: self.channel_id,
            shared: self.shared.clone(),
            channel_bytes,
            session_bytes,
        })
    }
}

/// Byte permits held from before producer read until physical writer consumption.
#[derive(Debug)]
pub struct FairDataPermit {
    channel_id: u32,
    shared: Arc<SharedQueue>,
    channel_bytes: OwnedSemaphorePermit,
    session_bytes: OwnedSemaphorePermit,
}

impl FairDataPermit {
    /// Commits one nonempty bounded data frame to round-robin scheduling.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid data or a closed receiver.
    pub fn send(self, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() || bytes.len() > MAX_DATA_BYTES {
            bail!("fair data payload length is outside bounds");
        }
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("fair data queue is poisoned"))?;
        if state.receiver_closed || self.channel_bytes.semaphore().is_closed() {
            bail!("fair data receiver or channel is closed");
        }
        let queue = state.channels.entry(self.channel_id).or_default();
        let was_empty = queue.is_empty();
        queue.push_back(FairData {
            channel_id: self.channel_id,
            bytes,
            _channel_bytes: self.channel_bytes,
            _session_bytes: self.session_bytes,
        });
        if was_empty {
            state.ready.push_back(self.channel_id);
        }
        drop(state);
        self.shared.ready.notify_one();
        Ok(())
    }
}

/// Consumer for round-robin channel data.
#[derive(Debug)]
pub struct FairDataReceiver {
    shared: Arc<SharedQueue>,
}

impl FairDataReceiver {
    /// Returns one frame, alternating ready channels after every frame.
    pub async fn recv(&mut self) -> Option<FairData> {
        loop {
            let notified = self.shared.ready.notified();
            {
                let mut state = self.shared.state.lock().ok()?;
                if let Some(channel_id) = state.ready.pop_front() {
                    let queue = state.channels.get_mut(&channel_id)?;
                    let item = queue.pop_front()?;
                    if queue.is_empty() {
                        state.channels.remove(&channel_id);
                    } else {
                        state.ready.push_back(channel_id);
                    }
                    return Some(item);
                }
                if state.sender_count == 0 {
                    return None;
                }
            }
            notified.await;
        }
    }
}

impl Drop for FairDataReceiver {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.receiver_closed = true;
            state.channels.clear();
            state.ready.clear();
        }
        self.shared.session_bytes.close();
        self.shared.ready.notify_waiters();
    }
}

/// Creates one bounded aggregate queue with fair per-channel scheduling.
#[must_use]
pub fn fair_data_queue() -> (FairDataSender, FairDataReceiver) {
    let shared = Arc::new(SharedQueue {
        state: Mutex::new(QueueState {
            sender_count: 1,
            ..QueueState::default()
        }),
        ready: Notify::new(),
        session_bytes: Arc::new(Semaphore::new(MAX_SESSION_QUEUED_BYTES)),
    });
    (
        FairDataSender {
            shared: shared.clone(),
        },
        FairDataReceiver { shared },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discard_reclaims_only_closed_channel_queue_and_rejects_late_permits() {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let (sender, mut receiver) = fair_data_queue();
            let closed = sender.channel(1);
            let sibling = sender.channel(2);
            closed.reserve().await.unwrap().send(vec![1]).unwrap();
            closed.reserve().await.unwrap().send(vec![2]).unwrap();
            let late = closed.reserve().await.unwrap();
            sibling.reserve().await.unwrap().send(vec![3]).unwrap();
            let in_flight = receiver.recv().await.unwrap();
            assert_eq!(in_flight.channel_id(), 1);
            closed.discard();
            assert!(late.send(vec![4]).is_err());
            assert!(closed.reserve().await.is_err());
            // Only the physical in-flight frame and the sibling still own byte permits.
            assert_eq!(
                sender.shared.session_bytes.available_permits(),
                MAX_SESSION_QUEUED_BYTES - 2 * MAX_DATA_BYTES
            );
            let preserved = receiver.recv().await.unwrap();
            assert_eq!(preserved.channel_id(), 2);
            assert_eq!(preserved.bytes(), &[3]);
            drop(preserved);
            drop(in_flight);
            assert_eq!(
                sender.shared.session_bytes.available_permits(),
                MAX_SESSION_QUEUED_BYTES
            );
            sibling.reserve().await.unwrap().send(vec![5]).unwrap();
            assert_eq!(receiver.recv().await.unwrap().bytes(), &[5]);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn scheduler_round_robins_channels_and_holds_byte_permits_until_receive() {
        let (sender, mut receiver) = fair_data_queue();
        let heavy = sender.channel(1);
        let small = sender.channel(2);
        for value in 0..4_u8 {
            heavy
                .reserve()
                .await
                .unwrap()
                .send(vec![value; MAX_DATA_BYTES])
                .unwrap();
        }
        small
            .reserve()
            .await
            .unwrap()
            .send(b"small".to_vec())
            .unwrap();
        assert_eq!(receiver.recv().await.unwrap().channel_id(), 1);
        let item = receiver.recv().await.unwrap();
        assert_eq!(item.channel_id(), 2);
        assert_eq!(item.bytes(), b"small");
        assert_eq!(receiver.recv().await.unwrap().channel_id(), 1);
    }

    #[tokio::test]
    async fn producer_cannot_reserve_beyond_its_channel_byte_bound() {
        let (sender, _receiver) = fair_data_queue();
        let channel = sender.channel(1);
        let mut permits = Vec::new();
        for _ in 0..(MAX_CHANNEL_QUEUED_BYTES / MAX_DATA_BYTES) {
            permits.push(channel.reserve().await.unwrap());
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), channel.reserve())
                .await
                .is_err()
        );
        drop(permits);
        assert!(channel.reserve().await.is_ok());
    }
}
