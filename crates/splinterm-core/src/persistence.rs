use std::{
    collections::{BTreeMap, HashSet},
    path::Component,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Dojo, DojoId, Lair, LayoutNode, SplintState, TopologyRevision};

pub const LAIR_SCHEMA_VERSION: u32 = 2;
pub const MAX_LAIR_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_DOJOS: usize = 64;
const MAX_WINDOWS_PER_DOJO: usize = 64;
const MAX_SPLINTS: usize = 256;
const MAX_LAYOUT_DEPTH: usize = 32;
const MAX_NAME_BYTES: usize = 128;
const MAX_ARGUMENTS: usize = 64;
const MAX_ARGUMENT_BYTES: usize = 4096;
const MAX_COMMAND_BYTES: usize = 32 * 1024;

/// Versioned metadata only; this document never represents live PTYs or processes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LairDocument {
    schema_version: u32,
    revision: TopologyRevision,
    dojos: Vec<Dojo>,
}

impl LairDocument {
    /// Creates and validates a metadata snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the model cannot be represented as safe durable metadata.
    pub fn from_lair(lair: &Lair) -> Result<Self, PersistenceError> {
        let mut dojos: Vec<_> = lair.dojos.values().cloned().collect();
        for dojo in &mut dojos {
            for window in &mut dojo.windows {
                mark_tree_restorable(&mut window.root);
            }
        }
        let document = Self {
            schema_version: LAIR_SCHEMA_VERSION,
            revision: lair.revision,
            dojos,
        };
        document.validate()?;
        Ok(document)
    }

    /// Decodes and validates a bounded metadata document.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, unsupported, unsafe, or oversized metadata.
    pub fn decode(bytes: &[u8]) -> Result<Self, PersistenceError> {
        if bytes.len() > MAX_LAIR_DOCUMENT_BYTES {
            return Err(PersistenceError::DocumentTooLarge);
        }
        let document: Self = serde_json::from_slice(bytes)
            .map_err(|error| PersistenceError::Decode(error.to_string()))?;
        document.validate()?;
        Ok(document)
    }

    /// Serializes validated metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when validation or serialization fails.
    pub fn encode(&self) -> Result<Vec<u8>, PersistenceError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| PersistenceError::Encode(error.to_string()))
    }

    /// Converts validated metadata into an in-memory Lair.
    ///
    /// # Errors
    ///
    /// Returns an error if this document has become invalid.
    pub fn into_lair(self) -> Result<Lair, PersistenceError> {
        self.validate()?;
        Ok(Lair {
            revision: self.revision,
            dojos: self.dojo_map(),
        })
    }

    fn dojo_map(self) -> BTreeMap<DojoId, Dojo> {
        self.dojos.into_iter().map(|dojo| (dojo.id, dojo)).collect()
    }

    fn validate(&self) -> Result<(), PersistenceError> {
        if self.schema_version != LAIR_SCHEMA_VERSION {
            return Err(PersistenceError::UnsupportedVersion(self.schema_version));
        }
        if self.dojos.len() > MAX_DOJOS {
            return Err(PersistenceError::CollectionTooLarge("dojos"));
        }

        let mut dojo_ids = HashSet::new();
        let mut dojo_names = HashSet::new();
        let mut window_ids = HashSet::new();
        let mut splint_ids = HashSet::new();
        let mut splint_count = 0;
        for dojo in &self.dojos {
            validate_name(&dojo.name, "dojo name")?;
            if !dojo_ids.insert(dojo.id) {
                return Err(PersistenceError::DuplicateId("dojo"));
            }
            if !dojo_names.insert(dojo.name.as_str()) {
                return Err(PersistenceError::DuplicateDojoName);
            }
            if dojo.windows.len() > MAX_WINDOWS_PER_DOJO {
                return Err(PersistenceError::CollectionTooLarge("windows"));
            }
            for window in &dojo.windows {
                validate_name(&window.title, "window title")?;
                if !window_ids.insert(window.id) {
                    return Err(PersistenceError::DuplicateId("window"));
                }
                if window.root.find_splint(window.default_focus).is_none() {
                    return Err(PersistenceError::InvalidWindowDefaultFocus);
                }
                validate_tree(&window.root, 1, &mut splint_count, &mut splint_ids)?;
            }
        }
        Ok(())
    }
}

fn mark_tree_restorable(node: &mut LayoutNode) {
    match node {
        LayoutNode::Leaf(splint) => {
            if !matches!(splint.state, SplintState::Exited(_)) {
                splint.state = SplintState::Exited(0);
            }
        }
        LayoutNode::Branch { first, second, .. } => {
            mark_tree_restorable(first);
            mark_tree_restorable(second);
        }
    }
}

