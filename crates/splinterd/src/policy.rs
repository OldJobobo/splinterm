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
use splinterm_core::{DojoId, Lair, LayoutNode, SplintId, WindowId};

use crate::{authorization::OperationScope, executable_identity::ExecutableIdentity};

const MAX_POLICY_BYTES: u64 = 256 * 1024;
const MAX_RULES: usize = 64;
const MAX_RESOURCES: usize = 64;
const MAX_RULE_ID_BYTES: usize = 64;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DIAGNOSTIC_BYTES: usize = 512;
const MAX_RESOLVED_RESOURCES_PER_RULE: usize = 512;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDocument {
    schema: String,
    rules: Vec<PolicyRule>,
    #[serde(skip)]
    resolved_resources: HashMap<String, Vec<ResolvedResourceSelector>>,
}

impl PolicyDocument {
    fn deny_all() -> Self {
        Self {
            schema: "splinterm.policy.v1".into(),
            rules: Vec::new(),
            resolved_resources: HashMap::new(),
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
            let resources = self.resolved_resources.get(&rule.id)?;
            rule.matches(executable, request, resources, now_unix_seconds)
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
                    && self
                        .resolved_resources
                        .get(&rule.id)
                        .is_some_and(|resources| {
                            resources.iter().any(|selector| selector.matches(resource))
                        })
            })
            .map(|rule| splinterm_protocol::PersistentAuthorizationStatus {
                policy_rule_id: rule.id.clone(),
                scopes: rule.scopes.clone(),
                expires_at_unix_seconds: rule.expires_at_unix_seconds,
            })
            .collect()
    }

    fn resolve_against(&mut self, lair: &Lair) -> Result<()> {
        let mut resolved = HashMap::with_capacity(self.rules.len());
        for rule in &self.rules {
            let mut resources = HashSet::new();
            for selector in &rule.resources {
                match selector {
                    ResourceSelector::Lair => {
                        insert_resolved(&mut resources, ResolvedResourceSelector::Lair)?;
                    }
                    ResourceSelector::Dojo { dojo_id } => {
                        let dojo = lair.dojos().find(|dojo| dojo.id == *dojo_id).context(
                            "policy Dojo selector is not present in the topology snapshot",
                        )?;
                        insert_resolved(&mut resources, ResolvedResourceSelector::Dojo(dojo.id))?;
                        for window in &dojo.windows {
                            insert_resolved(
                                &mut resources,
                                ResolvedResourceSelector::Window(window.id),
                            )?;
                            resolve_layout(&window.root, &mut resources)?;
                        }
                    }
                    ResourceSelector::Window { window_id } => {
                        let window = lair.find_window(*window_id).context(
                            "policy Window selector is not present in the topology snapshot",
                        )?;
                        insert_resolved(
                            &mut resources,
                            ResolvedResourceSelector::Window(window.id),
                        )?;
                        resolve_layout(&window.root, &mut resources)?;
                    }
                    ResourceSelector::Splint {
                        splint_id,
                        incarnation,
                    } => {
                        lair.find_splint(*splint_id).context(
                            "policy Splint selector is not present in the topology snapshot",
                        )?;
                        insert_resolved(
                            &mut resources,
                            ResolvedResourceSelector::Splint {
                                splint_id: *splint_id,
                                incarnation: incarnation.clone(),
                            },
                        )?;
                    }
                }
            }
            resolved.insert(rule.id.clone(), resources.into_iter().collect());
        }
        self.resolved_resources = resolved;
        Ok(())
    }
}

fn resolve_layout(
    node: &LayoutNode,
    resources: &mut HashSet<ResolvedResourceSelector>,
) -> Result<()> {
    match node {
        LayoutNode::Leaf(splint) => insert_resolved(
            resources,
            ResolvedResourceSelector::Splint {
                splint_id: splint.id,
                incarnation: IncarnationSelector::Current("current".into()),
            },
        ),
        LayoutNode::Branch { first, second, .. } => {
            resolve_layout(first, resources)?;
            resolve_layout(second, resources)
        }
    }
}

