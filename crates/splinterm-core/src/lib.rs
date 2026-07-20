//! Persistent session and layout primitives shared by the client and daemon.

mod layout;
mod model;
mod persistence;

pub use layout::{
    Axis, LayoutNode, Splint, SplintId, SplintLaunchMetadata, SplintState, SplitRatio, SplitSide,
    Window, WindowId,
};
pub use model::{Dojo, DojoId, Lair, LairError, TopologyRevision};
pub use persistence::{
    LAIR_SCHEMA_VERSION, LairDocument, MAX_LAIR_DOCUMENT_BYTES, PersistenceError,
};