fn validate_tree(
    node: &LayoutNode,
    depth: usize,
    count: &mut usize,
    ids: &mut HashSet<crate::SplintId>,
) -> Result<(), PersistenceError> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err(PersistenceError::LayoutTooDeep);
    }
    match node {
        LayoutNode::Leaf(splint) => {
            *count += 1;
            if *count > MAX_SPLINTS {
                return Err(PersistenceError::CollectionTooLarge("splints"));
            }
            if !ids.insert(splint.id) {
                return Err(PersistenceError::DuplicateId("Splint"));
            }
            validate_name(&splint.title, "Splint title")?;
            validate_cwd(&splint.cwd)?;
            validate_command(&splint.command)?;
            if splint.launch.shell.as_ref().is_some_and(|shell| {
                shell.is_empty()
                    || shell.len() > MAX_ARGUMENT_BYTES
                    || shell.as_bytes().contains(&0)
            }) || splint.launch.scrollback_lines > 1_000_000
                || !(1..=4096).contains(&splint.launch.columns)
                || !(1..=4096).contains(&splint.launch.rows)
                || splint.last_incarnation == Some(0)
            {
                return Err(PersistenceError::InvalidLaunchMetadata);
            }
            if !matches!(splint.state, SplintState::Exited(_)) {
                return Err(PersistenceError::LiveProcessState);
            }
        }
        LayoutNode::Branch { first, second, .. } => {
            validate_tree(first, depth + 1, count, ids)?;
            validate_tree(second, depth + 1, count, ids)?;
        }
    }
    Ok(())
}

fn validate_name(value: &str, kind: &'static str) -> Result<(), PersistenceError> {
    if value.trim().is_empty() || value.trim() != value || value.len() > MAX_NAME_BYTES {
        return Err(PersistenceError::InvalidName(kind));
    }
    Ok(())
}

fn validate_cwd(path: &std::path::Path) -> Result<(), PersistenceError> {
    if path.to_str().is_none()
        || !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(PersistenceError::UnsafeWorkingDirectory);
    }
    Ok(())
}

