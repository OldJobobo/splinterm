//! Persistent session and layout primitives shared by the client and daemon.

mod layout;
mod model;

pub use layout::{Axis, LayoutNode, Splint, SplintId, SplintState, Window, WindowId};
pub use model::{Dojo, DojoId, Lair, LairError};
