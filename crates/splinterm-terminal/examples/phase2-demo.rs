use std::{
    env,
    io::{self, Write},
    thread,
    time::Duration,
};

use splinterm_terminal::{CellContent, Color, Grid, ScrollDirection, ScrollRegion};

struct Demo {
    delay: Duration,
    frame: usize,
}

impl Demo {
    fn new() -> Self {
        let delay = env::var("SPLINTERM_DEMO_DELAY_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(6_000);
        Self {
            delay: Duration::from_millis(delay),
            frame: 0,
        }
    }

    fn show(&mut self, title: &str, note: &str, grid: &Grid) {
        self.frame += 1;
        print!("\x1b[2J\x1b[H");
        println!("\x1b[1;38;2;126;200;227mSPLINTERM  ·  PHASE 2 GRID DEMO\x1b[0m");
        println!(
            "\x1b[2mFoot-derived Rust state · frame {}\x1b[0m\n",
            self.frame
        );
        println!("\x1b[1;38;2;240;198;116m{title}\x1b[0m");
        println!("\x1b[38;2;180;180;180m{note}\x1b[0m\n");
        render_grid(grid);
        println!(
            "\n\x1b[2moffset={}  view={}  capacity={}  screen={}×{}\x1b[0m",
            grid.offset(),
            grid.view(),
            grid.row_capacity(),
            grid.columns(),
            grid.screen_rows()
        );
        std::io::stdout().flush().expect("flush demo frame");
        thread::sleep(self.delay);
    }
}

fn main() {
    let mut demo = Demo::new();
    loop {
        demo.frame = 0;
        circular_history(&mut demo);
        partial_scroll(&mut demo);
        erase_and_dirty(&mut demo);
        logical_reflow(&mut demo);
        wide_cells(&mut demo);
        if !finish_and_prompt() {
            break;
        }
    }
}

fn circular_history(demo: &mut Demo) {
    let mut grid = Grid::with_screen_size(16, 20, 5);
    for (row, text) in ["ALPHA", "BRAVO", "CHARLIE", "DELTA", "ECHO"]
        .into_iter()
        .enumerate()
    {
        write_row(&mut grid, row, text, true);
    }
    demo.show(
        "1 / 5  Circular history",
        "Five visible rows before a full-screen upward scroll.",
        &grid,
    );
    grid.scroll(
        ScrollDirection::Forward,
        ScrollRegion::new(0, 5),
        1,
        Color::default(),
    );
    write_row(&mut grid, 4, "FOXTROT", true);
    demo.show(
        "1 / 5  Circular history",
        "ALPHA moved into scrollback; FOXTROT entered without moving row storage.",
        &grid,
    );
}

fn partial_scroll(demo: &mut Demo) {
    let mut grid = Grid::with_screen_size(16, 24, 5);
    for (row, text) in ["── HEADER ──", "one", "two", "three", "── FOOTER ──"]
        .into_iter()
        .enumerate()
    {
        write_row(&mut grid, row, text, true);
    }
    demo.show(
        "2 / 5  Partial scroll region",
        "Only rows 1..4 will scroll; HEADER and FOOTER remain anchored.",
        &grid,
    );
    grid.scroll(
        ScrollDirection::Forward,
        ScrollRegion::new(1, 4),
        1,
        Color::default(),
    );
    write_row(&mut grid, 3, "four", true);
    demo.show(
        "2 / 5  Partial scroll region",
        "The middle region advanced while both non-scrolling regions survived.",
        &grid,
    );
}

fn erase_and_dirty(demo: &mut Demo) {
    let mut grid = Grid::with_screen_size(8, 30, 3);
    write_row(&mut grid, 0, "token: swordfish", true);
    write_row(&mut grid, 1, "background survives erase", true);
    demo.show(
        "3 / 5  Erase and dirty state",
        "The next frame erases the secret with an RGB erase background.",
        &grid,
    );
    grid.row_mut(0)
        .expect("visible row")
        .erase(7..16, Color::rgb(0x35_2f_44));
    demo.show(
        "3 / 5  Erase and dirty state",
        "Erased cells are empty, background-aware, and marked dirty for presentation.",
        &grid,
    );
}

fn logical_reflow(demo: &mut Demo) {
    let mut grid = Grid::with_screen_size(16, 20, 3);
    write_row(&mut grid, 0, "logical lines survive", false);
    write_row(&mut grid, 1, " width changes", true);
    demo.show(
        "4 / 5  Logical-line reflow",
        "A soft linebreak joins the first two physical rows into one logical line.",
        &grid,
    );
    grid.resize_with_reflow(16, 12, 4, |_| 1);
    demo.show(
        "4 / 5  Logical-line reflow",
        "Narrowed to 12 columns: content rewrapped and boundaries survived.",
        &grid,
    );
    grid.resize_with_reflow(16, 28, 3, |_| 1);
    demo.show(
        "4 / 5  Logical-line reflow",
        "Widened to 28 columns: the logical line joined without flattening history.",
        &grid,
    );
}

fn wide_cells(demo: &mut Demo) {
    let mut grid = Grid::with_screen_size(8, 14, 3);
    let row = grid.row_mut(1).expect("visible row");
    row[4].set_content(CellContent::Scalar('界'));
    row[5].set_content(CellContent::Spacer(1));
    row.set_linebreak(true);
    demo.show(
        "5 / 5  Wide-cell invariant",
        "The leader and continuation occupy one indivisible two-column unit.",
        &grid,
    );
    grid.resize_with_reflow(8, 5, 3, |_| 1);
    demo.show(
        "5 / 5  Wide-cell invariant",
        "After narrowing, the wide unit moved intact instead of splitting at the edge.",
        &grid,
    );
}

fn finish_and_prompt() -> bool {
    print!("\x1b[2J\x1b[H");
    println!("\n\x1b[1;38;2;126;200;227m  PHASE 2 COMPLETE\x1b[0m\n");
    println!("  Circular storage     \x1b[38;2;140;220;160m✓\x1b[0m");
    println!("  Partial scrolling    \x1b[38;2;140;220;160m✓\x1b[0m");
    println!("  Erase + dirty state  \x1b[38;2;140;220;160m✓\x1b[0m");
    println!("  Resize + reflow      \x1b[38;2;140;220;160m✓\x1b[0m");
    println!("  Wide-cell safety     \x1b[38;2;140;220;160m✓\x1b[0m\n");
    println!("\n\x1b[1m  [R] + Enter  replay     [Q] + Enter  close\x1b[0m");
    print!("  choice › ");
    io::stdout().flush().expect("flush replay prompt");
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "r" | "replay")
}