fn validate_command(command: &[String]) -> Result<(), PersistenceError> {
    if command.len() > MAX_ARGUMENTS
        || command.iter().any(|arg| {
            arg.is_empty() || arg.len() > MAX_ARGUMENT_BYTES || arg.as_bytes().contains(&0)
        })
        || command.iter().map(String::len).sum::<usize>() > MAX_COMMAND_BYTES
    {
        return Err(PersistenceError::InvalidCommand);
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PersistenceError {
    #[error("metadata document exceeds its size limit")]
    DocumentTooLarge,
    #[error("metadata schema version {0} is unsupported")]
    UnsupportedVersion(u32),
    #[error("metadata decoding failed: {0}")]
    Decode(String),
    #[error("metadata encoding failed: {0}")]
    Encode(String),
    #[error("metadata contains too many {0}")]
    CollectionTooLarge(&'static str),
    #[error("metadata contains a duplicate {0} ID")]
    DuplicateId(&'static str),
    #[error("metadata contains duplicate dojo names")]
    DuplicateDojoName,
    #[error("metadata contains an invalid {0}")]
    InvalidName(&'static str),
    #[error("metadata layout exceeds its depth limit")]
    LayoutTooDeep,
    #[error("metadata contains an unsafe working directory")]
    UnsafeWorkingDirectory,
    #[error("metadata contains invalid launch arguments")]
    InvalidCommand,
    #[error("metadata contains invalid launch policy or geometry")]
    InvalidLaunchMetadata,
    #[error("metadata window default focus does not reference its own layout")]
    InvalidWindowDefaultFocus,
    #[error("metadata claims a process is live")]
    LiveProcessState,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::*;

    fn valid_document() -> Value {
        let splint_id = Uuid::new_v4();
        json!({
            "schema_version": LAIR_SCHEMA_VERSION,
            "revision": 7,
            "dojos": [{
                "id": Uuid::new_v4(),
                "name": "main",
                "windows": [{
                    "id": Uuid::new_v4(),
                    "title": "terminal",
                    "default_focus": splint_id,
                    "root": {"Leaf": {
                        "id": splint_id,
                        "title": "shell",
                        "cwd": "/tmp",
                        "command": [],
                        "state": {"Exited": 0}
                    }}
                }]
            }]
        })
    }

    fn decode(value: &Value) -> Result<LairDocument, PersistenceError> {
        LairDocument::decode(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn live_model_is_serialized_only_as_restorable_metadata() {
        let mut lair = Lair::new();
        let dojo = lair
            .create_dojo("main", std::path::PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let LayoutNode::Leaf(created) = &dojo.windows[0].root else {
            unreachable!()
        };
        assert!(lair.set_splint_last_incarnation(created.id, 41));
        let document = LairDocument::from_lair(&lair).unwrap();
        let restored = LairDocument::decode(&document.encode().unwrap())
            .unwrap()
            .into_lair()
            .unwrap();
        let LayoutNode::Leaf(splint) = &restored.dojos().next().unwrap().windows[0].root else {
            unreachable!()
        };
        assert_eq!(splint.state, SplintState::Exited(0));
        assert_eq!(splint.last_incarnation, Some(41));
    }

    #[test]
    fn independent_window_trees_and_focus_hints_round_trip() {
        let mut lair = Lair::new();
        let dojo_id = lair
            .create_dojo("main", std::path::PathBuf::from("/tmp"))
            .unwrap()
            .id;
        let first_window = lair.dojos[&dojo_id].windows[0].id;
        let first_hint = lair.dojos[&dojo_id].windows[0].default_focus;
        let second = crate::Window::with_shell(std::path::PathBuf::from("/var/tmp"));
        let second_window = second.id;
        let second_hint = second.default_focus;
        lair.new_window_at(lair.revision(), dojo_id, second)
            .unwrap();
        let sibling = crate::Splint::shell(std::path::PathBuf::from("/tmp"));
        let sibling_id = sibling.id;
        lair.split_splint_at(
            lair.revision(),
            first_hint,
            sibling,
            crate::Axis::Horizontal,
            crate::SplitSide::Second,
            crate::SplitRatio::new(600).unwrap(),
        )
        .unwrap();
        lair.set_window_default_focus_at(lair.revision(), first_window, sibling_id)
            .unwrap();

        let restored =
            LairDocument::decode(&LairDocument::from_lair(&lair).unwrap().encode().unwrap())
                .unwrap()
                .into_lair()
                .unwrap();
        assert_eq!(
            restored
                .find_window(first_window)
                .unwrap()
                .root
                .splint_count(),
            2
        );
        assert_eq!(
            restored.find_window(first_window).unwrap().default_focus,
            sibling_id
        );
        assert_eq!(
            restored
                .find_window(second_window)
                .unwrap()
                .root
                .splint_count(),
            1
        );
        assert_eq!(
            restored.find_window(second_window).unwrap().default_focus,
            second_hint
        );
    }

    #[test]
    fn accepts_current_version_exited_metadata() {
        let document = decode(&valid_document()).unwrap();
        let encoded = document.encode().unwrap();
        let lair = LairDocument::decode(&encoded).unwrap().into_lair().unwrap();
        assert_eq!(lair.revision().get(), 7);
        assert_eq!(lair.dojos().count(), 1);
    }

    #[test]
    fn rejects_unknown_version_running_state_and_unsafe_path() {
        let mut value = valid_document();
        value["schema_version"] = json!(LAIR_SCHEMA_VERSION + 1);
        assert_eq!(
            decode(&value),
            Err(PersistenceError::UnsupportedVersion(
                LAIR_SCHEMA_VERSION + 1
            ))
        );

        let mut value = valid_document();
        value["dojos"][0]["windows"][0]["root"]["Leaf"]["state"] = json!("Running");
        assert_eq!(decode(&value), Err(PersistenceError::LiveProcessState));

        let mut value = valid_document();
        value["dojos"][0]["windows"][0]["root"]["Leaf"]["cwd"] = json!("../tmp");
        assert_eq!(
            decode(&value),
            Err(PersistenceError::UnsafeWorkingDirectory)
        );
    }

    #[test]
    fn rejects_invalid_window_focus_hint() {
        let mut value = valid_document();
        value["dojos"][0]["windows"][0]["default_focus"] = json!(Uuid::new_v4());
        assert_eq!(
            decode(&value),
            Err(PersistenceError::InvalidWindowDefaultFocus)
        );
    }

    #[test]
    fn rejects_duplicate_ids_invalid_ratios_and_oversized_collections() {
        let mut value = valid_document();
        let duplicate = value["dojos"][0].clone();
        value["dojos"].as_array_mut().unwrap().push(duplicate);
        assert_eq!(decode(&value), Err(PersistenceError::DuplicateId("dojo")));

        let mut value = valid_document();
        let leaf = value["dojos"][0]["windows"][0]["root"].clone();
        value["dojos"][0]["windows"][0]["root"] = json!({
            "Branch": {"axis": "Horizontal", "ratio": 0, "first": leaf.clone(), "second": leaf}
        });
        assert!(matches!(decode(&value), Err(PersistenceError::Decode(_))));

        let mut value = valid_document();
        let dojo = value["dojos"][0].clone();
        value["dojos"] = Value::Array(vec![dojo; MAX_DOJOS + 1]);
        assert_eq!(
            decode(&value),
            Err(PersistenceError::CollectionTooLarge("dojos"))
        );
    }

    #[test]
    fn rejects_oversized_documents_before_parsing() {
        assert_eq!(
            LairDocument::decode(&vec![b' '; MAX_LAIR_DOCUMENT_BYTES + 1]),
            Err(PersistenceError::DocumentTooLarge)
        );
    }
}
