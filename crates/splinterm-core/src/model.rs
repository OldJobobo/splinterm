use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{SplintId, SplintState, Window};

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

/// A persistent workspace containing windows and splints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lair {
    dojos: BTreeMap<DojoId, Dojo>,
}

impl Lair {
    #[must_use]
    pub const fn new() -> Self {
        Self {
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
        let name = name.into();
        if name.trim().is_empty() {
            return Err(LairError::EmptyName);
        }
        if self.dojos.values().any(|dojo| dojo.name == name) {
            return Err(LairError::DuplicateName(name));
        }

        let dojo = Dojo::new(name, cwd);
        let id = dojo.id;
        self.dojos.insert(id, dojo);
        Ok(&self.dojos[&id])
    }

    pub fn remove_dojo(&mut self, id: DojoId) -> Option<Dojo> {
        self.dojos.remove(&id)
    }

    pub fn set_splint_state(&mut self, id: SplintId, state: SplintState) -> bool {
        for dojo in self.dojos.values_mut() {
            for window in &mut dojo.windows {
                if let Some(splint) = window.root.find_splint_mut(id) {
                    splint.state = state;
                    return true;
                }
            }
        }
        false
    }

    #[must_use]
    pub fn dojos(&self) -> impl ExactSizeIterator<Item = &Dojo> {
        self.dojos.values()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LairError {
    #[error("dojo name cannot be empty")]
    EmptyName,
    #[error("a dojo named '{0}' already exists")]
    DuplicateName(String),
}

#[cfg(test)]
mod tests {
    use super::*;

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
        lair.create_dojo("main", PathBuf::from("/tmp")).unwrap();

        assert_eq!(
            lair.create_dojo("main", PathBuf::from("/tmp")),
            Err(LairError::DuplicateName("main".into()))
        );
    }
}
