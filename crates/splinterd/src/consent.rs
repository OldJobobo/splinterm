//! Trusted, daemon-owned authorization state and the private consent-client broker.
//!
//! The daemon never creates graphical objects. A one-use random capability and
//! bounded prompt cross only a private inherited socket connected to
//! `splinterm consent`; neither is exposed through argv, environment, or logs.

use std::{
    collections::{BTreeSet, VecDeque},
    fs,
    io::{Read, Write},
    os::{
        fd::OwnedFd,
        unix::{fs::MetadataExt, net::UnixStream as StdUnixStream},
    },
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use splinterm_core::SplintId;
use splinterm_protocol::{
    AccessGrant, AccessScope, CONSENT_CAPABILITY_BYTES, ConsentPrompt, ConsentReply,
    MAX_ACCESS_SCOPES, MAX_CONSENT_FRAME_BYTES,
};
use tokio::net::UnixStream;

pub const GRANT_LIFETIME: Duration = Duration::from_secs(5 * 60);
const CONSENT_DEADLINE: Duration = Duration::from_secs(20);
const MAX_AUDIT_RECORDS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerIdentity {
    pub uid: u32,
    pub pid: u32,
    executable: PathBuf,
    executable_device: u64,
    executable_inode: u64,
}

impl PeerIdentity {
    pub fn from_stream(stream: &UnixStream) -> Result<Self> {
        let credentials = stream.peer_cred().context("cannot read peer credentials")?;
        let pid = credentials.pid().context("peer credentials omit pid")?;
        let pid = u32::try_from(pid).context("peer pid is negative")?;
        let executable =
            fs::read_link(format!("/proc/{pid}/exe")).context("cannot resolve peer executable")?;
        let metadata = fs::metadata(&executable).context("cannot identify peer executable")?;
        Ok(Self {
            uid: credentials.uid(),
            pid,
            executable,
            executable_device: metadata.dev(),
            executable_inode: metadata.ino(),
        })
    }

    pub fn requester_label(&self) -> String {
        self.executable
            .to_string_lossy()
            .chars()
            .take(1024)
            .collect()
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        let executable = std::env::current_exe().expect("test executable");
        let metadata = fs::metadata(&executable).expect("test executable metadata");
        Self {
            uid: rustix::process::geteuid().as_raw(),
            pid: std::process::id(),
            executable,
            executable_device: metadata.dev(),
            executable_inode: metadata.ino(),
        }
    }

    pub fn is_matching_splinterm(&self) -> bool {
        let Ok(expected) = std::env::current_exe().map(|path| path.with_file_name("splinterm"))
        else {
            return false;
        };
        fs::metadata(expected).is_ok_and(|metadata| {
            metadata.dev() == self.executable_device && metadata.ino() == self.executable_inode
        })
    }
}

#[derive(Clone, Debug)]
struct Grant {
    id: u64,
    peer: PeerIdentity,
    splint_id: SplintId,
    incarnation: u64,
    scopes: BTreeSet<AccessScope>,
    expires: Instant,
    expires_at_unix_seconds: u64,
}

impl Grant {
    fn wire(&self) -> AccessGrant {
        AccessGrant {
            grant_id: self.id,
            splint_id: self.splint_id,
            incarnation: self.incarnation,
            scopes: self.scopes.iter().copied().collect(),
            requester: self.peer.requester_label(),
            expires_at_unix_seconds: self.expires_at_unix_seconds,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditDecision {
    Granted,
    Denied,
    Revoked,
}

#[allow(
    dead_code,
    reason = "bounded audit metadata is retained in memory for future trusted diagnostics"
)]
#[derive(Clone, Debug)]
pub struct AuditRecord {
    pub order: u64,
    pub unix_seconds: u64,
    pub peer_uid: u32,
    pub peer_pid: u32,
    pub requester: String,
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub scopes: Vec<AccessScope>,
    pub decision: AuditDecision,
    pub reason: &'static str,
}

#[derive(Debug, Default)]
pub struct GrantStore {
    next_id: u64,
    next_audit_order: u64,
    grants: Vec<Grant>,
    audit: VecDeque<AuditRecord>,
}

impl GrantStore {
    pub fn authorize(
        &mut self,
        peer: &PeerIdentity,
        splint_id: SplintId,
        incarnation: u64,
        required: &[AccessScope],
    ) -> Option<u64> {
        self.remove_expired();
        let required: BTreeSet<_> = required.iter().copied().collect();
        self.grants.iter().find_map(|grant| {
            (grant.peer == *peer
                && grant.splint_id == splint_id
                && grant.incarnation == incarnation
                && required.is_subset(&grant.scopes))
            .then_some(grant.id)
        })
    }

    pub fn status(&mut self, splint_id: SplintId, incarnation: u64) -> Vec<AccessGrant> {
        self.remove_expired();
        self.grants
            .iter()
            .filter(|grant| grant.splint_id == splint_id && grant.incarnation == incarnation)
            .map(Grant::wire)
            .collect()
    }

    pub fn grant(
        &mut self,
        peer: &PeerIdentity,
        splint_id: SplintId,
        incarnation: u64,
        scopes: Vec<AccessScope>,
    ) -> AccessGrant {
        self.remove_expired();
        let scopes: BTreeSet<_> = scopes.into_iter().collect();
        if let Some(index) = self.grants.iter().position(|grant| {
            grant.peer == *peer && grant.splint_id == splint_id && grant.incarnation == incarnation
        }) {
            let wire = {
                let grant = &mut self.grants[index];
                grant.scopes.extend(scopes.iter().copied());
                grant.expires = Instant::now() + GRANT_LIFETIME;
                grant.expires_at_unix_seconds = unix_seconds() + GRANT_LIFETIME.as_secs();
                grant.wire()
            };
            self.record(
                peer,
                splint_id,
                incarnation,
                scopes,
                AuditDecision::Granted,
                "grant once",
            );
            return wire;
        }
        self.next_id = self.next_id.saturating_add(1).max(1);
        let grant = Grant {
            id: self.next_id,
            peer: peer.clone(),
            splint_id,
            incarnation,
            scopes: scopes.clone(),
            expires: Instant::now() + GRANT_LIFETIME,
            expires_at_unix_seconds: unix_seconds() + GRANT_LIFETIME.as_secs(),
        };
        let wire = grant.wire();
        self.grants.push(grant);
        self.record(
            peer,
            splint_id,
            incarnation,
            scopes,
            AuditDecision::Granted,
            "grant once",
        );
        wire
    }

    pub fn deny(
        &mut self,
        peer: &PeerIdentity,
        splint_id: SplintId,
        incarnation: u64,
        scopes: &[AccessScope],
        reason: &'static str,
    ) {
        self.record(
            peer,
            splint_id,
            incarnation,
            scopes.iter().copied().collect(),
            AuditDecision::Denied,
            reason,
        );
    }

    pub fn revoke(&mut self, grant_id: u64) -> Option<AccessGrant> {
        let index = self.grants.iter().position(|grant| grant.id == grant_id)?;
        let grant = self.grants.remove(index);
        let wire = grant.wire();
        self.record(
            &grant.peer,
            grant.splint_id,
            grant.incarnation,
            grant.scopes,
            AuditDecision::Revoked,
            "explicit local revocation",
        );
        Some(wire)
    }

    pub fn revoke_identity(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        reason: &'static str,
    ) -> Vec<u64> {
        let mut removed = Vec::new();
        self.grants.retain(|grant| {
            if grant.splint_id == splint_id && grant.incarnation == incarnation {
                removed.push(grant.clone());
                false
            } else {
                true
            }
        });
        let ids = removed.iter().map(|grant| grant.id).collect();
        for grant in removed {
            self.record(
                &grant.peer,
                splint_id,
                incarnation,
                grant.scopes,
                AuditDecision::Revoked,
                reason,
            );
        }
        ids
    }

    fn remove_expired(&mut self) {
        self.grants.retain(|grant| grant.expires > Instant::now());
    }

    fn record(
        &mut self,
        peer: &PeerIdentity,
        splint_id: SplintId,
        incarnation: u64,
        scopes: BTreeSet<AccessScope>,
        decision: AuditDecision,
        reason: &'static str,
    ) {
        self.next_audit_order = self.next_audit_order.saturating_add(1);
        if self.audit.len() == MAX_AUDIT_RECORDS {
            self.audit.pop_front();
        }
        self.audit.push_back(AuditRecord {
            order: self.next_audit_order,
            unix_seconds: unix_seconds(),
            peer_uid: peer.uid,
            peer_pid: peer.pid,
            requester: peer.requester_label(),
            splint_id,
            incarnation,
            scopes: scopes.into_iter().collect(),
            decision,
            reason,
        });
    }
}

pub async fn prompt(
    peer: &PeerIdentity,
    splint_id: SplintId,
    incarnation: u64,
    scopes: Vec<AccessScope>,
) -> Result<bool> {
    if scopes.is_empty() || scopes.len() > MAX_ACCESS_SCOPES {
        bail!("invalid consent scope count");
    }
    let peer = peer.clone();
    tokio::task::spawn_blocking(move || prompt_blocking(&peer, splint_id, incarnation, scopes))
        .await
        .context("consent broker task failed")?
}

fn prompt_blocking(
    peer: &PeerIdentity,
    splint_id: SplintId,
    incarnation: u64,
    scopes: Vec<AccessScope>,
) -> Result<bool> {
    let mut capability = vec![0_u8; CONSENT_CAPABILITY_BYTES];
    let mut offset = 0;
    while offset < capability.len() {
        offset += rustix::rand::getrandom(
            &mut capability[offset..],
            rustix::rand::GetRandomFlags::empty(),
        )
        .context("OS randomness unavailable")?;
    }
    let (mut daemon, child) = StdUnixStream::pair().context("create private consent socket")?;
    daemon.set_read_timeout(Some(CONSENT_DEADLINE))?;
    daemon.set_write_timeout(Some(CONSENT_DEADLINE))?;
    let child_read = child
        .try_clone()
        .context("duplicate private consent descriptor")?;
    let executable = std::env::current_exe()
        .context("locate splinterd executable")?
        .with_file_name("splinterm");
    let mut process = Command::new(&executable)
        .arg("consent")
        .stdin(Stdio::from(OwnedFd::from(child_read)))
        .stdout(Stdio::from(OwnedFd::from(child)))
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("launch trusted consent client {}", executable.display()))?;
    let prompt = ConsentPrompt {
        capability: capability.clone(),
        requester: peer.requester_label(),
        requester_pid: peer.pid,
        requester_uid: peer.uid,
        splint_id,
        incarnation,
        scopes,
    };
    write_bounded(&mut daemon, &prompt)?;
    let reply: ConsentReply = match read_bounded(&mut daemon) {
        Ok(reply) => reply,
        Err(error) => {
            let _ = process.kill();
            let _ = process.wait();
            return Err(error);
        }
    };
    let status = process.wait().context("wait for consent client")?;
    if !status.success() || reply.capability != capability {
        bail!("consent client authentication failed");
    }
    Ok(reply.granted)
}

pub fn read_bounded<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_CONSENT_FRAME_BYTES {
        bail!("invalid consent frame length");
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).context("invalid consent frame")
}

