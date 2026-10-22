//! `splinterm mcp setup HOST` and `splinterm mcp remove HOST` (ADR 0014,
//! Plan 0053 PR 6): one-step MCP host setup for the installed adapter.
//!
//! Setup writes one server entry, named `splinterm`, that starts the
//! `splinterm-mcp` adapter installed next to this `splinterm`, in the form the
//! host's own `mcp add` command writes. It edits nothing else in the file,
//! never contacts the daemon, and creates no grant and no policy rule: the
//! agent asks for access the first time it needs a workspace, and the person
//! answers in the trusted prompt.

mod file;
mod json_config;
mod toml_config;

#[cfg(test)]
mod tests;

use std::{
    env,
    ffi::OsString,
    fmt, fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use splinterm::automation::{
    CliEnvelopeV2, CliErrorCodeV2, MAX_MCP_SETUP_PATH_BYTES, McpConfigFormat, McpHostName,
    McpSetupAction as Action, McpSetupOperation, McpSetupOutcome, mcp_setup_envelope,
    write_json_document,
};

use super::{
    commands::{McpCommand, McpHostArg, OutputMode},
    local_service::usage_error,
};

/// The server name setup writes and removes.
pub(super) const SERVER_NAME: &str = "splinterm";
const ADAPTER_NAME: &str = "splinterm-mcp";
/// Codex starts servers with a filtered environment, so its entry names the
/// Splinterm variables `splinterm.report_agent_status` reads.
const CODEX_ENV_VARS: &[&str] = &["SPLINTERM_SPLINT_ID", "SPLINTERM_SPLINT_INCARNATION"];
/// VS Code user-data directories under `$XDG_CONFIG_HOME`, for the Microsoft
/// build and the Code - OSS build distributions ship.
const VSCODE_DIRECTORIES: &[&str] = &["Code", "Code - OSS"];

pub(super) const GRANTS_NOTHING: &str = "This grants no access and writes no policy: the agent asks for access to a workspace the first time it needs it, and you answer in Splinterm's prompt.";
const REVOKES_NOTHING: &str = "Removing the entry revokes nothing: `splinterm access list` shows the adapter's grants, and `splinterm access revoke` ends them.";

/// What a planned edit does.
pub(super) struct Plan {
    pub(super) action: Action,
    /// The complete new file, or `None` when nothing changes.
    pub(super) new_text: Option<String>,
}

impl Plan {
    pub(super) fn change(action: Action, new_text: String) -> Self {
        Self {
            action,
            new_text: Some(new_text),
        }
    }

    pub(super) fn unchanged(action: Action) -> Self {
        Self {
            action,
            new_text: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FailureKind {
    /// A different `splinterm` entry exists and `--replace` was not given.
    Conflict,
    /// The file, its directory, a path, or the document cannot be used.
    Invalid,
    /// The adapter or a required directory does not exist.
    NotFound,
    /// The file changed between reading and replacing it.
    Changed,
}

/// A setup failure with a public category.
#[derive(Debug)]
pub(super) struct Failure {
    pub(super) kind: FailureKind,
    message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

pub(super) fn failure(kind: FailureKind, message: impl Into<String>) -> anyhow::Error {
    Failure {
        kind,
        message: message.into(),
    }
    .into()
}

/// Raised by the editors before the path is known; [`at_path`] words it.
pub(super) fn conflict() -> anyhow::Error {
    failure(FailureKind::Conflict, "conflict")
}

/// Raised by the editors before the path is known; [`at_path`] words it.
pub(super) fn unsafe_edit() -> anyhow::Error {
    failure(FailureKind::Invalid, "unsafe edit")
}

fn failure_kind(error: &anyhow::Error) -> Option<FailureKind> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<Failure>())
        .map(|failure| failure.kind)
}

/// Words an editor error for the file at `path`.
fn at_path(error: anyhow::Error, path: &Path) -> anyhow::Error {
    let path = path.display();
    match error
        .downcast_ref::<Failure>()
        .map(|failure| (failure.kind, failure.message.as_str()))
    {
        Some((FailureKind::Conflict, _)) => failure(
            FailureKind::Conflict,
            format!(
                "{path} already has a different `{SERVER_NAME}` MCP server entry, which setup leaves alone. Review it, then run setup again with --replace to overwrite it"
            ),
        ),
        Some((FailureKind::Invalid, "unsafe edit")) => failure(
            FailureKind::Invalid,
            format!(
                "{path} cannot be edited safely in place: its `{SERVER_NAME}` entry or the table that holds it uses a form setup does not rewrite. Nothing was written; edit the entry by hand as docs/mcp.md shows"
            ),
        ),
        Some(_) => error,
        None => failure(
            FailureKind::Invalid,
            format!("{path}: {error:#}. Nothing was written; fix the file first"),
        ),
    }
}

/// Process environment that selects host configuration locations. Each value
/// is used only when it is set, non-empty, and absolute.
#[derive(Clone, Debug, Default)]
pub(super) struct Environment {
    pub(super) home: Option<PathBuf>,
    pub(super) xdg_config_home: Option<PathBuf>,
    pub(super) claude_config_dir: Option<PathBuf>,
    pub(super) codex_home: Option<PathBuf>,
    pub(super) user: Option<String>,
}

fn absolute_env(name: &str, value: Option<OsString>) -> Result<Option<PathBuf>> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(failure(
            FailureKind::Invalid,
            format!("{name} must be an absolute path"),
        ));
    }
    Ok(Some(path))
}

impl Environment {
    fn from_process() -> Result<Self> {
        Ok(Self {
            home: absolute_env("HOME", env::var_os("HOME"))?,
            xdg_config_home: absolute_env("XDG_CONFIG_HOME", env::var_os("XDG_CONFIG_HOME"))?,
            claude_config_dir: absolute_env("CLAUDE_CONFIG_DIR", env::var_os("CLAUDE_CONFIG_DIR"))?,
            codex_home: absolute_env("CODEX_HOME", env::var_os("CODEX_HOME"))?,
            user: env::var("USER").ok().filter(|user| {
                !user.is_empty() && !user.contains('/') && user != "." && user != ".."
            }),
        })
    }

    fn home(&self) -> Result<&Path> {
        self.home.as_deref().ok_or_else(|| {
            failure(
                FailureKind::Invalid,
                "HOME must name an absolute directory, or pass --config PATH",
            )
        })
    }
}

/// One supported host.
#[derive(Clone, Copy, Debug)]
pub(super) struct Host {
    pub(super) name: McpHostName,
    pub(super) label: &'static str,
    pub(super) format: McpConfigFormat,
    reload: &'static str,
}

impl Host {
    pub(super) const fn of(argument: McpHostArg) -> Self {
        match argument {
            McpHostArg::ClaudeCode => Self {
                name: McpHostName::ClaudeCode,
                label: "Claude Code",
                format: McpConfigFormat::Json,
                reload: "Start a new Claude Code session to load it.",
            },
            McpHostArg::Codex => Self {
                name: McpHostName::Codex,
                label: "Codex",
                format: McpConfigFormat::Toml,
                reload: "Start a new Codex session to load it.",
            },
            McpHostArg::Vscode => Self {
                name: McpHostName::Vscode,
                label: "VS Code",
                format: McpConfigFormat::Jsonc,
                reload: "VS Code lists it under MCP servers; start it from there.",
            },
        }
    }

    /// The host's own user-level configuration file, and whether setup may
    /// create its directory.
    pub(super) fn default_path(self, environment: &Environment) -> Result<(PathBuf, bool)> {
        match self.name {
            // What `claude mcp add --scope user` writes.
            McpHostName::ClaudeCode => Ok((
                match &environment.claude_config_dir {
                    Some(directory) => directory.join(".claude.json"),
                    None => environment.home()?.join(".claude.json"),
                },
                false,
            )),
            // What `codex mcp add` writes.
            McpHostName::Codex => Ok(match &environment.codex_home {
                Some(directory) => (directory.join("config.toml"), false),
                None => (environment.home()?.join(".codex").join("config.toml"), true),
            }),
            // What `code --add-mcp` writes: the default profile's `mcp.json`.
            McpHostName::Vscode => {
                let base = match &environment.xdg_config_home {
                    Some(directory) => directory.clone(),
                    None => environment.home()?.join(".config"),
                };
                let found: Vec<PathBuf> = VSCODE_DIRECTORIES
                    .iter()
                    .map(|name| base.join(name).join("User"))
                    .filter(|directory| directory.is_dir())
                    .collect();
                match found.as_slice() {
                    [directory] => Ok((directory.join("mcp.json"), false)),
                    [] => Err(failure(
                        FailureKind::NotFound,
                        format!(
                            "VS Code's user settings directory was not found ({} or {}). Start VS Code once, or pass --config PATH",
                            base.join("Code/User").display(),
                            base.join("Code - OSS/User").display()
                        ),
                    )),
                    _ => Err(failure(
                        FailureKind::Invalid,
                        format!(
                            "both {} and {} exist; pass --config PATH to choose the VS Code build to set up",
                            found[0].display(),
                            found[1].display()
                        ),
                    )),
                }
            }
        }
    }

    fn json_entry(self, adapter: &str) -> json_config::Ordered {
        use json_config::Ordered::{Array, Object, Str};
        let mut members = vec![
            ("type".to_owned(), Str("stdio".to_owned())),
            ("command".to_owned(), Str(adapter.to_owned())),
            ("args".to_owned(), Array(Vec::new())),
        ];
        // `claude mcp add` always writes an empty `env`; VS Code's form omits it.
        if self.name == McpHostName::ClaudeCode {
            members.push(("env".to_owned(), Object(Vec::new())));
        }
        Object(members)
    }

    fn json_target(self, entry: &json_config::Ordered) -> json_config::JsonTarget<'_> {
        json_config::JsonTarget {
            dialect: if self.format == McpConfigFormat::Jsonc {
                json_config::Dialect::Jsonc
            } else {
                json_config::Dialect::Json
            },
            container: if self.name == McpHostName::Vscode {
                "servers"
            } else {
                "mcpServers"
            },
            entry,
        }
    }
}

