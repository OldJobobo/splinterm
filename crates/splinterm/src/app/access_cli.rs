//! `splinterm access`: see which agents can reach which workspaces, and
//! revoke their access (ADR 0014, Plan 0053 PR 4).
//!
//! Human output connects as the installed trusted client, so it works only
//! for the packaged `splinterm` next to the running daemon. Machine output
//! (`--output json`) uses the automation role and needs `authorization_inspect`
//! or `authorization_revoke` from the policy file; no consent level grants
//! either.

use std::{
    fmt::Write as _,
    io::{self, IsTerminal as _, Write as _},
};

use anyhow::{Context, Result, bail};
use splinterm::automation::{
    Connection, access_entry_id, consent_scope_name, parse_access_entry_id,
    parse_access_revoke_target,
};
use splinterm_protocol::{
    AccessEntry, AccessEntryKey, AccessEntryKind, AccessEntryState, AccessIdentityKind,
    AccessPolicyResource, AccessRevocation, AccessRevocationCounts, AccessRevokeTarget,
    DurableRemoval, MAX_ACCESS_INVENTORY_PAGE, RememberedStoreStatus, Request, Response,
};

use super::commands::AccessCommand;

/// Pages a human listing reads at most (64 entries each).
const MAX_HUMAN_PAGES: usize = 64;

/// One resolved `splinterm access` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum AccessRequest {
    List {
        after: Option<AccessEntryKey>,
        max_entries: Option<u16>,
    },
    Revoke {
        target: AccessRevokeTarget,
        yes: bool,
    },
    RevokeAll {
        dry_run: bool,
        yes: bool,
    },
}

impl AccessRequest {
    pub(in crate::app) const fn operation(&self) -> &'static str {
        match self {
            Self::List { .. } => "access_list",
            Self::Revoke { .. } => "access_revoke",
            Self::RevokeAll { .. } => "access_revoke_all",
        }
    }

    /// A revoke that changes something needs `--yes` in machine output.
    pub(in crate::app) const fn confirmation_missing(&self) -> bool {
        matches!(
            self,
            Self::Revoke { yes: false, .. }
                | Self::RevokeAll {
                    dry_run: false,
                    yes: false
                }
        )
    }
}

/// Resolves the command line before connecting, so a malformed ID is a
/// usage error (exit 2, no machine document) in every output mode.
///
/// # Errors
/// A usage message.
pub(in crate::app) fn resolve_access_request(
    command: AccessCommand,
) -> Result<AccessRequest, String> {
    match command {
        AccessCommand::List { after, max_entries } => Ok(AccessRequest::List {
            after: after
                .map(|id| {
                    parse_access_entry_id(&id).ok_or_else(|| {
                        "--after takes an entry ID from `splinterm access list`".to_owned()
                    })
                })
                .transpose()?,
            max_entries,
        }),
        AccessCommand::Revoke {
            id: Some(_),
            dry_run: true,
            ..
        } => Err("--dry-run applies only to --all".to_owned()),
        AccessCommand::Revoke {
            id: Some(id),
            all: false,
            yes,
            ..
        } => Ok(AccessRequest::Revoke {
            target: parse_access_revoke_target(&id).map_err(|error| error.to_string())?,
            yes,
        }),
        AccessCommand::Revoke {
            all: true,
            dry_run,
            yes,
            ..
        } => Ok(AccessRequest::RevokeAll { dry_run, yes }),
        AccessCommand::Revoke { .. } => Err("access revoke takes an ID or --all".to_owned()),
    }
}

/// Text for a terminal: control and bidirectional formatting characters are
/// shown escaped, so agent-reported names, paths, and workspace names can
/// neither move the cursor nor reorder the line.
fn printable(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control()
            || matches!(
                character,
                '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
            )
        {
            let _ = write!(shown, "\\u{{{:04x}}}", u32::from(character));
        } else {
            shown.push(character);
        }
    }
    shown
}

/// `YYYY-MM-DD HH:MM UTC`, from Unix seconds.
fn utc(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX / 2);
    let minutes = seconds % 86_400 / 60;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        minutes / 60,
        minutes % 60
    )
}

