use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::Window;

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
    fn rejects_duplicate_dojo_names() {
        let mut lair = Lair::new();
        lair.create_dojo("main", PathBuf::from("/tmp")).unwrap();

        assert_eq!(
            lair.create_dojo("main", PathBuf::from("/tmp")),
            Err(LairError::DuplicateName("main".into()))
        );
    }
}
