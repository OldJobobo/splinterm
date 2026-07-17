//! Observable semantic effects produced while parsing terminal input.
//!
//! Phase 4 will add revisions, snapshots, and renderer damage. These effects
//! exist now because bell, replies, title changes, and bounded-string failures
//! are part of Phase 3 terminal semantics.

/// A terminal effect in parser order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalEvent {
    /// BEL was executed.
    Bell,
    /// Bytes must be written back to the PTY.
    PtyWrite(Vec<u8>),
    /// OSC changed the terminal title.
    TitleChanged(String),
    /// OSC changed or reset one palette entry.
    PaletteChanged {
        /// Palette index.
        index: u16,
        /// Packed `0xRRGGBB` value.
        color: u32,
    },
    /// A recognized family has no semantic handler in this milestone.
    UnsupportedSequence(&'static str),
    /// A string exceeded its configured retention limit but remained synced.
    StringTruncated(&'static str),
}
