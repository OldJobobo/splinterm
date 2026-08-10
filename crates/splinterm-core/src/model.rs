use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Axis, LayoutNode, Splint, SplintId, SplintState, SplitRatio, SplitSide};

const MAX_NAME_BYTES: usize = 128;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TopologyRevision(pub(crate) u64);

impl TopologyRevision {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LairId(Uuid);

impl LairId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub(crate) const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

impl Default for LairId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for LairId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::str::FromStr for LairId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DojoId(Uuid);

impl DojoId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub(crate) const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

impl Default for DojoId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DojoId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::str::FromStr for DojoId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// One persistent terminal layout within a Lair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dojo {
    pub id: DojoId,
    pub name: String,
    /// Daemon-persisted convenience hint; connected clients keep actual focus locally.
    pub default_focus: SplintId,
    pub root: LayoutNode,
}

impl Dojo {
    #[must_use]
    pub fn with_shell(name: impl Into<String>, cwd: PathBuf) -> Self {
        let splint = Splint::shell(cwd);
        Self {
            id: DojoId::new(),
            name: name.into(),
            default_focus: splint.id,
            root: LayoutNode::Leaf(splint),
        }
    }
}

/// In-memory lifetime policy for a Lair.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LairLifetime {
    #[default]
    Persistent,
    Transient,
}

impl LairLifetime {
    #[must_use]
    pub const fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent)
    }
}

/// A named session containing Dojos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lair {
    pub id: LairId,
    pub name: String,
    #[serde(default, skip_serializing_if = "LairLifetime::is_persistent")]
    pub lifetime: LairLifetime,
    pub dojos: Vec<Dojo>,
}

impl Lair {
    #[must_use]
    pub fn new(name: impl Into<String>, cwd: PathBuf) -> Self {
        Self::with_lifetime(name, cwd, LairLifetime::Persistent)
    }

    #[must_use]
    pub fn transient(name: impl Into<String>, cwd: PathBuf) -> Self {
        Self::with_lifetime(name, cwd, LairLifetime::Transient)
    }

    fn with_lifetime(name: impl Into<String>, cwd: PathBuf, lifetime: LairLifetime) -> Self {
        Self {
            id: LairId::new(),
            name: name.into(),
            lifetime,
            dojos: vec![Dojo::with_shell("terminal", cwd)],
        }
    }
}

/// The daemon-owned collection of live Lairs.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    pub(crate) revision: TopologyRevision,
    pub(crate) lairs: BTreeMap<LairId, Lair>,
}

