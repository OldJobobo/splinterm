use std::{
    env,
    io::{self, Write},
    thread,
    time::Duration,
};

use splinterm_terminal::{
    ActiveScreen, CellContent, ColorSource, Terminal, TerminalConfig, TerminalEvent,
};

struct Demo {
    delay: Duration,
    frame: usize,
}

impl Demo {
    fn new() -> Self {
        let milliseconds = env::var("SPLINTERM_DEMO_DELAY_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1_350);
        Self {
            delay: Duration::from_millis(milliseconds),
            frame: 0,
        }
    }

    fn show(&mut self, heading: &str, note: &str, terminal: &Terminal, effects: &[TerminalEvent]) {
        self.frame += 1;
        print!("\x1b[2J\x1b[H");
        println!("\x1b[1;38;2;126;200;227mSPLINTERM  ·  PHASE 3 VT DEMO\x1b[0m");
        println!(
            "\x1b[2mStreaming Foot-derived Rust kernel · frame {}\x1b[0m\n",
            self.frame
        );
        println!("\x1b[1;38;2;240;198;116m{heading}\x1b[0m");
        println!("\x1b[38;2;180;180;180m{note}\x1b[0m\n");
        render_terminal(terminal);
        if !effects.is_empty() {
            println!("\n\x1b[1mSemantic effects\x1b[0m");
            for effect in effects {
                println!("  \x1b[38;2;170;210;170m→\x1b[0m {effect:?}");
            }
        }
        std::io::stdout().flush().expect("flush demo frame");
        thread::sleep(self.delay);
    }
}

fn main() {
    let mut demo = Demo::new();
    loop {
        demo.frame = 0;
        printable_and_chunking(&mut demo);
        cursor_erase_and_sgr(&mut demo);
        unicode_composition(&mut demo);
        alternate_screen(&mut demo);
        osc_and_replies(&mut demo);
        bounded_recovery(&mut demo);
        if !finish_and_prompt() {
            break;
        }
    }
}

fn printable_and_chunking(demo: &mut Demo) {
    let mut terminal = Terminal::new(24, 4, TerminalConfig::default());
    for byte in b"bytewise parser input wraps without caring about chunk boundaries" {
        terminal.advance(std::slice::from_ref(byte));
    }
    demo.show(
        "1 / 6  Bytewise streaming + deferred wrap",
        "Every byte was fed separately. Soft arrows mark physical rows in one logical line.",
        &terminal,
        &[],
    );
}

fn cursor_erase_and_sgr(demo: &mut Demo) {
    let mut terminal = Terminal::new(30, 4, TerminalConfig::default());
    terminal.advance(b"plain text\x1b[2;4H\x1b[1;38;2;240;170;80mRUST\x1b[0m");
    demo.show(
        "2 / 6  CSI cursor positioning + SGR",
        "CSI H moved the cursor; bold RGB attributes belong to the four RUST cells.",
        &terminal,
        &[],
    );
    terminal.advance(b"\x1b[2;6H\x1b[K");
    demo.show(
        "2 / 6  CSI erase line",
        "CSI K erased from the cursor while preserving semantic background behavior.",
        &terminal,
        &[],
    );
}

fn unicode_composition(demo: &mut Demo) {
    let mut terminal = Terminal::new(24, 4, TerminalConfig::default());
    terminal.advance("wide: 界  composed: e\u{301}".as_bytes());
    demo.show(
        "3 / 6  UTF-8, wide cells, and composition",
        "› is a wide continuation; ◌ is an interned base-plus-combining sequence.",
        &terminal,
        &[],
    );
    terminal.resize(14, 5);
    demo.show(
        "3 / 6  Parser state meets grid reflow",
        "The terminal narrowed after parsing; wide and composed content survived reflow.",
        &terminal,
        &[],
    );
}

fn alternate_screen(demo: &mut Demo) {
    let mut terminal = Terminal::new(28, 4, TerminalConfig::default());
    terminal.advance(b"normal screen survives here");
    terminal.advance(b"\x1b[?1049hALT SCREEN\x1b[2;1Htemporary app state");
    demo.show(
        "4 / 6  DEC alternate screen",
        "The active buffer is alternate; normal content and cursor are saved independently.",
        &terminal,
        &[],
    );
    terminal.advance(b"\x1b[?1049l");
    demo.show(
        "4 / 6  DEC alternate screen",
        "Leaving mode 1049 restored the normal buffer and its saved cursor.",
        &terminal,
        &[],
    );
}

fn osc_and_replies(demo: &mut Demo) {
    let mut terminal = Terminal::new(30, 4, TerminalConfig::default());
    terminal.advance(b"\x1b]2;Splinterm Demo\x07\x1b]4;1;#d46a6a\x07\x1b[2;3H\x1b[5n\x1b[6n");
    let effects = terminal.drain_events().collect::<Vec<_>>();
    demo.show(
        "5 / 6  OSC state + ordered PTY replies",
        "Title/palette mutations and DSR replies are emitted in parser order.",
        &terminal,
        &effects,
    );
}

fn bounded_recovery(demo: &mut Demo) {
    let config = TerminalConfig {
        osc_limit: 18,
        dcs_limit: 12,
        event_limit: 8,
        ..TerminalConfig::default()
    };
    let mut terminal = Terminal::new(30, 4, config);
    terminal.advance(b"\x1b]2;this title is intentionally too long\x07");
    terminal.advance(b"\x1bPqoversized-dcs-payload\x1b\\RECOVERED");
    terminal.advance(b"\xf0(\x8c(  UTF-8 recovery");
    let effects = terminal.drain_events().collect::<Vec<_>>();
    demo.show(
        "6 / 6  Bounded strings + malformed recovery",
        "Oversized strings stayed synchronized; malformed UTF-8 could not panic the kernel.",
        &terminal,
        &effects,
    );
}

fn finish_and_prompt() -> bool {
    print!("\x1b[2J\x1b[H");
    println!("\n\x1b[1;38;2;126;200;227m  PHASE 3 COMPLETE\x1b[0m\n");
    for item in [
        "Streaming VT recognition",
        "UTF-8 + wide composition",
        "CSI editing + SGR colors",
        "DEC modes + alternate screen",
        "OSC state + PTY replies",
        "Bounded malformed recovery",
    ] {
        println!("  {item:<34} \x1b[38;2;140;220;160m✓\x1b[0m");
    }
    println!("\n\x1b[1m  [R] + Enter  replay     [Q] + Enter  close\x1b[0m");
    print!("  choice › ");
    io::stdout().flush().expect("flush replay prompt");
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "r" | "replay")
}

