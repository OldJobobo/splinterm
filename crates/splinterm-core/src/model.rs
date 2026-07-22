use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Axis, Splint, SplintId, SplintState, SplitRatio, SplitSide, Window, WindowId};

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
pub struct DojoId(Uuid);

impl DojoId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
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

/// A persistent workspace containing windows and splints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dojo {
    pub id: DojoId,
    pub name: String,
    pub windows: Vec<Window>,
}

impl Dojo {
    #[must_use]
    pub fn new(name: impl Into<String>, cwd: PathBuf) -> Self {
        Self {
            id: DojoId::new(),
            name: name.into(),
            windows: vec![Window::with_shell(cwd)],
        }
    }
}

/// The daemon-owned collection of persistent dojos.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lair {
    pub(crate) revision: TopologyRevision,
    pub(crate) dojos: BTreeMap<DojoId, Dojo>,
}

#[allow(
    clippy::missing_errors_doc,
    reason = "transaction errors are exhaustively represented by LairError"
)]
impl Lair {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            revision: TopologyRevision(0),
            dojos: BTreeMap::new(),
        }
    }

    /// Adds a named dojo with one initial shell window.
    ///
    /// # Errors
    ///
    /// Returns [`LairError::EmptyName`] for a blank name or
    /// [`LairError::DuplicateName`] when that name is already in use.
    pub fn create_dojo(
        &mut self,
        name: impl Into<String>,
        cwd: PathBuf,
    ) -> Result<&Dojo, LairError> {
        self.create_dojo_at(self.revision, name, cwd)
    }

    pub fn create_dojo_at(
        &mut self,
        expected: TopologyRevision,
        name: impl Into<String>,
        cwd: PathBuf,
    ) -> Result<&Dojo, LairError> {
        self.check_revision(expected)?;
        let name = name.into();
        let name = name.trim();
        if name.is_empty() {
            return Err(LairError::EmptyName);
        }
        if name.len() > MAX_NAME_BYTES {
            return Err(LairError::NameTooLong);
        }
        if self.dojos.values().any(|dojo| dojo.name == name) {
            return Err(LairError::DuplicateName(name.into()));
        }

        let dojo = Dojo::new(name, cwd);
        let id = dojo.id;
        self.insert_dojo_at(expected, dojo)?;
        Ok(&self.dojos[&id])
    }

    pub fn insert_dojo_at(
        &mut self,
        expected: TopologyRevision,
        dojo: Dojo,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        validate_name(&dojo.name)?;
        if self.dojos.contains_key(&dojo.id) {
            return Err(LairError::DuplicateDojoId(dojo.id));
        }
        if self.dojos.values().any(|current| current.name == dojo.name) {
            return Err(LairError::DuplicateName(dojo.name));
        }
        self.dojos.insert(dojo.id, dojo);
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn remove_dojo(&mut self, id: DojoId) -> Option<Dojo> {
        let removed = self.dojos.remove(&id);
        if removed.is_some() {
            self.advance_revision();
        }
        removed
    }

    /// Inserts a new leaf beside `target` and commits one topology revision.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the Lair when either identity is invalid.
    pub fn split_splint(
        &mut self,
        target: SplintId,
        new_splint: Splint,
        axis: Axis,
        side: SplitSide,
        ratio: SplitRatio,
    ) -> Result<TopologyRevision, LairError> {
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
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        if self.find_splint(new_splint.id).is_some() {
            return Err(LairError::DuplicateSplintId(new_splint.id));
        }

        let window = self
            .dojos
            .values_mut()
            .flat_map(|dojo| &mut dojo.windows)
            .find(|window| window.root.find_splint(target).is_some())
            .ok_or(LairError::SplintNotFound(target))?;
        if !window.root.split(target, new_splint, axis, side, ratio) {
            return Err(LairError::SplintNotFound(target));
        }

        self.advance_revision();
        Ok(self.revision)
    }

    /// Removes an exited leaf, collapsing its parent or removing its final window.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the Lair when the Splint is missing or live.
    pub fn close_splint(&mut self, id: SplintId) -> Result<TopologyRevision, LairError> {
        self.close_splint_at(self.revision, id)
    }

    pub fn close_splint_at(
        &mut self,
        expected: TopologyRevision,
        id: SplintId,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        let splint = self.find_splint(id).ok_or(LairError::SplintNotFound(id))?;
        if !matches!(splint.state, SplintState::Exited(_)) {
            return Err(LairError::SplintStillLive(id));
        }

        'dojos: for dojo in self.dojos.values_mut() {
            for window_index in 0..dojo.windows.len() {
                if dojo.windows[window_index].root.find_splint(id).is_none() {
                    continue;
                }
                let root = dojo.windows[window_index].root.clone();
                match root.remove(id) {
                    Ok(Some(root)) => {
                        let window = &mut dojo.windows[window_index];
                        window.root = root;
                        if window.default_focus == id {
                            window.default_focus = window.root.first_splint_id();
                        }
                    }
                    Ok(None) => {
                        dojo.windows.remove(window_index);
                    }
                    Err(_) => return Err(LairError::SplintNotFound(id)),
                }
                break 'dojos;
            }
        }

        self.advance_revision();
        Ok(self.revision)
    }

    pub fn set_split_ratio_at(
        &mut self,
        expected: TopologyRevision,
        target: SplintId,
        ratio: SplitRatio,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        let window = self
            .dojos
            .values_mut()
            .flat_map(|dojo| &mut dojo.windows)
            .find(|window| window.root.find_splint(target).is_some())
            .ok_or(LairError::SplintNotFound(target))?;
        if !window.root.set_parent_ratio(target, ratio) {
            return Err(LairError::SplintHasNoParent(target));
        }
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn new_window_at(
        &mut self,
        expected: TopologyRevision,
        dojo_id: DojoId,
        window: Window,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        if self.find_window(window.id).is_some() {
            return Err(LairError::DuplicateWindowId(window.id));
        }
        if let Some(id) = existing_splint_id(self, &window.root) {
            return Err(LairError::DuplicateSplintId(id));
        }
        if window.root.find_splint(window.default_focus).is_none() {
            return Err(LairError::InvalidWindowDefaultFocus {
                window_id: window.id,
                splint_id: window.default_focus,
            });
        }
        validate_name(&window.title)?;
        self.dojos
            .get_mut(&dojo_id)
            .ok_or(LairError::DojoNotFound(dojo_id))?
            .windows
            .push(window);
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn close_window_at(
        &mut self,
        expected: TopologyRevision,
        window_id: WindowId,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        let dojo = self
            .dojos
            .values_mut()
            .find(|dojo| dojo.windows.iter().any(|window| window.id == window_id))
            .ok_or(LairError::WindowNotFound(window_id))?;
        let index = dojo
            .windows
            .iter()
            .position(|window| window.id == window_id)
            .ok_or(LairError::WindowNotFound(window_id))?;
        if !dojo.windows[index].root.all_exited() {
            return Err(LairError::WindowStillLive(window_id));
        }
        dojo.windows.remove(index);
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn rename_dojo_at(
        &mut self,
        expected: TopologyRevision,
        dojo_id: DojoId,
        name: impl Into<String>,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        let name = normalized_name(&name.into())?;
        if self
            .dojos
            .values()
            .any(|dojo| dojo.id != dojo_id && dojo.name == name)
        {
            return Err(LairError::DuplicateName(name));
        }
        self.dojos
            .get_mut(&dojo_id)
            .ok_or(LairError::DojoNotFound(dojo_id))?
            .name = name;
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn rename_window_at(
        &mut self,
        expected: TopologyRevision,
        window_id: WindowId,
        title: impl Into<String>,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        let title = normalized_name(&title.into())?;
        self.find_window_mut(window_id)
            .ok_or(LairError::WindowNotFound(window_id))?
            .title = title;
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn set_window_default_focus_at(
        &mut self,
        expected: TopologyRevision,
        window_id: WindowId,
        splint_id: SplintId,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        let window = self
            .find_window_mut(window_id)
            .ok_or(LairError::WindowNotFound(window_id))?;
        if window.root.find_splint(splint_id).is_none() {
            return Err(LairError::InvalidWindowDefaultFocus {
                window_id,
                splint_id,
            });
        }
        window.default_focus = splint_id;
        self.advance_revision();
        Ok(self.revision)
    }

    pub fn rename_splint_at(
        &mut self,
        expected: TopologyRevision,
        splint_id: SplintId,
        title: impl Into<String>,
    ) -> Result<TopologyRevision, LairError> {
        self.check_revision(expected)?;
        let title = normalized_name(&title.into())?;
        self.find_splint_mut(splint_id)
            .ok_or(LairError::SplintNotFound(splint_id))?
            .title = title;
        self.advance_revision();
        Ok(self.revision)
    }

    /// Replaces launch metadata and marks an exited Splint running without changing topology.
    ///
    /// # Errors
    ///
    /// Returns an error without mutation when the Splint is missing or still live.
    pub fn commit_relaunch(
        &mut self,
        id: SplintId,
        cwd: PathBuf,
        command: Vec<String>,
    ) -> Result<(), LairError> {
        let splint = self
            .find_splint_mut(id)
            .ok_or(LairError::SplintNotFound(id))?;
        if !matches!(splint.state, SplintState::Exited(_)) {
            return Err(LairError::SplintStillLive(id));
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
            splint.launch = Box::new(launch);
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
    pub fn find_window(&self, id: WindowId) -> Option<&Window> {
        self.dojos
            .values()
            .flat_map(|dojo| &dojo.windows)
            .find(|window| window.id == id)
    }

    #[must_use]
    pub fn find_splint(&self, id: SplintId) -> Option<&Splint> {
        self.dojos
            .values()
            .flat_map(|dojo| &dojo.windows)
            .find_map(|window| window.root.find_splint(id))
    }

    fn find_window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.dojos
            .values_mut()
            .flat_map(|dojo| &mut dojo.windows)
            .find(|window| window.id == id)
    }

    fn find_splint_mut(&mut self, id: SplintId) -> Option<&mut Splint> {
        self.dojos
            .values_mut()
            .flat_map(|dojo| &mut dojo.windows)
            .find_map(|window| window.root.find_splint_mut(id))
    }

    #[must_use]
    pub fn dojos(&self) -> impl ExactSizeIterator<Item = &Dojo> {
        self.dojos.values()
    }

    fn check_revision(&self, expected: TopologyRevision) -> Result<(), LairError> {
        if expected != self.revision {
            return Err(LairError::StaleTopology {
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

fn normalized_name(value: &str) -> Result<String, LairError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(LairError::EmptyName);
    }
    if trimmed.len() > MAX_NAME_BYTES {
        return Err(LairError::NameTooLong);
    }
    Ok(trimmed.to_owned())
}

fn validate_name(value: &str) -> Result<(), LairError> {
    normalized_name(value).map(|_| ())
}

fn existing_splint_id(lair: &Lair, node: &crate::LayoutNode) -> Option<SplintId> {
    match node {
        crate::LayoutNode::Leaf(splint) => lair.find_splint(splint.id).map(|_| splint.id),
        crate::LayoutNode::Branch { first, second, .. } => {
            existing_splint_id(lair, first).or_else(|| existing_splint_id(lair, second))
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LairError {
    #[error("dojo name cannot be empty")]
    EmptyName,
    #[error("dojo name cannot exceed 128 UTF-8 bytes")]
    NameTooLong,
    #[error("a dojo named '{0}' already exists")]
    DuplicateName(String),
    #[error("Splint {0:?} does not exist")]
    SplintNotFound(SplintId),
    #[error("Splint {0:?} already exists")]
    DuplicateSplintId(SplintId),
    #[error("Splint {0:?} is still live")]
    SplintStillLive(SplintId),
    #[error("Splint {0:?} has no parent split")]
    SplintHasNoParent(SplintId),
    #[error("dojo {0:?} does not exist")]
    DojoNotFound(DojoId),
    #[error("dojo {0:?} already exists")]
    DuplicateDojoId(DojoId),
    #[error("window {0:?} does not exist")]
    WindowNotFound(WindowId),
    #[error("window {0:?} already exists")]
    DuplicateWindowId(WindowId),
    #[error("window {0:?} still contains a live Splint")]
    WindowStillLive(WindowId),
    #[error("window {window_id:?} default focus references missing Splint {splint_id:?}")]
    InvalidWindowDefaultFocus {
        window_id: WindowId,
        splint_id: SplintId,
    },
    #[error("topology revision is stale: expected {expected:?}, current {current:?}")]
    StaleTopology {
        expected: TopologyRevision,
        current: TopologyRevision,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LayoutNode;

    #[test]
    fn runtime_state_can_transition_and_failed_creation_can_roll_back() {
        let mut lair = Lair::new();
        let dojo = lair
            .create_dojo("main", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let splint_id = match &dojo.windows[0].root {
            crate::LayoutNode::Leaf(splint) => splint.id,
            crate::LayoutNode::Branch { .. } => unreachable!(),
        };
        assert!(lair.set_splint_state(splint_id, SplintState::Running));
        assert_eq!(lair.remove_dojo(dojo.id).unwrap().id, dojo.id);
        assert_eq!(lair.dojos().count(), 0);
    }

    #[test]
    fn rejects_duplicate_dojo_names() {
        let mut lair = Lair::new();
        lair.create_dojo(" main ", PathBuf::from("/tmp")).unwrap();

        assert_eq!(
            lair.create_dojo("main", PathBuf::from("/tmp")),
            Err(LairError::DuplicateName("main".into()))
        );
        assert_eq!(
            lair.create_dojo("x".repeat(129), PathBuf::from("/tmp")),
            Err(LairError::NameTooLong)
        );
    }

    #[test]
    fn split_and_close_preserve_ids_and_commit_once() {
        let mut lair = Lair::new();
        let dojo_id = lair.create_dojo("main", PathBuf::from("/tmp")).unwrap().id;
        let target_id = lair.dojos[&dojo_id].windows[0]
            .root
            .find_splint_id_for_test();
        let inserted = Splint::shell(PathBuf::from("/var/tmp"));
        let inserted_id = inserted.id;

        assert_eq!(
            lair.split_splint(
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
        } = &lair.dojos[&dojo_id].windows[0].root
        else {
            panic!("split did not create a branch");
        };
        assert_eq!((*axis, ratio.get()), (Axis::Vertical, 400));
        assert_eq!(first.find_splint(inserted_id).unwrap().id, inserted_id);
        assert_eq!(second.find_splint(target_id).unwrap().id, target_id);

        let unchanged = lair.clone();
        assert_eq!(
            lair.close_splint(target_id),
            Err(LairError::SplintStillLive(target_id))
        );
        assert_eq!(lair, unchanged);

        assert!(lair.set_splint_state(target_id, SplintState::Exited(0)));
        assert_eq!(lair.close_splint(target_id).unwrap().get(), 3);
        assert_eq!(
            lair.dojos[&dojo_id].windows[0]
                .root
                .find_splint(inserted_id)
                .unwrap()
                .id,
            inserted_id
        );

        assert_eq!(lair.dojos[&dojo_id].windows[0].default_focus, inserted_id);
        assert!(lair.set_splint_state(inserted_id, SplintState::Exited(0)));
        assert_eq!(lair.close_splint(inserted_id).unwrap().get(), 4);
        assert!(lair.dojos[&dojo_id].windows.is_empty());
        assert_eq!(lair.dojos().count(), 1);
    }

    #[test]
    fn relaunch_updates_exited_leaf_without_advancing_topology() {
        let mut lair = Lair::new();
        let dojo = lair
            .create_dojo("main", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let id = dojo.windows[0].root.find_splint_id_for_test();
        let unchanged = lair.clone();
        assert_eq!(
            lair.commit_relaunch(id, PathBuf::from("/var/tmp"), vec!["echo".into()]),
            Err(LairError::SplintStillLive(id))
        );
        assert_eq!(lair, unchanged);

        assert!(lair.set_splint_state(id, SplintState::Exited(0)));
        let revision = lair.revision();
        lair.commit_relaunch(
            id,
            PathBuf::from("/var/tmp"),
            vec!["printf".into(), "ready".into()],
        )
        .unwrap();
        let splint = lair.find_splint(id).unwrap();
        assert_eq!(splint.id, id);
        assert_eq!(splint.cwd, PathBuf::from("/var/tmp"));
        assert_eq!(splint.command, vec!["printf", "ready"]);
        assert_eq!(splint.state, SplintState::Running);
        assert_eq!(lair.revision(), revision);
    }

    #[test]
    fn stale_revision_and_complete_tree_edits_are_transactional() {
        let mut lair = Lair::new();
        let dojo = lair
            .create_dojo("main", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let dojo_id = dojo.id;
        let window_id = dojo.windows[0].id;
        let first_id = dojo.windows[0].root.find_splint_id_for_test();
        let base = lair.revision();
        let second = Splint::shell(PathBuf::from("/tmp"));
        let second_id = second.id;
        lair.split_splint_at(
            base,
            first_id,
            second,
            Axis::Horizontal,
            SplitSide::Second,
            SplitRatio::new(500).unwrap(),
        )
        .unwrap();
        let before = lair.clone();
        assert_eq!(
            lair.rename_splint_at(base, first_id, "stale"),
            Err(LairError::StaleTopology {
                expected: base,
                current: before.revision(),
            })
        );
        assert_eq!(lair, before);

        let revision = lair
            .set_split_ratio_at(lair.revision(), second_id, SplitRatio::new(650).unwrap())
            .unwrap();
        lair.rename_dojo_at(revision, dojo_id, "renamed").unwrap();
        lair.rename_window_at(lair.revision(), window_id, "work")
            .unwrap();
        lair.set_window_default_focus_at(lair.revision(), window_id, second_id)
            .unwrap();
        assert_eq!(
            lair.find_window(window_id).unwrap().default_focus,
            second_id
        );
        let before = lair.clone();
        assert!(matches!(
            lair.set_window_default_focus_at(lair.revision(), window_id, SplintId::new()),
            Err(LairError::InvalidWindowDefaultFocus { .. })
        ));
        assert_eq!(lair, before);
        lair.rename_splint_at(lair.revision(), first_id, "editor")
            .unwrap();

        let mut extra = Window::with_shell(PathBuf::from("/var/tmp"));
        extra.title = "extra".into();
        let extra_id = extra.id;
        let extra_splint = extra.root.find_splint_id_for_test();
        assert_eq!(extra.default_focus, extra_splint);
        lair.new_window_at(lair.revision(), dojo_id, extra).unwrap();
        assert_eq!(
            lair.close_window_at(lair.revision(), extra_id),
            Err(LairError::WindowStillLive(extra_id))
        );
        assert!(lair.set_splint_state(extra_splint, SplintState::Exited(0)));
        lair.close_window_at(lair.revision(), extra_id).unwrap();
        assert!(lair.find_window(extra_id).is_none());
        assert_unique_valid_tree(&lair);
    }

    #[test]
    fn deterministic_edit_sequence_preserves_tree_invariants() {
        let mut lair = Lair::new();
        let dojo = lair
            .create_dojo("random", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let mut ids = vec![dojo.windows[0].root.find_splint_id_for_test()];
        let mut seed = 0x5eed_u64;
        for index in 0..64 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let target = ids[usize::try_from(seed).unwrap() % ids.len()];
            let splint = Splint::shell(PathBuf::from("/tmp"));
            ids.push(splint.id);
            let ratio = SplitRatio::new(u16::try_from(seed % 999 + 1).unwrap()).unwrap();
            lair.split_splint_at(
                lair.revision(),
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
            lair.rename_splint_at(lair.revision(), target, format!("pane-{index}"))
                .unwrap();
            assert_unique_valid_tree(&lair);
        }
    }

    #[test]
    fn failed_split_rolls_back_completely() {
        let mut lair = Lair::new();
        lair.create_dojo("main", PathBuf::from("/tmp")).unwrap();
        let before = lair.clone();
        let missing = SplintId::new();

        assert_eq!(
            lair.split_splint(
                missing,
                Splint::shell(PathBuf::from("/tmp")),
                Axis::Horizontal,
                SplitSide::Second,
                SplitRatio::new(500).unwrap(),
            ),
            Err(LairError::SplintNotFound(missing))
        );
        assert_eq!(lair, before);
    }

    fn assert_unique_valid_tree(lair: &Lair) {
        fn visit(node: &LayoutNode, ids: &mut std::collections::HashSet<SplintId>) {
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
        let mut dojo_ids = std::collections::HashSet::new();
        let mut window_ids = std::collections::HashSet::new();
        let mut splint_ids = std::collections::HashSet::new();
        for dojo in lair.dojos() {
            assert!(dojo_ids.insert(dojo.id));
            for window in &dojo.windows {
                assert!(window_ids.insert(window.id));
                visit(&window.root, &mut splint_ids);
            }
        }
    }

    trait TestLayoutExt {
        fn find_splint_id_for_test(&self) -> SplintId;
    }

    impl TestLayoutExt for crate::LayoutNode {
        fn find_splint_id_for_test(&self) -> SplintId {
            match self {
                Self::Leaf(splint) => splint.id,
                Self::Branch { first, .. } => first.find_splint_id_for_test(),
            }
        }
    }
}
