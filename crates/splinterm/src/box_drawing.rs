//! Narrow safe-Rust translation of Foot's terminal box-drawing geometry.
//!
//! Source: Foot 1.27.0 `box-drawing.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e` (MIT). U+2500,
//! U+250C, U+2510, U+253C, and the U+2800–U+28FF Braille range are translated.
//! The integer midpoint and Braille dot formulas intentionally match Foot's
//! helpers; this is not yet a general box-drawing renderer.

/// An 8-bit alpha mask occupying one complete terminal cell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoxMask {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) data: Vec<u8>,
}

/// Generates a translated box or Braille glyph.
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
    if ('\u{2800}'..='\u{28ff}').contains(&character) {
        mask.draw_braille(u8::try_from(u32::from(character) - 0x2800).ok()?);
        return Some(mask);
    }
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
    #[allow(
        clippy::too_many_lines,
        reason = "the adjustment order is a direct translation of Foot's Braille geometry"
    )]
    fn draw_braille(&mut self, dots: u8) {
        let width = i32::try_from(self.width).expect("cell width fits i32");
        let height = i32::try_from(self.height).expect("cell height fits i32");
        let mut dot = (width / 4).min(height / 8);
        let mut x_spacing = width / 4;
        let mut y_spacing = height / 8;
        let mut x_margin = x_spacing / 2;
        let mut y_margin = y_spacing / 2;
        let mut x_left = width - 2 * x_margin - x_spacing - 2 * dot;
        let mut y_left = height - 2 * y_margin - 3 * y_spacing - 4 * dot;

        if x_left >= 2 && y_left >= 4 && dot == 0 {
            dot += 1;
            x_left -= 2;
            y_left -= 4;
        }
        if x_left >= 2 && x_margin == 0 {
            x_margin = 1;
            x_left -= 2;
        }
        if y_left >= 2 && y_margin == 0 {
            y_margin = 1;
            y_left -= 2;
        }
        if x_left >= 1 {
            x_spacing += 1;
            x_left -= 1;
        }
        if y_left >= 3 {
            y_spacing += 1;
            y_left -= 3;
        }
        if x_left >= 2 {
            x_margin += 1;
            x_left -= 2;
        }
        if y_left >= 2 {
            y_margin += 1;
            y_left -= 2;
        }
        if x_left >= 2 && y_left >= 4 {
            dot += 1;
        }
        if dot <= 0 {
            return;
        }

        let xs = [x_margin, x_margin + dot + x_spacing];
        let ys = [
            y_margin,
            y_margin + dot + y_spacing,
            y_margin + 2 * (dot + y_spacing),
            y_margin + 3 * (dot + y_spacing),
        ];
        let positions = [
            (0_usize, 0_usize, 0_u8),
            (0, 1, 1),
            (0, 2, 2),
            (1, 0, 3),
            (1, 1, 4),
            (1, 2, 5),
            (0, 3, 6),
            (1, 3, 7),
        ];
        for (column, row, bit) in positions {
            if dots & (1 << bit) == 0 {
                continue;
            }
            let x = u32::try_from(xs[column]).expect("Braille x is nonnegative");
            let y = u32::try_from(ys[row]).expect("Braille y is nonnegative");
            let dot = u32::try_from(dot).expect("Braille dot is positive");
            self.fill(x, y, x + dot, y + dot);
        }
    }

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
    fn braille_bits_map_to_foot_dot_positions() {
        let top_left = generate('\u{2801}', 14, 30, 1).expect("Braille supported");
        let bottom_right = generate('\u{2880}', 14, 30, 1).expect("Braille supported");
        assert!(top_left.data.iter().any(|alpha| *alpha != 0));
        assert!(bottom_right.data.iter().any(|alpha| *alpha != 0));
        let first_top = top_left
            .data
            .iter()
            .position(|alpha| *alpha != 0)
            .expect("top-left dot");
        let first_bottom = bottom_right
            .data
            .iter()
            .position(|alpha| *alpha != 0)
            .expect("bottom-right dot");
        assert!(first_bottom > first_top);
    }

    #[test]
    fn unsupported_or_invalid_requests_are_rejected() {
        assert!(generate('A', 8, 16, 1).is_none());
        assert!(generate('─', 0, 16, 1).is_none());
    }
}