fn render_terminal(terminal: &Terminal) {
    let grid = terminal.grid();
    let screen = match terminal.active_screen() {
        ActiveScreen::Normal => "normal",
        ActiveScreen::Alternate => "alternate",
    };
    println!(
        "  screen={screen}  size={}×{}  cursor={},{}  title={:?}",
        grid.columns(),
        grid.screen_rows(),
        grid.cursor().position().column,
        grid.cursor().position().row,
        terminal.title()
    );
    println!("    ┌{}┐", "─".repeat(grid.columns()));
    for row_number in 0..grid.screen_rows() {
        let row = grid
            .row(i32::try_from(row_number).expect("demo row fits i32"))
            .expect("visible row");
        let mut text = String::new();
        let mut styled_count = 0;
        for cell in row.cells() {
            text.push(match cell.content() {
                CellContent::Empty => ' ',
                CellContent::Scalar(character) => character,
                CellContent::Composed(_) => '◌',
                CellContent::Spacer(0) => '·',
                CellContent::Spacer(_) => '›',
            });
            let attributes = cell.attributes();
            if attributes.bold()
                || attributes.foreground().source() != ColorSource::Default
                || attributes.background().source() != ColorSource::Default
            {
                styled_count += 1;
            }
        }
        let boundary = if row.has_linebreak() { '↵' } else { '↪' };
        let attrs = if styled_count > 0 {
            format!(" \x1b[38;2;240;170;80m{styled_count} styled\x1b[0m")
        } else {
            String::new()
        };
        println!(" {row_number:>2} │{text}│ {boundary}{attrs}");
    }
    println!("    └{}┘", "─".repeat(grid.columns()));
}
