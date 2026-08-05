//! Platform-independent contracts shared by application orchestration and presentation adapters.

mod message;
mod options;
mod picker;
mod topology;

pub use message::{AuthorityStatus, ThemeUpdate, WindowCommand, WindowUpdate};
pub use options::{TrustedConsentUi, WindowOptions, WindowPaneOptions};
pub(crate) use picker::PickerHitTarget;
pub use picker::{SessionPickerDecision, SessionPickerItem, SessionPickerUi};
pub use topology::{WindowDojoIdentity, WindowTopologyCommand, WindowTopologyUpdate};