fn insert_resolved(
    resources: &mut HashSet<ResolvedResourceSelector>,
    resource: ResolvedResourceSelector,
) -> Result<()> {
    resources.insert(resource);
    if resources.len() > MAX_RESOLVED_RESOURCES_PER_RULE {
        bail!("policy rule expands beyond {MAX_RESOLVED_RESOURCES_PER_RULE} snapshot resources");
    }
    Ok(())
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ResolvedResourceSelector {
    Lair,
    Dojo(DojoId),
    Window(WindowId),
    Splint {
        splint_id: SplintId,
        incarnation: IncarnationSelector,
    },
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
        resolved_resources: &[ResolvedResourceSelector],
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
                resolved_resources
                    .iter()
                    .any(|selector| selector.matches(*resource))
            })
            && self.limits.allows(request.limits)
    }
}

impl ResolvedResourceSelector {
    fn matches(&self, resource: PolicyResource) -> bool {
        match (self, resource) {
            (Self::Lair, PolicyResource::Lair) => true,
            (Self::Dojo(expected), PolicyResource::Dojo { dojo_id }) => *expected == dojo_id,
            (Self::Window(expected), PolicyResource::Window { window_id, .. }) => {
                *expected == window_id
            }
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

    pub fn reload(&mut self, path: Option<&Path>, lair: &Lair) -> PolicyGeneration {
        self.publish(prepare(path.map(Path::to_path_buf)), lair)
    }

    pub fn publish(&mut self, mut candidate: PolicyCandidate, lair: &Lair) -> PolicyGeneration {
        if candidate.diagnostic.is_none() {
            if let Err(error) = candidate.document.resolve_against(lair) {
                candidate.document = PolicyDocument::deny_all();
                candidate.diagnostic = Some(bounded_diagnostic(&error.to_string()));
            }
        }
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

#[allow(
    clippy::unnecessary_debug_formatting,
    reason = "Debug formatting escapes untrusted policy path components"
)]
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

    fn lair_with_splint(splint_id: SplintId) -> (Lair, DojoId, WindowId) {
        let mut dojo = splinterm_core::Dojo::new("policy", PathBuf::from("/tmp"));
        let window_id = dojo.windows[0].id;
        let LayoutNode::Leaf(splint) = &mut dojo.windows[0].root else {
            unreachable!("new Dojo starts with one leaf");
        };
        splint.id = splint_id;
        dojo.windows[0].default_focus = splint_id;
        let dojo_id = dojo.id;
        let mut lair = Lair::new();
        lair.insert_dojo_at(lair.revision(), dojo).unwrap();
        (lair, dojo_id, window_id)
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
        let document = inspect_file(&path).unwrap();
        let ResourceSelector::Splint { splint_id, .. } = document.rules[0].resources[0] else {
            panic!("expected Splint selector");
        };
        let (lair, _, _) = lair_with_splint(splint_id);
        let mut store = PolicyStore::default();
        let loaded = store.reload(Some(&path), &lair);
        assert_eq!(loaded.document.rules.len(), 1);
        assert!(loaded.diagnostic.is_none());

        write_policy(&path, "{\"schema\":\"wrong\",\"rules\":[]}", 0o600);
        let denied = store.reload(Some(&path), &lair);

        assert!(denied.id > loaded.id);
        assert!(denied.document.rules.is_empty());
        assert!(denied.diagnostic.is_some());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn exact_identity_scope_resource_and_limit_match_is_narrow() {
        let path = test_path("match");
        write_policy(&path, &valid_policy(), 0o600);
        let mut document = inspect_file(&path).unwrap();
        let rule = &document.rules[0];
        let ResourceSelector::Splint { splint_id, .. } = rule.resources[0] else {
            panic!("expected Splint selector");
        };
        let (lair, dojo_id, window_id) = lair_with_splint(splint_id);
        document.resolve_against(&lair).unwrap();
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
            dojo_id,
            window_id,
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
        let lair = Lair::new();
        store.reload(Some(&path), &lair);

        assert!(store.authorize(&executable, &request, 1).is_some());
        assert!(store.authorize(&executable, &request, 1).is_none());
        store.reload(Some(&path), &lair);
        assert!(store.authorize(&executable, &request, 1).is_some());
        fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one publication snapshot scenario covers later Splints, windows, reload, and deny-all"
    )]
    fn publication_snapshot_excludes_later_descendants_and_reloads_explicitly() {
        let original_id = SplintId::new();
        let (mut lair, dojo_id, window_id) = lair_with_splint(original_id);
        let mut document: PolicyDocument = serde_json::from_value(serde_json::json!({
            "schema": "splinterm.policy.v1",
            "rules": [{
                "id": "dojo-reader",
                "executable": {
                    "path": "/usr/bin/client",
                    "sha256": "a".repeat(64),
                },
                "scopes": ["topology_metadata_read", "terminal_visible_read"],
                "resources": [{"kind": "dojo", "dojo_id": dojo_id}],
                "limits": {"max_returned_rows": 40},
            }],
        }))
        .unwrap();
        validate_document(&document).unwrap();
        document.resolve_against(&lair).unwrap();

        let executable = ExecutableIdentity {
            path: PathBuf::from("/usr/bin/client"),
            device: 1,
            inode: 2,
            owner_uid: 0,
            mode: 0o755,
            size: 1,
            sha256: "a".repeat(64),
        };
        let original_resource = PolicyResource::Splint {
            dojo_id,
            window_id,
            splint_id: original_id,
            incarnation: Some(1),
        };
        let original_request = PolicyRequest {
            required_scopes: &[OperationScope::TerminalVisibleRead],
            any_scope: &[],
            resources: &[original_resource],
            limits: RequestedLimits {
                returned_rows: Some(1),
                ..RequestedLimits::default()
            },
        };
        assert!(
            document
                .authorize(&executable, &original_request, 1)
                .is_some()
        );

        let added = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let added_id = added.id;
        lair.split_splint_at(
            lair.revision(),
            original_id,
            added,
            splinterm_core::Axis::Horizontal,
            splinterm_core::SplitSide::Second,
            splinterm_core::SplitRatio::new(500).unwrap(),
        )
        .unwrap();
        let added_resource = PolicyResource::Splint {
            dojo_id,
            window_id,
            splint_id: added_id,
            incarnation: Some(1),
        };
        let added_request = PolicyRequest {
            resources: &[added_resource],
            ..original_request
        };
        assert!(document.authorize(&executable, &added_request, 1).is_none());
        assert!(document.status(&executable, added_resource, 1).is_empty());

        let added_window = splinterm_core::Window::with_shell(PathBuf::from("/tmp"));
        let added_window_id = added_window.id;
        let added_window_splint_id = added_window.default_focus;
        lair.new_window_at(lair.revision(), dojo_id, added_window)
            .unwrap();
        let added_window_resource = PolicyResource::Splint {
            dojo_id,
            window_id: added_window_id,
            splint_id: added_window_splint_id,
            incarnation: Some(1),
        };
        let added_window_request = PolicyRequest {
            resources: &[added_window_resource],
            ..original_request
        };
        assert!(
            document
                .authorize(&executable, &added_window_request, 1)
                .is_none()
        );
        assert!(
            document
                .status(&executable, added_window_resource, 1)
                .is_empty()
        );
        let direct_window_resource = PolicyResource::Window {
            dojo_id,
            window_id: added_window_id,
        };
        let direct_window_request = PolicyRequest {
            required_scopes: &[OperationScope::TopologyMetadataRead],
            any_scope: &[],
            resources: &[direct_window_resource],
            limits: RequestedLimits::default(),
        };
        assert!(
            document
                .authorize(&executable, &direct_window_request, 1)
                .is_none()
        );
        assert!(
            document
                .status(&executable, direct_window_resource, 1)
                .is_empty()
        );

        document.resolve_against(&lair).unwrap();
        assert!(document.authorize(&executable, &added_request, 1).is_some());
        assert_eq!(document.status(&executable, added_resource, 1).len(), 1);
        assert!(
            document
                .authorize(&executable, &added_window_request, 1)
                .is_some()
        );
        assert_eq!(
            document.status(&executable, added_window_resource, 1).len(),
            1
        );
        assert!(
            document
                .authorize(&executable, &direct_window_request, 1)
                .is_some()
        );
        assert_eq!(
            document
                .status(&executable, direct_window_resource, 1)
                .len(),
            1
        );

        let mut absent: PolicyDocument = serde_json::from_value(serde_json::json!({
            "schema": "splinterm.policy.v1",
            "rules": [{
                "id": "missing-window",
                "executable": {
                    "path": "/usr/bin/client",
                    "sha256": "a".repeat(64),
                },
                "scopes": ["topology_metadata_read"],
                "resources": [{"kind": "window", "window_id": WindowId::new()}],
                "limits": {"deadline_ms": 1},
            }],
        }))
        .unwrap();
        assert!(absent.resolve_against(&lair).is_err());
        let candidate = PolicyCandidate {
            document: absent,
            diagnostic: None,
        };
        let mut store = PolicyStore::default();
        let generation = store.publish(candidate, &lair);
        assert!(generation.document.rules.is_empty());
        assert!(generation.diagnostic.is_some());
    }

    #[test]
    fn exact_and_current_incarnations_remain_distinct_after_publication() {
        let splint_id = SplintId::new();
        let (lair, dojo_id, window_id) = lair_with_splint(splint_id);
        let executable = ExecutableIdentity {
            path: PathBuf::from("/usr/bin/client"),
            device: 1,
            inode: 2,
            owner_uid: 0,
            mode: 0o755,
            size: 1,
            sha256: "a".repeat(64),
        };
        let make_document = |incarnation: serde_json::Value| {
            serde_json::from_value::<PolicyDocument>(serde_json::json!({
                "schema": "splinterm.policy.v1",
                "rules": [{
                    "id": "incarnation-reader",
                    "executable": {
                        "path": "/usr/bin/client",
                        "sha256": "a".repeat(64),
                    },
                    "scopes": ["terminal_visible_read"],
                    "resources": [{
                        "kind": "splint",
                        "splint_id": splint_id,
                        "incarnation": incarnation,
                    }],
                    "limits": {"max_returned_rows": 1},
                }],
            }))
            .unwrap()
        };
        let resource = PolicyResource::Splint {
            dojo_id,
            window_id,
            splint_id,
            incarnation: Some(2),
        };
        let request = PolicyRequest {
            required_scopes: &[OperationScope::TerminalVisibleRead],
            any_scope: &[],
            resources: &[resource],
            limits: RequestedLimits {
                returned_rows: Some(1),
                ..RequestedLimits::default()
            },
        };

        let mut exact = make_document(serde_json::json!(1));
        exact.resolve_against(&lair).unwrap();
        assert!(exact.authorize(&executable, &request, 1).is_none());

        let mut current = make_document(serde_json::json!("current"));
        current.resolve_against(&lair).unwrap();
        assert!(current.authorize(&executable, &request, 1).is_some());
    }

    #[test]
    fn resolved_expansion_is_deduplicated_and_bounded() {
        let splint_id = SplintId::new();
        let (mut lair, dojo_id, window_id) = lair_with_splint(splint_id);
        let mut deduplicated: PolicyDocument = serde_json::from_value(serde_json::json!({
            "schema": "splinterm.policy.v1",
            "rules": [{
                "id": "overlap",
                "executable": {
                    "path": "/usr/bin/client",
                    "sha256": "a".repeat(64),
                },
                "scopes": ["terminal_visible_read"],
                "resources": [
                    {"kind": "dojo", "dojo_id": dojo_id},
                    {"kind": "window", "window_id": window_id},
                    {"kind": "splint", "splint_id": splint_id, "incarnation": "current"}
                ],
                "limits": {"max_returned_rows": 1},
            }],
        }))
        .unwrap();
        deduplicated.resolve_against(&lair).unwrap();
        assert_eq!(deduplicated.resolved_resources["overlap"].len(), 3);

        for _ in 0..256 {
            let window = splinterm_core::Window::with_shell(PathBuf::from("/tmp"));
            lair.new_window_at(lair.revision(), dojo_id, window)
                .unwrap();
        }
        assert!(deduplicated.resolve_against(&lair).is_err());
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
