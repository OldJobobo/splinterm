use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identity for a terminal surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SplintId(Uuid);

impl SplintId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SplintId {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable identity for a window in a dojo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowId(Uuid);

impl WindowId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for WindowId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplintState {
    Starting,
    Running,
    Exited(i32),
}

/// One terminal surface, eventually backed by a daemon-owned PTY.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Splint {
    pub id: SplintId,
    pub title: String,
    pub cwd: PathBuf,
    pub command: Vec<String>,
    pub state: SplintState,
}

impl Splint {
    #[must_use]
    pub fn shell(cwd: PathBuf) -> Self {
        Self {
            id: SplintId::new(),
            title: "shell".into(),
            cwd,
            command: Vec::new(),
            state: SplintState::Starting,
        }
    }
}

/// Binary layout tree used to arrange splints inside a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayoutNode {
    Leaf(Splint),
    Branch {
        axis: Axis,
        /// Fraction of available space assigned to `first`.
        ratio: f32,
        first: Box<Self>,
        second: Box<Self>,
    },
}

impl LayoutNode {
    #[must_use]
    pub const fn splint_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Branch { first, second, .. } => first.splint_count() + second.splint_count(),
        }
    }

    #[must_use]
    pub fn find_splint(&self, id: SplintId) -> Option<&Splint> {
        match self {
            Self::Leaf(splint) => (splint.id == id).then_some(splint),
            Self::Branch { first, second, .. } => {
                first.find_splint(id).or_else(|| second.find_splint(id))
            }
        }
    }

    pub fn find_splint_mut(&mut self, id: SplintId) -> Option<&mut Splint> {
        match self {
            Self::Leaf(splint) => (splint.id == id).then_some(splint),
            Self::Branch { first, second, .. } => first
                .find_splint_mut(id)
                .or_else(|| second.find_splint_mut(id)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub root: LayoutNode,
}

impl Window {
    #[must_use]
    pub fn with_shell(cwd: PathBuf) -> Self {
        Self {
            id: WindowId::new(),
            title: "terminal".into(),
            root: LayoutNode::Leaf(Splint::shell(cwd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_nested_splints() {
        let leaf = || LayoutNode::Leaf(Splint::shell(PathBuf::from("/tmp")));
        let layout = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(leaf()),
            second: Box::new(LayoutNode::Branch {
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(leaf()),
                second: Box::new(leaf()),
            }),
        };

        assert_eq!(layout.splint_count(), 3);
    }
}