fn write_row(grid: &mut Grid, row: usize, text: &str, linebreak: bool) {
    let columns = grid.columns();
    let row = grid
        .row_mut(i32::try_from(row).expect("demo row fits in i32"))
        .expect("demo row is visible");
    row.erase_all(Color::default());
    for (column, character) in text.chars().take(columns).enumerate() {
        row[column].set_content(CellContent::Scalar(character));
    }
    row.set_linebreak(linebreak);
}

fn render_grid(grid: &Grid) {
    println!("    ┌{}┐", "─".repeat(grid.columns()));
    for row_number in 0..grid.screen_rows() {
        let row = grid
            .row(i32::try_from(row_number).expect("demo row fits in i32"))
            .expect("visible row is initialized");
        let mut cells = String::new();
        for cell in row.cells() {
            cells.push(match cell.content() {
                CellContent::Empty => ' ',
                CellContent::Scalar(character) => character,
                CellContent::Composed(_) => '◌',
                CellContent::Spacer(0) => '·',
                CellContent::Spacer(_) => '›',
            });
        }
        let boundary = if row.has_linebreak() { '↵' } else { '↪' };
        let dirty = if row.is_dirty() {
            "\x1b[38;2;240;140;140m●\x1b[0m"
        } else {
            "○"
        };
        println!(" {row_number:>2} {dirty}│{cells}│ {boundary}");
    }
    println!("    └{}┘", "─".repeat(grid.columns()));
}
