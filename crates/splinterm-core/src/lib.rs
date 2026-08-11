//! Persistent session and layout primitives shared by the client and daemon.

mod layout;
mod model;
mod persistence;

pub use layout::{
    Axis, LayoutNode, Splint, SplintId, SplintLaunchMetadata, SplintState, SplitRatio, SplitSide,
};
pub use model::{
    Dojo, DojoId, Lair, LairId, LairLifetime, Topology, TopologyError, TopologyRevision,
};
pub use persistence::{
    MAX_PERSISTENT_LAIRS, MAX_TOPOLOGY_DOCUMENT_BYTES, PersistenceError, TOPOLOGY_SCHEMA_VERSION,
    TopologyDocument,
};
