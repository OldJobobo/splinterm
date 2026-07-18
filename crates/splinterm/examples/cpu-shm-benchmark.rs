//! CPU canvas baseline for the Roadmap Phase 2 SHM renderer spike.
//!
//! Run with `cargo run --release -p splinterm --example cpu-shm-benchmark`.

use std::{hint::black_box, time::Instant};

const CELL_WIDTH: usize = 10;
const CELL_HEIGHT: usize = 20;

fn main() {
    println!("Splinterm CPU/SHM paint baseline");
    println!("cell={CELL_WIDTH}x{CELL_HEIGHT} px");
    println!();
    println!("grid       pixels       bytes       iterations   allocate+paint   reuse+paint");

    for (columns, rows, iterations) in [(80, 24, 100), (120, 40, 50), (240, 80, 20)] {
        let width = columns * CELL_WIDTH;
        let height = rows * CELL_HEIGHT;
        let bytes = width * height * 4;
        let allocate = measure_allocate(width, height, iterations);
        let reuse = measure_reuse(width, height, iterations);
        println!(
            "{columns:>3}x{rows:<3}   {width:>4}x{height:<4}   {bytes:>10}   {iterations:>10}   {:>10.3} ms   {:>8.3} ms",
            allocate * 1_000.0,
            reuse * 1_000.0,
        );
    }
}

fn measure_allocate(width: usize, height: usize, iterations: u32) -> f64 {
    let start = Instant::now();
    for frame in 0..iterations {
        let mut canvas = vec![0_u8; width * height * 4];
        let frame = usize::try_from(frame).expect("frame fits usize");
        paint(&mut canvas, width, height, frame);
        black_box(canvas[frame % canvas.len()]);
    }
    (start.elapsed() / iterations).as_secs_f64()
}

fn measure_reuse(width: usize, height: usize, iterations: u32) -> f64 {
    let mut canvas = vec![0_u8; width * height * 4];
    let start = Instant::now();
    for frame in 0..iterations {
        let frame = usize::try_from(frame).expect("frame fits usize");
        paint(&mut canvas, width, height, frame);
        black_box(canvas[frame % canvas.len()]);
    }
    (start.elapsed() / iterations).as_secs_f64()
}

fn paint(canvas: &mut [u8], width: usize, height: usize, frame: usize) {
    let pulse = u8::try_from((frame / 2) % 24).expect("pulse fits u8");
    let cell_width = (width / 12).max(1);
    let cell_height = (height / 8).max(1);
    for (index, pixel) in canvas.chunks_exact_mut(4).enumerate() {
        let x = index % width;
        let y = index / width;
        let header = y < height / 6;
        let grid_line = x % cell_width < 2 || y % cell_height < 2;
        let accent = x < width / 80 + 8;
        let (red, green, blue) = if header {
            (20, 40 + pulse, 52 + pulse)
        } else if accent {
            (45, 190, 170)
        } else if grid_line {
            (24, 42, 48)
        } else {
            (10, 18, 22)
        };
        pixel.copy_from_slice(&[blue, green, red, 0xff]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_is_deterministic_and_fills_every_alpha_byte() {
        let mut first = vec![0_u8; 32 * 20 * 4];
        let mut second = first.clone();
        paint(&mut first, 32, 20, 7);
        paint(&mut second, 32, 20, 7);
        assert_eq!(first, second);
        assert!(first.chunks_exact(4).all(|pixel| pixel[3] == 0xff));
    }
}
