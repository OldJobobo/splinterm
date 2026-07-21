//! Bounded, fail-closed persistent automation policy loading.

use std::{
    collections::{HashMap, HashSet},
    fs::{File, Metadata},
    io::Read,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rustix::fs::{Mode, OFlags};
use serde::{Deserialize, Serialize};
use splinterm_core::{DojoId, SplintId, WindowId};

use crate::{authorization::OperationScope, executable_identity::ExecutableIdentity};

const MAX_POLICY_BYTES: u64 = 256 * 1024;
const MAX_RULES: usize = 64;
const MAX_RESOURCES: usize = 64;
const MAX_RULE_ID_BYTES: usize = 64;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DIAGNOSTIC_BYTES: usize = 512;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDocument {
    schema: String,
    rules: Vec<PolicyRule>,
}

impl PolicyDocument {
    fn deny_all() -> Self {
        Self {
            schema: "splinterm.policy.v1".into(),
            rules: Vec::new(),
        }
    }

    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    #[must_use]
    pub fn authorize(
        &self,
        executable: &ExecutableIdentity,
        request: &PolicyRequest<'_>,
        now_unix_seconds: u64,
    ) -> Option<PolicyMatch> {
        self.rules.iter().find_map(|rule| {
            rule.matches(executable, request, now_unix_seconds)
                .then(|| PolicyMatch {
                    rule_id: rule.id.clone(),
                    max_spawn_count: rule.limits.max_spawn_count,
                    max_returned_bytes: rule.limits.max_returned_bytes,
                })
        })
    }

    fn status(
        &self,
        executable: &ExecutableIdentity,
        resource: PolicyResource,
        now_unix_seconds: u64,
    ) -> Vec<splinterm_protocol::PersistentAuthorizationStatus> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.executable.path == executable.path
                    && rule.executable.sha256 == executable.sha256
                    && rule
                        .expires_at_unix_seconds
                        .is_none_or(|expiration| expiration > now_unix_seconds)
                    && rule
                        .resources
                        .iter()
                        .any(|selector| selector.matches(resource))
            })
            .map(|rule| splinterm_protocol::PersistentAuthorizationStatus {
                policy_rule_id: rule.id.clone(),
                scopes: rule.scopes.clone(),
                expires_at_unix_seconds: rule.expires_at_unix_seconds,
            })
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    pub id: String,
    pub executable: ExecutableMatcher,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_seconds: Option<u64>,
    pub scopes: Vec<OperationScope>,
    pub resources: Vec<ResourceSelector>,
    pub limits: PolicyLimits,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutableMatcher {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceSelector {
    Lair,
    Dojo {
        dojo_id: DojoId,
    },
    Window {
        window_id: WindowId,
    },
    Splint {
        splint_id: SplintId,
        incarnation: IncarnationSelector,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum IncarnationSelector {
    Exact(u64),
    Current(String),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_returned_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_returned_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_live_subscriptions: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_spawn_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RequestedLimits {
    pub returned_rows: Option<usize>,
    pub results: Option<usize>,
    pub returned_bytes: Option<usize>,
    pub live_subscriptions: Option<usize>,
    pub spawn_count: Option<usize>,
    pub deadline_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyResource {
    Lair,
    Dojo {
        dojo_id: DojoId,
    },
    Window {
        dojo_id: DojoId,
        window_id: WindowId,
    },
    Splint {
        dojo_id: DojoId,
        window_id: WindowId,
        splint_id: SplintId,
        incarnation: Option<u64>,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct PolicyRequest<'a> {
    pub required_scopes: &'a [OperationScope],
    pub any_scope: &'a [OperationScope],
    pub resources: &'a [PolicyResource],
    pub limits: RequestedLimits,
}

#[derive(Clone, Debug)]
pub struct PolicyMatch {
    pub rule_id: String,
    max_spawn_count: Option<usize>,
    pub max_returned_bytes: Option<usize>,
}

impl PolicyRule {
    fn matches(
        &self,
        executable: &ExecutableIdentity,
        request: &PolicyRequest<'_>,
        now_unix_seconds: u64,
    ) -> bool {
        self.executable.path == executable.path
            && self.executable.sha256 == executable.sha256
            && self
                .expires_at_unix_seconds
                .is_none_or(|expiration| expiration > now_unix_seconds)
            && !request.resources.is_empty()
            && request
                .required_scopes
                .iter()
                .all(|scope| self.scopes.contains(scope))
            && (request.any_scope.is_empty()
                || request
                    .any_scope
                    .iter()
                    .any(|scope| self.scopes.contains(scope)))
            && request.resources.iter().all(|resource| {
                self.resources
                    .iter()
                    .any(|selector| selector.matches(*resource))
            })
            && self.limits.allows(request.limits)
    }
}

impl ResourceSelector {
    fn matches(&self, resource: PolicyResource) -> bool {
        match (self, resource) {
            (Self::Lair, PolicyResource::Lair) => true,
            (Self::Dojo { dojo_id }, PolicyResource::Dojo { dojo_id: requested }) => {
                *dojo_id == requested
            }
            (
                Self::Dojo { dojo_id },
                PolicyResource::Window {
                    dojo_id: requested, ..
                }
                | PolicyResource::Splint {
                    dojo_id: requested, ..
                },
            ) => *dojo_id == requested,
            (
                Self::Window { window_id },
                PolicyResource::Window {
                    window_id: requested,
                    ..
                }
                | PolicyResource::Splint {
                    window_id: requested,
                    ..
                },
            ) => *window_id == requested,
            (
                Self::Splint {
                    splint_id,
                    incarnation,
                },
                PolicyResource::Splint {
                    splint_id: requested_id,
                    incarnation: requested_incarnation,
                    ..
                },
            ) => {
                *splint_id == requested_id
                    && match incarnation {
                        IncarnationSelector::Exact(expected) => {
                            Some(*expected) == requested_incarnation
                        }
                        IncarnationSelector::Current(value) => value == "current",
                    }
            }
            _ => false,
        }
    }
}

impl PolicyLimits {
    fn populated_fields(&self) -> usize {
        [
            self.max_returned_rows.is_some(),
            self.max_results.is_some(),
            self.max_returned_bytes.is_some(),
            self.max_live_subscriptions.is_some(),
            self.max_spawn_count.is_some(),
            self.deadline_ms.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
    }

    fn allows(&self, requested: RequestedLimits) -> bool {
        limit_allows(self.max_returned_rows, requested.returned_rows)
            && limit_allows(self.max_results, requested.results)
            && limit_allows(self.max_returned_bytes, requested.returned_bytes)
            && limit_allows(self.max_live_subscriptions, requested.live_subscriptions)
            && limit_allows(self.max_spawn_count, requested.spawn_count)
            && limit_allows(self.deadline_ms, requested.deadline_ms)
    }

    fn validate(&self) -> Result<()> {
        if self.populated_fields() == 0 {
            bail!("policy limits must contain at least one bound");
        }
        bounded(self.max_returned_rows, 80, "max_returned_rows")?;
        bounded(self.max_results, 64, "max_results")?;
        bounded(
            self.max_returned_bytes,
            8 * 1024 * 1024,
            "max_returned_bytes",
        )?;
        bounded(self.max_live_subscriptions, 4, "max_live_subscriptions")?;
        bounded(self.max_spawn_count, 64, "max_spawn_count")?;
        if self
            .deadline_ms
            .is_some_and(|value| value == 0 || value > 300_000)
        {
            bail!("deadline_ms is outside policy bounds");
        }
        Ok(())
    }
}

fn limit_allows<T: PartialOrd>(allowed: Option<T>, requested: Option<T>) -> bool {
    requested.is_none_or(|requested| allowed.is_some_and(|allowed| requested <= allowed))
}

fn bounded(value: Option<usize>, maximum: usize, name: &str) -> Result<()> {
    if value.is_some_and(|value| value == 0 || value > maximum) {
        bail!("{name} is outside policy bounds");
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct PolicyGeneration {
    pub id: u64,
    pub document: Arc<PolicyDocument>,
    pub diagnostic: Option<String>,
}

#[derive(Debug)]
pub struct PolicyStore {
    generation: PolicyGeneration,
    spawn_counts: HashMap<String, usize>,
}

impl Default for PolicyStore {
    fn default() -> Self {
        Self {
            generation: PolicyGeneration {
                id: 1,
                document: Arc::new(PolicyDocument::deny_all()),
                diagnostic: None,
            },
            spawn_counts: HashMap::new(),
        }
    }
}

impl PolicyStore {
    #[must_use]
    pub fn snapshot(&self) -> PolicyGeneration {
        self.generation.clone()
    }

    pub fn authorize(
        &mut self,
        executable: &ExecutableIdentity,
        request: &PolicyRequest<'_>,
        now_unix_seconds: u64,
    ) -> Option<PolicyMatch> {
        let matched = self
            .generation
            .document
            .authorize(executable, request, now_unix_seconds)?;
        if let Some(requested) = request.limits.spawn_count {
            let allowed = matched.max_spawn_count?;
            let used = self
                .spawn_counts
                .entry(matched.rule_id.clone())
                .or_default();
            let next = used.checked_add(requested)?;
            if next > allowed {
                return None;
            }
            *used = next;
        }
        Some(matched)
    }

    #[must_use]
    pub fn status(
        &self,
        executable: &ExecutableIdentity,
        resource: PolicyResource,
        now_unix_seconds: u64,
    ) -> Vec<splinterm_protocol::PersistentAuthorizationStatus> {
        self.generation
            .document
            .status(executable, resource, now_unix_seconds)
    }

    pub fn reload(&mut self, path: Option<&Path>) -> PolicyGeneration {
        self.publish(prepare(path.map(Path::to_path_buf)))
    }

    pub fn publish(&mut self, candidate: PolicyCandidate) -> PolicyGeneration {
        let next_id = self.generation.id.saturating_add(1).max(1);
        self.generation = PolicyGeneration {
            id: next_id,
            document: Arc::new(candidate.document),
            diagnostic: candidate.diagnostic,
        };
        self.spawn_counts.clear();
        self.snapshot()
    }
}

pub struct PolicyCandidate {
    document: PolicyDocument,
    diagnostic: Option<String>,
}

#[must_use]
pub fn prepare(path: Option<PathBuf>) -> PolicyCandidate {
    let (document, diagnostic) = match path {
        None => (PolicyDocument::deny_all(), None),
        Some(path) => match inspect_file(&path) {
            Ok(document) => (document, None),
            Err(error) => (
                PolicyDocument::deny_all(),
                Some(bounded_diagnostic(&error.to_string())),
            ),
        },
    };
    PolicyCandidate {
        document,
        diagnostic,
    }
}

pub fn configured_path() -> Option<PathBuf> {
    std::env::var_os("SPLINTERM_POLICY").map(PathBuf::from)
}

/// Loads and semantically validates one policy without publishing it.
///
/// This uses the same bounded, owner-checked, no-symlink path as daemon startup
/// and reload, making it suitable for local administrative validation.
/// # Errors
///
/// Returns an error when the path, file metadata, bounded JSON, or semantic
/// policy constraints fail validation.
pub fn inspect_file(path: &Path) -> Result<PolicyDocument> {
    validate_policy_path(path)?;
    let file = open_without_symlinks(path)?;
    let metadata = file.metadata().context("cannot stat opened policy")?;
    validate_policy_metadata(&metadata)?;
    let bytes = read_bounded(file, metadata.len())?;
    let document: PolicyDocument =
        serde_json::from_slice(&bytes).context("policy is not valid bounded JSON")?;
    validate_document(&document)?;
    Ok(document)
}

fn validate_policy_path(path: &Path) -> Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_PATH_BYTES || !path.is_absolute() {
        bail!("policy path must be a bounded absolute path");
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        bail!("policy path must be canonical");
    }
    Ok(())
}

fn open_without_symlinks(path: &Path) -> Result<File> {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(component) => Some(component),
            _ => None,
        })
        .collect::<Vec<_>>();
    let (file_name, directories) = components
        .split_last()
        .context("policy path does not name a file")?;
    let mut directory = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .context("cannot open policy root")?;
    for component in directories {
        directory = rustix::fs::openat(
            &directory,
            *component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .with_context(|| format!("cannot open policy directory component {component:?}"))?;
    }
    let descriptor = rustix::fs::openat(
        &directory,
        *file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .context("cannot open policy file")?;
    Ok(File::from(descriptor))
}

fn validate_policy_metadata(metadata: &Metadata) -> Result<()> {
    if !metadata.is_file() || metadata.nlink() != 1 {
        bail!("policy must be a regular file with one hard link");
    }
    if metadata.len() == 0 || metadata.len() > MAX_POLICY_BYTES {
        bail!("policy size is outside 1..={MAX_POLICY_BYTES} bytes");
    }
    let owner = metadata.uid();
    let daemon_uid = rustix::process::geteuid().as_raw();
    let mode = metadata.mode() & 0o777;
    if owner == daemon_uid {
        if mode != 0o600 {
            bail!("daemon-owned policy mode must be 0600");
        }
    } else if owner != 0 || mode & 0o022 != 0 {
        bail!("policy owner or write permissions are unsafe");
    }
    Ok(())
}

fn read_bounded(mut file: File, expected_size: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(usize::try_from(expected_size).unwrap_or(0));
    file.by_ref()
        .take(MAX_POLICY_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("cannot read policy")?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_size {
        bail!("policy changed while reading");
    }
    Ok(bytes)
}

fn validate_document(document: &PolicyDocument) -> Result<()> {
    if document.schema != "splinterm.policy.v1" {
        bail!("unsupported policy schema");
    }
    if document.rules.len() > MAX_RULES {
        bail!("policy contains too many rules");
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut ids = HashSet::with_capacity(document.rules.len());
    for rule in &document.rules {
        validate_rule(rule, now)?;
        if !ids.insert(&rule.id) {
            bail!("policy rule IDs must be unique");
        }
    }
    Ok(())
}

fn validate_rule(rule: &PolicyRule, now: u64) -> Result<()> {
    let id = rule.id.as_bytes();
    if id.is_empty()
        || id.len() > MAX_RULE_ID_BYTES
        || !id[0].is_ascii_lowercase()
        || !id
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        bail!("policy rule ID is invalid");
    }
    validate_policy_path(&rule.executable.path).context("invalid executable policy path")?;
    if rule.executable.sha256.len() != 64
        || !rule
            .executable
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("policy executable SHA-256 is invalid");
    }
    if rule
        .expires_at_unix_seconds
        .is_some_and(|expiration| expiration <= now)
    {
        bail!("policy contains an expired rule");
    }
    if rule.scopes.is_empty() || rule.scopes.len() > 18 {
        bail!("policy rule scopes are empty or exceed limits");
    }
    if rule.scopes.iter().collect::<HashSet<_>>().len() != rule.scopes.len() {
        bail!("policy rule scopes must be unique");
    }
    if rule.resources.is_empty() || rule.resources.len() > MAX_RESOURCES {
        bail!("policy rule resources are empty or exceed limits");
    }
    if rule.resources.iter().collect::<HashSet<_>>().len() != rule.resources.len() {
        bail!("policy rule resources must be unique");
    }
    for resource in &rule.resources {
        if let ResourceSelector::Splint { incarnation, .. } = resource {
            match incarnation {
                IncarnationSelector::Exact(value) if *value == 0 => {
                    bail!("policy Splint incarnation must be nonzero");
                }
                IncarnationSelector::Current(value) if value != "current" => {
                    bail!("policy current incarnation selector is invalid");
                }
                _ => {}
            }
        }
    }
    rule.limits.validate()
}

fn bounded_diagnostic(message: &str) -> String {
    message.chars().take(MAX_DIAGNOSTIC_BYTES).collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::*;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "splinterm-policy-{}-{name}.json",
            std::process::id()
        ))
    }

    fn valid_policy() -> String {
        format!(
            r#"{{
              "schema":"splinterm.policy.v1",
              "rules":[{{
                "id":"reader",
                "executable":{{"path":"/usr/bin/client","sha256":"{}"}},
                "scopes":["terminal_visible_read"],
                "resources":[{{"kind":"splint","splint_id":"{}","incarnation":"current"}}],
                "limits":{{"max_returned_rows":40}}
              }}]
            }}"#,
            "a".repeat(64),
            SplintId::new()
        )
    }

    fn write_policy(path: &Path, contents: &str, mode: u32) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn loads_owner_only_policy_through_no_symlink_path_walk() {
        let path = test_path("valid");
        write_policy(&path, &valid_policy(), 0o600);

        let document = inspect_file(&path).unwrap();

        assert_eq!(document.rules.len(), 1);
        assert_eq!(document.rules[0].id, "reader");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reload_failure_atomically_installs_deny_all_generation() {
        let path = test_path("reload");
        write_policy(&path, &valid_policy(), 0o600);
        let mut store = PolicyStore::default();
        let loaded = store.reload(Some(&path));
        assert_eq!(loaded.document.rules.len(), 1);
        assert!(loaded.diagnostic.is_none());

        write_policy(&path, "{\"schema\":\"wrong\",\"rules\":[]}", 0o600);
        let denied = store.reload(Some(&path));

        assert!(denied.id > loaded.id);
        assert!(denied.document.rules.is_empty());
        assert!(denied.diagnostic.is_some());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn exact_identity_scope_resource_and_limit_match_is_narrow() {
        let path = test_path("match");
        write_policy(&path, &valid_policy(), 0o600);
        let document = inspect_file(&path).unwrap();
        let rule = &document.rules[0];
        let ResourceSelector::Splint { splint_id, .. } = rule.resources[0] else {
            panic!("expected Splint selector");
        };
        let executable = ExecutableIdentity {
            path: PathBuf::from("/usr/bin/client"),
            device: 1,
            inode: 2,
            owner_uid: 0,
            mode: 0o755,
            size: 1,
            sha256: "a".repeat(64),
        };
        let resource = PolicyResource::Splint {
            dojo_id: DojoId::new(),
            window_id: WindowId::new(),
            splint_id,
            incarnation: Some(9),
        };
        let request = PolicyRequest {
            required_scopes: &[OperationScope::TerminalVisibleRead],
            any_scope: &[],
            resources: &[resource],
            limits: RequestedLimits {
                returned_rows: Some(40),
                ..RequestedLimits::default()
            },
        };

        let matched = document.authorize(&executable, &request, u64::MAX / 2);
        assert_eq!(matched.unwrap().rule_id, "reader");
        let status = document.status(&executable, resource, u64::MAX / 2);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].policy_rule_id, "reader");
        assert_eq!(status[0].scopes, vec![OperationScope::TerminalVisibleRead]);
        assert_eq!(status[0].expires_at_unix_seconds, None);

        let excessive = PolicyRequest {
            limits: RequestedLimits {
                returned_rows: Some(41),
                ..RequestedLimits::default()
            },
            ..request
        };
        assert!(
            document
                .authorize(&executable, &excessive, u64::MAX / 2)
                .is_none()
        );
        let wrong_scope = PolicyRequest {
            required_scopes: &[OperationScope::Input],
            ..request
        };
        assert!(
            document
                .authorize(&executable, &wrong_scope, u64::MAX / 2)
                .is_none()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn spawn_limit_is_consumed_atomically_within_generation() {
        let path = test_path("spawn-limit");
        let policy = format!(
            r#"{{"schema":"splinterm.policy.v1","rules":[{{
                "id":"spawner",
                "executable":{{"path":"/usr/bin/client","sha256":"{}"}},
                "scopes":["process_spawn","topology_layout_mutate"],
                "resources":[{{"kind":"lair"}}],
                "limits":{{"max_spawn_count":1}}
            }}]}}"#,
            "a".repeat(64)
        );
        write_policy(&path, &policy, 0o600);
        let executable = ExecutableIdentity {
            path: PathBuf::from("/usr/bin/client"),
            device: 1,
            inode: 2,
            owner_uid: 0,
            mode: 0o755,
            size: 1,
            sha256: "a".repeat(64),
        };
        let request = PolicyRequest {
            required_scopes: &[
                OperationScope::ProcessSpawn,
                OperationScope::TopologyLayoutMutate,
            ],
            any_scope: &[],
            resources: &[PolicyResource::Lair],
            limits: RequestedLimits {
                spawn_count: Some(1),
                ..RequestedLimits::default()
            },
        };
        let mut store = PolicyStore::default();
        store.reload(Some(&path));

        assert!(store.authorize(&executable, &request, 1).is_some());
        assert!(store.authorize(&executable, &request, 1).is_none());
        store.reload(Some(&path));
        assert!(store.authorize(&executable, &request, 1).is_some());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_unsafe_mode_symlink_duplicate_and_expired_rules() {
        let path = test_path("unsafe");
        write_policy(&path, &valid_policy(), 0o644);
        assert!(inspect_file(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let link = test_path("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(inspect_file(&link).is_err());
        fs::remove_file(link).unwrap();

        let duplicate = valid_policy().replace(
            "]\n            }",
            ",\n              {\"id\":\"reader\",\"executable\":{\"path\":\"/usr/bin/client\",\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"},\"scopes\":[\"input\"],\"resources\":[{\"kind\":\"lair\"}],\"limits\":{\"deadline_ms\":1}}]\n            }",
        );
        write_policy(&path, &duplicate, 0o600);
        assert!(inspect_file(&path).is_err());

        let expired =
            valid_policy().replace("\"scopes\"", "\"expires_at_unix_seconds\":1,\"scopes\"");
        write_policy(&path, &expired, 0o600);
        assert!(inspect_file(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