fn plural(count: u32, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

fn access_text(entry: &AccessEntry) -> String {
    if let Some(level) = entry.level {
        return level.label().to_owned();
    }
    let scopes: Vec<_> = entry
        .scopes
        .iter()
        .filter_map(|scope| consent_scope_name(*scope))
        .collect();
    format!("Custom ({})", scopes.join(", "))
}

fn workspace_text(entry: &AccessEntry) -> String {
    let Some(workspace) = &entry.workspace else {
        return "no workspace".to_owned();
    };
    let short: String = workspace.lair_id.to_string().chars().take(8).collect();
    match &workspace.name {
        Some(name) => format!("workspace '{}' ({short})", printable(name)),
        None => format!("a workspace that no longer exists ({short})"),
    }
}

fn identity_text(entry: &AccessEntry) -> String {
    let identity = &entry.identity;
    let kind = match identity.kind {
        AccessIdentityKind::Process => match identity.pid {
            Some(pid) => format!("process {pid}"),
            None => "one process".to_owned(),
        },
        AccessIdentityKind::ExactBuild => "this exact build".to_owned(),
        AccessIdentityKind::ProtectedInstall => "protected install".to_owned(),
        AccessIdentityKind::NixProfile => "Nix profile".to_owned(),
        AccessIdentityKind::PolicyDigest => "exact digest in the policy file".to_owned(),
    };
    let path = printable(&identity.executable_path);
    match &identity.display_name {
        Some(name) => format!(
            "{} (reported by the agent) · {path}, {kind}",
            printable(name)
        ),
        None => format!("{path}, {kind}"),
    }
}

fn headline(entry: &AccessEntry) -> String {
    let access = access_text(entry);
    let workspace = workspace_text(entry);
    match (entry.kind, entry.state) {
        (AccessEntryKind::Once, _) => match (entry.splint_id, entry.incarnation) {
            (Some(splint_id), Some(incarnation)) if entry.workspace.is_none() => format!(
                "Just this once for Splint {splint_id} incarnation {incarnation}, no longer in a workspace: {access}"
            ),
            (Some(splint_id), Some(incarnation)) => format!(
                "Just this once for Splint {splint_id} incarnation {incarnation} in {workspace}: {access}"
            ),
            _ => format!("Just this once for {workspace}: {access}"),
        },
        (AccessEntryKind::WorkspaceSession, _) => {
            format!("While {workspace} is open: {access}")
        }
        (AccessEntryKind::Remembered, AccessEntryState::RemovalPending) => {
            format!("Revoked, but still in the store: always for {workspace}: {access}")
        }
        (AccessEntryKind::Remembered, AccessEntryState::WorkspaceMissing) => {
            format!("Always for {workspace}: {access} (authorizes nothing until revoked)")
        }
        (AccessEntryKind::Remembered, _) => format!("Always for {workspace}: {access}"),
        (AccessEntryKind::Pending, state) => format!(
            "Waiting for an answer ({}): {access} for {workspace}",
            if state == AccessEntryState::Prompting {
                "prompt showing"
            } else {
                "queued"
            }
        ),
        (AccessEntryKind::PersistentPolicy, _) => format!(
            "Policy file rule, read-only here: {}",
            plural(
                u32::try_from(entry.policy_scopes.len()).unwrap_or(u32::MAX),
                "scope",
                "scopes"
            )
        ),
    }
}

fn policy_resources_text(resources: &[AccessPolicyResource]) -> String {
    resources
        .iter()
        .map(|resource| match resource {
            AccessPolicyResource::Daemon => "the daemon".to_owned(),
            AccessPolicyResource::Lair { lair_id } => format!("Lair {lair_id}"),
            AccessPolicyResource::Dojo { dojo_id } => format!("Dojo {dojo_id}"),
            AccessPolicyResource::Splint { splint_id } => format!("Splint {splint_id}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders the whole inventory for a person.
pub(in crate::app) fn render_inventory(
    entries: &[AccessEntry],
    store: &RememberedStoreStatus,
) -> String {
    let mut output = String::new();
    match store {
        RememberedStoreStatus::Available | RememberedStoreStatus::NotConfigured => {}
        RememberedStoreStatus::Unavailable { reason } => {
            let _ = writeln!(
                output,
                "Remembered access is unavailable: {}. \"Always\" grants authorize nothing until the store is repaired; policy and shorter grants still work.\n",
                printable(reason)
            );
        }
    }
    if entries.is_empty() {
        output.push_str("No agent has access to any workspace.\n");
        return output;
    }
    for entry in entries {
        let _ = writeln!(output, "{}", access_entry_id(&entry.key));
        let _ = writeln!(output, "  {}", headline(entry));
        let _ = writeln!(output, "  Agent: {}", identity_text(entry));
        if entry.kind == AccessEntryKind::PersistentPolicy {
            let _ = writeln!(
                output,
                "  Covers: {}",
                policy_resources_text(&entry.policy_resources)
            );
        }
        let mut times = Vec::new();
        if let Some(created) = entry.created_at_unix_seconds {
            times.push(format!(
                "{} {}",
                if entry.kind == AccessEntryKind::Pending {
                    "asked"
                } else {
                    "created"
                },
                utc(created)
            ));
        }
        if entry.kind == AccessEntryKind::Remembered {
            times.push(entry.last_used_at_unix_seconds.map_or_else(
                || "never used".to_owned(),
                |used| format!("last used {}", utc(used)),
            ));
        }
        if let Some(expires) = entry.expires_at_unix_seconds {
            times.push(format!("expires {}", utc(expires)));
        }
        if !times.is_empty() {
            let mut line = times.join("; ");
            if let Some(first) = line.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            let _ = writeln!(output, "  {line}");
        }
        if entry.kind != AccessEntryKind::Pending {
            let _ = writeln!(
                output,
                "  In use: {}, {}",
                plural(entry.connections, "connection", "connections"),
                plural(entry.controllers, "controller", "controllers"),
            );
        }
        match (entry.kind, entry.state) {
            (AccessEntryKind::Remembered, AccessEntryState::RemovalPending) => {
                let _ = writeln!(
                    output,
                    "  It authorizes nothing now, but returns after a restart until the store is written; the daemon retries on its next write and at shutdown."
                );
            }
            (AccessEntryKind::Pending, _) => {
                let _ = writeln!(
                    output,
                    "  Not a grant: it ends when answered or when it expires, and `splinterm access revoke --all` ends it."
                );
            }
            (AccessEntryKind::PersistentPolicy, _) => {
                let _ = writeln!(
                    output,
                    "  Edit the policy file and reload the policy to change it."
                );
            }
            _ => {}
        }
        if !entry.policy_rules.is_empty() {
            let _ = writeln!(
                output,
                "  Access continues under policy rule {} after a revoke; edit the policy file to remove it.",
                entry.policy_rules.join(", ")
            );
        }
        output.push('\n');
    }
    output.push_str(
        "Revoke one with `splinterm access revoke ID`, or everything with `splinterm access revoke --all`.\n",
    );
    output
}

/// What a person should know about a listing read across pages: it
/// changed meanwhile, or it stopped at the page cap.
pub(in crate::app) fn listing_notes(
    changed: bool,
    stopped_before: Option<&AccessEntryKey>,
) -> String {
    let mut notes = String::new();
    if let Some(next) = stopped_before {
        let _ = writeln!(
            notes,
            "The list stopped after {MAX_HUMAN_PAGES} pages; continue with `splinterm access list --after {}`.",
            access_entry_id(next)
        );
    }
    if changed {
        notes.push_str(
            "Access changed while the list was read; run it again for a consistent view.\n",
        );
    }
    notes
}

fn durable_failure_text(message: &str) -> String {
    format!(
        "removing it from the remembered-grant store failed: {}. It authorizes nothing now, but returns after a restart until a later write succeeds; the daemon retries on its next write and at shutdown",
        printable(message)
    )
}

/// The person-facing result of one revoke; `Err` when the durable removal
/// failed, after the revoke itself succeeded.
pub(in crate::app) fn render_revocation(revocation: &AccessRevocation) -> Result<String, String> {
    let id = access_entry_id(&revocation.key);
    let mut output = match revocation.kind {
        AccessEntryKind::Remembered => format!("Revoked remembered grant {id}."),
        _ => format!("Revoked grant {id}."),
    };
    output.push_str(" Its subscriptions and controllers ended.\n");
    if !revocation.policy_rules.is_empty() {
        let _ = writeln!(
            output,
            "Access continues under policy rule {}; edit the policy file and reload the policy to remove it.",
            revocation.policy_rules.join(", ")
        );
    }
    match &revocation.durable_removal {
        DurableRemoval::Failed { message } => Err(format!(
            "{output}Warning: {}.",
            durable_failure_text(message)
        )),
        DurableRemoval::Written => {
            output.push_str("It was removed from the remembered-grant store.\n");
            Ok(output)
        }
        DurableRemoval::NotNeeded => Ok(output),
    }
}

/// The person-facing counts of **Revoke all access**.
pub(in crate::app) fn render_counts(counts: AccessRevocationCounts, done: bool) -> String {
    format!(
        "{} {}, and {} {}, {}, and {}. Policy rules are not touched.",
        if done { "Removed" } else { "This removes" },
        plural(counts.remembered, "remembered grant", "remembered grants"),
        if done { "ended" } else { "ends" },
        plural(counts.once, "just-this-once grant", "just-this-once grants"),
        plural(
            counts.workspace_session,
            "while-open grant",
            "while-open grants"
        ),
        plural(
            counts.pending_requests,
            "pending request",
            "pending requests"
        ),
    )
}

fn confirm_revoke_all(counts: AccessRevocationCounts) -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!(
            "refusing to revoke all access without an interactive terminal; pass --yes to confirm"
        );
    }
    eprint!(
        "{}\nRevoke all access? [y/N] ",
        render_counts(counts, false)
    );
    io::stderr()
        .flush()
        .context("failed to display confirmation")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("failed to read confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

async fn revoke_all(connection: &mut Connection, dry_run: bool) -> Result<Response> {
    connection
        .request(Request::RevokeAllAccess { dry_run })
        .await
}

/// Runs one resolved access operation with human output, as the installed
/// trusted client.
pub(in crate::app) async fn run_human_access(request: AccessRequest) -> Result<()> {
    let mut connection = Connection::connect().await?;
    match request {
        AccessRequest::List { after, max_entries } => {
            let mut entries = Vec::new();
            let mut store = RememberedStoreStatus::NotConfigured;
            let mut cursor = after;
            let mut first_revision = None;
            let mut changed = false;
            let mut complete = false;
            for _ in 0..MAX_HUMAN_PAGES {
                let Response::AccessInventory {
                    authorization_revision,
                    remembered_store,
                    entries: page,
                    next_after,
                    ..
                } = connection
                    .request(Request::ListAccess {
                        after: cursor.clone(),
                        max_entries: max_entries.unwrap_or(MAX_ACCESS_INVENTORY_PAGE),
                    })
                    .await?
                else {
                    bail!("splinterd returned an unexpected access inventory response");
                };
                store = remembered_store;
                entries.extend(page);
                changed |=
                    *first_revision.get_or_insert(authorization_revision) != authorization_revision;
                match next_after {
                    None => {
                        complete = true;
                        break;
                    }
                    // An explicit page size prints exactly one page.
                    Some(next) if max_entries.is_some() => {
                        print!("{}", render_inventory(&entries, &store));
                        println!("More entries follow: --after {}", access_entry_id(&next));
                        return Ok(());
                    }
                    Some(next) => cursor = Some(next),
                }
            }
            print!("{}", render_inventory(&entries, &store));
            print!(
                "{}",
                listing_notes(changed, (!complete).then_some(cursor).flatten().as_ref())
            );
            Ok(())
        }
        AccessRequest::Revoke { target, .. } => {
            let Response::AccessEntryRevoked { revocation, .. } = connection
                .request(Request::RevokeAccessEntry { target })
                .await?
            else {
                bail!("splinterd returned an unexpected revoke response");
            };
            match render_revocation(&revocation) {
                Ok(text) => {
                    print!("{text}");
                    Ok(())
                }
                Err(text) => bail!("{text}"),
            }
        }
        AccessRequest::RevokeAll { dry_run, yes } => {
            let Response::AllAccessRevoked { counts, .. } =
                revoke_all(&mut connection, true).await?
            else {
                bail!("splinterd returned an unexpected revoke response");
            };
            if dry_run {
                println!("{}", render_counts(counts, false));
                return Ok(());
            }
            if !yes && !confirm_revoke_all(counts)? {
                println!("Revoke all access cancelled.");
                return Ok(());
            }
            let Response::AllAccessRevoked {
                counts,
                durable_removal,
                ..
            } = revoke_all(&mut connection, false).await?
            else {
                bail!("splinterd returned an unexpected revoke response");
            };
            println!("{}", render_counts(counts, true));
            if let DurableRemoval::Failed { message } = durable_removal {
                bail!("Warning: {}.", durable_failure_text(&message));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::commands::{Cli, Command};
    use clap::Parser as _;
    use splinterm_core::LairId;
    use splinterm_protocol::{AccessIdentity, AccessLevel, AccessScope, AccessWorkspace};

    fn resolve(arguments: &[&str]) -> Result<AccessRequest, String> {
        let Some(Command::Access { command }) = Cli::try_parse_from(arguments).unwrap().command
        else {
            panic!("not an access command");
        };
        resolve_access_request(command)
    }

    #[test]
    fn ids_resolve_to_exactly_one_kind_and_only_grants_are_revocable() {
        let entry = format!("ra_{}", "0123456789abcdef".repeat(2));
        assert_eq!(
            resolve(&["splinterm", "access", "revoke", "17"]),
            Ok(AccessRequest::Revoke {
                target: AccessRevokeTarget::Grant { grant_id: 17 },
                yes: false,
            })
        );
        assert_eq!(
            resolve(&["splinterm", "access", "revoke", &entry, "--yes"]),
            Ok(AccessRequest::Revoke {
                target: AccessRevokeTarget::Remembered {
                    entry_id: entry.clone()
                },
                yes: true,
            })
        );
        assert!(resolve(&["splinterm", "access", "revoke", "17", "--dry-run"]).is_err());
        for refused in ["pending:3", "policy:mcp-read", "0", "017", "ra_ABC", "x"] {
            assert!(
                resolve(&["splinterm", "access", "revoke", refused]).is_err(),
                "{refused}"
            );
        }
        assert_eq!(
            resolve(&["splinterm", "access", "list", "--after", "pending:3"]),
            Ok(AccessRequest::List {
                after: Some(AccessEntryKey::Pending { request_id: 3 }),
                max_entries: None,
            })
        );
        assert!(resolve(&["splinterm", "access", "list", "--after", "nope"]).is_err());
        assert_eq!(
            resolve(&["splinterm", "access", "revoke", "--all", "--dry-run"]),
            Ok(AccessRequest::RevokeAll {
                dry_run: true,
                yes: false
            })
        );
    }

    #[test]
    fn the_grammar_refuses_ambiguous_revokes_and_oversized_pages() {
        for refused in [
            &["splinterm", "access", "revoke"][..],
            &["splinterm", "access", "revoke", "17", "--all"],
            &["splinterm", "access", "list", "--max-entries", "65"],
            &["splinterm", "access", "list", "--max-entries", "0"],
        ] {
            assert!(Cli::try_parse_from(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn machine_revokes_that_change_something_need_yes() {
        assert!(
            AccessRequest::Revoke {
                target: AccessRevokeTarget::Grant { grant_id: 1 },
                yes: false
            }
            .confirmation_missing()
        );
        assert!(
            AccessRequest::RevokeAll {
                dry_run: false,
                yes: false
            }
            .confirmation_missing()
        );
        assert!(
            !AccessRequest::RevokeAll {
                dry_run: true,
                yes: false
            }
            .confirmation_missing()
        );
    }

    fn entry(kind: AccessEntryKind, state: AccessEntryState) -> AccessEntry {
        let key = match kind {
            AccessEntryKind::Remembered => AccessEntryKey::Remembered {
                entry_id: format!("ra_{}", "a".repeat(32)),
            },
            AccessEntryKind::Pending => AccessEntryKey::Pending { request_id: 2 },
            _ => AccessEntryKey::Grant { grant_id: 9 },
        };
        AccessEntry {
            grant_id: matches!(key, AccessEntryKey::Grant { .. }).then_some(9),
            key,
            kind,
            state,
            revocable: kind != AccessEntryKind::Pending
                && state != AccessEntryState::RemovalPending,
            identity: AccessIdentity {
                kind: AccessIdentityKind::ProtectedInstall,
                executable_path: "/usr/bin/splinterm-mcp\u{1b}[2J".to_owned(),
                pid: None,
                display_name: Some("claude\u{202e}edoc".to_owned()),
            },
            workspace: Some(AccessWorkspace {
                lair_id: LairId::new(),
                name: Some("work".to_owned()),
            }),
            splint_id: None,
            incarnation: None,
            level: Some(AccessLevel::Look),
            scopes: AccessLevel::Look.scopes().to_vec(),
            policy_scopes: Vec::new(),
            policy_resources: Vec::new(),
            created_at_unix_seconds: Some(1_760_918_400),
            last_used_at_unix_seconds: None,
            expires_at_unix_seconds: None,
            connections: 1,
            controllers: 0,
            policy_rules: vec!["mcp-read".to_owned()],
        }
    }

    #[test]
    fn the_listing_names_each_duration_and_escapes_untrusted_text() {
        let remembered = entry(AccessEntryKind::Remembered, AccessEntryState::Active);
        let text = render_inventory(
            &[
                remembered.clone(),
                entry(
                    AccessEntryKind::Remembered,
                    AccessEntryState::RemovalPending,
                ),
                entry(AccessEntryKind::WorkspaceSession, AccessEntryState::Active),
                entry(AccessEntryKind::Pending, AccessEntryState::Queued),
            ],
            &RememberedStoreStatus::Available,
        );
        assert!(text.contains("Always for workspace 'work'"), "{text}");
        assert!(text.contains("While workspace 'work'"), "{text}");
        assert!(text.contains("Waiting for an answer (queued)"), "{text}");
        assert!(text.contains("returns after a restart"), "{text}");
        assert!(
            text.contains("Created 2025-10-20 00:00 UTC; never used"),
            "{text}"
        );
        assert!(text.contains("policy rule mcp-read"), "{text}");
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{202e}'),
            "{text}"
        );
        assert!(
            text.contains("\\u{001b}") && text.contains("\\u{202e}"),
            "{text}"
        );
        let mut custom = remembered;
        custom.level = None;
        custom.scopes = vec![AccessScope::Observe, AccessScope::Input];
        assert!(
            render_inventory(&[custom], &RememberedStoreStatus::Available)
                .contains("Custom (observe, input)")
        );
        assert!(
            render_inventory(
                &[],
                &RememberedStoreStatus::Unavailable {
                    reason: "corrupt".to_owned()
                }
            )
            .contains("Remembered access is unavailable: corrupt")
        );
    }

    #[test]
    fn a_failed_durable_removal_is_reported_as_a_failure() {
        let revocation = AccessRevocation {
            key: AccessEntryKey::Remembered {
                entry_id: format!("ra_{}", "a".repeat(32)),
            },
            kind: AccessEntryKind::Remembered,
            grant_id: 5,
            lair_id: None,
            durable_removal: DurableRemoval::Failed {
                message: "read-only file system".to_owned(),
            },
            policy_rules: Vec::new(),
        };
        let failed = render_revocation(&revocation).unwrap_err();
        assert!(failed.contains("Revoked remembered grant ra_"), "{failed}");
        assert!(failed.contains("returns after a restart"), "{failed}");
        let written = AccessRevocation {
            durable_removal: DurableRemoval::Written,
            ..revocation
        };
        assert!(
            render_revocation(&written)
                .unwrap()
                .contains("removed from")
        );
    }

    #[test]
    fn a_listing_says_when_it_moved_or_stopped_at_the_page_cap() {
        assert_eq!(listing_notes(false, None), "");
        let stopped = listing_notes(false, Some(&AccessEntryKey::Grant { grant_id: 9 }));
        assert!(stopped.contains("--after 9"), "{stopped}");
        assert!(listing_notes(true, None).contains("Access changed while the list was read"));
    }

    #[test]
    fn a_splint_grant_without_a_workspace_is_listed_as_such() {
        let mut orphan = entry(AccessEntryKind::Once, AccessEntryState::Active);
        orphan.workspace = None;
        orphan.splint_id = Some(splinterm_core::SplintId::new());
        orphan.incarnation = Some(3);
        orphan.policy_rules.clear();
        assert_eq!(orphan.validate(), Ok(()));
        let text = render_inventory(&[orphan], &RememberedStoreStatus::Available);
        assert!(
            text.contains("incarnation 3, no longer in a workspace"),
            "{text}"
        );
    }

    #[test]
    fn counts_and_times_render_exactly() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc(1_760_961_540), "2025-10-20 11:59 UTC");
        assert_eq!(
            render_counts(
                AccessRevocationCounts {
                    once: 1,
                    workspace_session: 0,
                    remembered: 2,
                    pending_requests: 1,
                },
                false
            ),
            "This removes 2 remembered grants, and ends 1 just-this-once grant, 0 while-open grants, and 1 pending request. Policy rules are not touched."
        );
    }
}