#[allow(
    clippy::missing_errors_doc,
    reason = "transaction errors are exhaustively represented by TopologyError"
)]
impl Topology {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            revision: TopologyRevision(0),
            lairs: BTreeMap::new(),
        }
    }

    /// Adds a named Lair with one initial shell Dojo.
    pub fn create_lair(
        &mut self,
        name: impl Into<String>,
        cwd: PathBuf,
    ) -> Result<&Lair, TopologyError> {
        self.create_lair_at(self.revision, name, cwd)
    }

    pub fn create_lair_at(
        &mut self,
        expected: TopologyRevision,
        name: impl Into<String>,
        cwd: PathBuf,
    ) -> Result<&Lair, TopologyError> {
        self.check_revision(expected)?;
        let name = normalized_name(&name.into())?;
        if self.lairs.values().any(|lair| lair.name == name) {
            return Err(TopologyError::DuplicateLairName(name));
        }

        let lair = Lair::new(name, cwd);
        let id = lair.id;
        self.insert_lair_at(expected, lair)?;
        Ok(&self.lairs[&id])
    }

    pub fn insert_lair_at(
        &mut self,
        expected: TopologyRevision,
        lair: Lair,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        validate_name(&lair.name)?;
        if self.lairs.contains_key(&lair.id) {
            return Err(TopologyError::DuplicateLairId(lair.id));
        }
        if self.lairs.values().any(|current| current.name == lair.name) {
            return Err(TopologyError::DuplicateLairName(lair.name));
        }
        self.validate_new_dojos(&lair.dojos)?;
        self.lairs.insert(lair.id, lair);
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn remove_lair(&mut self, id: LairId) -> Option<Lair> {
        let removed = self.lairs.remove(&id);
        if removed.is_some() {
            self.advance_revision();
        }
        removed
    }

    /// Inserts a new leaf beside `target` and commits one topology revision.
    pub fn split_splint(
        &mut self,
        target: SplintId,
        new_splint: Splint,
        axis: Axis,
        side: SplitSide,
        ratio: SplitRatio,
    ) -> Result<TopologyRevision, TopologyError> {
        self.split_splint_at(self.revision, target, new_splint, axis, side, ratio)
    }

    pub fn split_splint_at(
        &mut self,
        expected: TopologyRevision,
        target: SplintId,
        new_splint: Splint,
        axis: Axis,
        side: SplitSide,
        ratio: SplitRatio,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        if self.find_splint(new_splint.id).is_some() {
            return Err(TopologyError::DuplicateSplintId(new_splint.id));
        }

        let dojo = self
            .lairs
            .values_mut()
            .flat_map(|lair| &mut lair.dojos)
            .find(|dojo| dojo.root.find_splint(target).is_some())
            .ok_or(TopologyError::SplintNotFound(target))?;
        if !dojo.root.split(target, new_splint, axis, side, ratio) {
            return Err(TopologyError::SplintNotFound(target));
        }

        self.advance_revision();
        Ok(self.revision)
    }

    /// Removes an exited leaf, collapsing its parent or removing its final Dojo.
    pub fn close_splint(&mut self, id: SplintId) -> Result<TopologyRevision, TopologyError> {
        self.close_splint_at(self.revision, id)
    }

    pub fn close_splint_at(
        &mut self,
        expected: TopologyRevision,
        id: SplintId,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        let splint = self
            .find_splint(id)
            .ok_or(TopologyError::SplintNotFound(id))?;
        if !matches!(splint.state, SplintState::Exited(_)) {
            return Err(TopologyError::SplintStillLive(id));
        }

        'lairs: for lair in self.lairs.values_mut() {
            for dojo_index in 0..lair.dojos.len() {
                if lair.dojos[dojo_index].root.find_splint(id).is_none() {
                    continue;
                }
                let root = lair.dojos[dojo_index].root.clone();
                match root.remove(id) {
                    Ok(Some(root)) => {
                        let dojo = &mut lair.dojos[dojo_index];
                        dojo.root = root;
                        if dojo.default_focus == id {
                            dojo.default_focus = dojo.root.first_splint_id();
                        }
                    }
                    Ok(None) => {
                        lair.dojos.remove(dojo_index);
                    }
                    Err(_) => return Err(TopologyError::SplintNotFound(id)),
                }
                break 'lairs;
            }
        }

        self.advance_revision();
        Ok(self.revision)
    }

    pub fn set_split_ratio_at(
        &mut self,
        expected: TopologyRevision,
        target: SplintId,
        ancestor: u16,
        ratio: SplitRatio,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        let dojo = self
            .lairs
            .values_mut()
            .flat_map(|lair| &mut lair.dojos)
            .find(|dojo| dojo.root.find_splint(target).is_some())
            .ok_or(TopologyError::SplintNotFound(target))?;
        if !dojo.root.set_ancestor_ratio(target, ancestor, ratio) {
            return Err(TopologyError::SplintHasNoParent(target));
        }
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn new_dojo_at(
        &mut self,
        expected: TopologyRevision,
        lair_id: LairId,
        dojo: Dojo,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        self.validate_new_dojos(std::slice::from_ref(&dojo))?;
        self.lairs
            .get_mut(&lair_id)
            .ok_or(TopologyError::LairNotFound(lair_id))?
            .dojos
            .push(dojo);
        self.advance_revision();
        Ok(self.revision)
    }

    /// Atomically appends a complete bounded Dojo set and optionally renames its Lair.
    pub fn materialize_dojos_at(
        &mut self,
        expected: TopologyRevision,
        lair_id: LairId,
        rename: Option<String>,
        dojos: Vec<Dojo>,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        if dojos.is_empty() {
            return Err(TopologyError::EmptyDojoSet);
        }
        self.validate_new_dojos(&dojos)?;
        let rename = rename.map(|name| normalized_name(&name)).transpose()?;
        if let Some(name) = rename.as_ref()
            && self
                .lairs
                .values()
                .any(|lair| lair.id != lair_id && lair.name == *name)
        {
            return Err(TopologyError::DuplicateLairName(name.clone()));
        }
        let lair = self
            .lairs
            .get_mut(&lair_id)
            .ok_or(TopologyError::LairNotFound(lair_id))?;
        if let Some(rename) = rename {
            lair.name = rename;
        }
        lair.dojos.extend(dojos);
        self.advance_revision();
        Ok(self.revision)
    }

    /// Atomically inserts a new Lair containing only the supplied complete Dojos.
    pub fn materialize_lair_at(
        &mut self,
        expected: TopologyRevision,
        name: impl Into<String>,
        dojos: Vec<Dojo>,
    ) -> Result<(LairId, TopologyRevision), TopologyError> {
        self.check_revision(expected)?;
        if dojos.is_empty() {
            return Err(TopologyError::EmptyDojoSet);
        }
        let name = normalized_name(&name.into())?;
        if self.lairs.values().any(|lair| lair.name == name) {
            return Err(TopologyError::DuplicateLairName(name));
        }
        self.validate_new_dojos(&dojos)?;
        let lair_id = LairId::new();
        self.lairs.insert(
            lair_id,
            Lair {
                id: lair_id,
                name,
                lifetime: LairLifetime::Persistent,
                dojos,
            },
        );
        self.advance_revision();
        Ok((lair_id, self.revision))
    }

    pub fn close_dojo_at(
        &mut self,
        expected: TopologyRevision,
        dojo_id: DojoId,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        let lair = self
            .lairs
            .values_mut()
            .find(|lair| lair.dojos.iter().any(|dojo| dojo.id == dojo_id))
            .ok_or(TopologyError::DojoNotFound(dojo_id))?;
        let index = lair
            .dojos
            .iter()
            .position(|dojo| dojo.id == dojo_id)
            .ok_or(TopologyError::DojoNotFound(dojo_id))?;
        if !lair.dojos[index].root.all_exited() {
            return Err(TopologyError::DojoStillLive(dojo_id));
        }
        lair.dojos.remove(index);
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn terminate_lair_at(
        &mut self,
        expected: TopologyRevision,
        lair_id: LairId,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        self.lairs
            .remove(&lair_id)
            .ok_or(TopologyError::LairNotFound(lair_id))?;
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn rename_lair_at(
        &mut self,
        expected: TopologyRevision,
        lair_id: LairId,
        name: impl Into<String>,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        let name = normalized_name(&name.into())?;
        if self
            .lairs
            .values()
            .any(|lair| lair.id != lair_id && lair.name == name)
        {
            return Err(TopologyError::DuplicateLairName(name));
        }
        self.lairs
            .get_mut(&lair_id)
            .ok_or(TopologyError::LairNotFound(lair_id))?
            .name = name;
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn rename_dojo_at(
        &mut self,
        expected: TopologyRevision,
        dojo_id: DojoId,
        name: impl Into<String>,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        let name = normalized_name(&name.into())?;
        self.find_dojo_mut(dojo_id)
            .ok_or(TopologyError::DojoNotFound(dojo_id))?
            .name = name;
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn set_dojo_default_focus_at(
        &mut self,
        expected: TopologyRevision,
        dojo_id: DojoId,
        splint_id: SplintId,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        let dojo = self
            .find_dojo_mut(dojo_id)
            .ok_or(TopologyError::DojoNotFound(dojo_id))?;
        if dojo.root.find_splint(splint_id).is_none() {
            return Err(TopologyError::InvalidDojoDefaultFocus { dojo_id, splint_id });
        }
        dojo.default_focus = splint_id;
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn rename_splint_at(
        &mut self,
        expected: TopologyRevision,
        splint_id: SplintId,
        title: impl Into<String>,
    ) -> Result<TopologyRevision, TopologyError> {
        self.check_revision(expected)?;
        let title = normalized_name(&title.into())?;
        self.find_splint_mut(splint_id)
            .ok_or(TopologyError::SplintNotFound(splint_id))?
            .title = title;
        self.advance_revision();
        Ok(self.revision)
    }

    /// Replaces launch metadata and marks an exited Splint running without changing topology.
    pub fn commit_relaunch(
        &mut self,
        id: SplintId,
        cwd: PathBuf,
        command: Vec<String>,
    ) -> Result<(), TopologyError> {
        let splint = self
            .find_splint_mut(id)
            .ok_or(TopologyError::SplintNotFound(id))?;
        if !matches!(splint.state, SplintState::Exited(_)) {
            return Err(TopologyError::SplintStillLive(id));
        }
        splint.cwd = cwd;
        splint.command = command;
        splint.state = SplintState::Running;
        Ok(())
    }

    pub fn set_splint_launch_metadata(
        &mut self,
        id: SplintId,
        launch: crate::SplintLaunchMetadata,
    ) -> bool {
        self.find_splint_mut(id).is_some_and(|splint| {
            *splint.launch = launch;
            true
        })
    }

    pub fn set_splint_last_incarnation(&mut self, id: SplintId, incarnation: u64) -> bool {
        self.find_splint_mut(id).is_some_and(|splint| {
            splint.last_incarnation = Some(incarnation);
            true
        })
    }

    pub fn set_splint_dimensions(&mut self, id: SplintId, columns: u16, rows: u16) -> bool {
        self.find_splint_mut(id).is_some_and(|splint| {
            let changed = splint.launch.columns != columns || splint.launch.rows != rows;
            splint.launch.columns = columns;
            splint.launch.rows = rows;
            changed
        })
    }

    pub fn set_splint_state(&mut self, id: SplintId, state: SplintState) -> bool {
        self.find_splint_mut(id).is_some_and(|splint| {
            splint.state = state;
            true
        })
    }

    #[must_use]
    pub fn revision(&self) -> TopologyRevision {
        self.revision
    }

    #[must_use]
    pub fn find_lair(&self, id: LairId) -> Option<&Lair> {
        self.lairs.get(&id)
    }

    #[must_use]
    pub fn find_dojo(&self, id: DojoId) -> Option<&Dojo> {
        self.lairs
            .values()
            .flat_map(|lair| &lair.dojos)
            .find(|dojo| dojo.id == id)
    }

    #[must_use]
    pub fn find_splint(&self, id: SplintId) -> Option<&Splint> {
        self.lairs
            .values()
            .flat_map(|lair| &lair.dojos)
            .find_map(|dojo| dojo.root.find_splint(id))
    }

    fn find_dojo_mut(&mut self, id: DojoId) -> Option<&mut Dojo> {
        self.lairs
            .values_mut()
            .flat_map(|lair| &mut lair.dojos)
            .find(|dojo| dojo.id == id)
    }

    fn find_splint_mut(&mut self, id: SplintId) -> Option<&mut Splint> {
        self.lairs
            .values_mut()
            .flat_map(|lair| &mut lair.dojos)
            .find_map(|dojo| dojo.root.find_splint_mut(id))
    }

    #[must_use]
    pub fn lairs(&self) -> impl ExactSizeIterator<Item = &Lair> {
        self.lairs.values()
    }

    fn validate_new_dojos(&self, dojos: &[Dojo]) -> Result<(), TopologyError> {
        let mut dojo_ids = HashSet::new();
        let mut splint_ids = HashSet::new();
        for dojo in dojos {
            validate_name(&dojo.name)?;
            if self.find_dojo(dojo.id).is_some() || !dojo_ids.insert(dojo.id) {
                return Err(TopologyError::DuplicateDojoId(dojo.id));
            }
            if dojo.root.find_splint(dojo.default_focus).is_none() {
                return Err(TopologyError::InvalidDojoDefaultFocus {
                    dojo_id: dojo.id,
                    splint_id: dojo.default_focus,
                });
            }
            collect_new_splint_ids(self, &dojo.root, &mut splint_ids)?;
        }
        Ok(())
    }

    fn check_revision(&self, expected: TopologyRevision) -> Result<(), TopologyError> {
        if expected != self.revision {
            return Err(TopologyError::StaleTopology {
                expected,
                current: self.revision,
            });
        }
        Ok(())
    }

    fn advance_revision(&mut self) {
        self.revision.0 = self
            .revision
            .0
            .checked_add(1)
            .expect("topology revision exhausted");
    }
}

fn normalized_name(value: &str) -> Result<String, TopologyError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(TopologyError::EmptyName);
    }
    if trimmed.len() > MAX_NAME_BYTES {
        return Err(TopologyError::NameTooLong);
    }
    Ok(trimmed.to_owned())
}

fn validate_name(value: &str) -> Result<(), TopologyError> {
    normalized_name(value).map(|_| ())
}

fn collect_new_splint_ids(
    topology: &Topology,
    node: &LayoutNode,
    ids: &mut HashSet<SplintId>,
) -> Result<(), TopologyError> {
    match node {
        LayoutNode::Leaf(splint) => {
            if topology.find_splint(splint.id).is_some() || !ids.insert(splint.id) {
                return Err(TopologyError::DuplicateSplintId(splint.id));
            }
        }
        LayoutNode::Branch { first, second, .. } => {
            collect_new_splint_ids(topology, first, ids)?;
            collect_new_splint_ids(topology, second, ids)?;
        }
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TopologyError {
    #[error("name cannot be empty")]
    EmptyName,
    #[error("name cannot exceed 128 UTF-8 bytes")]
    NameTooLong,
    #[error("a Lair named '{0}' already exists")]
    DuplicateLairName(String),
    #[error("Lair {0:?} does not exist")]
    LairNotFound(LairId),
    #[error("Lair {0:?} already exists")]
    DuplicateLairId(LairId),
    #[error("Dojo {0:?} does not exist")]
    DojoNotFound(DojoId),
    #[error("preset materialization requires at least one Dojo")]
    EmptyDojoSet,
    #[error("Dojo {0:?} already exists")]
    DuplicateDojoId(DojoId),
    #[error("Dojo {0:?} still contains a live Splint")]
    DojoStillLive(DojoId),
    #[error("Dojo {dojo_id:?} default focus references missing Splint {splint_id:?}")]
    InvalidDojoDefaultFocus {
        dojo_id: DojoId,
        splint_id: SplintId,
    },
    #[error("Splint {0:?} does not exist")]
    SplintNotFound(SplintId),
    #[error("Splint {0:?} already exists")]
    DuplicateSplintId(SplintId),
    #[error("Splint {0:?} is still live")]
    SplintStillLive(SplintId),
    #[error("Splint {0:?} has no parent split")]
    SplintHasNoParent(SplintId),
    #[error("topology revision is stale: expected {expected:?}, current {current:?}")]
    StaleTopology {
        expected: TopologyRevision,
        current: TopologyRevision,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_state_can_transition_and_failed_creation_can_roll_back() {
        let mut topology = Topology::new();
        let lair = topology
            .create_lair("main", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let splint_id = lair.dojos[0].root.find_splint_id_for_test();
        assert!(topology.set_splint_state(splint_id, SplintState::Running));
        assert_eq!(topology.remove_lair(lair.id).unwrap().id, lair.id);
        assert_eq!(topology.lairs().count(), 0);
    }

    #[test]
    fn atomic_lair_termination_checks_revision_and_removes_exact_lair() {
        let mut topology = Topology::new();
        let lair_id = topology
            .create_lair("main", PathBuf::from("/tmp"))
            .unwrap()
            .id;
        let revision = topology.revision();
        assert_eq!(
            topology.terminate_lair_at(revision, lair_id).unwrap(),
            TopologyRevision(revision.0 + 1)
        );
        assert!(topology.lairs().all(|lair| lair.id != lair_id));
        assert!(matches!(
            topology.terminate_lair_at(revision, lair_id),
            Err(TopologyError::StaleTopology { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_lair_names_but_allows_duplicate_dojo_names() {
        let mut topology = Topology::new();
        let first = topology
            .create_lair(" main ", PathBuf::from("/tmp"))
            .unwrap()
            .id;
        assert_eq!(
            topology.create_lair("main", PathBuf::from("/tmp")),
            Err(TopologyError::DuplicateLairName("main".into()))
        );
        assert_eq!(
            topology.create_lair("x".repeat(129), PathBuf::from("/tmp")),
            Err(TopologyError::NameTooLong)
        );

        let duplicate_name = Dojo::with_shell("terminal", PathBuf::from("/var/tmp"));
        topology
            .new_dojo_at(topology.revision(), first, duplicate_name)
            .unwrap();
        assert_eq!(topology.find_lair(first).unwrap().dojos.len(), 2);
    }

    #[test]
    fn preset_materialization_commits_complete_dojo_sets_once() {
        let mut topology = Topology::new();
        let lair_id = topology
            .create_lair("main", PathBuf::from("/tmp"))
            .unwrap()
            .id;
        let before = topology.revision();
        let first = Dojo::with_shell("first", PathBuf::from("/tmp"));
        let first_id = first.id;
        let second = Dojo::with_shell("second", PathBuf::from("/var/tmp"));
        let second_id = second.id;
        assert_eq!(
            topology
                .materialize_dojos_at(before, lair_id, Some("renamed".into()), vec![first, second],)
                .unwrap()
                .get(),
            before.get() + 1
        );
        let lair = topology.find_lair(lair_id).unwrap();
        assert_eq!(lair.name, "renamed");
        assert!(lair.dojos.iter().any(|dojo| dojo.id == first_id));
        assert!(lair.dojos.iter().any(|dojo| dojo.id == second_id));

        let unchanged = topology.clone();
        let duplicate = unchanged.find_dojo(first_id).unwrap().clone();
        assert_eq!(
            topology.materialize_dojos_at(topology.revision(), lair_id, None, vec![duplicate]),
            Err(TopologyError::DuplicateDojoId(first_id))
        );
        assert_eq!(topology, unchanged);

        let revision = topology.revision();
        let dojo = Dojo::with_shell("new", PathBuf::from("/tmp"));
        let dojo_id = dojo.id;
        let (new_lair_id, committed) = topology
            .materialize_lair_at(revision, "new-lair", vec![dojo])
            .unwrap();
        assert_eq!(committed.get(), revision.get() + 1);
        assert_eq!(
            topology.find_lair(new_lair_id).unwrap().dojos[0].id,
            dojo_id
        );
    }

    #[test]
    fn split_and_close_preserve_ids_and_commit_once() {
        let mut topology = Topology::new();
        let lair_id = topology
            .create_lair("main", PathBuf::from("/tmp"))
            .unwrap()
            .id;
        let dojo_id = topology.lairs[&lair_id].dojos[0].id;
        let target_id = topology.lairs[&lair_id].dojos[0]
            .root
            .find_splint_id_for_test();
        let inserted = Splint::shell(PathBuf::from("/var/tmp"));
        let inserted_id = inserted.id;

        assert_eq!(
            topology
                .split_splint(
                    target_id,
                    inserted,
                    Axis::Vertical,
                    SplitSide::First,
                    SplitRatio::new(400).unwrap(),
                )
                .unwrap()
                .get(),
            2
        );
        let LayoutNode::Branch {
            axis,
            ratio,
            first,
            second,
        } = &topology.lairs[&lair_id].dojos[0].root
        else {
            panic!("split did not create a branch");
        };
        assert_eq!((*axis, ratio.get()), (Axis::Vertical, 400));
        assert_eq!(first.find_splint(inserted_id).unwrap().id, inserted_id);
        assert_eq!(second.find_splint(target_id).unwrap().id, target_id);

        let unchanged = topology.clone();
        assert_eq!(
            topology.close_splint(target_id),
            Err(TopologyError::SplintStillLive(target_id))
        );
        assert_eq!(topology, unchanged);

        assert!(topology.set_splint_state(target_id, SplintState::Exited(0)));
        assert_eq!(topology.close_splint(target_id).unwrap().get(), 3);
        let dojo = topology.find_dojo(dojo_id).unwrap();
        assert_eq!(dojo.root.find_splint(inserted_id).unwrap().id, inserted_id);
        assert_eq!(dojo.default_focus, inserted_id);

        assert!(topology.set_splint_state(inserted_id, SplintState::Exited(0)));
        assert_eq!(topology.close_splint(inserted_id).unwrap().get(), 4);
        assert!(topology.find_lair(lair_id).unwrap().dojos.is_empty());
        assert_eq!(topology.lairs().count(), 1);
    }

    #[test]
    fn relaunch_updates_exited_leaf_without_advancing_topology() {
        let mut topology = Topology::new();
        let lair = topology
            .create_lair("main", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let id = lair.dojos[0].root.find_splint_id_for_test();
        let unchanged = topology.clone();
        assert_eq!(
            topology.commit_relaunch(id, PathBuf::from("/var/tmp"), vec!["echo".into()]),
            Err(TopologyError::SplintStillLive(id))
        );
        assert_eq!(topology, unchanged);

        assert!(topology.set_splint_state(id, SplintState::Exited(0)));
        let revision = topology.revision();
        topology
            .commit_relaunch(
                id,
                PathBuf::from("/var/tmp"),
                vec!["printf".into(), "ready".into()],
            )
            .unwrap();
        let splint = topology.find_splint(id).unwrap();
        assert_eq!(splint.cwd, PathBuf::from("/var/tmp"));
        assert_eq!(splint.command, vec!["printf", "ready"]);
        assert_eq!(splint.state, SplintState::Running);
        assert_eq!(topology.revision(), revision);
    }

    #[test]
    fn stale_revision_and_complete_tree_edits_are_transactional() {
        let mut topology = Topology::new();
        let lair = topology
            .create_lair("main", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let lair_id = lair.id;
        let dojo_id = lair.dojos[0].id;
        let first_id = lair.dojos[0].root.find_splint_id_for_test();
        let base = topology.revision();
        let second = Splint::shell(PathBuf::from("/tmp"));
        let second_id = second.id;
        topology
            .split_splint_at(
                base,
                first_id,
                second,
                Axis::Horizontal,
                SplitSide::Second,
                SplitRatio::new(500).unwrap(),
            )
            .unwrap();
        let before = topology.clone();
        assert_eq!(
            topology.rename_splint_at(base, first_id, "stale"),
            Err(TopologyError::StaleTopology {
                expected: base,
                current: before.revision(),
            })
        );
        assert_eq!(topology, before);

        let revision = topology
            .set_split_ratio_at(
                topology.revision(),
                second_id,
                0,
                SplitRatio::new(650).unwrap(),
            )
            .unwrap();
        topology
            .rename_lair_at(revision, lair_id, "renamed")
            .unwrap();
        topology
            .rename_dojo_at(topology.revision(), dojo_id, "work")
            .unwrap();
        topology
            .set_dojo_default_focus_at(topology.revision(), dojo_id, second_id)
            .unwrap();
        assert_eq!(
            topology.find_dojo(dojo_id).unwrap().default_focus,
            second_id
        );
        let before = topology.clone();
        assert!(matches!(
            topology.set_dojo_default_focus_at(topology.revision(), dojo_id, SplintId::new()),
            Err(TopologyError::InvalidDojoDefaultFocus { .. })
        ));
        assert_eq!(topology, before);

        let extra = Dojo::with_shell("extra", PathBuf::from("/var/tmp"));
        let extra_id = extra.id;
        let extra_splint = extra.root.find_splint_id_for_test();
        topology
            .new_dojo_at(topology.revision(), lair_id, extra)
            .unwrap();
        assert_eq!(
            topology.close_dojo_at(topology.revision(), extra_id),
            Err(TopologyError::DojoStillLive(extra_id))
        );
        assert!(topology.set_splint_state(extra_splint, SplintState::Exited(0)));
        topology
            .close_dojo_at(topology.revision(), extra_id)
            .unwrap();
        assert!(topology.find_dojo(extra_id).is_none());
        assert_unique_valid_tree(&topology);
    }

    #[test]
    fn deterministic_edit_sequence_preserves_tree_invariants() {
        let mut topology = Topology::new();
        let lair = topology
            .create_lair("random", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let mut ids = vec![lair.dojos[0].root.find_splint_id_for_test()];
        let mut seed = 0x5eed_u64;
        for index in 0..64 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let target = ids[usize::try_from(seed).unwrap() % ids.len()];
            let splint = Splint::shell(PathBuf::from("/tmp"));
            ids.push(splint.id);
            let ratio = SplitRatio::new(u16::try_from(seed % 999 + 1).unwrap()).unwrap();
            topology
                .split_splint_at(
                    topology.revision(),
                    target,
                    splint,
                    if seed & 1 == 0 {
                        Axis::Horizontal
                    } else {
                        Axis::Vertical
                    },
                    if seed & 2 == 0 {
                        SplitSide::First
                    } else {
                        SplitSide::Second
                    },
                    ratio,
                )
                .unwrap();
            topology
                .rename_splint_at(topology.revision(), target, format!("pane-{index}"))
                .unwrap();
            assert_unique_valid_tree(&topology);
        }
    }

    #[test]
    fn failed_split_rolls_back_completely() {
        let mut topology = Topology::new();
        topology.create_lair("main", PathBuf::from("/tmp")).unwrap();
        let before = topology.clone();
        let missing = SplintId::new();

        assert_eq!(
            topology.split_splint(
                missing,
                Splint::shell(PathBuf::from("/tmp")),
                Axis::Horizontal,
                SplitSide::Second,
                SplitRatio::new(500).unwrap(),
            ),
            Err(TopologyError::SplintNotFound(missing))
        );
        assert_eq!(topology, before);
    }

    fn assert_unique_valid_tree(topology: &Topology) {
        fn visit(node: &LayoutNode, ids: &mut HashSet<SplintId>) {
            match node {
                LayoutNode::Leaf(splint) => assert!(ids.insert(splint.id)),
                LayoutNode::Branch {
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    assert!((SplitRatio::MIN..=SplitRatio::MAX).contains(&ratio.get()));
                    visit(first, ids);
                    visit(second, ids);
                }
            }
        }
        let mut lair_ids = HashSet::new();
        let mut dojo_ids = HashSet::new();
        let mut splint_ids = HashSet::new();
        for lair in topology.lairs() {
            assert!(lair_ids.insert(lair.id));
            for dojo in &lair.dojos {
                assert!(dojo_ids.insert(dojo.id));
                assert!(dojo.root.find_splint(dojo.default_focus).is_some());
                visit(&dojo.root, &mut splint_ids);
            }
        }
    }

    trait TestLayoutExt {
        fn find_splint_id_for_test(&self) -> SplintId;
    }

    impl TestLayoutExt for LayoutNode {
        fn find_splint_id_for_test(&self) -> SplintId {
            match self {
                Self::Leaf(splint) => splint.id,
                Self::Branch { first, .. } => first.find_splint_id_for_test(),
            }
        }
    }
}