/// A public path: UTF-8, absolute, bounded, and free of control characters.
fn public_path(path: &Path, what: &str) -> Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| failure(FailureKind::Invalid, format!("the {what} is not UTF-8")))?;
    if !path.is_absolute() {
        return Err(failure(
            FailureKind::Invalid,
            format!("the {what} must be an absolute path"),
        ));
    }
    if text.len() > MAX_MCP_SETUP_PATH_BYTES || text.chars().any(char::is_control) {
        return Err(failure(
            FailureKind::Invalid,
            format!("the {what} is too long or contains control characters"),
        ));
    }
    Ok(text.to_owned())
}

/// The Nix profile entries ADR 0014 accepts for a remembered grant, in its
/// order, for the adapter's name.
fn nix_profile_entries(environment: &Environment) -> Vec<PathBuf> {
    let mut entries = vec![PathBuf::from("/run/current-system/sw/bin")];
    if let Some(user) = &environment.user {
        entries.push(Path::new("/etc/profiles/per-user").join(user).join("bin"));
    }
    entries.push(PathBuf::from("/nix/var/nix/profiles/default/bin"));
    if let Some(user) = &environment.user {
        entries.push(
            Path::new("/nix/var/nix/profiles/per-user")
                .join(user)
                .join("profile/bin"),
        );
    }
    if let Some(home) = &environment.home {
        entries.push(home.join(".nix-profile/bin"));
        entries.push(home.join(".local/state/nix/profile/bin"));
    }
    entries
        .into_iter()
        .map(|directory| directory.join(ADAPTER_NAME))
        .collect()
}

