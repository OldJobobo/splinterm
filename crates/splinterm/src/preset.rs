//! Strict client-local Dojo preset schema and static launch-tree compiler.
//!
//! Presets describe direct process launches. They never evaluate shell source and
//! do not carry daemon authority; Milestone 6 translates compiled trees into one
//! trusted atomic protocol request.

use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    fmt, fs,
    io::Read as _,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use splinterm_core::{Axis, SplitRatio};
use splinterm_protocol::LaunchParameters;

const PRESET_VERSION: u16 = 1;
const MAX_CATALOG_BYTES: usize = 256 * 1024;
const MAX_CATALOG_COMMANDS: usize = 64;
const MAX_CATALOG_PRESETS: usize = 64;
const MAX_IDENTIFIER_BYTES: usize = 64;
const MAX_LABEL_BYTES: usize = 128;
const MAX_PRESET_DEPTH: usize = 32;
const MAX_PRESET_PANES: usize = 32;
const MAX_PRESET_PARAMETERS: usize = 16;
const MAX_PARAMETER_VALUE_BYTES: usize = 4 * 1024;
const BUNDLED_OMARCHY_PRESETS: &str = include_str!("presets/omarchy.toml");

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PresetPaneKey(String);

impl PresetPaneKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresetLayoutLaunch {
    Pane {
        key: PresetPaneKey,
        title: String,
        launch: LaunchParameters,
    },
    Split {
        orientation: PresetOrientation,
        ratio: SplitRatio,
        first: Box<Self>,
        second: Box<Self>,
    },
}

