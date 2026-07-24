//! Bounded admission for daemon-to-UI image content transfers.

use std::{collections::HashMap, time::Instant};

use rustix::rand::{GetRandomFlags, getrandom};
use splinterm_core::SplintId;
use splinterm_protocol::{
    IMAGE_TRANSFER_TOKEN_BYTES, IMAGE_TRANSFER_TOKEN_TTL_MILLIS, ImageContentRequest,
    ImageContentTransfer, ImageTransferMode, MAX_IMAGE_TRANSFERS_PER_DAEMON,
    MAX_IMAGE_TRANSFERS_PER_SPLINT,
};
use splinterm_terminal::ImageContent;
use thiserror::Error;

const MAX_PENDING_TOKENS_PER_PEER: usize = 4;
const MAX_PENDING_TOKENS_PER_DAEMON: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferPeer {
    pub uid: u32,
    pub pid: u32,
    pub executable_device: u64,
    pub executable_inode: u64,
    pub executable_sha256: String,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TransferAdmissionError {
    #[error("image transfer identity is stale or mismatched")]
    Identity,
    #[error("image transfer capacity is exhausted")]
    Capacity,
    #[error("image transfer token is invalid, expired, replayed, or mismatched")]
    Token,
    #[error("operating-system randomness is unavailable")]
    Random,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransferAdmissionMetrics {
    pub pending_tokens: usize,
    pub active_transfers: usize,
    pub high_water_pending_tokens: usize,
    pub high_water_active_transfers: usize,
}

#[derive(Clone, Debug)]
struct PendingTransfer {
    peer: TransferPeer,
    request: ImageContentRequest,
    content: ImageContent,
    expires_at: Instant,
}

#[derive(Clone, Debug)]
pub struct ClaimedTransfer {
    pub transfer_id: u64,
    pub request: ImageContentRequest,
    pub content: ImageContent,
}

#[derive(Debug, Default)]
pub struct TransferAdmission {
    pending: HashMap<[u8; IMAGE_TRANSFER_TOKEN_BYTES], PendingTransfer>,
    active: HashMap<u64, (SplintId, u64)>,
    next_transfer_id: u64,
    metrics: TransferAdmissionMetrics,
}

impl TransferAdmission {
    /// Mints one short-lived token after exact identity and capacity admission.
    ///
    /// # Errors
    ///
    /// Returns an identity, capacity, or randomness error without retaining a token.
    pub fn mint(
        &mut self,
        peer: TransferPeer,
        request: &ImageContentRequest,
        content: ImageContent,
        now: Instant,
    ) -> Result<ImageContentTransfer, TransferAdmissionError> {
        request
            .validate()
            .map_err(|_| TransferAdmissionError::Identity)?;
        let metadata = content.metadata();
        if request.content_id != metadata.id.value()
            || request.generation != metadata.generation
            || request.digest != metadata.digest
            || !request
                .accepted_transfers
                .contains(&ImageTransferMode::BinaryChunks)
        {
            return Err(TransferAdmissionError::Identity);
        }
        self.expire(now);
        if self.pending.len() >= MAX_PENDING_TOKENS_PER_DAEMON
            || self
                .pending
                .values()
                .filter(|pending| pending.peer == peer)
                .count()
                >= MAX_PENDING_TOKENS_PER_PEER
        {
            return Err(TransferAdmissionError::Capacity);
        }
        let token = self.unique_token()?;
        let expires_at = now
            .checked_add(std::time::Duration::from_millis(u64::from(
                IMAGE_TRANSFER_TOKEN_TTL_MILLIS,
            )))
            .ok_or(TransferAdmissionError::Capacity)?;
        self.pending.insert(
            token,
            PendingTransfer {
                peer,
                request: request.clone(),
                content,
                expires_at,
            },
        );
        self.observe_metrics();
        Ok(ImageContentTransfer {
            splint_id: request.splint_id,
            incarnation: request.incarnation,
            content_id: request.content_id,
            generation: request.generation,
            digest: request.digest,
            byte_length: metadata.byte_charge,
            transfer: ImageTransferMode::BinaryChunks,
            token,
            token_ttl_millis: IMAGE_TRANSFER_TOKEN_TTL_MILLIS,
        })
    }

    /// Atomically consumes and validates one token before charging an active transfer.
    ///
    /// # Errors
    ///
    /// Returns a token or capacity error. Failed claims never restore the token.
    pub fn claim(
        &mut self,
        token: [u8; IMAGE_TRANSFER_TOKEN_BYTES],
        peer: &TransferPeer,
        now: Instant,
    ) -> Result<ClaimedTransfer, TransferAdmissionError> {
        let pending = self
            .pending
            .remove(&token)
            .ok_or(TransferAdmissionError::Token)?;
        self.metrics.pending_tokens = self.pending.len();
        if pending.expires_at < now || pending.peer != *peer {
            return Err(TransferAdmissionError::Token);
        }
        let active_for_splint = self
            .active
            .values()
            .filter(|(splint_id, incarnation)| {
                *splint_id == pending.request.splint_id
                    && *incarnation == pending.request.incarnation
            })
            .count();
        if self.active.len() >= MAX_IMAGE_TRANSFERS_PER_DAEMON
            || active_for_splint >= MAX_IMAGE_TRANSFERS_PER_SPLINT
        {
            return Err(TransferAdmissionError::Capacity);
        }
        let transfer_id = self
            .next_transfer_id
            .checked_add(1)
            .ok_or(TransferAdmissionError::Capacity)?;
        self.next_transfer_id = transfer_id;
        self.active.insert(
            transfer_id,
            (pending.request.splint_id, pending.request.incarnation),
        );
        self.observe_metrics();
        Ok(ClaimedTransfer {
            transfer_id,
            request: pending.request,
            content: pending.content,
        })
    }

    /// Releases one exact active transfer charge.
    ///
    /// # Errors
    ///
    /// Returns an identity error for unknown or already-finished transfers.
    pub fn finish(&mut self, transfer_id: u64) -> Result<(), TransferAdmissionError> {
        self.active
            .remove(&transfer_id)
            .ok_or(TransferAdmissionError::Identity)?;
        self.metrics.active_transfers = self.active.len();
        Ok(())
    }

    pub fn expire(&mut self, now: Instant) {
        self.pending.retain(|_, pending| pending.expires_at >= now);
        self.metrics.pending_tokens = self.pending.len();
    }

    #[must_use]
    pub const fn metrics(&self) -> TransferAdmissionMetrics {
        self.metrics
    }

    fn unique_token(&self) -> Result<[u8; IMAGE_TRANSFER_TOKEN_BYTES], TransferAdmissionError> {
        for _ in 0..4 {
            let mut token = [0_u8; IMAGE_TRANSFER_TOKEN_BYTES];
            getrandom(&mut token, GetRandomFlags::empty())
                .map_err(|_| TransferAdmissionError::Random)?;
            if token != [0; IMAGE_TRANSFER_TOKEN_BYTES] && !self.pending.contains_key(&token) {
                return Ok(token);
            }
        }
        Err(TransferAdmissionError::Random)
    }

    fn observe_metrics(&mut self) {
        self.metrics.pending_tokens = self.pending.len();
        self.metrics.active_transfers = self.active.len();
        self.metrics.high_water_pending_tokens = self
            .metrics
            .high_water_pending_tokens
            .max(self.metrics.pending_tokens);
        self.metrics.high_water_active_transfers = self
            .metrics
            .high_water_active_transfers
            .max(self.metrics.active_transfers);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use splinterm_protocol::ImageTransferMode;
    use splinterm_terminal::{
        ActiveScreen, ImageAlphaMode, ImagePlane, ImageRetention, ImageSourceFormat,
        NewImageContent,
    };

    use super::*;

    fn image() -> ImageContent {
        let mut plane = ImagePlane::default();
        let id = plane
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 1,
                    height: 1,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &[1, 2, 3, 255],
                    retention: ImageRetention::ExplicitDelete,
                },
            )
            .unwrap();
        plane.content(ActiveScreen::Normal, id).unwrap().clone()
    }

    fn peer(pid: u32) -> TransferPeer {
        TransferPeer {
            uid: 1000,
            pid,
            executable_device: 2,
            executable_inode: 3,
            executable_sha256: "4".repeat(64),
        }
    }

    fn request(
        content: &ImageContent,
        splint_id: SplintId,
        incarnation: u64,
    ) -> ImageContentRequest {
        let metadata = content.metadata();
        ImageContentRequest {
            splint_id,
            incarnation,
            content_id: metadata.id.value(),
            generation: metadata.generation,
            digest: metadata.digest,
            accepted_transfers: vec![ImageTransferMode::BinaryChunks],
        }
    }

    #[test]
    fn tokens_are_exact_single_use_expiring_and_capacity_bounded() {
        let now = Instant::now();
        let content = image();
        let splint_id = SplintId::new();
        let request = request(&content, splint_id, 7);
        let mut admission = TransferAdmission::default();
        let grant = admission
            .mint(peer(4), &request, content.clone(), now)
            .unwrap();
        assert_eq!(grant.byte_length, 4);
        assert!(matches!(
            admission.claim(grant.token, &peer(5), now),
            Err(TransferAdmissionError::Token)
        ));
        assert!(matches!(
            admission.claim(grant.token, &peer(4), now),
            Err(TransferAdmissionError::Token)
        ));

        let grant = admission
            .mint(peer(4), &request, content.clone(), now)
            .unwrap();
        assert!(matches!(
            admission.claim(
                grant.token,
                &peer(4),
                now + Duration::from_millis(u64::from(IMAGE_TRANSFER_TOKEN_TTL_MILLIS) + 1),
            ),
            Err(TransferAdmissionError::Token)
        ));

        for _ in 0..MAX_PENDING_TOKENS_PER_PEER {
            admission
                .mint(peer(4), &request, content.clone(), now)
                .unwrap();
        }
        assert!(matches!(
            admission.mint(peer(4), &request, content.clone(), now),
            Err(TransferAdmissionError::Capacity)
        ));
        assert_eq!(
            admission.metrics().high_water_pending_tokens,
            MAX_PENDING_TOKENS_PER_PEER
        );
    }

    #[test]
    fn active_transfers_are_bounded_per_splint_and_released_exactly_once() {
        let now = Instant::now();
        let content = image();
        let splint_id = SplintId::new();
        let request = request(&content, splint_id, 9);
        let mut admission = TransferAdmission::default();
        let mut transfers = Vec::new();
        for pid in 1..=MAX_IMAGE_TRANSFERS_PER_SPLINT {
            let peer = peer(u32::try_from(pid).unwrap());
            let grant = admission
                .mint(peer.clone(), &request, content.clone(), now)
                .unwrap();
            transfers.push(admission.claim(grant.token, &peer, now).unwrap());
        }
        let extra_peer = peer(99);
        let extra = admission
            .mint(extra_peer.clone(), &request, content, now)
            .unwrap();
        assert!(matches!(
            admission.claim(extra.token, &extra_peer, now),
            Err(TransferAdmissionError::Capacity)
        ));
        assert_eq!(
            admission.metrics().high_water_active_transfers,
            MAX_IMAGE_TRANSFERS_PER_SPLINT
        );
        admission.finish(transfers[0].transfer_id).unwrap();
        assert!(matches!(
            admission.finish(transfers[0].transfer_id),
            Err(TransferAdmissionError::Identity)
        ));
    }
}
