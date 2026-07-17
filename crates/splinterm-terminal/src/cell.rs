//! Cell colors, attributes, and content.
//!
//! Derived from Foot 1.27.0 `terminal.h` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, specifically `color_source`,
//! `attributes`, `cell`, and the `CELL_*` constants. The two-word attribute
//! representation preserves Foot's compact eight-byte baseline without Rust
//! bitfields or unsafe code.

const COLOR_MASK: u32 = 0x00ff_ffff;
const COMPOSED_BASE: u32 = 0x0020_0000;
const COMPOSED_KEY_MAX: u32 = 0x3fff_ffff;
const COMPOSED_END: u32 = COMPOSED_BASE + COMPOSED_KEY_MAX;
const SPACER_BASE: u32 = COMPOSED_END + 1;

const BOLD: u32 = 1 << 0;
const DIM: u32 = 1 << 1;
const ITALIC: u32 = 1 << 2;
const UNDERLINE: u32 = 1 << 3;
const STRIKETHROUGH: u32 = 1 << 4;
const BLINK: u32 = 1 << 5;
const CONCEAL: u32 = 1 << 6;
const REVERSE: u32 = 1 << 7;
const FOREGROUND_SHIFT: u32 = 8;

const CLEAN: u32 = 1 << 0;
const FOREGROUND_SOURCE_SHIFT: u32 = 1;
const BACKGROUND_SOURCE_SHIFT: u32 = 3;
const SOURCE_MASK: u32 = 0b11;
const CONFINED: u32 = 1 << 5;
const SELECTED: u32 = 1 << 6;
const URL: u32 = 1 << 7;
const BACKGROUND_SHIFT: u32 = 8;

/// Identifies how a cell color value should be interpreted.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum ColorSource {
    /// Use the terminal's configured default color.
    #[default]
    Default = 0,
    /// Index into the first 16 configured palette entries.
    Base16 = 1,
    /// Index into the 256-color palette.
    Base256 = 2,
    /// A direct 24-bit RGB value.
    Rgb = 3,
}

impl ColorSource {
    const fn from_bits(bits: u32) -> Self {
        match bits & SOURCE_MASK {
            0 => Self::Default,
            1 => Self::Base16,
            2 => Self::Base256,
            3 => Self::Rgb,
            _ => unreachable!(),
        }
    }
}

/// A 24-bit value paired with its interpretation source.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Color {
    source: ColorSource,
    value: u32,
}

impl Color {
    /// Constructs a color.
    ///
    /// # Panics
    ///
    /// Panics when `value` does not fit Foot's 24-bit color field.
    #[must_use]
    pub const fn new(source: ColorSource, value: u32) -> Self {
        assert!(value <= COLOR_MASK, "color values must fit in 24 bits");
        Self { source, value }
    }

    /// Constructs a direct RGB color from `0xRRGGBB`.
    #[must_use]
    pub const fn rgb(value: u32) -> Self {
        Self::new(ColorSource::Rgb, value)
    }

    /// Returns the color's interpretation source.
    #[must_use]
    pub const fn source(self) -> ColorSource {
        self.source
    }

    /// Returns the source-dependent 24-bit value.
    #[must_use]
    pub const fn value(self) -> u32 {
        self.value
    }
}

/// Compact cell rendition and renderer-state attributes.
///
/// Foot stores eight style flags and the foreground in one 32-bit word, then
/// renderer flags, both color sources, and the background in another. Keeping
/// the same logical packing makes this type eight bytes while leaving its API
/// independent of the physical representation.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Attributes {
    style_and_foreground: u32,
    state_and_background: u32,
}

macro_rules! boolean_attribute {
    ($getter:ident, $setter:ident, $word:ident, $mask:ident) => {
        #[must_use]
        pub const fn $getter(self) -> bool {
            self.$word & $mask != 0
        }

        pub fn $setter(&mut self, enabled: bool) {
            Self::set_flag(&mut self.$word, $mask, enabled);
        }
    };
}

