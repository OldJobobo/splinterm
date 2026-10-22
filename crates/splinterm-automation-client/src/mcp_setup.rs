//! Public v2 projection of `splinterm mcp setup` and `splinterm mcp remove`
//! (ADR 0014, Plan 0053 PR 6): `mcp_setup` and `mcp_remove`.
//!
//! Both are local edits of an MCP host's configuration file. Neither contacts
//! the daemon, and neither creates a grant or a policy rule: every document
//! says so with `grants_access: false`. A removal never echoes the entry it
//! removed, because a hand-edited entry may carry secrets in its environment.

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;

use super::CliEnvelopeV2;

/// The longest path either document carries.
pub const MAX_MCP_SETUP_PATH_BYTES: usize = 4096;

/// Which of the two operations produced a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpSetupOperation {
    Setup,
    Remove,
}

impl McpSetupOperation {
    /// The public operation name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Setup => "mcp_setup",
            Self::Remove => "mcp_remove",
        }
    }
}

/// A supported MCP host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpHostName {
    ClaudeCode,
    Codex,
    Vscode,
}

/// The syntax of the host's configuration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConfigFormat {
    Json,
    Jsonc,
    Toml,
}

/// What the operation did, or with `dry_run` would do, to the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpSetupAction {
    /// Setup: the file did not exist and holds only the new entry.
    Created,
    /// Setup: the entry was added beside the existing ones.
    Added,
    /// Setup with `--replace`: a different `splinterm` entry was replaced.
    Replaced,
    /// Setup: the file already holds exactly this entry.
    Unchanged,
    /// Remove: the `splinterm` entry was removed.
    Removed,
    /// Remove: there was no `splinterm` entry.
    Absent,
}

impl McpSetupAction {
    /// Whether the file differs, or would differ, afterwards.
    #[must_use]
    pub const fn changes_file(self) -> bool {
        !matches!(self, Self::Unchanged | Self::Absent)
    }

    const fn belongs_to(self, operation: McpSetupOperation) -> bool {
        match operation {
            McpSetupOperation::Setup => matches!(
                self,
                Self::Created | Self::Added | Self::Replaced | Self::Unchanged
            ),
            McpSetupOperation::Remove => matches!(self, Self::Removed | Self::Absent),
        }
    }
}

/// One completed or planned host configuration change.
#[derive(Debug, Clone, PartialEq)]
pub struct McpSetupOutcome {
    pub operation: McpSetupOperation,
    pub host: McpHostName,
    pub config_path: String,
    pub config_format: McpConfigFormat,
    pub server_name: &'static str,
    pub action: McpSetupAction,
    pub dry_run: bool,
    /// Setup only: the adapter executable the entry starts.
    pub adapter_path: Option<String>,
    /// Setup only: the server entry, as JSON.
    pub entry: Option<Value>,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "the public document's independent flags, named by the schema"
)]
#[derive(Debug, Serialize)]
struct McpSetupDataV2<'a> {
    host: McpHostName,
    config_path: &'a str,
    config_format: McpConfigFormat,
    server_name: &'static str,
    action: McpSetupAction,
    changed: bool,
    dry_run: bool,
    written: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    adapter_path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<&'a Value>,
    grants_access: bool,
}

fn public_path(path: &str, label: &str) -> Result<()> {
    if !path.starts_with('/')
        || path.len() > MAX_MCP_SETUP_PATH_BYTES
        || path.chars().any(char::is_control)
    {
        bail!("{label} must be an absolute path of at most 4096 bytes without control characters");
    }
    Ok(())
}

