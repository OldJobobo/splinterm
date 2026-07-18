//! Narrow safe-Rust translation of Foot's terminal box-drawing geometry.
//!
//! Source: Foot 1.27.0 `box-drawing.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e` (MIT). Only U+2500,
//! U+250C, U+2510, and U+253C are translated for the deterministic corpus.
//! The integer midpoint formulas intentionally match Foot's `_hline_middle*`
//! and `_vline_middle*` helpers; this is not a general box-drawing renderer.

/// An 8-bit alpha mask occupying one complete terminal cell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoxMask {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) data: Vec<u8>,
}

/// Generates one of the four box glyphs translated for the evidence corpus.
pub(crate) fn generate(
    character: char,
    width: u32,
    height: u32,
    thickness: u32,
) -> Option<BoxMask> {
    if width == 0 || height == 0 || thickness == 0 {
        return None;
    }
    let mut mask = BoxMask {
        width,
        height,
        data: vec![0; usize::try_from(width.checked_mul(height)?).ok()?],
    };
    let horizontal_y = height.saturating_sub(thickness) / 2;
    let vertical_x = width.saturating_sub(thickness) / 2;
    match character {
        '─' => mask.horizontal(0, width, horizontal_y, thickness),
        '┌' => {
            mask.horizontal(vertical_x, width, horizontal_y, thickness);
            mask.vertical(horizontal_y, height, vertical_x, thickness);
        }
        '┐' => {
            mask.horizontal(
                0,
                width.saturating_add(thickness) / 2,
                horizontal_y,
                thickness,
            );
            mask.vertical(horizontal_y, height, vertical_x, thickness);
        }
        '┼' => {
            mask.horizontal(0, width, horizontal_y, thickness);
            mask.vertical(0, height, vertical_x, thickness);
        }
        _ => return None,
    }
    Some(mask)
}

impl BoxMask {
    fn horizontal(&mut self, x1: u32, x2: u32, y: u32, thickness: u32) {
        self.fill(x1, y, x2, y.saturating_add(thickness));
    }

    fn vertical(&mut self, y1: u32, y2: u32, x: u32, thickness: u32) {
        self.fill(x, y1, x.saturating_add(thickness), y2);
    }

    fn fill(&mut self, x1: u32, y1: u32, x2: u32, y2: u32) {
        for y in y1.min(self.height)..y2.min(self.height) {
            for x in x1.min(self.width)..x2.min(self.width) {
                let index =
                    usize::try_from(y * self.width + x).expect("cell mask index fits usize");
                self.data[index] = 0xff;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(mask: &BoxMask, x: u32, y: u32) -> u8 {
        mask.data[usize::try_from(y * mask.width + x).expect("index fits")]
    }

    #[test]
    fn horizontal_uses_foot_centering_for_odd_and_even_cells() {
        let odd = generate('─', 7, 9, 1).expect("supported");
        let even = generate('─', 8, 10, 2).expect("supported");
        assert!((0..7).all(|x| pixel(&odd, x, 4) == 0xff));
        assert_eq!(odd.data.iter().filter(|value| **value != 0).count(), 7);
        assert!((0..8).all(|x| pixel(&even, x, 4) == 0xff && pixel(&even, x, 5) == 0xff));
        assert_eq!(even.data.iter().filter(|value| **value != 0).count(), 16);
    }

    #[test]
    fn corners_and_cross_have_continuous_centered_joins() {
        for (width, height, thickness) in [(7, 9, 1), (8, 10, 2)] {
            let center_x = (width - thickness) / 2;
            let center_y = (height - thickness) / 2;
            for character in ['┌', '┐', '┼'] {
                let mask = generate(character, width, height, thickness).expect("supported");
                for y in center_y..center_y + thickness {
                    for x in center_x..center_x + thickness {
                        assert_eq!(pixel(&mask, x, y), 0xff, "gap in {character}");
                    }
                }
            }
        }
    }

    #[test]
    fn unsupported_or_invalid_requests_are_rejected() {
        assert!(generate('A', 8, 16, 1).is_none());
        assert!(generate('─', 0, 16, 1).is_none());
    }
}