impl Attributes {
    const fn set_flag(word: &mut u32, mask: u32, enabled: bool) {
        if enabled {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }

    boolean_attribute!(bold, set_bold, style_and_foreground, BOLD);
    boolean_attribute!(dim, set_dim, style_and_foreground, DIM);
    boolean_attribute!(italic, set_italic, style_and_foreground, ITALIC);
    boolean_attribute!(underline, set_underline, style_and_foreground, UNDERLINE);
    boolean_attribute!(
        strikethrough,
        set_strikethrough,
        style_and_foreground,
        STRIKETHROUGH
    );
    boolean_attribute!(blink, set_blink, style_and_foreground, BLINK);
    boolean_attribute!(conceal, set_conceal, style_and_foreground, CONCEAL);
    boolean_attribute!(reverse, set_reverse, style_and_foreground, REVERSE);
    boolean_attribute!(clean, set_clean, state_and_background, CLEAN);
    boolean_attribute!(confined, set_confined, state_and_background, CONFINED);
    boolean_attribute!(selected, set_selected, state_and_background, SELECTED);
    boolean_attribute!(url, set_url, state_and_background, URL);

    /// Returns the cell foreground color.
    #[must_use]
    pub const fn foreground(self) -> Color {
        let source = ColorSource::from_bits(self.state_and_background >> FOREGROUND_SOURCE_SHIFT);
        let value = (self.style_and_foreground >> FOREGROUND_SHIFT) & COLOR_MASK;
        Color { source, value }
    }

    /// Sets the cell foreground color.
    pub fn set_foreground(&mut self, color: Color) {
        self.style_and_foreground = (self.style_and_foreground & !(COLOR_MASK << FOREGROUND_SHIFT))
            | (color.value << FOREGROUND_SHIFT);
        self.state_and_background = (self.state_and_background
            & !(SOURCE_MASK << FOREGROUND_SOURCE_SHIFT))
            | ((color.source as u32) << FOREGROUND_SOURCE_SHIFT);
    }

    /// Returns the cell background color.
    #[must_use]
    pub const fn background(self) -> Color {
        let source = ColorSource::from_bits(self.state_and_background >> BACKGROUND_SOURCE_SHIFT);
        let value = (self.state_and_background >> BACKGROUND_SHIFT) & COLOR_MASK;
        Color { source, value }
    }

    /// Sets the cell background color.
    pub fn set_background(&mut self, color: Color) {
        self.state_and_background = (self.state_and_background
            & !((COLOR_MASK << BACKGROUND_SHIFT) | (SOURCE_MASK << BACKGROUND_SOURCE_SHIFT)))
            | (color.value << BACKGROUND_SHIFT)
            | ((color.source as u32) << BACKGROUND_SOURCE_SHIFT);
    }
}

/// Semantic content encoded in a cell's compact content word.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CellContent {
    /// An unused cell.
    Empty,
    /// One non-NUL Unicode scalar value. Foot reserves zero for an empty cell.
    Scalar(char),
    /// A key into the future composed-character table.
    Composed(u32),
    /// Foot's spacer sentinel plus its payload.
    ///
    /// Zero is the exact `CELL_SPACER` padding sentinel; positive values are
    /// wide-character continuations recording columns remaining in the leader.
    Spacer(u32),
}

/// One terminal grid cell.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cell {
    content: u32,
    attributes: Attributes,
}

impl Cell {
    /// Constructs a cell with the supplied content and default attributes.
    ///
    /// # Panics
    ///
    /// Panics for [`CellContent::Scalar`] containing NUL, an out-of-range
    /// composed key, or a spacer payload that cannot fit the content word.
    #[must_use]
    pub fn new(content: CellContent) -> Self {
        let mut cell = Self::default();
        cell.set_content(content);
        cell
    }

    /// Decodes the cell's semantic content.
    ///
    /// # Panics
    ///
    /// Panics only if the internal content invariant is violated. The public
    /// constructors and setters cannot create such a value.
    #[must_use]
    pub fn content(self) -> CellContent {
        match self.content {
            0 => CellContent::Empty,
            value if value < COMPOSED_BASE => {
                CellContent::Scalar(char::from_u32(value).expect("cell scalar invariant"))
            }
            value if value <= COMPOSED_END => CellContent::Composed(value - COMPOSED_BASE),
            value => CellContent::Spacer(value - SPACER_BASE),
        }
    }

    /// Replaces the cell's content.
    ///
    /// # Panics
    ///
    /// Panics for a scalar NUL (which Foot reserves for empty cells), if a
    /// composed key exceeds Foot's 30-bit key range, or if a spacer payload
    /// cannot fit in the compact content word.
    pub fn set_content(&mut self, content: CellContent) {
        self.content = match content {
            CellContent::Empty => 0,
            CellContent::Scalar(character) => {
                assert!(character != '\0', "NUL is reserved for an empty cell");
                u32::from(character)
            }
            CellContent::Composed(key) => {
                assert!(key <= COMPOSED_KEY_MAX, "composed key exceeds Foot's range");
                COMPOSED_BASE + key
            }
            CellContent::Spacer(remaining) => SPACER_BASE
                .checked_add(remaining)
                .expect("spacer width exceeds the cell representation"),
        };
    }

    /// Returns the cell's attributes.
    #[must_use]
    pub const fn attributes(self) -> Attributes {
        self.attributes
    }

