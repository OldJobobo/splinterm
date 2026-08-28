//! Canonical, bounded terminal checkpoint schema v1.
//!
//! This format owns terminal/parser state only. Descriptor storage, sealing,
//! actor write queues, PTY state, and image bodies belong to later handoff
//! boundaries.

use std::{
    fmt,
    io::{self, Write},
    marker::PhantomData,
};

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{SeqAccess, Visitor},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::SharedImageBudget;

use super::{
    ActiveScreen, Attributes, ChangeSet, ComposedTable, Grid, ImageMetrics, ImagePlane,
    KittyImageIds, MAX_SIXEL_COLORS, Parser, ScrollRegion, SharedKittyUploadBudget,
    SixelPaletteMode, Terminal, TerminalConfig, TerminalEvent, TerminalModes, TerminalRevision,
};

pub const TERMINAL_CHECKPOINT_SCHEMA_V1: u16 = 1;
pub const MAX_TERMINAL_CHECKPOINT_BYTES: usize = 64 * 1024 * 1024;
const CHECKPOINT_MAGIC: [u8; 8] = *b"SPLTCP01";
const MAX_CHECKPOINT_TITLE_BYTES: usize = 64 * 1024;
const MAX_CHECKPOINT_EVENT_BODY_BYTES: usize = 1024 * 1024;
const MAX_CHECKPOINT_EVENTS: usize = 1 << 16;

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("terminal checkpoint exceeds {MAX_TERMINAL_CHECKPOINT_BYTES} bytes")]
    TooLarge,
    #[error("terminal checkpoint encoding is malformed: {0}")]
    Encoding(#[source] serde_json::Error),
    #[error("terminal checkpoint encoding is not canonical")]
    NonCanonical,
    #[error("terminal checkpoint configuration does not match the restore authority")]
    IncompatibleConfig,
    #[error("terminal state is not representable by checkpoint schema v1")]
    UnsupportedState,
    #[error("terminal checkpoint state violates terminal invariants")]
    InvalidState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckpointEnvelopeV1 {
    magic: [u8; 8],
    schema: u16,
    body_sha256: [u8; 32],
    body: TerminalCheckpointV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TerminalCheckpointV1 {
    config: ConfigEvidence,
    normal: Grid,
    alternate: Grid,
    active: ActiveScreen,
    parser: Parser,
    attributes: Attributes,
    saved_attributes: Attributes,
    scroll_region: ScrollRegion,
    modes: TerminalModes,
    #[serde(deserialize_with = "deserialize_tab_stops")]
    tab_stops: Vec<bool>,
    composed: ComposedTable,
    cell_pixels: Option<(u32, u32)>,
    sixel_scrolling: bool,
    sixel_cursor_right: bool,
    sixel_palette_mode: SixelPaletteMode,
    #[serde(deserialize_with = "deserialize_sixel_palette")]
    sixel_shared_palette: Vec<u32>,
    sixel_palette_size: usize,
    sixel_maximum_width: u32,
    sixel_maximum_height: u32,
    title: String,
    #[serde(deserialize_with = "deserialize_palette")]
    palette: Vec<u32>,
    #[serde(deserialize_with = "deserialize_palette")]
    initial_palette: Vec<u32>,
    default_colors: [u32; 3],
    initial_default_colors: [u32; 3],
    #[serde(deserialize_with = "deserialize_events")]
    events: Vec<CheckpointEvent>,
    revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigEvidence {
    scrollback_lines: usize,
    tab_width: usize,
    osc_limit: usize,
    dcs_limit: usize,
    composed_limit: usize,
    event_limit: usize,
    update_history_limit: usize,
    image_bytes_per_content: usize,
    image_bytes_per_terminal: usize,
    image_contents_per_terminal: usize,
    image_placements_per_terminal: usize,
    image_maximum_dimension: u32,
    image_maximum_pixels: usize,
    shared_image_budget_limit: Option<usize>,
    shared_kitty_upload_budget_limit: Option<usize>,
    sixel_enabled: bool,
    sixel_private_palette: bool,
    sixel_palette: [u32; 16],
}

impl ConfigEvidence {
    fn from_config(config: &TerminalConfig) -> Self {
        Self {
            scrollback_lines: config.scrollback_lines,
            tab_width: config.tab_width,
            osc_limit: config.osc_limit,
            dcs_limit: config.dcs_limit,
            composed_limit: config.composed_limit,
            event_limit: config.event_limit,
            update_history_limit: config.update_history_limit,
            image_bytes_per_content: config.image_limits.bytes_per_content,
            image_bytes_per_terminal: config.image_limits.bytes_per_terminal,
            image_contents_per_terminal: config.image_limits.contents_per_terminal,
            image_placements_per_terminal: config.image_limits.placements_per_terminal,
            image_maximum_dimension: config.image_limits.maximum_dimension,
            image_maximum_pixels: config.image_limits.maximum_pixels,
            shared_image_budget_limit: config
                .shared_image_budget
                .as_ref()
                .map(SharedImageBudget::limit),
            shared_kitty_upload_budget_limit: config
                .shared_kitty_upload_budget
                .as_ref()
                .map(SharedKittyUploadBudget::limit),
            sixel_enabled: config.sixel.enabled,
            sixel_private_palette: config.sixel.private_palette,
            sixel_palette: config.sixel.palette,
        }
    }

    fn matches(&self, config: &TerminalConfig) -> bool {
        *self == Self::from_config(config)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
enum CheckpointEvent {
    Bell,
    PtyWrite(#[serde(deserialize_with = "deserialize_event_bytes")] Vec<u8>),
    TitleChanged(String),
    PaletteChanged { index: u16, color: u32 },
}

impl CheckpointEvent {
    fn from_terminal(event: &TerminalEvent) -> Result<Self, CheckpointError> {
        match event {
            TerminalEvent::Bell => Ok(Self::Bell),
            TerminalEvent::PtyWrite(bytes) => Ok(Self::PtyWrite(bytes.clone())),
            TerminalEvent::TitleChanged(title) => Ok(Self::TitleChanged(title.clone())),
            TerminalEvent::PaletteChanged { index, color } => Ok(Self::PaletteChanged {
                index: *index,
                color: *color,
            }),
            TerminalEvent::UnsupportedSequence(_)
            | TerminalEvent::ImageRejected(_)
            | TerminalEvent::StringTruncated(_)
            | TerminalEvent::EventQueueOverflow => Err(CheckpointError::UnsupportedState),
        }
    }

    fn into_terminal(self) -> TerminalEvent {
        match self {
            Self::Bell => TerminalEvent::Bell,
            Self::PtyWrite(bytes) => TerminalEvent::PtyWrite(bytes),
            Self::TitleChanged(title) => TerminalEvent::TitleChanged(title),
            Self::PaletteChanged { index, color } => TerminalEvent::PaletteChanged { index, color },
        }
    }

    fn body_bytes(&self) -> usize {
        match self {
            Self::PtyWrite(bytes) => bytes.len(),
            Self::TitleChanged(title) => title.len(),
            Self::Bell | Self::PaletteChanged { .. } => 0,
        }
    }
}

impl TerminalCheckpointV1 {
    fn capture(terminal: &Terminal) -> Result<Self, CheckpointError> {
        let image_metrics = terminal.images.metrics();
        if terminal.synchronized_updates
            || !terminal.synchronized_change.is_empty()
            || terminal.current_change.is_some()
            || image_metrics != ImageMetrics::default()
            || terminal.sixel_decoder.is_some()
            || terminal.kitty_chunk.is_some()
            || terminal.kitty_upload.is_some()
            || terminal.iterm_transfer.is_some()
            || !terminal.kitty_image_ids.normal.is_empty()
            || !terminal.kitty_image_ids.alternate.is_empty()
            || terminal.event_overflowed
        {
            return Err(CheckpointError::UnsupportedState);
        }
        let events = terminal
            .events
            .iter()
            .map(CheckpointEvent::from_terminal)
            .collect::<Result<Vec<_>, _>>()?;
        let mut composed = terminal.composed.clone();
        let event_body_bytes = events.iter().try_fold(0_usize, |total, event| {
            total.checked_add(event.body_bytes())
        });
        if terminal.title.len() > MAX_CHECKPOINT_TITLE_BYTES
            || events.len() > terminal.config.event_limit
            || events.len() > MAX_CHECKPOINT_EVENTS
            || event_body_bytes.is_none_or(|bytes| bytes > MAX_CHECKPOINT_EVENT_BODY_BYTES)
            || events.iter().any(
                |event| matches!(event, CheckpointEvent::TitleChanged(title) if title.len() > MAX_CHECKPOINT_TITLE_BYTES),
            )
            || !terminal.attributes.checkpoint_is_valid()
            || !terminal.saved_attributes.checkpoint_is_valid()
            || !composed.validate_and_rebuild_checkpoint(terminal.config.composed_limit)
            || !terminal
                .parser
                .checkpoint_is_valid(terminal.config.osc_limit, terminal.config.dcs_limit)
            || !terminal
                .normal
                .checkpoint_is_valid(terminal.composed.checkpoint_entry_count())
            || !terminal
                .alternate
                .checkpoint_is_valid(terminal.composed.checkpoint_entry_count())
        {
            return Err(CheckpointError::InvalidState);
        }
        Ok(Self {
            config: ConfigEvidence::from_config(&terminal.config),
            normal: terminal.normal.clone(),
            alternate: terminal.alternate.clone(),
            active: terminal.active,
            parser: terminal.parser.clone(),
            attributes: terminal.attributes,
            saved_attributes: terminal.saved_attributes,
            scroll_region: terminal.scroll_region,
            modes: terminal.modes,
            tab_stops: terminal.tab_stops.clone(),
            composed,
            cell_pixels: terminal.cell_pixels,
            sixel_scrolling: terminal.sixel_scrolling,
            sixel_cursor_right: terminal.sixel_cursor_right,
            sixel_palette_mode: terminal.sixel_palette_mode,
            sixel_shared_palette: terminal.sixel_shared_palette.to_vec(),
            sixel_palette_size: terminal.sixel_palette_size,
            sixel_maximum_width: terminal.sixel_maximum_width,
            sixel_maximum_height: terminal.sixel_maximum_height,
            title: terminal.title.clone(),
            palette: terminal.palette.to_vec(),
            initial_palette: terminal.initial_palette.to_vec(),
            default_colors: terminal.default_colors,
            initial_default_colors: terminal.initial_default_colors,
            events,
            revision: terminal.revision.value(),
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "all-or-nothing validation and construction keep decoded checkpoint state local"
    )]
    fn restore(mut self, config: TerminalConfig) -> Result<Terminal, CheckpointError> {
        if !self.config.matches(&config) {
            return Err(CheckpointError::IncompatibleConfig);
        }
        if !self
            .composed
            .validate_and_rebuild_checkpoint(config.composed_limit)
        {
            return Err(CheckpointError::InvalidState);
        }
        let composed_entries = self.composed.checkpoint_entry_count();
        let columns = self.normal.columns();
        let rows = self.normal.screen_rows();
        let expected_normal_capacity = config
            .scrollback_lines
            .checked_add(rows)
            .map(|requested| requested.max(rows))
            .and_then(usize::checked_next_power_of_two)
            .map(|capacity| capacity.min(1_usize << 30));
        let expected_alternate_capacity = rows
            .checked_next_power_of_two()
            .map(|capacity| capacity.min(1_usize << 30));
        let scroll_start = self.scroll_region.start();
        let scroll_end = self.scroll_region.end();
        let event_body_bytes = self.events.iter().try_fold(0_usize, |total, event| {
            total.checked_add(event.body_bytes())
        });
        if !self.normal.checkpoint_is_valid(composed_entries)
            || !self.alternate.checkpoint_is_valid(composed_entries)
            || self.alternate.columns() != columns
            || self.alternate.screen_rows() != rows
            || expected_normal_capacity != Some(self.normal.row_capacity())
            || expected_alternate_capacity != Some(self.alternate.row_capacity())
            || !self
                .parser
                .checkpoint_is_valid(config.osc_limit, config.dcs_limit)
            || !self.attributes.checkpoint_is_valid()
            || !self.saved_attributes.checkpoint_is_valid()
            || self.tab_stops.len() != columns
            || scroll_start < 0
            || scroll_start >= scroll_end
            || usize::try_from(scroll_end).map_or(true, |end| end > rows)
            || self.title.len() > MAX_CHECKPOINT_TITLE_BYTES
            || self.events.len() > config.event_limit
            || self.events.len() > MAX_CHECKPOINT_EVENTS
            || event_body_bytes.is_none_or(|bytes| bytes > MAX_CHECKPOINT_EVENT_BODY_BYTES)
            || self
                .events
                .iter()
                .any(|event| matches!(event, CheckpointEvent::TitleChanged(title) if title.len() > MAX_CHECKPOINT_TITLE_BYTES))
            || self
                .cell_pixels
                .is_some_and(|(width, height)| width == 0 || height == 0)
            || self.sixel_shared_palette.len() != MAX_SIXEL_COLORS
            || self.palette.len() != 256
            || self.initial_palette.len() != 256
            || !(2..=MAX_SIXEL_COLORS).contains(&self.sixel_palette_size)
            || self.sixel_maximum_width > config.image_limits.maximum_dimension
            || self.sixel_maximum_height > config.image_limits.maximum_dimension
        {
            return Err(CheckpointError::InvalidState);
        }

        let sixel_shared_palette = self
            .sixel_shared_palette
            .into_boxed_slice()
            .try_into()
            .map_err(|_| CheckpointError::InvalidState)?;
        let palette = self
            .palette
            .try_into()
            .map_err(|_| CheckpointError::InvalidState)?;
        let initial_palette = self
            .initial_palette
            .try_into()
            .map_err(|_| CheckpointError::InvalidState)?;
        let images = ImagePlane::new_with_shared_budget(
            config.image_limits,
            config.shared_image_budget.clone(),
        );
        let kitty_upload_budget = config
            .shared_kitty_upload_budget
            .clone()
            .unwrap_or_else(|| {
                SharedKittyUploadBudget::new(crate::image::DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON)
            });
        Ok(Terminal {
            normal: self.normal,
            alternate: self.alternate,
            active: self.active,
            parser: self.parser,
            config,
            attributes: self.attributes,
            saved_attributes: self.saved_attributes,
            scroll_region: self.scroll_region,
            modes: self.modes,
            synchronized_updates: false,
            synchronized_change: ChangeSet::default(),
            tab_stops: self.tab_stops,
            composed: self.composed,
            images,
            sixel_decoder: None,
            kitty_chunk: None,
            kitty_upload: None,
            iterm_transfer: None,
            kitty_upload_budget,
            kitty_image_ids: KittyImageIds::default(),
            cell_pixels: self.cell_pixels,
            sixel_scrolling: self.sixel_scrolling,
            sixel_cursor_right: self.sixel_cursor_right,
            sixel_palette_mode: self.sixel_palette_mode,
            sixel_shared_palette,
            sixel_palette_size: self.sixel_palette_size,
            sixel_maximum_width: self.sixel_maximum_width,
            sixel_maximum_height: self.sixel_maximum_height,
            title: self.title,
            palette,
            initial_palette,
            default_colors: self.default_colors,
            initial_default_colors: self.initial_default_colors,
            events: self
                .events
                .into_iter()
                .map(CheckpointEvent::into_terminal)
                .collect(),
            event_overflowed: false,
            revision: TerminalRevision::new(self.revision),
            update_history: std::collections::VecDeque::new(),
            current_change: None,
        })
    }
}

impl Terminal {
    /// Encodes the terminal into canonical checkpoint schema v1 bytes.
    ///
    /// # Errors
    /// Rejects active transactions, image state/transfers, unsupported pending
    /// events, invalid state, or an aggregate larger than the hard byte bound.
    pub fn encode_checkpoint(&self) -> Result<Vec<u8>, CheckpointError> {
        let body = TerminalCheckpointV1::capture(self)?;
        let body_bytes = encode_json(&body)?;
        let envelope = CheckpointEnvelopeV1 {
            magic: CHECKPOINT_MAGIC,
            schema: TERMINAL_CHECKPOINT_SCHEMA_V1,
            body_sha256: Sha256::digest(&body_bytes).into(),
            body,
        };
        encode_json(&envelope)
    }

    /// Restores canonical checkpoint schema v1 under the supplied configuration.
    ///
    /// # Errors
    /// Rejects oversized, malformed, noncanonical, incompatible, unsupported,
    /// or internally invalid state without publishing a partial terminal.
    pub fn restore_checkpoint(
        bytes: &[u8],
        config: TerminalConfig,
    ) -> Result<Self, CheckpointError> {
        if bytes.len() > MAX_TERMINAL_CHECKPOINT_BYTES {
            return Err(CheckpointError::TooLarge);
        }
        let envelope: CheckpointEnvelopeV1 =
            serde_json::from_slice(bytes).map_err(CheckpointError::Encoding)?;
        let canonical = encode_json(&envelope)?;
        if canonical != bytes {
            return Err(CheckpointError::NonCanonical);
        }
        if envelope.magic != CHECKPOINT_MAGIC || envelope.schema != TERMINAL_CHECKPOINT_SCHEMA_V1 {
            return Err(CheckpointError::InvalidState);
        }
        let body_bytes = encode_json(&envelope.body)?;
        if <[u8; 32]>::from(Sha256::digest(&body_bytes)) != envelope.body_sha256 {
            return Err(CheckpointError::InvalidState);
        }
        envelope.body.restore(config)
    }
}

fn encode_json(value: &impl Serialize) -> Result<Vec<u8>, CheckpointError> {
    let mut writer = BoundedWriter::default();
    match serde_json::to_writer(&mut writer, value) {
        Ok(()) => Ok(writer.bytes),
        Err(_) if writer.exceeded => Err(CheckpointError::TooLarge),
        Err(error) => Err(CheckpointError::Encoding(error)),
    }
}

fn deserialize_tab_stops<'de, D>(deserializer: D) -> Result<Vec<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_sequence(
        deserializer,
        crate::row::MAX_CHECKPOINT_COLUMNS,
        "terminal tab stops",
    )
}

fn deserialize_sixel_palette<'de, D>(deserializer: D) -> Result<Vec<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_sequence(deserializer, MAX_SIXEL_COLORS, "Sixel palette entries")
}

fn deserialize_palette<'de, D>(deserializer: D) -> Result<Vec<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_sequence(deserializer, 256, "terminal palette entries")
}

fn deserialize_events<'de, D>(deserializer: D) -> Result<Vec<CheckpointEvent>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_sequence(deserializer, MAX_CHECKPOINT_EVENTS, "terminal events")
}

fn deserialize_event_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_sequence(
        deserializer,
        MAX_CHECKPOINT_EVENT_BODY_BYTES,
        "terminal event bytes",
    )
}

fn deserialize_bounded_sequence<'de, D, T>(
    deserializer: D,
    maximum: usize,
    name: &'static str,
) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct BoundedVisitor<T> {
        maximum: usize,
        name: &'static str,
        marker: PhantomData<T>,
    }

    impl<'de, T> Visitor<'de> for BoundedVisitor<T>
    where
        T: Deserialize<'de>,
    {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "at most {} {}", self.maximum, self.name)
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            if sequence
                .size_hint()
                .is_some_and(|length| length > self.maximum)
            {
                return Err(serde::de::Error::custom(format_args!(
                    "{} exceed checkpoint bound",
                    self.name
                )));
            }
            let mut values =
                Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(self.maximum));
            while let Some(value) = sequence.next_element()? {
                if values.len() == self.maximum {
                    return Err(serde::de::Error::custom(format_args!(
                        "{} exceed checkpoint bound",
                        self.name
                    )));
                }
                values.push(value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_seq(BoundedVisitor {
        maximum,
        name,
        marker: PhantomData,
    })
}

#[derive(Default)]
struct BoundedWriter {
    bytes: Vec<u8>,
    exceeded: bool,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(next) = self.bytes.len().checked_add(bytes.len()) else {
            self.exceeded = true;
            return Err(io::Error::other("terminal checkpoint byte count overflow"));
        };
        if next > MAX_TERMINAL_CHECKPOINT_BYTES {
            self.exceeded = true;
            return Err(io::Error::other("terminal checkpoint exceeds hard bound"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn config() -> TerminalConfig {
        TerminalConfig::default()
    }

    fn mutate_body(mut mutate: impl FnMut(&mut Value)) -> Vec<u8> {
        let terminal = Terminal::new(4, 2, config());
        let bytes = terminal.encode_checkpoint().unwrap();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        mutate(value.get_mut("body").unwrap());
        let mut envelope: CheckpointEnvelopeV1 = serde_json::from_value(value).unwrap();
        let body_bytes = encode_json(&envelope.body).unwrap();
        envelope.body_sha256 = Sha256::digest(&body_bytes).into();
        encode_json(&envelope).unwrap()
    }

    #[test]
    fn digest_consistent_but_invalid_internal_states_fail_without_panicking() {
        let corruptions = [
            mutate_body(|body| body["normal"]["row_ids"][0] = json!(0)),
            mutate_body(|body| body["normal"]["next_row_id"] = json!(1)),
            mutate_body(|body| {
                let row = body["normal"]["rows"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|row| !row.is_null())
                    .unwrap();
                row["cells"][1]["content"] = json!(u32::from(b'A'));
                row["cells"][2]["content"] = json!(u32::MAX);
            }),
            mutate_body(|body| body["parser"]["osc_limit"] = json!(1)),
            mutate_body(|body| body["tab_stops"] = json!([])),
            mutate_body(|body| body["scroll_region"]["end"] = json!(99)),
            mutate_body(|body| body["sixel_shared_palette"] = json!([])),
            mutate_body(|body| body["palette"] = json!([])),
            mutate_body(|body| {
                body["title"] = json!("x".repeat(MAX_CHECKPOINT_TITLE_BYTES + 1));
            }),
        ];
        for bytes in corruptions {
            assert!(matches!(
                Terminal::restore_checkpoint(&bytes, config()),
                Err(CheckpointError::InvalidState)
            ));
        }
    }

    #[test]
    fn wrong_magic_schema_and_body_digest_fail_closed() {
        for mutate in [
            (|envelope: &mut CheckpointEnvelopeV1| envelope.magic[0] ^= 1)
                as fn(&mut CheckpointEnvelopeV1),
            |envelope| envelope.schema += 1,
            |envelope| envelope.body_sha256[0] ^= 1,
        ] {
            let terminal = Terminal::new(4, 2, config());
            let bytes = terminal.encode_checkpoint().unwrap();
            let mut envelope: CheckpointEnvelopeV1 = serde_json::from_slice(&bytes).unwrap();
            mutate(&mut envelope);
            let corrupted = encode_json(&envelope).unwrap();
            assert!(matches!(
                Terminal::restore_checkpoint(&corrupted, config()),
                Err(CheckpointError::InvalidState)
            ));
        }
    }
}