/// The adapter installed next to `executable`. When that is a Nix store path,
/// which changes with every upgrade, the first profile entry that resolves to
/// the same file is used instead.
pub(super) fn adjacent_adapter(
    executable: &Path,
    store: &Path,
    profile_entries: &[PathBuf],
) -> PathBuf {
    let adjacent = executable.parent().map_or_else(
        || PathBuf::from(ADAPTER_NAME),
        |directory| directory.join(ADAPTER_NAME),
    );
    if adjacent.starts_with(store)
        && let Ok(target) = fs::canonicalize(&adjacent)
        && let Some(entry) = profile_entries
            .iter()
            .find(|entry| fs::canonicalize(entry).is_ok_and(|resolved| resolved == target))
    {
        return entry.clone();
    }
    adjacent
}

/// Resolves and checks the adapter the entry will start.
pub(super) fn resolve_adapter(
    explicit: Option<&Path>,
    environment: &Environment,
) -> Result<String> {
    let path = if let Some(path) = explicit {
        path.to_path_buf()
    } else {
        let executable = env::current_exe().context("locate the running splinterm")?;
        adjacent_adapter(
            &executable,
            Path::new("/nix/store"),
            &nix_profile_entries(environment),
        )
    };
    let text = public_path(&path, "adapter path")?;
    let executable = fs::metadata(&path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
    if !executable {
        return Err(failure(
            FailureKind::NotFound,
            format!(
                "no {ADAPTER_NAME} adapter is installed at {text}. Install the optional splinterm-mcp package, or pass --adapter PATH"
            ),
        ));
    }
    Ok(text)
}

/// One resolved invocation.
#[derive(Clone, Debug)]
pub(super) struct Request {
    pub(super) operation: McpSetupOperation,
    pub(super) host: McpHostArg,
    pub(super) dry_run: bool,
    pub(super) replace: bool,
    pub(super) config: Option<PathBuf>,
    pub(super) adapter: Option<PathBuf>,
}

impl From<McpCommand> for Request {
    fn from(command: McpCommand) -> Self {
        match command {
            McpCommand::Setup {
                host,
                dry_run,
                replace,
                config,
                adapter,
            } => Self {
                operation: McpSetupOperation::Setup,
                host,
                dry_run,
                replace,
                config,
                adapter,
            },
            McpCommand::Remove {
                host,
                dry_run,
                config,
            } => Self {
                operation: McpSetupOperation::Remove,
                host,
                dry_run,
                replace: false,
                config,
                adapter: None,
            },
        }
    }
}

/// A completed (or, for a dry run, planned) operation.
#[derive(Debug)]
pub(super) struct Report {
    pub(super) outcome: McpSetupOutcome,
    pub(super) host: Host,
    pub(super) old_text: Option<String>,
    pub(super) new_text: Option<String>,
}

/// Plans the change and, unless this is a dry run, writes it.
pub(super) fn execute(request: &Request, environment: &Environment) -> Result<Report> {
    let host = Host::of(request.host);
    let (path, create_directory) = match &request.config {
        Some(path) => (path.clone(), false),
        None => host.default_path(environment)?,
    };
    let config_path = public_path(&path, "host configuration path")?;
    let setup = request.operation == McpSetupOperation::Setup;
    let adapter = if setup {
        Some(resolve_adapter(request.adapter.as_deref(), environment)?)
    } else {
        None
    };
    let existing = file::read(&path)?;
    let text = existing.as_ref().map(|existing| existing.text.as_str());
    let (plan, entry) = match (host.format, &adapter) {
        (McpConfigFormat::Toml, Some(adapter)) => {
            let entry = toml_config::TomlEntry {
                command: adapter.clone(),
                env_vars: CODEX_ENV_VARS,
            };
            let value = serde_json::json!({"command": adapter, "env_vars": CODEX_ENV_VARS});
            (
                toml_config::plan_setup(text, &entry, request.replace),
                Some(value),
            )
        }
        (McpConfigFormat::Toml, None) => (toml_config::plan_remove(text), None),
        (_, Some(adapter)) => {
            let entry = host.json_entry(adapter);
            let plan = json_config::plan_setup(text, &host.json_target(&entry), request.replace);
            (plan, Some(entry.to_value()))
        }
        (_, None) => {
            let entry = host.json_entry("");
            (
                json_config::plan_remove(text, &host.json_target(&entry)),
                None,
            )
        }
    };
    let plan = plan.map_err(|error| at_path(error, &path))?;
    if let Some(new_text) = &plan.new_text
        && !request.dry_run
    {
        file::write(&path, new_text, existing.as_ref(), create_directory)?;
    }
    Ok(Report {
        outcome: McpSetupOutcome {
            operation: request.operation,
            host: host.name,
            config_path,
            config_format: host.format,
            server_name: SERVER_NAME,
            action: plan.action,
            dry_run: request.dry_run,
            adapter_path: adapter,
            entry,
        },
        host,
        old_text: existing.map(|existing| existing.text),
        new_text: plan.new_text,
    })
}

/// The changed lines of `old` and `new` as one unified hunk, without context
/// lines, so unchanged neighbouring entries (and any secrets in them) are not
/// printed. A neighbour that gains a separating comma shows that one line.
pub(super) fn diff(old: &str, new: &str, path: &str) -> String {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let removed = &old_lines[prefix..old_lines.len() - suffix];
    let added = &new_lines[prefix..new_lines.len() - suffix];
    let start = |count: usize| if count == 0 { prefix } else { prefix + 1 };
    let mut out = format!(
        "--- {path}\n+++ {path}\n@@ -{},{} +{},{} @@\n",
        start(removed.len()),
        removed.len(),
        start(added.len()),
        added.len()
    );
    for (sign, lines) in [('-', removed), ('+', added)] {
        for line in lines {
            out.push(sign);
            out.push_str(line.trim_end_matches('\n'));
            out.push('\n');
        }
    }
    out
}

fn push_line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

/// The human report.
pub(super) fn human(report: &Report) -> String {
    let outcome = &report.outcome;
    let label = report.host.label;
    let path = &outcome.config_path;
    let mut out = String::new();
    if outcome.dry_run && outcome.action.changes_file() {
        let change = match outcome.action {
            Action::Created => format!("create {path} with the `{SERVER_NAME}` MCP server"),
            Action::Replaced => format!("replace the `{SERVER_NAME}` MCP server in {path}"),
            Action::Removed => format!("remove the `{SERVER_NAME}` MCP server from {path}"),
            _ => format!("add the `{SERVER_NAME}` MCP server to {path}"),
        };
        push_line(
            &mut out,
            &format!("Dry run: this would {change} for {label}:"),
        );
        out.push_str(&diff(
            report.old_text.as_deref().unwrap_or(""),
            report.new_text.as_deref().unwrap_or(""),
            path,
        ));
        push_line(&mut out, "Nothing was written.");
    } else {
        let message = match outcome.action {
            Action::Created => {
                format!("Created {path} with the `{SERVER_NAME}` MCP server for {label}.")
            }
            Action::Added => format!(
                "Added the `{SERVER_NAME}` MCP server to {label}'s configuration in {path}."
            ),
            Action::Replaced => format!(
                "Replaced the `{SERVER_NAME}` MCP server in {label}'s configuration in {path}."
            ),
            Action::Unchanged => format!(
                "{label}'s configuration in {path} already has this `{SERVER_NAME}` MCP server; nothing changed."
            ),
            Action::Removed => format!(
                "Removed the `{SERVER_NAME}` MCP server from {label}'s configuration in {path}."
            ),
            Action::Absent => format!(
                "{label}'s configuration in {path} has no `{SERVER_NAME}` MCP server; nothing changed."
            ),
        };
        push_line(&mut out, &message);
        if let Some(adapter) = &outcome.adapter_path {
            push_line(&mut out, &format!("  command: {adapter}"));
        }
    }
    if outcome.operation == McpSetupOperation::Setup {
        push_line(&mut out, GRANTS_NOTHING);
        if outcome.action.changes_file() && !outcome.dry_run {
            push_line(&mut out, report.host.reload);
        }
    } else if outcome.action == Action::Removed {
        push_line(&mut out, REVOKES_NOTHING);
    }
    out
}

fn public_failure(error: &anyhow::Error) -> (CliErrorCodeV2, bool, i32) {
    match failure_kind(error) {
        Some(FailureKind::Conflict) => (CliErrorCodeV2::ConfirmationRequired, false, 3),
        Some(FailureKind::Invalid) => (CliErrorCodeV2::InvalidArgument, false, 5),
        Some(FailureKind::NotFound) => (CliErrorCodeV2::NotFound, false, 5),
        Some(FailureKind::Changed) => (CliErrorCodeV2::Internal, true, 70),
        None => (CliErrorCodeV2::Internal, false, 70),
    }
}

fn bounded(message: &str) -> String {
    let message: String = message.chars().take(1024).collect();
    if message.is_empty() {
        "MCP setup failed".to_owned()
    } else {
        message
    }
}

/// Runs `splinterm mcp` in human or `--output json` mode.
pub(super) fn run(
    command: McpCommand,
    output: Option<OutputMode>,
    schema_major: Option<u16>,
    timeout_ms: Option<u64>,
    remote: bool,
) -> Result<()> {
    if remote {
        usage_error(
            "--remote cannot select MCP setup; it edits this computer's host configuration",
        );
    }
    if timeout_ms.is_some() {
        usage_error("--timeout-ms does not apply to MCP setup, which never contacts the daemon");
    }
    let request = Request::from(command);
    match output {
        Some(OutputMode::Ndjson) => {
            usage_error("NDJSON output is reserved for subscription commands")
        }
        Some(OutputMode::Json) => {
            let operation = request.operation.name();
            let schema_major = schema_major.unwrap_or(2);
            if schema_major != 2 {
                write_json_document(&CliEnvelopeV2::failure(
                    operation,
                    CliErrorCodeV2::UnsupportedSchema,
                    format!("unsupported schema major {schema_major}"),
                    false,
                )?)?;
                std::process::exit(4);
            }
            let result = Environment::from_process()
                .and_then(|environment| execute(&request, &environment))
                .and_then(|report| mcp_setup_envelope(&report.outcome));
            match result {
                Ok(envelope) => write_json_document(&envelope),
                Err(error) => {
                    let (code, retryable, exit) = public_failure(&error);
                    write_json_document(&CliEnvelopeV2::failure(
                        operation,
                        code,
                        bounded(&format!("{error:#}")),
                        retryable,
                    )?)?;
                    std::process::exit(exit);
                }
            }
        }
        Some(OutputMode::Human) | None => {
            if schema_major.is_some() {
                usage_error("--schema-major and --timeout-ms require --output json or ndjson");
            }
            let report = execute(&request, &Environment::from_process()?)?;
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(human(&report).as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
    }
}
