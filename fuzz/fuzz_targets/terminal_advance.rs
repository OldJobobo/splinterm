#![no_main]

use libfuzzer_sys::fuzz_target;
use splinterm_terminal::{Terminal, TerminalConfig};

fuzz_target!(|data: &[u8]| {
    let mut terminal = Terminal::new(80, 24, TerminalConfig::default());
    for chunk in data.chunks(1 + data.len() % 31) {
        terminal.advance(chunk);
        let grid = terminal.grid();
        let cursor = grid.cursor().position();
        assert!(cursor.column >= 0 && usize::try_from(cursor.column).unwrap() < grid.columns());
        assert!(cursor.row >= 0 && usize::try_from(cursor.row).unwrap() < grid.screen_rows());
        for row in 0..grid.screen_rows() {
            let row = grid.row(i32::try_from(row).unwrap()).unwrap();
            assert_eq!(row.len(), grid.columns());
            assert!(row.has_valid_wide_cells());
        }
    }
});
