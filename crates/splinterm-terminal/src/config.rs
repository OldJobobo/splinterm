//! Terminal-kernel configuration and parser hard limits.

use crate::{ImageLimits, SharedImageBudget};

/// Foot-compatible defaults for the VT340 Sixel color map.
pub const DEFAULT_SIXEL_PALETTE: [u32; 16] = [
    0xff00_0000,
    0xff33_33cc,
    0xffcc_2121,
    0xff33_cc33,
    0xffcc_33cc,
    0xff33_cccc,
    0xffcc_cc33,
    0xff87_8787,
    0xff42_4242,
    0xff54_5499,
    0xff99_4242,
    0xff54_9954,
    0xff99_5499,
    0xff54_9999,
    0xff99_9954,
    0xffcc_cccc,
];

/// Initial Sixel policy and palette.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SixelConfig {
    /// Whether DCS `q` graphics are decoded. Disabled graphics are discarded.
    pub enabled: bool,
    /// Whether each image starts from the configured palette. DEC private mode
    /// 1070 may change this at runtime; shared mode retains color definitions.
    pub private_palette: bool,
    /// Initial VT340-compatible palette entries.
    pub palette: [u32; 16],
}

impl Default for SixelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            private_palette: true,
            palette: DEFAULT_SIXEL_PALETTE,
        }
    }
}

/// Configuration independent of PTY, renderer, and daemon policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalConfig {
    /// Requested normal-screen scrollback rows before power-of-two rounding.
    pub scrollback_lines: usize,
    /// Distance between default horizontal tab stops.
    pub tab_width: usize,
    /// Maximum retained OSC payload bytes.
    pub osc_limit: usize,
    /// Maximum observed DCS payload bytes before truncation.
    pub dcs_limit: usize,
    /// Maximum interned composed-character sequences.
    pub composed_limit: usize,
    /// Maximum queued semantic effects awaiting a drain.
    pub event_limit: usize,
    /// Number of committed updates retained for replay.
    pub update_history_limit: usize,
    /// Hard canonical image content, placement, dimension, and byte limits.
    pub image_limits: ImageLimits,
    /// Optional process-wide authoritative image-content byte admission.
    pub shared_image_budget: Option<SharedImageBudget>,
    /// Initial Sixel protocol policy and palette.
    pub sixel: SixelConfig,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 1_000,
            tab_width: 8,
            osc_limit: 4_096,
            dcs_limit: 4_096,
            composed_limit: 4_096,
            event_limit: 1_024,
            update_history_limit: 256,
            image_limits: ImageLimits::default(),
            shared_image_budget: None,
            sixel: SixelConfig::default(),
        }
    }
}