impl PresetLayoutLaunch {
    #[must_use]
    pub const fn pane_count(&self) -> usize {
        match self {
            Self::Pane { .. } => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresetOrientation {
    Columns,
    Rows,
}

impl PresetOrientation {
    #[must_use]
    pub const fn axis(self) -> Axis {
        match self {
            Self::Columns => Axis::Horizontal,
            Self::Rows => Axis::Vertical,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DojoLaunchSpec {
    pub name: String,
    pub focus: PresetPaneKey,
    pub root: PresetLayoutLaunch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetCatalog {
    source: PresetFile,
    bundled: Option<PresetFile>,
}

impl PresetCatalog {
    /// Parses and structurally validates one strict version-1 preset catalog.
    ///
    /// # Errors
    /// Returns an error for malformed TOML, unknown fields, unsupported versions,
    /// invalid aliases, or a preset whose named nodes are not one bounded tree.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_CATALOG_BYTES,
            "preset catalog exceeds maximum size of {MAX_CATALOG_BYTES} bytes"
        );
        let source = parse_preset_file(text)?;
        ensure!(
            source
                .presets
                .values()
                .all(|preset| preset.kind == PresetKind::Dojo),
            "user preset catalogs cannot define packaged workflow kinds"
        );
        Ok(Self {
            source,
            bundled: None,
        })
    }

    /// Loads and validates one explicitly configured catalog path.
    ///
    /// # Errors
    /// Returns an error when the file is unreadable or invalid.
    pub fn load(path: &Path) -> Result<Self> {
        let text = read_catalog(path)?;
        Self::parse(&text).with_context(|| format!("validate preset catalog {}", path.display()))
    }

    /// Loads one user overlay while allowing it to reference packaged aliases.
    /// User aliases shadow packaged aliases only for user-owned presets.
    ///
    /// # Errors
    /// Returns an error when the file is unreadable or the effective user scope
    /// is invalid.
    pub fn load_user_overlay(path: &Path) -> Result<Self> {
        let text = read_catalog(path)?;
        ensure!(
            text.len() <= MAX_CATALOG_BYTES,
            "preset catalog exceeds maximum size of {MAX_CATALOG_BYTES} bytes"
        );
        let source: PresetFile = toml::from_str(&text).context("parse presets.toml")?;
        ensure!(
            source.version == PRESET_VERSION,
            "unsupported presets.toml version {}; expected {PRESET_VERSION}",
            source.version
        );
        let bundled = Self::bundled().source;
        let mut commands = bundled.commands.clone();
        commands.extend(source.commands.clone());
        ensure!(
            source
                .presets
                .values()
                .all(|preset| preset.kind == PresetKind::Dojo),
            "user preset catalogs cannot define packaged workflow kinds"
        );
        validate_catalog_with_commands(&source, &commands)?;
        Ok(Self {
            source,
            bundled: Some(bundled),
        })
    }

    /// Returns the packaged Omarchy catalog with no user overlay.
    ///
    /// # Panics
    /// Panics only when the checked-in bundled catalog violates its own schema.
    #[must_use]
    pub fn bundled() -> Self {
        let source = parse_preset_file(BUNDLED_OMARCHY_PRESETS)
            .expect("bundled Omarchy presets must remain valid");
        Self {
            source,
            bundled: None,
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.source.presets.keys().map(String::as_str).chain(
            self.bundled
                .iter()
                .flat_map(|file| file.presets.keys())
                .filter(|name| !self.source.presets.contains_key(*name))
                .map(String::as_str),
        )
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.source.presets.contains_key(name)
            || self
                .bundled
                .as_ref()
                .is_some_and(|file| file.presets.contains_key(name))
    }

    /// Compiles one static Dojo preset against an exact invocation context.
    ///
    /// # Errors
    /// Returns an error when the preset is absent, placeholder expansion fails,
    /// a cwd is not an existing directory, `$EDITOR` is unsafe, or final direct
    /// launch parameters exceed protocol bounds.
    pub fn compile(
        &self,
        name: &str,
        context: &PresetCompileContext<'_>,
    ) -> Result<DojoLaunchSpec> {
        self.compile_with_parameters(name, context, &[], false)
    }

    /// Compiles one preset with bounded typed parameters and command-risk policy.
    ///
    /// # Errors
    /// Returns an error for unknown, duplicate, missing, malformed, out-of-range,
    /// or disabled unrestricted parameters before producing a launch tree.
    pub fn compile_with_parameters(
        &self,
        name: &str,
        context: &PresetCompileContext<'_>,
        supplied: &[String],
        allow_unrestricted_commands: bool,
    ) -> Result<DojoLaunchSpec> {
        let (preset, commands) = self.resolve(name)?;
        ensure!(
            preset.kind == PresetKind::Dojo,
            "preset {name:?} is a workflow and must be run through the Splinterm client"
        );
        validate_root_cwd(context.root_cwd)?;
        let parameters = resolve_parameters(
            &preset.parameter,
            supplied,
            &commands,
            context,
            allow_unrestricted_commands,
        )?;
        let compiled_name =
            expand_placeholders(&preset.name, context.root_cwd).context("expand preset name")?;
        validate_label("compiled Dojo name", &compiled_name)?;
        let root = compile_node(
            &preset.root,
            preset,
            &commands,
            &parameters,
            context,
            allow_unrestricted_commands,
            1,
        )?
        .context("preset parameters removed every pane")?;
        ensure!(
            root.pane_count() <= MAX_PRESET_PANES,
            "compiled preset contains too many panes"
        );
        let focus = PresetPaneKey(preset.focus.clone());
        ensure!(
            layout_contains_key(&root, &focus),
            "preset focus {:?} was removed or was not generated",
            preset.focus
        );
        Ok(DojoLaunchSpec {
            name: compiled_name,
            focus,
            root,
        })
    }

    fn resolve(
        &self,
        name: &str,
    ) -> Result<(&PresetDefinition, BTreeMap<String, CommandDefinition>)> {
        if let Some(preset) = self.source.presets.get(name) {
            let mut commands = self
                .bundled
                .as_ref()
                .map_or_else(BTreeMap::new, |file| file.commands.clone());
            commands.extend(self.source.commands.clone());
            return Ok((preset, commands));
        }
        let bundled = self
            .bundled
            .as_ref()
            .with_context(|| format!("unknown preset {name:?}"))?;
        Ok((
            bundled
                .presets
                .get(name)
                .with_context(|| format!("unknown preset {name:?}"))?,
            bundled.commands.clone(),
        ))
    }

    #[must_use]
    pub fn workflow(&self, name: &str) -> Option<PresetWorkflow> {
        let preset = self
            .source
            .presets
            .get(name)
            .or_else(|| self.bundled.as_ref()?.presets.get(name))?;
        match preset.kind {
            PresetKind::Dojo => None,
            PresetKind::Attach => Some(PresetWorkflow::Attach),
            PresetKind::DirectorySet => Some(PresetWorkflow::DirectorySet),
        }
    }

    #[must_use]
    pub fn summary(&self, name: &str) -> Option<PresetSummary<'_>> {
        let (catalog_name, preset) = self.source.presets.get_key_value(name).or_else(|| {
            self.bundled
                .as_ref()
                .and_then(|file| file.presets.get_key_value(name))
        })?;
        Some(PresetSummary {
            name: catalog_name,
            display_name: preset.display_name.as_deref().unwrap_or(catalog_name),
            panes: count_panes(&preset.root, &preset.nodes, &preset.parameter),
            focus: &preset.focus,
            parameterized: !preset.parameter.is_empty(),
            workflow: match preset.kind {
                PresetKind::Dojo => None,
                PresetKind::Attach => Some(PresetWorkflow::Attach),
                PresetKind::DirectorySet => Some(PresetWorkflow::DirectorySet),
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresetWorkflow {
    Attach,
    DirectorySet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresetSummary<'a> {
    pub name: &'a str,
    pub display_name: &'a str,
    pub panes: usize,
    pub focus: &'a str,
    pub parameterized: bool,
    pub workflow: Option<PresetWorkflow>,
}

#[derive(Clone, Copy, Debug)]
pub struct PresetCompileContext<'a> {
    pub root_cwd: &'a Path,
    pub editor: Option<&'a OsStr>,
    pub shell: Option<&'a str>,
    pub login_shell: bool,
    pub scrollback_lines: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetFile {
    version: u16,
    #[serde(default)]
    commands: BTreeMap<String, CommandDefinition>,
    #[serde(default)]
    presets: BTreeMap<String, PresetDefinition>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum CommandDefinition {
    Argv {
        argv: Vec<String>,
        #[serde(default)]
        unrestricted: bool,
    },
    EditorEnv {
        fallback: Vec<String>,
        #[serde(default)]
        append: Vec<String>,
        #[serde(default)]
        unrestricted: bool,
    },
}

impl CommandDefinition {
    const fn unrestricted(&self) -> bool {
        match self {
            Self::Argv { unrestricted, .. } | Self::EditorEnv { unrestricted, .. } => *unrestricted,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetDefinition {
    kind: PresetKind,
    #[serde(rename = "display-name")]
    display_name: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    root: String,
    #[serde(default)]
    focus: String,
    #[serde(default)]
    parameter: Vec<ParameterDefinition>,
    #[serde(default)]
    nodes: BTreeMap<String, NodeDefinition>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PresetKind {
    Dojo,
    Attach,
    DirectorySet,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParameterDefinition {
    name: String,
    #[serde(rename = "type")]
    kind: ParameterKind,
    min: Option<u16>,
    max: Option<u16>,
    #[serde(default)]
    required: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ParameterKind {
    Integer,
    Command,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ResolvedParameter {
    Integer(u16),
    Command(Vec<String>),
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum NodeDefinition {
    Split {
        orientation: OrientationDefinition,
        ratio: u16,
        first: String,
        second: String,
    },
    Pane {
        command: Option<String>,
        #[serde(rename = "parameter-command")]
        parameter_command: Option<String>,
        #[serde(default)]
        shell: bool,
        cwd: Option<String>,
        title: Option<String>,
        #[serde(rename = "when-parameter")]
        when_parameter: Option<String>,
    },
    Grid {
        count: String,
        #[serde(rename = "pane-command")]
        pane_command: String,
        cwd: Option<String>,
        title: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum OrientationDefinition {
    Columns,
    Rows,
}

impl From<OrientationDefinition> for PresetOrientation {
    fn from(value: OrientationDefinition) -> Self {
        match value {
            OrientationDefinition::Columns => Self::Columns,
            OrientationDefinition::Rows => Self::Rows,
        }
    }
}

fn read_catalog(path: &Path) -> Result<String> {
    let file =
        fs::File::open(path).with_context(|| format!("open preset catalog {}", path.display()))?;
    let mut text = String::with_capacity(MAX_CATALOG_BYTES + 1);
    file.take((MAX_CATALOG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .with_context(|| format!("read preset catalog {}", path.display()))?;
    Ok(text)
}

fn parse_preset_file(text: &str) -> Result<PresetFile> {
    let source: PresetFile = toml::from_str(text).context("parse presets.toml")?;
    ensure!(
        source.version == PRESET_VERSION,
        "unsupported presets.toml version {}; expected {PRESET_VERSION}",
        source.version
    );
    validate_catalog(&source)?;
    Ok(source)
}

fn validate_catalog(file: &PresetFile) -> Result<()> {
    validate_catalog_with_commands(file, &file.commands)
}

fn validate_catalog_with_commands(
    file: &PresetFile,
    effective_commands: &BTreeMap<String, CommandDefinition>,
) -> Result<()> {
    ensure!(
        file.commands.len() <= MAX_CATALOG_COMMANDS,
        "preset catalog contains {} command aliases; maximum is {MAX_CATALOG_COMMANDS}",
        file.commands.len()
    );
    ensure!(
        !file.presets.is_empty(),
        "preset catalog contains no presets"
    );
    ensure!(
        file.presets.len() <= MAX_CATALOG_PRESETS,
        "preset catalog contains {} presets; maximum is {MAX_CATALOG_PRESETS}",
        file.presets.len()
    );
    for (name, command) in &file.commands {
        validate_identifier("command alias", name)?;
        match command {
            CommandDefinition::Argv { argv, .. } => validate_direct_argv(argv, "command alias")?,
            CommandDefinition::EditorEnv {
                fallback, append, ..
            } => {
                validate_direct_argv(fallback, "editor fallback")?;
                validate_argv_items(append, true, "editor append arguments")?;
                let mut combined = fallback.clone();
                combined.extend(append.iter().cloned());
                validate_direct_argv(&combined, "editor fallback with append arguments")?;
            }
        }
    }
    for (name, preset) in &file.presets {
        validate_identifier("preset", name)?;
        validate_label(
            "preset display name",
            preset.display_name.as_deref().unwrap_or(name),
        )?;
        ensure!(
            preset.parameter.len() <= MAX_PRESET_PARAMETERS,
            "preset {name:?} contains too many parameters"
        );
        let mut parameter_names = HashSet::new();
        for parameter in &preset.parameter {
            validate_identifier("preset parameter", &parameter.name)?;
            ensure!(
                parameter_names.insert(parameter.name.as_str()),
                "preset {name:?} repeats parameter {:?}",
                parameter.name
            );
            match parameter.kind {
                ParameterKind::Integer => {
                    let min = parameter.min.unwrap_or(0);
                    let max = parameter.max.unwrap_or(u16::MAX);
                    ensure!(min <= max, "preset parameter range is inverted");
                }
                ParameterKind::Command => ensure!(
                    parameter.min.is_none() && parameter.max.is_none(),
                    "command parameter {:?} cannot have integer bounds",
                    parameter.name
                ),
            }
        }
        if preset.kind == PresetKind::Dojo {
            ensure!(
                !preset.name.is_empty(),
                "preset {name:?} name must not be empty"
            );
            validate_template("preset name", &preset.name)?;
            if !preset.name.contains("{cwd") {
                validate_label("preset name", &preset.name)?;
            }
            validate_identifier("preset root", &preset.root)?;
            validate_identifier("preset focus", &preset.focus)?;
            ensure!(
                !preset.nodes.is_empty(),
                "preset {name:?} contains no nodes"
            );
            for node_name in preset.nodes.keys() {
                validate_identifier("node", node_name)?;
            }
            validate_preset_tree(name, preset, effective_commands)?;
        } else {
            ensure!(
                preset.name.is_empty()
                    && preset.root.is_empty()
                    && preset.focus.is_empty()
                    && preset.nodes.is_empty(),
                "workflow preset {name:?} cannot define a static layout"
            );
        }
    }
    Ok(())
}

fn validate_preset_tree(
    preset_name: &str,
    preset: &PresetDefinition,
    commands: &BTreeMap<String, CommandDefinition>,
) -> Result<()> {
    ensure!(
        preset.nodes.contains_key(&preset.root),
        "preset {preset_name:?} root {:?} does not exist",
        preset.root
    );
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut panes = 0;
    walk_node(
        &preset.root,
        preset_name,
        preset,
        commands,
        1,
        &mut visiting,
        &mut visited,
        &mut panes,
    )?;
    ensure!(
        visited.len() == preset.nodes.len(),
        "preset {preset_name:?} contains unreachable node(s): {}",
        preset
            .nodes
            .keys()
            .filter(|node| !visited.contains(*node))
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    ensure!(
        panes <= MAX_PRESET_PANES,
        "preset {preset_name:?} contains {panes} panes; maximum is {MAX_PRESET_PANES}"
    );
    ensure!(
        matches!(
            preset.nodes.get(&preset.focus),
            Some(NodeDefinition::Pane { .. })
        ) || preset
            .nodes
            .iter()
            .any(|(name, node)| matches!(node, NodeDefinition::Grid { .. })
                && preset.focus == format!("{name}.0")),
        "preset {preset_name:?} focus {:?} must name a pane or first generated grid pane",
        preset.focus
    );
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one recursive validator keeps node-type invariants and graph bookkeeping adjacent"
)]
fn walk_node(
    node_name: &str,
    preset_name: &str,
    preset: &PresetDefinition,
    commands: &BTreeMap<String, CommandDefinition>,
    depth: usize,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    panes: &mut usize,
) -> Result<()> {
    ensure!(
        depth <= MAX_PRESET_DEPTH,
        "preset {preset_name:?} exceeds maximum depth {MAX_PRESET_DEPTH} at node {node_name:?}"
    );
    if visiting.contains(node_name) {
        bail!("preset {preset_name:?} contains a cycle at node {node_name:?}");
    }
    if visited.contains(node_name) {
        bail!("preset {preset_name:?} reuses node {node_name:?}");
    }
    let node = preset
        .nodes
        .get(node_name)
        .with_context(|| format!("preset {preset_name:?} references missing node {node_name:?}"))?;
    visiting.insert(node_name.to_owned());
    match node {
        NodeDefinition::Split {
            orientation: _,
            ratio,
            first,
            second,
        } => {
            ensure!(
                SplitRatio::new(*ratio).is_ok(),
                "preset {preset_name:?} node {node_name:?} ratio must be in 1..=999"
            );
            walk_node(
                first,
                preset_name,
                preset,
                commands,
                depth + 1,
                visiting,
                visited,
                panes,
            )?;
            walk_node(
                second,
                preset_name,
                preset,
                commands,
                depth + 1,
                visiting,
                visited,
                panes,
            )?;
        }
        NodeDefinition::Pane {
            command,
            parameter_command,
            shell,
            cwd,
            title,
            when_parameter,
        } => {
            ensure!(
                usize::from(command.is_some())
                    + usize::from(parameter_command.is_some())
                    + usize::from(*shell)
                    == 1,
                "preset {preset_name:?} pane {node_name:?} must set exactly one command source"
            );
            if let Some(alias) = command {
                validate_identifier("pane command alias", alias)?;
                ensure!(
                    commands.contains_key(alias),
                    "preset {preset_name:?} pane {node_name:?} references unknown command alias {alias:?}"
                );
            }
            for parameter in parameter_command.iter().chain(when_parameter.iter()) {
                validate_identifier("pane parameter", parameter)?;
                ensure!(
                    preset.parameter.iter().any(|item| item.name == *parameter),
                    "preset {preset_name:?} pane {node_name:?} references unknown parameter {parameter:?}"
                );
            }
            if let Some(parameter) = parameter_command {
                ensure!(
                    preset
                        .parameter
                        .iter()
                        .any(|item| item.name == *parameter && item.kind == ParameterKind::Command),
                    "preset pane parameter-command must name a command parameter"
                );
            }
            if let Some(cwd) = cwd {
                ensure!(
                    !cwd.is_empty(),
                    "preset {preset_name:?} pane {node_name:?} cwd must not be empty"
                );
                validate_template("pane cwd", cwd)?;
            }
            if let Some(title) = title {
                ensure!(
                    !title.is_empty(),
                    "preset {preset_name:?} pane {node_name:?} title must not be empty"
                );
                validate_template("pane title", title)?;
                if !title.contains("{cwd") {
                    validate_label("pane title", title)?;
                }
            }
            *panes += 1;
        }
        NodeDefinition::Grid {
            count,
            pane_command,
            cwd,
            title,
        } => {
            let count_name = parameter_placeholder(count, "grid count")?;
            let command_name = parameter_placeholder(pane_command, "grid pane-command")?;
            let count_parameter = preset
                .parameter
                .iter()
                .find(|item| item.name == count_name)
                .context("grid count references unknown parameter")?;
            ensure!(
                count_parameter.kind == ParameterKind::Integer,
                "grid count must reference an integer parameter"
            );
            ensure!(
                count_parameter.min.unwrap_or(0) >= 1,
                "grid integer parameter minimum must be at least 1"
            );
            ensure!(
                usize::from(count_parameter.max.unwrap_or(u16::MAX)) <= MAX_PRESET_PANES,
                "grid integer parameter maximum exceeds {MAX_PRESET_PANES} panes"
            );
            ensure!(
                preset
                    .parameter
                    .iter()
                    .any(|item| item.name == command_name && item.kind == ParameterKind::Command),
                "grid pane-command must reference a command parameter"
            );
            if let Some(cwd) = cwd {
                validate_template("grid cwd", cwd)?;
            }
            if let Some(title) = title {
                validate_template("grid title", title)?;
            }
            *panes += usize::from(count_parameter.max.unwrap_or(0));
        }
    }
    visiting.remove(node_name);
    visited.insert(node_name.to_owned());
    Ok(())
}

fn validate_identifier(kind: &str, value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= MAX_IDENTIFIER_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
        "{kind} {value:?} must be a 1..={MAX_IDENTIFIER_BYTES} byte ASCII identifier"
    );
    Ok(())
}

fn validate_label(kind: &str, value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty()
            && value.len() <= MAX_LABEL_BYTES
            && !value.contains(['\0', '\r', '\n']),
        "{kind} must be nonempty, single-line UTF-8 of at most {MAX_LABEL_BYTES} bytes"
    );
    Ok(())
}

fn validate_template(kind: &str, value: &str) -> Result<()> {
    ensure!(
        !value.contains(['\0', '\r', '\n']),
        "{kind} contains a forbidden control character"
    );
    let stripped = value.replace("{cwd.basename}", "").replace("{cwd}", "");
    ensure!(
        !stripped.contains(['{', '}']),
        "{kind} contains an unsupported placeholder"
    );
    Ok(())
}

fn validate_root_cwd(root: &Path) -> Result<()> {
    ensure!(root.is_absolute(), "preset root cwd must be absolute");
    ensure!(
        root.to_str().is_some(),
        "preset root cwd must be valid UTF-8"
    );
    let metadata = fs::metadata(root)
        .with_context(|| format!("preset root cwd {} is unavailable", root.display()))?;
    ensure!(metadata.is_dir(), "preset root cwd must be a directory");
    Ok(())
}

fn resolve_parameters(
    definitions: &[ParameterDefinition],
    supplied: &[String],
    commands: &BTreeMap<String, CommandDefinition>,
    context: &PresetCompileContext<'_>,
    allow_unrestricted_commands: bool,
) -> Result<BTreeMap<String, ResolvedParameter>> {
    ensure!(
        supplied.len() <= MAX_PRESET_PARAMETERS,
        "too many preset parameters"
    );
    let mut raw = BTreeMap::new();
    for item in supplied {
        ensure!(
            item.len() <= MAX_PARAMETER_VALUE_BYTES + MAX_IDENTIFIER_BYTES + 1,
            "preset parameter exceeds maximum size"
        );
        let (name, value) = item
            .split_once('=')
            .context("preset parameters must use NAME=VALUE")?;
        validate_identifier("preset parameter", name)?;
        ensure!(!value.is_empty(), "preset parameter {name:?} is empty");
        ensure!(
            raw.insert(name.to_owned(), value.to_owned()).is_none(),
            "preset parameter {name:?} was supplied more than once"
        );
    }
    ensure!(
        raw.keys().all(|name| definitions
            .iter()
            .any(|definition| definition.name == *name)),
        "unknown preset parameter was supplied"
    );
    let mut resolved = BTreeMap::new();
    for definition in definitions {
        let Some(value) = raw.get(&definition.name) else {
            ensure!(
                !definition.required,
                "required preset parameter {:?} is missing",
                definition.name
            );
            continue;
        };
        let value = match definition.kind {
            ParameterKind::Integer => {
                let value = value.parse::<u16>().with_context(|| {
                    format!("preset parameter {:?} must be an integer", definition.name)
                })?;
                ensure!(
                    definition.min.is_none_or(|minimum| value >= minimum)
                        && definition.max.is_none_or(|maximum| value <= maximum),
                    "preset parameter {:?} is outside its allowed range",
                    definition.name
                );
                ResolvedParameter::Integer(value)
            }
            ParameterKind::Command => {
                let argv = if let Some(command) = commands.get(value) {
                    compile_command(command, context, allow_unrestricted_commands)?
                } else {
                    let argv =
                        parse_compatibility_command(value).context("parse command parameter")?;
                    validate_direct_argv(&argv, "command parameter")?;
                    argv
                };
                ResolvedParameter::Command(argv)
            }
        };
        resolved.insert(definition.name.clone(), value);
    }
    Ok(resolved)
}

fn parameter_placeholder<'a>(value: &'a str, kind: &str) -> Result<&'a str> {
    let name = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .with_context(|| format!("{kind} must be one parameter placeholder"))?;
    validate_identifier(kind, name)?;
    Ok(name)
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one recursive compiler keeps pruning, command resolution, and grid expansion adjacent"
)]
fn compile_node(
    node_name: &str,
    preset: &PresetDefinition,
    commands: &BTreeMap<String, CommandDefinition>,
    parameters: &BTreeMap<String, ResolvedParameter>,
    context: &PresetCompileContext<'_>,
    allow_unrestricted_commands: bool,
    depth: usize,
) -> Result<Option<PresetLayoutLaunch>> {
    ensure!(depth <= MAX_PRESET_DEPTH, "preset compiler depth exceeded");
    let node = preset
        .nodes
        .get(node_name)
        .context("validated preset node disappeared")?;
    match node {
        NodeDefinition::Split {
            orientation,
            ratio,
            first,
            second,
        } => {
            let first = compile_node(
                first,
                preset,
                commands,
                parameters,
                context,
                allow_unrestricted_commands,
                depth + 1,
            )?;
            let second = compile_node(
                second,
                preset,
                commands,
                parameters,
                context,
                allow_unrestricted_commands,
                depth + 1,
            )?;
            Ok(match (first, second) {
                (Some(first), Some(second)) => Some(PresetLayoutLaunch::Split {
                    orientation: (*orientation).into(),
                    ratio: SplitRatio::new(*ratio)
                        .map_err(|_| anyhow::anyhow!("invalid split ratio"))?,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            })
        }
        NodeDefinition::Pane {
            command,
            parameter_command,
            shell,
            cwd,
            title,
            when_parameter,
        } => {
            if when_parameter
                .as_ref()
                .is_some_and(|name| !parameters.contains_key(name))
            {
                return Ok(None);
            }
            let cwd = compile_cwd(cwd.as_deref(), context.root_cwd)
                .with_context(|| format!("compile cwd for pane {node_name:?}"))?;
            let command = if *shell {
                Vec::new()
            } else if let Some(name) = parameter_command {
                let Some(ResolvedParameter::Command(argv)) = parameters.get(name) else {
                    bail!("required command parameter {name:?} is absent");
                };
                argv.clone()
            } else {
                compile_command(
                    commands
                        .get(
                            command
                                .as_deref()
                                .context("validated pane command disappeared")?,
                        )
                        .context("validated command alias disappeared")?,
                    context,
                    allow_unrestricted_commands,
                )?
            };
            let title =
                expand_placeholders(title.as_deref().unwrap_or(node_name), context.root_cwd)
                    .with_context(|| format!("expand title for pane {node_name:?}"))?;
            Ok(Some(compile_pane(
                PresetPaneKey(node_name.to_owned()),
                title,
                cwd,
                command,
                context,
            )?))
        }
        NodeDefinition::Grid {
            count,
            pane_command,
            cwd,
            title,
        } => {
            let count_name = parameter_placeholder(count, "grid count")?;
            let command_name = parameter_placeholder(pane_command, "grid pane-command")?;
            let Some(ResolvedParameter::Integer(count)) = parameters.get(count_name) else {
                bail!("required grid count parameter {count_name:?} is absent");
            };
            let Some(ResolvedParameter::Command(command)) = parameters.get(command_name) else {
                bail!("required grid command parameter {command_name:?} is absent");
            };
            let cwd = compile_cwd(cwd.as_deref(), context.root_cwd)
                .with_context(|| format!("compile cwd for grid {node_name:?}"))?;
            let mut panes = Vec::with_capacity(usize::from(*count));
            for index in 0..usize::from(*count) {
                let key = PresetPaneKey(format!("{node_name}.{index}"));
                let title = if let Some(title) = title.as_deref() {
                    format!(
                        "{} {}",
                        expand_placeholders(title, context.root_cwd)?,
                        index + 1
                    )
                } else {
                    key.as_str().to_owned()
                };
                panes.push(compile_pane(
                    key,
                    title,
                    cwd.clone(),
                    command.clone(),
                    context,
                )?);
            }
            Ok(Some(compile_grid(&panes)?))
        }
    }
}

fn compile_pane(
    key: PresetPaneKey,
    title: String,
    cwd: PathBuf,
    command: Vec<String>,
    context: &PresetCompileContext<'_>,
) -> Result<PresetLayoutLaunch> {
    validate_label("compiled pane title", &title)?;
    let launch = LaunchParameters {
        cwd,
        command,
        shell: context.shell.map(str::to_owned),
        login_shell: context.login_shell,
        scrollback_lines: context.scrollback_lines,
    };
    launch
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    Ok(PresetLayoutLaunch::Pane { key, title, launch })
}

#[derive(Clone)]
struct WeightedLayout {
    node: PresetLayoutLaunch,
    occupied_panes: usize,
}

fn compile_grid(panes: &[PresetLayoutLaunch]) -> Result<PresetLayoutLaunch> {
    ensure!(!panes.is_empty(), "grid must contain at least one pane");
    let mut columns = 1;
    while columns * columns < panes.len() {
        columns += 1;
    }
    let rows = panes
        .chunks(columns)
        .map(|row| {
            combine_balanced(
                row.iter()
                    .cloned()
                    .map(|node| WeightedLayout {
                        node,
                        occupied_panes: 1,
                    })
                    .collect(),
                PresetOrientation::Columns,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(combine_balanced(rows, PresetOrientation::Rows)?.node)
}

fn combine_balanced(
    mut nodes: Vec<WeightedLayout>,
    orientation: PresetOrientation,
) -> Result<WeightedLayout> {
    ensure!(!nodes.is_empty(), "cannot combine an empty layout");
    if nodes.len() == 1 {
        return Ok(nodes.remove(0));
    }
    let midpoint = nodes.len().div_ceil(2);
    let second = nodes.split_off(midpoint);
    let first = combine_balanced(nodes, orientation)?;
    let second = combine_balanced(second, orientation)?;
    let occupied_panes = first.occupied_panes + second.occupied_panes;
    let ratio = u16::try_from((first.occupied_panes * 1_000 + occupied_panes / 2) / occupied_panes)
        .context("grid ratio overflow")?;
    Ok(WeightedLayout {
        node: PresetLayoutLaunch::Split {
            orientation,
            ratio: SplitRatio::new(ratio).map_err(|_| anyhow::anyhow!("invalid grid ratio"))?,
            first: Box::new(first.node),
            second: Box::new(second.node),
        },
        occupied_panes,
    })
}

fn layout_contains_key(layout: &PresetLayoutLaunch, key: &PresetPaneKey) -> bool {
    match layout {
        PresetLayoutLaunch::Pane { key: candidate, .. } => candidate == key,
        PresetLayoutLaunch::Split { first, second, .. } => {
            layout_contains_key(first, key) || layout_contains_key(second, key)
        }
    }
}

fn compile_cwd(template: Option<&str>, root: &Path) -> Result<PathBuf> {
    let expanded = template.map_or_else(
        || Ok(root.to_path_buf()),
        |value| expand_placeholders(value, root).map(PathBuf::from),
    )?;
    let relative = !expanded.is_absolute();
    if relative {
        ensure!(
            expanded
                .components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
            "relative pane cwd must remain beneath the invocation root"
        );
    }
    let path = if relative {
        root.join(&expanded)
    } else {
        expanded
    };
    ensure!(path.to_str().is_some(), "pane cwd must be valid UTF-8");
    let metadata = fs::metadata(&path)
        .with_context(|| format!("pane cwd {} is unavailable", path.display()))?;
    ensure!(
        metadata.is_dir(),
        "pane cwd {} is not a directory",
        path.display()
    );
    if relative {
        let canonical_root = fs::canonicalize(root).context("canonicalize preset root cwd")?;
        let canonical_path = fs::canonicalize(&path).context("canonicalize relative pane cwd")?;
        ensure!(
            canonical_path.starts_with(&canonical_root),
            "relative pane cwd must remain beneath the invocation root"
        );
    }
    Ok(path)
}

fn compile_command(
    command: &CommandDefinition,
    context: &PresetCompileContext<'_>,
    allow_unrestricted_commands: bool,
) -> Result<Vec<String>> {
    ensure!(
        allow_unrestricted_commands || !command.unrestricted(),
        "preset command is unrestricted; set [presets] allow-unrestricted-commands=yes to opt in"
    );
    let argv = match command {
        CommandDefinition::Argv { argv, .. } => argv.clone(),
        CommandDefinition::EditorEnv {
            fallback, append, ..
        } => {
            let mut argv = match context.editor.filter(|editor| {
                editor
                    .as_encoded_bytes()
                    .iter()
                    .any(|byte| !matches!(byte, b' ' | b'\t'))
            }) {
                Some(editor) => parse_compatibility_command_bytes(editor.as_encoded_bytes())
                    .context("parse $EDITOR")?,
                None => fallback.clone(),
            };
            argv.extend(append.iter().cloned());
            argv
        }
    };
    let expanded = argv
        .iter()
        .map(|argument| expand_placeholders(argument, context.root_cwd))
        .collect::<Result<Vec<_>>>()?;
    validate_direct_argv(&expanded, "compiled command")?;
    Ok(expanded)
}

fn expand_placeholders(value: &str, root: &Path) -> Result<String> {
    validate_template("string", value)?;
    let cwd = root
        .to_str()
        .context("preset root cwd must be valid UTF-8")?;
    let expanded = if value.contains("{cwd.basename}") {
        let basename = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .context("preset root cwd has no UTF-8 basename")?;
        value.replace("{cwd.basename}", basename)
    } else {
        value.to_owned()
    };
    Ok(expanded.replace("{cwd}", cwd))
}

fn validate_direct_argv(argv: &[String], kind: &str) -> Result<()> {
    ensure!(!argv.is_empty(), "{kind} argv must not be empty");
    validate_argv_items(argv, false, kind)?;
    let launch = LaunchParameters {
        cwd: PathBuf::from("/"),
        command: argv.to_vec(),
        shell: None,
        login_shell: false,
        scrollback_lines: 0,
    };
    launch
        .validate()
        .map_err(|error| anyhow::anyhow!("{kind} {}", error.message))
}

fn validate_argv_items(argv: &[String], allow_empty: bool, kind: &str) -> Result<()> {
    ensure!(
        allow_empty || argv.first().is_some_and(|program| !program.is_empty()),
        "{kind} executable must not be empty"
    );
    ensure!(
        argv.iter().all(|item| !item.contains('\0')),
        "{kind} contains a forbidden NUL"
    );
    Ok(())
}

fn count_panes(
    root: &str,
    nodes: &BTreeMap<String, NodeDefinition>,
    parameters: &[ParameterDefinition],
) -> usize {
    match nodes.get(root) {
        Some(NodeDefinition::Pane { .. }) => 1,
        Some(NodeDefinition::Grid { count, .. }) => parameter_placeholder(count, "grid count")
            .ok()
            .and_then(|name| parameters.iter().find(|item| item.name == name))
            .and_then(|parameter| parameter.max)
            .map_or(0, usize::from),
        Some(NodeDefinition::Split { first, second, .. }) => {
            count_panes(first, nodes, parameters) + count_panes(second, nodes, parameters)
        }
        None => 0,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompatibilityLexErrorKind {
    Empty,
    ForbiddenControl,
    NonUtf8,
    UnclosedSingleQuote,
    UnclosedDoubleQuote,
    TrailingBackslash,
    InvalidDoubleQuoteEscape,
    ShellMetacharacter,
    LeadingTilde,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompatibilityLexError {
    pub offset: usize,
    pub kind: CompatibilityLexErrorKind,
}

impl fmt::Display for CompatibilityLexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unsafe compatibility command at byte {}: {:?}",
            self.offset, self.kind
        )
    }
}

impl std::error::Error for CompatibilityLexError {}

/// Tokenizes one compatibility command without invoking or emulating a shell.
///
/// # Errors
/// Returns the byte offset and bounded error class for malformed quoting,
/// forbidden controls, implicit expansion, or shell-evaluation metacharacters.
pub fn parse_compatibility_command(
    input: &str,
) -> std::result::Result<Vec<String>, CompatibilityLexError> {
    parse_compatibility_command_bytes(input.as_bytes())
}

/// Tokenizes a possibly non-UTF-8 environment value without exposing its bytes.
///
/// # Errors
/// Returns `NonUtf8` at the first invalid byte, or a normal compatibility lexer
/// error after UTF-8 validation.
pub fn parse_compatibility_command_bytes(
    input: &[u8],
) -> std::result::Result<Vec<String>, CompatibilityLexError> {
    let input = std::str::from_utf8(input).map_err(|error| CompatibilityLexError {
        offset: error.valid_up_to(),
        kind: CompatibilityLexErrorKind::NonUtf8,
    })?;
    parse_compatibility_command_str(input)
}

#[allow(
    clippy::too_many_lines,
    reason = "the closed quote/escape grammar remains auditable as one state machine"
)]
fn parse_compatibility_command_str(
    input: &str,
) -> std::result::Result<Vec<String>, CompatibilityLexError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut argv = Vec::new();
    let mut argument = String::new();
    let mut argument_started = false;
    let mut quote = Quote::None;
    let mut index = 0;
    let bytes = input.as_bytes();
    while index < bytes.len() {
        let byte = bytes[index];
        if matches!(byte, 0 | b'\r' | b'\n') {
            return Err(CompatibilityLexError {
                offset: index,
                kind: CompatibilityLexErrorKind::ForbiddenControl,
            });
        }
        match quote {
            Quote::None => match byte {
                b' ' | b'\t' => {
                    if argument_started {
                        argv.push(std::mem::take(&mut argument));
                        argument_started = false;
                    }
                    index += 1;
                }
                b'\'' => {
                    quote = Quote::Single;
                    argument_started = true;
                    index += 1;
                }
                b'"' => {
                    quote = Quote::Double;
                    argument_started = true;
                    index += 1;
                }
                b'\\' => {
                    let Some(next) = bytes.get(index + 1).copied() else {
                        return Err(CompatibilityLexError {
                            offset: index,
                            kind: CompatibilityLexErrorKind::TrailingBackslash,
                        });
                    };
                    if matches!(next, b'\r' | b'\n' | 0) {
                        return Err(CompatibilityLexError {
                            offset: index + 1,
                            kind: CompatibilityLexErrorKind::ForbiddenControl,
                        });
                    }
                    let width = push_compatibility_character(input, index + 1, &mut argument)?;
                    argument_started = true;
                    index += width + 1;
                }
                _ => {
                    let width = push_compatibility_character(input, index, &mut argument)?;
                    argument_started = true;
                    index += width;
                }
            },
            Quote::Single => {
                if byte == b'\'' {
                    quote = Quote::None;
                    index += 1;
                } else {
                    let width = push_compatibility_character(input, index, &mut argument)?;
                    index += width;
                }
            }
            Quote::Double => match byte {
                b'"' => {
                    quote = Quote::None;
                    index += 1;
                }
                b'\\' => match bytes.get(index + 1).copied() {
                    Some(b'"' | b'\\') => {
                        argument.push(char::from(bytes[index + 1]));
                        index += 2;
                    }
                    Some(_) => {
                        return Err(CompatibilityLexError {
                            offset: index,
                            kind: CompatibilityLexErrorKind::InvalidDoubleQuoteEscape,
                        });
                    }
                    None => {
                        return Err(CompatibilityLexError {
                            offset: index,
                            kind: CompatibilityLexErrorKind::TrailingBackslash,
                        });
                    }
                },
                _ => {
                    let width = push_compatibility_character(input, index, &mut argument)?;
                    index += width;
                }
            },
        }
    }
    match quote {
        Quote::Single => {
            return Err(CompatibilityLexError {
                offset: input.len(),
                kind: CompatibilityLexErrorKind::UnclosedSingleQuote,
            });
        }
        Quote::Double => {
            return Err(CompatibilityLexError {
                offset: input.len(),
                kind: CompatibilityLexErrorKind::UnclosedDoubleQuote,
            });
        }
        Quote::None => {}
    }
    if argument_started {
        argv.push(argument);
    }
    if argv.is_empty() || argv.first().is_some_and(String::is_empty) {
        return Err(CompatibilityLexError {
            offset: 0,
            kind: CompatibilityLexErrorKind::Empty,
        });
    }
    Ok(argv)
}

fn push_compatibility_character(
    input: &str,
    index: usize,
    output: &mut String,
) -> std::result::Result<usize, CompatibilityLexError> {
    let character = input[index..]
        .chars()
        .next()
        .expect("parser index remains inside valid UTF-8 input");
    if output.is_empty() && character == '~' {
        return Err(CompatibilityLexError {
            offset: index,
            kind: CompatibilityLexErrorKind::LeadingTilde,
        });
    }
    if is_shell_metacharacter(character) {
        return Err(CompatibilityLexError {
            offset: index,
            kind: CompatibilityLexErrorKind::ShellMetacharacter,
        });
    }
    output.push(character);
    Ok(character.len_utf8())
}

fn is_shell_metacharacter(character: char) -> bool {
    matches!(
        character,
        '$' | '`'
            | '|'
            | '&'
            | ';'
            | '<'
            | '>'
            | '*'
            | '?'
            | '['
            | ']'
            | '#'
            | '('
            | ')'
            | '{'
            | '}'
    )
}

#[cfg(test)]
mod tests {
    use std::{fmt::Write as _, fs, time::SystemTime};

    use super::*;

    const STATIC_PRESET: &str = r#"
version = 1

[commands.editor]
kind = "editor-env"
fallback = ["nvim"]
append = ["."]

[commands.review]
kind = "argv"
argv = ["codex", "literal;$HOME", "*.rs"]

[presets.personal-review]
kind = "dojo"
display-name = "Review workspace"
name = "{cwd.basename}"
root = "main"
focus = "editor"

[presets.personal-review.nodes.main]
type = "split"
orientation = "columns"
ratio = 650
first = "editor"
second = "review"

[presets.personal-review.nodes.editor]
type = "pane"
command = "editor"
cwd = "{cwd}"
title = "editor"

[presets.personal-review.nodes.review]
type = "pane"
command = "review"
cwd = "subdir"
title = "review"
"#;

    fn test_root() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("splinterm-preset-{}-{unique}", std::process::id()));
        fs::create_dir_all(root.join("subdir")).unwrap();
        root
    }

    fn context(root: &Path) -> PresetCompileContext<'_> {
        PresetCompileContext {
            root_cwd: root,
            editor: Some(OsStr::new("nvim -u 'my config'")),
            shell: Some("/bin/bash"),
            login_shell: true,
            scrollback_lines: 1_000,
        }
    }

    fn balanced_catalog(pane_count: usize) -> String {
        fn append_tree(panes: &[usize], serial: &mut usize, output: &mut String) -> String {
            if let [pane] = panes {
                let name = format!("p{pane}");
                write!(
                    output,
                    "\n[presets.many.nodes.{name}]\ntype=\"pane\"\ncommand=\"run\"\n"
                )
                .unwrap();
                return name;
            }
            let name = format!("b{serial}");
            *serial += 1;
            let middle = panes.len() / 2;
            let first = append_tree(&panes[..middle], serial, output);
            let second = append_tree(&panes[middle..], serial, output);
            write!(
                output,
                "\n[presets.many.nodes.{name}]\ntype=\"split\"\norientation=\"rows\"\nratio=500\nfirst=\"{first}\"\nsecond=\"{second}\"\n"
            )
            .unwrap();
            name
        }

        let panes = (0..pane_count).collect::<Vec<_>>();
        let mut nodes = String::new();
        let root = append_tree(&panes, &mut 0, &mut nodes);
        format!(
            "version=1\n[commands.run]\nkind=\"argv\"\nargv=[\"true\"]\n[presets.many]\nkind=\"dojo\"\nname=\"many\"\nroot=\"{root}\"\nfocus=\"p0\"\n{nodes}"
        )
    }

    fn catalog_with_counts(command_count: usize, preset_count: usize) -> String {
        let mut catalog = "version=1\n".to_owned();
        for index in 0..command_count {
            write!(
                catalog,
                "[commands.c{index}]\nkind=\"argv\"\nargv=[\"true\"]\n"
            )
            .unwrap();
        }
        for index in 0..preset_count {
            write!(
                catalog,
                "[presets.p{index}]\nkind=\"dojo\"\nname=\"p{index}\"\nroot=\"pane\"\nfocus=\"pane\"\n[presets.p{index}.nodes.pane]\ntype=\"pane\"\nshell=true\n"
            )
            .unwrap();
        }
        catalog
    }

    fn deep_catalog(split_count: usize) -> String {
        let mut nodes = String::new();
        for index in 0..=split_count {
            write!(
                nodes,
                "\n[presets.deep.nodes.p{index}]\ntype=\"pane\"\ncommand=\"run\"\n"
            )
            .unwrap();
        }
        for index in 0..split_count {
            let first = if index + 1 == split_count {
                format!("p{}", index + 1)
            } else {
                format!("b{}", index + 1)
            };
            write!(
                nodes,
                "\n[presets.deep.nodes.b{index}]\ntype=\"split\"\norientation=\"columns\"\nratio=500\nfirst=\"{first}\"\nsecond=\"p{index}\"\n"
            )
            .unwrap();
        }
        format!(
            "version=1\n[commands.run]\nkind=\"argv\"\nargv=[\"true\"]\n[presets.deep]\nkind=\"dojo\"\nname=\"deep\"\nroot=\"b0\"\nfocus=\"p0\"\n{nodes}"
        )
    }

    #[test]
    fn static_schema_compiles_named_binary_tree_and_literal_argv() {
        let root = test_root();
        let catalog = PresetCatalog::parse(STATIC_PRESET).unwrap();
        let compiled = catalog.compile("personal-review", &context(&root)).unwrap();
        assert_eq!(compiled.name, root.file_name().unwrap().to_str().unwrap());
        assert_eq!(compiled.focus.as_str(), "editor");
        assert_eq!(compiled.root.pane_count(), 2);
        let PresetLayoutLaunch::Split {
            orientation,
            ratio,
            first,
            second,
        } = compiled.root
        else {
            panic!("expected split");
        };
        assert_eq!(orientation.axis(), Axis::Horizontal);
        assert_eq!(ratio.get(), 650);
        let PresetLayoutLaunch::Pane { launch, .. } = *first else {
            panic!("expected editor pane");
        };
        assert_eq!(launch.command, ["nvim", "-u", "my config", "."]);
        let PresetLayoutLaunch::Pane { launch, .. } = *second else {
            panic!("expected review pane");
        };
        assert_eq!(launch.command, ["codex", "literal;$HOME", "*.rs"]);
        assert_eq!(launch.cwd, root.join("subdir"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fixed_names_compile_at_filesystem_root_and_empty_editor_uses_fallback() {
        let source = STATIC_PRESET
            .replace("name = \"{cwd.basename}\"", "name = \"fixed\"")
            .replace("cwd = \"subdir\"", "cwd = \"{cwd}\"");
        let catalog = PresetCatalog::parse(&source).unwrap();
        let mut compile_context = context(Path::new("/"));
        compile_context.editor = Some(OsStr::new("  "));
        let compiled = catalog
            .compile("personal-review", &compile_context)
            .unwrap();
        assert_eq!(compiled.name, "fixed");
        let PresetLayoutLaunch::Split { first, .. } = compiled.root else {
            panic!("expected split");
        };
        let PresetLayoutLaunch::Pane { launch, .. } = *first else {
            panic!("expected pane");
        };
        assert_eq!(launch.command, ["nvim", "."]);
    }

    #[test]
    fn strict_schema_rejects_unknown_fields_and_versions() {
        let unknown = STATIC_PRESET.replace("version = 1", "version = 1\nextra = true");
        assert!(
            format!("{:#}", PresetCatalog::parse(&unknown).unwrap_err()).contains("unknown field")
        );
        let version = STATIC_PRESET.replace("version = 1", "version = 2");
        assert!(
            PresetCatalog::parse(&version)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
    }

    #[test]
    fn catalog_bytes_and_top_level_counts_are_bounded() {
        let mut exact_bytes = STATIC_PRESET.to_owned();
        exact_bytes.push_str(&" ".repeat(MAX_CATALOG_BYTES - exact_bytes.len()));
        PresetCatalog::parse(&exact_bytes).unwrap();
        exact_bytes.push(' ');
        assert!(
            PresetCatalog::parse(&exact_bytes)
                .unwrap_err()
                .to_string()
                .contains("maximum size")
        );

        PresetCatalog::parse(&catalog_with_counts(
            MAX_CATALOG_COMMANDS,
            MAX_CATALOG_PRESETS,
        ))
        .unwrap();
        assert!(
            PresetCatalog::parse(&catalog_with_counts(MAX_CATALOG_COMMANDS + 1, 1))
                .unwrap_err()
                .to_string()
                .contains("command aliases; maximum is 64")
        );
        assert!(
            PresetCatalog::parse(&catalog_with_counts(0, MAX_CATALOG_PRESETS + 1))
                .unwrap_err()
                .to_string()
                .contains("presets; maximum is 64")
        );
    }

    #[test]
    fn configured_catalog_read_stops_at_the_byte_limit() {
        let root = test_root();
        let path = root.join("oversized.toml");
        let mut oversized = STATIC_PRESET.to_owned();
        oversized.push_str(&" ".repeat(MAX_CATALOG_BYTES + 1 - oversized.len()));
        fs::write(&path, oversized).unwrap();
        assert!(
            PresetCatalog::load(&path)
                .unwrap_err()
                .to_string()
                .contains("validate preset catalog")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graph_validation_rejects_cycle_reuse_orphan_focus_ratio_and_count() {
        let cycle = STATIC_PRESET.replace("second = \"review\"", "second = \"main\"");
        assert!(
            PresetCatalog::parse(&cycle)
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );

        let reuse = STATIC_PRESET.replace("second = \"review\"", "second = \"editor\"");
        assert!(
            PresetCatalog::parse(&reuse)
                .unwrap_err()
                .to_string()
                .contains("reuses")
        );

        let orphan = STATIC_PRESET.replace("second = \"review\"", "second = \"editor2\"")
            + "\n[presets.personal-review.nodes.editor2]\ntype=\"pane\"\ncommand=\"review\"\n";
        assert!(
            PresetCatalog::parse(&orphan)
                .unwrap_err()
                .to_string()
                .contains("unreachable")
        );

        let focus = STATIC_PRESET.replace("focus = \"editor\"", "focus = \"main\"");
        assert!(
            PresetCatalog::parse(&focus)
                .unwrap_err()
                .to_string()
                .contains("must name a pane")
        );

        let ratio = STATIC_PRESET.replace("ratio = 650", "ratio = 1000");
        assert!(
            PresetCatalog::parse(&ratio)
                .unwrap_err()
                .to_string()
                .contains("ratio")
        );

        assert!(
            PresetCatalog::parse(&balanced_catalog(33))
                .unwrap_err()
                .to_string()
                .contains("maximum is 32")
        );
        assert!(
            PresetCatalog::parse(&deep_catalog(32))
                .unwrap_err()
                .to_string()
                .contains("maximum depth 32")
        );
    }

    #[test]
    fn schema_rejects_argv_bounds_launch_ambiguity_and_unknown_placeholders() {
        let empty = STATIC_PRESET.replace(
            "argv = [\"codex\", \"literal;$HOME\", \"*.rs\"]",
            "argv = []",
        );
        assert!(
            PresetCatalog::parse(&empty)
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );

        let arguments = std::iter::repeat_n("\"x\"", 257)
            .collect::<Vec<_>>()
            .join(",");
        let too_many = STATIC_PRESET.replace(
            "argv = [\"codex\", \"literal;$HOME\", \"*.rs\"]",
            &format!("argv = [{arguments}]"),
        );
        assert!(
            PresetCatalog::parse(&too_many)
                .unwrap_err()
                .to_string()
                .contains("launch parameters")
        );

        let ambiguous =
            STATIC_PRESET.replace("command = \"editor\"", "command = \"editor\"\nshell = true");
        assert!(
            PresetCatalog::parse(&ambiguous)
                .unwrap_err()
                .to_string()
                .contains("exactly one")
        );

        let placeholder = STATIC_PRESET.replace("title = \"review\"", "title = \"{project}\"");
        assert!(
            PresetCatalog::parse(&placeholder)
                .unwrap_err()
                .to_string()
                .contains("unsupported placeholder")
        );

        let empty_name = STATIC_PRESET.replace("name = \"{cwd.basename}\"", "name = \"\"");
        assert!(
            PresetCatalog::parse(&empty_name)
                .unwrap_err()
                .to_string()
                .contains("name must not be empty")
        );
    }

    #[test]
    fn rows_map_to_vertical_axis() {
        let root = test_root();
        let source = STATIC_PRESET.replace("orientation = \"columns\"", "orientation = \"rows\"");
        let catalog = PresetCatalog::parse(&source).unwrap();
        let compiled = catalog.compile("personal-review", &context(&root)).unwrap();
        let PresetLayoutLaunch::Split { orientation, .. } = compiled.root else {
            panic!("expected split");
        };
        assert_eq!(orientation.axis(), Axis::Vertical);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compile_rejects_missing_relative_or_non_directory_cwd() {
        let root = test_root();
        assert!(
            PresetCatalog::parse(STATIC_PRESET)
                .unwrap()
                .compile("personal-review", &context(Path::new("relative")))
                .unwrap_err()
                .to_string()
                .contains("must be absolute")
        );
        let parent = STATIC_PRESET.replace("cwd = \"subdir\"", "cwd = \"../\"");
        let catalog = PresetCatalog::parse(&parent).unwrap();
        assert!(
            format!(
                "{:#}",
                catalog
                    .compile("personal-review", &context(&root))
                    .unwrap_err()
            )
            .contains("beneath")
        );
        std::os::unix::fs::symlink(root.parent().unwrap(), root.join("escape")).unwrap();
        let symlink = STATIC_PRESET.replace("cwd = \"subdir\"", "cwd = \"escape\"");
        let catalog = PresetCatalog::parse(&symlink).unwrap();
        assert!(
            format!(
                "{:#}",
                catalog
                    .compile("personal-review", &context(&root))
                    .unwrap_err()
            )
            .contains("beneath")
        );
        let missing = STATIC_PRESET.replace("cwd = \"subdir\"", "cwd = \"missing\"");
        let catalog = PresetCatalog::parse(&missing).unwrap();
        assert!(
            catalog
                .compile("personal-review", &context(&root))
                .unwrap_err()
                .to_string()
                .contains("compile cwd")
        );
        fs::write(root.join("file"), "not a directory").unwrap();
        let file = STATIC_PRESET.replace("cwd = \"subdir\"", "cwd = \"file\"");
        let catalog = PresetCatalog::parse(&file).unwrap();
        assert!(
            format!(
                "{:#}",
                catalog
                    .compile("personal-review", &context(&root))
                    .unwrap_err()
            )
            .contains("not a directory")
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn layout_signature(layout: &PresetLayoutLaunch) -> String {
        match layout {
            PresetLayoutLaunch::Pane { key, .. } => format!("P{}", key.as_str()),
            PresetLayoutLaunch::Split {
                orientation,
                ratio,
                first,
                second,
            } => format!(
                "{}{}({},{})",
                match orientation {
                    PresetOrientation::Columns => "C",
                    PresetOrientation::Rows => "R",
                },
                ratio.get(),
                layout_signature(first),
                layout_signature(second)
            ),
        }
    }

    fn collect_panes<'a>(layout: &'a PresetLayoutLaunch, panes: &mut Vec<(&'a str, &'a [String])>) {
        match layout {
            PresetLayoutLaunch::Pane { key, launch, .. } => {
                panes.push((key.as_str(), &launch.command));
            }
            PresetLayoutLaunch::Split { first, second, .. } => {
                collect_panes(first, panes);
                collect_panes(second, panes);
            }
        }
    }

    #[test]
    fn bundled_tdl_golden_collapses_optional_ai_and_keeps_editor_focus() {
        let root = test_root();
        let catalog = PresetCatalog::bundled();
        let compiled = catalog
            .compile_with_parameters(
                "omarchy.tdl",
                &context(&root),
                &["ai=opencode".into()],
                false,
            )
            .unwrap();
        assert_eq!(compiled.focus.as_str(), "editor");
        assert_eq!(compiled.root.pane_count(), 3);
        assert_eq!(
            layout_signature(&compiled.root),
            "R850(C650(Peditor,Pai1),Pterminal)"
        );
        let PresetLayoutLaunch::Split {
            orientation: PresetOrientation::Rows,
            ratio,
            first,
            second,
        } = &compiled.root
        else {
            panic!("tdl root must be an 85/15 row split");
        };
        assert_eq!(ratio.get(), 850);
        assert!(matches!(
            first.as_ref(),
            PresetLayoutLaunch::Split {
                orientation: PresetOrientation::Columns,
                ratio,
                ..
            } if ratio.get() == 650
        ));
        assert!(matches!(
            second.as_ref(),
            PresetLayoutLaunch::Pane { key, .. } if key.as_str() == "terminal"
        ));
        let mut panes = Vec::new();
        collect_panes(&compiled.root, &mut panes);
        assert_eq!(
            panes.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            ["editor", "ai1", "terminal"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_tdl_two_ai_and_tds_match_reference_trees() {
        let root = test_root();
        let catalog = PresetCatalog::bundled();
        let tdl_preset = catalog
            .compile_with_parameters(
                "omarchy.tdl",
                &context(&root),
                &["ai=opencode".into(), "ai2=codex".into()],
                false,
            )
            .unwrap();
        assert_eq!(tdl_preset.root.pane_count(), 4);
        assert_eq!(
            layout_signature(&tdl_preset.root),
            "R850(C650(Peditor,R500(Pai1,Pai2)),Pterminal)"
        );
        let mut panes = Vec::new();
        collect_panes(&tdl_preset.root, &mut panes);
        assert_eq!(
            panes.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            ["editor", "ai1", "ai2", "terminal"]
        );
        assert!(
            panes
                .iter()
                .any(|(key, argv)| { *key == "ai2" && *argv == ["codex"] })
        );

        let tds_preset = catalog.compile("omarchy.tds", &context(&root)).unwrap();
        assert_eq!(tds_preset.focus.as_str(), "editor");
        assert_eq!(
            layout_signature(&tds_preset.root),
            "R500(C500(Peditor,Pdiff),C500(Pterminal,Pai))"
        );
        let mut panes = Vec::new();
        collect_panes(&tds_preset.root, &mut panes);
        assert_eq!(
            panes.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            ["editor", "diff", "terminal", "ai"]
        );
        assert!(
            panes
                .iter()
                .any(|(key, argv)| { *key == "diff" && *argv == ["hunk", "diff", "--watch"] })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deterministic_swarm_grid_covers_every_count_and_bound() {
        let root = test_root();
        let catalog = PresetCatalog::bundled();
        let mut signatures = BTreeMap::new();
        for count in 1..=16 {
            let params = [format!("count={count}"), "command=printf worker".into()];
            let first = catalog
                .compile_with_parameters("omarchy.tsl", &context(&root), &params, false)
                .unwrap();
            let second = catalog
                .compile_with_parameters("omarchy.tsl", &context(&root), &params, false)
                .unwrap();
            assert_eq!(first, second);
            assert_eq!(first.root.pane_count(), count);
            assert_eq!(first.focus.as_str(), "root.0");
            signatures.insert(count, layout_signature(&first.root));
            let mut panes = Vec::new();
            collect_panes(&first.root, &mut panes);
            assert_eq!(panes.len(), count);
            for (index, (key, argv)) in panes.iter().enumerate() {
                assert_eq!(*key, format!("root.{index}"));
                assert_eq!(*argv, ["printf", "worker"]);
            }
        }
        assert_eq!(signatures[&1], "Proot.0");
        assert_eq!(signatures[&2], "C500(Proot.0,Proot.1)");
        assert_eq!(signatures[&3], "R667(C500(Proot.0,Proot.1),Proot.2)");
        assert_eq!(
            signatures[&4],
            "R500(C500(Proot.0,Proot.1),C500(Proot.2,Proot.3))"
        );
        assert_eq!(
            signatures[&5],
            "R600(C667(C500(Proot.0,Proot.1),Proot.2),C500(Proot.3,Proot.4))"
        );
        let error = catalog
            .compile_with_parameters(
                "omarchy.tsl",
                &context(&root),
                &["count=17".into(), "command=printf".into()],
                false,
            )
            .unwrap_err();
        assert!(error.to_string().contains("outside its allowed range"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parameter_errors_are_bounded_and_optional_focus_cannot_disappear() {
        let root = test_root();
        let catalog = PresetCatalog::bundled();
        for params in [
            vec![],
            vec!["ai=opencode".into(), "ai=codex".into()],
            vec!["unknown=value".into(), "ai=opencode".into()],
        ] {
            assert!(
                catalog
                    .compile_with_parameters("omarchy.tdl", &context(&root), &params, false)
                    .is_err()
            );
        }
        let optional_focus = PresetCatalog::parse(
            r#"version=1
[commands.tool]
kind="argv"
argv=["tool"]
[presets.optional]
kind="dojo"
name="optional"
root="pane"
focus="pane"
[[presets.optional.parameter]]
name="enabled"
type="command"
[presets.optional.nodes.pane]
type="pane"
command="tool"
when-parameter="enabled"
"#,
        )
        .unwrap();
        assert!(
            optional_focus
                .compile("optional", &context(&root))
                .unwrap_err()
                .to_string()
                .contains("removed every pane")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unrestricted_aliases_require_opt_in_and_user_aliases_stay_scoped() {
        let root = test_root();
        let bundled = PresetCatalog::bundled();
        for alias in ["c", "cx", "cy"] {
            let params = [format!("ai={alias}")];
            assert!(
                bundled
                    .compile_with_parameters("omarchy.tdl", &context(&root), &params, false)
                    .unwrap_err()
                    .to_string()
                    .contains("allow-unrestricted-commands=yes")
            );
        }
        let params = ["ai=c".to_owned()];
        let allowed = bundled
            .compile_with_parameters("omarchy.tdl", &context(&root), &params, true)
            .unwrap();
        let mut panes = Vec::new();
        collect_panes(&allowed.root, &mut panes);
        assert!(
            panes
                .iter()
                .any(|(key, argv)| { *key == "ai1" && *argv == ["opencode", "--auto"] })
        );

        let user_path = root.join("user.toml");
        fs::write(
            &user_path,
            r#"version=1
[commands.c]
kind="argv"
argv=["printf", "safe"]
[presets.mine]
kind="dojo"
name="mine"
root="pane"
focus="pane"
[presets.mine.nodes.pane]
type="pane"
command="c"
"#,
        )
        .unwrap();
        let effective = PresetCatalog::load_user_overlay(&user_path).unwrap();
        assert!(effective.compile("mine", &context(&root)).is_ok());
        assert!(
            effective
                .compile_with_parameters("omarchy.tdl", &context(&root), &params, false)
                .is_err()
        );

        let overlay_path = root.join("overlay.toml");
        fs::write(
            &overlay_path,
            r#"version=1
[presets.uses-bundled]
kind="dojo"
name="uses-bundled"
root="pane"
focus="pane"
[presets.uses-bundled.nodes.pane]
type="pane"
command="editor"
"#,
        )
        .unwrap();
        let overlay = PresetCatalog::load_user_overlay(&overlay_path).unwrap();
        assert!(overlay.compile("uses-bundled", &context(&root)).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compatibility_lexer_preserves_quotes_and_empty_arguments() {
        assert_eq!(
            parse_compatibility_command("nvim -u 'my config' \"\" a\\ b").unwrap(),
            ["nvim", "-u", "my config", "", "a b"]
        );
        assert_eq!(
            parse_compatibility_command("écho café").unwrap(),
            ["écho", "café"]
        );
    }

    #[test]
    fn compatibility_lexer_rejects_every_shell_metacharacter_and_implicit_tilde() {
        for character in "$`|&;<>*?[]#(){}".chars() {
            let input = format!("editor x{character}y");
            let error = parse_compatibility_command(&input).unwrap_err();
            assert_eq!(
                error.kind,
                CompatibilityLexErrorKind::ShellMetacharacter,
                "{character}"
            );
            assert_eq!(error.offset, 8, "{character}");
        }
        assert_eq!(
            parse_compatibility_command("editor x\\$y")
                .unwrap_err()
                .offset,
            9
        );
        assert_eq!(
            parse_compatibility_command("editor ~/x").unwrap_err().kind,
            CompatibilityLexErrorKind::LeadingTilde
        );
    }

    #[test]
    fn compatibility_lexer_reports_bounded_syntax_errors() {
        let invalid_utf8 = parse_compatibility_command_bytes(b"editor \xffsecret").unwrap_err();
        assert_eq!(invalid_utf8.kind, CompatibilityLexErrorKind::NonUtf8);
        assert_eq!(invalid_utf8.offset, 7);
        assert!(!invalid_utf8.to_string().contains("secret"));

        let cases = [
            ("", CompatibilityLexErrorKind::Empty),
            ("editor '", CompatibilityLexErrorKind::UnclosedSingleQuote),
            ("editor \"", CompatibilityLexErrorKind::UnclosedDoubleQuote),
            ("editor \\", CompatibilityLexErrorKind::TrailingBackslash),
            (
                "editor \"a\\q\"",
                CompatibilityLexErrorKind::InvalidDoubleQuoteEscape,
            ),
            (
                "editor\nsecret",
                CompatibilityLexErrorKind::ForbiddenControl,
            ),
        ];
        for (input, kind) in cases {
            let error = parse_compatibility_command(input).unwrap_err();
            assert_eq!(error.kind, kind);
            assert!(error.offset <= input.len());
            assert!(!error.to_string().contains("secret"));
        }
    }
}
