//! Terminal-kernel configuration and parser hard limits.

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
        }
    }
}