/// Projects one host configuration change to public v2.
pub fn mcp_setup_envelope(outcome: &McpSetupOutcome) -> Result<CliEnvelopeV2> {
    if !outcome.action.belongs_to(outcome.operation) {
        bail!("MCP setup action does not belong to its operation");
    }
    public_path(&outcome.config_path, "host configuration path")?;
    match outcome.operation {
        McpSetupOperation::Setup => {
            let (Some(adapter_path), Some(Value::Object(_))) =
                (&outcome.adapter_path, &outcome.entry)
            else {
                bail!("MCP setup reports its adapter and entry");
            };
            public_path(adapter_path, "adapter path")?;
        }
        McpSetupOperation::Remove => {
            if outcome.adapter_path.is_some() || outcome.entry.is_some() {
                bail!("MCP removal never echoes an adapter or entry");
            }
        }
    }
    let changed = outcome.action.changes_file();
    CliEnvelopeV2::success(
        outcome.operation.name(),
        None,
        serde_json::to_value(McpSetupDataV2 {
            host: outcome.host,
            config_path: &outcome.config_path,
            config_format: outcome.config_format,
            server_name: outcome.server_name,
            action: outcome.action,
            changed,
            dry_run: outcome.dry_run,
            written: changed && !outcome.dry_run,
            adapter_path: outcome.adapter_path.as_deref(),
            entry: outcome.entry.as_ref(),
            grants_access: false,
        })?,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(action: McpSetupAction, dry_run: bool) -> McpSetupOutcome {
        McpSetupOutcome {
            operation: McpSetupOperation::Setup,
            host: McpHostName::Codex,
            config_path: "/home/u/.codex/config.toml".to_owned(),
            config_format: McpConfigFormat::Toml,
            server_name: "splinterm",
            action,
            dry_run,
            adapter_path: Some("/usr/bin/splinterm-mcp".to_owned()),
            entry: Some(serde_json::json!({"command": "/usr/bin/splinterm-mcp"})),
        }
    }

    fn data(outcome: &McpSetupOutcome) -> Value {
        serde_json::to_value(mcp_setup_envelope(outcome).unwrap()).unwrap()
    }

    #[test]
    fn setup_reports_the_entry_and_grants_nothing() {
        let document = data(&setup(McpSetupAction::Added, false));
        assert_eq!(document["operation"], "mcp_setup");
        assert_eq!(document["data"]["host"], "codex");
        assert_eq!(document["data"]["changed"], true);
        assert_eq!(document["data"]["written"], true);
        assert_eq!(document["data"]["grants_access"], false);
        assert_eq!(document["data"]["adapter_path"], "/usr/bin/splinterm-mcp");
        assert!(document.get("resource").is_none());
    }

    #[test]
    fn a_dry_run_or_unchanged_file_writes_nothing() {
        let dry = data(&setup(McpSetupAction::Added, true));
        assert_eq!(dry["data"]["changed"], true);
        assert_eq!(dry["data"]["written"], false);
        let same = data(&setup(McpSetupAction::Unchanged, false));
        assert_eq!(same["data"]["changed"], false);
        assert_eq!(same["data"]["written"], false);
    }

    #[test]
    fn inconsistent_outcomes_are_rejected() {
        assert!(mcp_setup_envelope(&setup(McpSetupAction::Removed, false)).is_err());
        let mut relative = setup(McpSetupAction::Added, false);
        relative.config_path = "config.toml".to_owned();
        assert!(mcp_setup_envelope(&relative).is_err());
        let mut control = setup(McpSetupAction::Added, false);
        control.adapter_path = Some("/usr/bin/splinterm-mcp\n".to_owned());
        assert!(mcp_setup_envelope(&control).is_err());
        let echoing = McpSetupOutcome {
            operation: McpSetupOperation::Remove,
            action: McpSetupAction::Removed,
            ..setup(McpSetupAction::Removed, false)
        };
        assert!(mcp_setup_envelope(&echoing).is_err());
        let remove = McpSetupOutcome {
            adapter_path: None,
            entry: None,
            ..echoing
        };
        let document = data(&remove);
        assert_eq!(document["operation"], "mcp_remove");
        assert!(document["data"].get("entry").is_none());
        assert!(document["data"].get("adapter_path").is_none());
    }
}
