//! The Window's reader of the daemon access inventory (ADR 0014, Plan 0053
//! PR 5), run inside the topology manager on its connection, so it shares
//! the manager's handoff pause and reconnection.
//!
//! It polls `ListAccess` every few seconds and forwards a read only when it
//! differs from the last one sent, so the Window redraws only on a change:
//! a grant or pending-request change advances `authorization_revision`, and
//! a connection coming or going changes the entries' use counts. It serves
//! the Window's revokes, and confirms **Revoke all access** only against a
//! fresh dry run that counts exactly what the person confirmed.

use std::{
    fmt::Write as _,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use splinterm::{
    WindowTopologyCommand, WindowTopologyUpdate,
    automation::{Connection, is_connection_loss, protocol_error},
    frontend::{
        AccessSnapshot, MAX_WINDOW_ACCESS_ENTRIES, RevokeAllCheck, WindowAccessCommand,
        WindowAccessUpdate, check_revoke_all, revoke_all_summary, shown,
    },
};
use splinterm_protocol::{
    AccessRevocationCounts, AccessRevokeTarget, DurableRemoval, MAX_ACCESS_INVENTORY_PAGE, Request,
    Response,
};
use tokio::sync::mpsc;

use super::access_cli::render_revocation;

/// How often the inventory is read while nothing asks for it sooner.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How long to wait before reading again after the daemon refused a read.
const UNAVAILABLE_RETRY: Duration = Duration::from_secs(30);
const MAX_PAGES: usize = MAX_WINDOW_ACCESS_ENTRIES / MAX_ACCESS_INVENTORY_PAGE as usize;
const MAX_REASON_SCALARS: usize = 200;
/// A revoke whose connection was lost after the request was sent.
const UNCERTAIN: &str = "The connection to splinterd was lost; the revoke may not have completed. Check the list again.";

#[derive(Debug, Default)]
pub(super) struct AccessMonitor {
    /// When to read next; `None` reads at the next chance.
    next_read: Option<Instant>,
    last_sent: Option<AccessSnapshot>,
    unavailable: bool,
}

impl AccessMonitor {
    pub(super) fn due(&self, now: Instant) -> bool {
        self.next_read.is_none_or(|at| now >= at)
    }

    /// After a reconnection: read at once and send the read even if it
    /// matches the old daemon's.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Reads the inventory and sends it if it changed. `false` once the
    /// Window stopped listening.
    pub(super) async fn poll(
        &mut self,
        connection: &mut Connection,
        updates: &mpsc::Sender<WindowTopologyUpdate>,
    ) -> bool {
        let now = Instant::now();
        self.next_read = Some(now + POLL_INTERVAL);
        let update = match read_inventory(connection).await {
            Ok((snapshot, consistent)) => {
                if !consistent {
                    // It moved while it was read: read it again at once.
                    self.next_read = None;
                }
                self.unavailable = false;
                if self.last_sent.as_ref() == Some(&snapshot) {
                    return true;
                }
                self.last_sent = Some(snapshot.clone());
                WindowAccessUpdate::Snapshot(snapshot)
            }
            // The topology read that follows notices the loss and
            // reconnects the Window.
            Err(error) if is_connection_loss(&error) => return true,
            Err(error) => {
                self.next_read = Some(now + UNAVAILABLE_RETRY);
                if self.unavailable {
                    return true;
                }
                self.unavailable = true;
                self.last_sent = None;
                WindowAccessUpdate::Unavailable(reason(&error))
            }
        };
        send(updates, update).await
    }

    /// Serves one access command from the Window. `false` once the Window
    /// stopped listening.
    pub(super) async fn handle(
        &mut self,
        command: WindowAccessCommand,
        connection: &mut Connection,
        updates: &mpsc::Sender<WindowTopologyUpdate>,
    ) -> bool {
        let update = match command {
            WindowAccessCommand::Refresh => None,
            WindowAccessCommand::RevokeEntry { target } => {
                Some(revoke_entry(connection, target).await)
            }
            WindowAccessCommand::CountRevokeAll => Some(match count_revoke_all(connection).await {
                Ok(counts) => WindowAccessUpdate::RevokeAllCounts {
                    counts,
                    recounted: false,
                },
                Err(error) => failed_revoke_all(&error),
            }),
            WindowAccessCommand::RevokeAll { expected } => {
                Some(revoke_all(connection, expected).await)
            }
        };
        if let Some(update) = update
            && !send(updates, update).await
        {
            return false;
        }
        // Every command shows its effect at once.
        self.next_read = None;
        self.poll(connection, updates).await
    }
}

/// What the Window hears for an access command abandoned during a handoff
/// or reconnection: a revoke says it did not happen; a refresh needs
/// nothing. `None` for any other command.
pub(super) fn abandoned_access_updates(
    command: &WindowTopologyCommand,
    message: &str,
) -> Option<Vec<WindowTopologyUpdate>> {
    let WindowTopologyCommand::Access(command) = command else {
        return None;
    };
    let (all, entry) = match command {
        WindowAccessCommand::Refresh => return Some(Vec::new()),
        WindowAccessCommand::RevokeEntry { target } => (false, Some(target.key())),
        WindowAccessCommand::CountRevokeAll | WindowAccessCommand::RevokeAll { .. } => (true, None),
    };
    Some(vec![WindowTopologyUpdate::Access(
        WindowAccessUpdate::Revoked {
            message: format!("Nothing was revoked: {message}"),
            failed: true,
            all,
            entry,
        },
    )])
}

async fn send(updates: &mpsc::Sender<WindowTopologyUpdate>, update: WindowAccessUpdate) -> bool {
    updates
        .send(WindowTopologyUpdate::Access(update))
        .await
        .is_ok()
}

fn reason(error: &anyhow::Error) -> String {
    let text =
        protocol_error(error).map_or_else(|| format!("{error:#}"), |error| error.message.clone());
    shown(&text, MAX_REASON_SCALARS)
}

/// Reads every inventory page up to the Window's bound; whether no grant or
/// request changed between the pages.
async fn read_inventory(connection: &mut Connection) -> Result<(AccessSnapshot, bool)> {
    let mut snapshot: Option<AccessSnapshot> = None;
    let mut consistent = true;
    let mut after = None;
    for _ in 0..MAX_PAGES {
        let Response::AccessInventory {
            authorization_revision,
            remembered_store,
            entries,
            next_after,
            ..
        } = connection
            .request(Request::ListAccess {
                after: after.take(),
                max_entries: MAX_ACCESS_INVENTORY_PAGE,
            })
            .await?
        else {
            bail!("splinterd returned an unexpected access inventory response");
        };
        for entry in &entries {
            entry.validate().map_err(|problem| {
                anyhow!("splinterd listed an invalid access entry: {problem}")
            })?;
        }
        let read = snapshot.get_or_insert_with(|| {
            AccessSnapshot::new(authorization_revision, remembered_store.clone())
        });
        consistent &= read.revision == authorization_revision;
        read.revision = authorization_revision;
        read.remembered_store = remembered_store;
        read.entries.extend(entries);
        match next_after {
            None => return Ok((snapshot.expect("read above"), consistent)),
            Some(next) => after = Some(next),
        }
    }
    let mut snapshot = snapshot.expect("at least one page was read");
    snapshot.truncated = true;
    Ok((snapshot, consistent))
}

async fn revoke_entry(
    connection: &mut Connection,
    target: AccessRevokeTarget,
) -> WindowAccessUpdate {
    let entry = target.key();
    let result = connection
        .request(Request::RevokeAccessEntry { target })
        .await;
    let (message, failed) = match result {
        Ok(Response::AccessEntryRevoked { revocation, .. }) => match render_revocation(&revocation)
        {
            Ok(text) => (text, false),
            Err(text) => (text, true),
        },
        Ok(_) => (
            "Revoke failed: splinterd returned an unexpected response".to_owned(),
            true,
        ),
        // The request was sent: the daemon may have revoked it before the
        // connection went.
        Err(error) if is_connection_loss(&error) => (UNCERTAIN.to_owned(), true),
        Err(error) => (format!("Revoke failed: {}", reason(&error)), true),
    };
    WindowAccessUpdate::Revoked {
        message: one_line(&message),
        failed,
        all: false,
        entry: Some(entry),
    }
}

async fn revoke_all_request(
    connection: &mut Connection,
    dry_run: bool,
) -> Result<(AccessRevocationCounts, DurableRemoval)> {
    match connection
        .request(Request::RevokeAllAccess { dry_run })
        .await?
    {
        Response::AllAccessRevoked {
            dry_run: answered,
            counts,
            durable_removal,
            ..
        } if answered == dry_run => Ok((counts, durable_removal)),
        _ => bail!("splinterd returned an unexpected revoke response"),
    }
}

async fn count_revoke_all(connection: &mut Connection) -> Result<AccessRevocationCounts> {
    revoke_all_request(connection, true)
        .await
        .map(|(counts, _)| counts)
}

/// Revokes everything only if a fresh dry run counts exactly what the
/// person confirmed; otherwise nothing is revoked and the new counts are
/// shown for a new confirmation. A grant committed between that dry run and
/// the revoke is included, and the result reports what was removed.
async fn revoke_all(
    connection: &mut Connection,
    expected: AccessRevocationCounts,
) -> WindowAccessUpdate {
    let current = match count_revoke_all(connection).await {
        Ok(current) => current,
        Err(error) => return failed_revoke_all(&error),
    };
    if let RevokeAllCheck::Recount(counts) = check_revoke_all(expected, current) {
        return WindowAccessUpdate::RevokeAllCounts {
            counts,
            recounted: true,
        };
    }
    match revoke_all_request(connection, false).await {
        Ok((counts, durable_removal)) => {
            let mut message = revoke_all_summary(counts, true);
            let failed = if let DurableRemoval::Failed { message: problem } = durable_removal {
                let _ = write!(
                    message,
                    " Removing the remembered grants from the store failed: {}. They authorize nothing now, but return after a restart until a later write succeeds; Splinterm retries on its next write and at shutdown.",
                    shown(&problem, MAX_REASON_SCALARS)
                );
                true
            } else {
                false
            };
            WindowAccessUpdate::Revoked {
                message,
                failed,
                all: true,
                entry: None,
            }
        }
        Err(error) if is_connection_loss(&error) => WindowAccessUpdate::Revoked {
            message: UNCERTAIN.to_owned(),
            failed: true,
            all: true,
            entry: None,
        },
        Err(error) => failed_revoke_all(&error),
    }
}

fn failed_revoke_all(error: &anyhow::Error) -> WindowAccessUpdate {
    WindowAccessUpdate::Revoked {
        message: format!("Nothing was revoked: {}", reason(error)),
        failed: true,
        all: true,
        entry: None,
    }
}

fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_monitor_reads_at_once_then_on_its_interval() {
        let mut monitor = AccessMonitor::default();
        let now = Instant::now();
        assert!(monitor.due(now));
        monitor.next_read = Some(now + POLL_INTERVAL);
        assert!(!monitor.due(now));
        assert!(monitor.due(now + POLL_INTERVAL));
        monitor.unavailable = true;
        monitor.reset();
        assert!(monitor.due(now));
        assert!(!monitor.unavailable);
        assert_eq!(MAX_PAGES, 8);
    }

    #[test]
    fn abandoned_access_commands_say_nothing_was_revoked() {
        assert!(abandoned_access_updates(&WindowTopologyCommand::CloseWindow, "lost").is_none());
        assert!(matches!(
            abandoned_access_updates(
                &WindowTopologyCommand::Access(WindowAccessCommand::Refresh),
                "lost"
            ),
            Some(updates) if updates.is_empty()
        ));
        for (command, all, entry) in [
            (
                WindowAccessCommand::RevokeEntry {
                    target: AccessRevokeTarget::Grant { grant_id: 4 },
                },
                false,
                Some(splinterm_protocol::AccessEntryKey::Grant { grant_id: 4 }),
            ),
            (WindowAccessCommand::CountRevokeAll, true, None),
            (
                WindowAccessCommand::RevokeAll {
                    expected: AccessRevocationCounts::default(),
                },
                true,
                None,
            ),
        ] {
            let updates = abandoned_access_updates(&WindowTopologyCommand::Access(command), "lost");
            let Some([WindowTopologyUpdate::Access(update)]) = updates.as_deref() else {
                panic!("a revoke is answered");
            };
            assert_eq!(
                update,
                &WindowAccessUpdate::Revoked {
                    message: "Nothing was revoked: lost".into(),
                    failed: true,
                    all,
                    entry,
                }
            );
        }
    }

    #[test]
    fn revocation_text_is_one_line() {
        assert_eq!(
            one_line("Revoked grant 17.\n  More.\n\n"),
            "Revoked grant 17. More."
        );
    }
}