pub fn write_bounded<T: serde::Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode consent frame")?;
    if body.is_empty() || body.len() > MAX_CONSENT_FRAME_BYTES {
        bail!("consent frame exceeds bound");
    }
    writer.write_all(&u32::try_from(body.len())?.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_are_bound_to_peer_splint_incarnation_and_scope() {
        let peer = PeerIdentity::for_test();
        let other_peer = PeerIdentity {
            pid: peer.pid.saturating_add(1),
            ..peer.clone()
        };
        let splint = SplintId::new();
        let other_splint = SplintId::new();
        let mut store = GrantStore::default();
        let grant = store.grant(
            &peer,
            splint,
            7,
            vec![AccessScope::Observe, AccessScope::Input],
        );

        assert_eq!(
            store.authorize(&peer, splint, 7, &[AccessScope::Observe]),
            Some(grant.grant_id)
        );
        assert_eq!(
            store.authorize(&peer, splint, 7, &[AccessScope::Resize]),
            None
        );
        assert_eq!(
            store.authorize(&other_peer, splint, 7, &[AccessScope::Observe]),
            None
        );
        assert_eq!(
            store.authorize(&peer, other_splint, 7, &[AccessScope::Observe]),
            None
        );
        assert_eq!(
            store.authorize(&peer, splint, 8, &[AccessScope::Observe]),
            None
        );
    }

    #[test]
    fn revocation_removes_authority_and_records_bounded_metadata() {
        let peer = PeerIdentity::for_test();
        let splint = SplintId::new();
        let mut store = GrantStore::default();
        let grant = store.grant(&peer, splint, 1, vec![AccessScope::Observe]);
        assert!(store.revoke(grant.grant_id).is_some());
        assert_eq!(
            store.authorize(&peer, splint, 1, &[AccessScope::Observe]),
            None
        );
        assert_eq!(store.audit.len(), 2);
        assert!(
            store
                .audit
                .iter()
                .all(|record| record.requester.len() <= 1024)
        );
        assert!(
            store
                .audit
                .iter()
                .all(|record| record.scopes.len() <= MAX_ACCESS_SCOPES)
        );
    }

    #[test]
    fn private_frames_reject_zero_and_oversized_lengths() {
        let mut zero = &0_u32.to_be_bytes()[..];
        assert!(read_bounded::<ConsentReply>(&mut zero).is_err());
        let oversized = u32::try_from(MAX_CONSENT_FRAME_BYTES + 1)
            .unwrap()
            .to_be_bytes();
        assert!(read_bounded::<ConsentReply>(&mut &oversized[..]).is_err());
    }
}