    /// Returns mutable access to the cell's attributes.
    pub const fn attributes_mut(&mut self) -> &mut Attributes {
        &mut self.attributes
    }

    /// Replaces the cell's attributes.
    pub fn set_attributes(&mut self, attributes: Attributes) {
        self.attributes = attributes;
    }

    /// Returns whether this is a wide-cell continuation.
    #[must_use]
    pub const fn is_spacer(self) -> bool {
        self.content >= SPACER_BASE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn attributes_default_to_foot_zero_state() {
        let attributes = Attributes::default();

        assert!(!attributes.bold());
        assert!(!attributes.clean());
        assert_eq!(attributes.foreground(), Color::default());
        assert_eq!(attributes.background(), Color::default());
    }

    #[test]
    fn attributes_transition_independently() {
        let mut attributes = Attributes::default();
        attributes.set_bold(true);
        attributes.set_underline(true);
        attributes.set_clean(true);
        attributes.set_selected(true);
        attributes.set_foreground(Color::new(ColorSource::Base256, 200));
        attributes.set_background(Color::rgb(0x12_34_56));

        assert!(attributes.bold());
        assert!(attributes.underline());
        assert!(attributes.clean());
        assert!(attributes.selected());
        assert!(!attributes.italic());
        assert_eq!(
            attributes.foreground(),
            Color::new(ColorSource::Base256, 200)
        );
        assert_eq!(attributes.background(), Color::rgb(0x12_34_56));

        attributes.set_bold(false);
        attributes.set_foreground(Color::default());
        assert!(!attributes.bold());
        assert_eq!(attributes.foreground(), Color::default());
        assert_eq!(attributes.background(), Color::rgb(0x12_34_56));
    }

    #[test]
    fn every_boolean_attribute_uses_an_independent_bit() {
        type Getter = fn(Attributes) -> bool;
        type Setter = fn(&mut Attributes, bool);
        let flags: [(Getter, Setter); 12] = [
            (Attributes::bold, Attributes::set_bold),
            (Attributes::dim, Attributes::set_dim),
            (Attributes::italic, Attributes::set_italic),
            (Attributes::underline, Attributes::set_underline),
            (Attributes::strikethrough, Attributes::set_strikethrough),
            (Attributes::blink, Attributes::set_blink),
            (Attributes::conceal, Attributes::set_conceal),
            (Attributes::reverse, Attributes::set_reverse),
            (Attributes::clean, Attributes::set_clean),
            (Attributes::confined, Attributes::set_confined),
            (Attributes::selected, Attributes::set_selected),
            (Attributes::url, Attributes::set_url),
        ];

        for (changed, (_, setter)) in flags.iter().enumerate() {
            let mut attributes = Attributes::default();
            setter(&mut attributes, true);
            for (checked, (getter, _)) in flags.iter().enumerate() {
                assert_eq!(getter(attributes), changed == checked);
            }
        }
    }

    #[test]
    fn color_fields_preserve_each_other_at_boundaries() {
        let mut attributes = Attributes::default();
        attributes.set_foreground(Color::new(ColorSource::Base16, COLOR_MASK));
        attributes.set_background(Color::rgb(COLOR_MASK));

        assert_eq!(
            attributes.foreground(),
            Color::new(ColorSource::Base16, COLOR_MASK)
        );
        assert_eq!(attributes.background(), Color::rgb(COLOR_MASK));

        attributes.set_background(Color::default());
        assert_eq!(
            attributes.foreground(),
            Color::new(ColorSource::Base16, COLOR_MASK)
        );
        assert_eq!(attributes.background(), Color::default());
    }

    #[test]
    fn cell_content_preserves_scalar_composed_and_spacer_states() {
        let max_spacer = u32::MAX - SPACER_BASE;
        for content in [
            CellContent::Empty,
            CellContent::Scalar('界'),
            CellContent::Composed(0),
            CellContent::Composed(COMPOSED_KEY_MAX),
            CellContent::Spacer(0),
            CellContent::Spacer(1),
            CellContent::Spacer(max_spacer),
        ] {
            let cell = Cell::new(content);
            assert_eq!(cell.content(), content);
            assert_eq!(cell.is_spacer(), matches!(content, CellContent::Spacer(_)));
        }
    }

    #[test]
    #[should_panic(expected = "NUL is reserved for an empty cell")]
    fn scalar_nul_is_rejected_instead_of_becoming_empty() {
        let _ = Cell::new(CellContent::Scalar('\0'));
    }

    #[test]
    fn compact_types_match_the_phase_one_baseline() {
        assert_eq!(size_of::<ColorSource>(), 1);
        assert_eq!(size_of::<Attributes>(), 8);
        assert_eq!(size_of::<Cell>(), 12);
    }
}
