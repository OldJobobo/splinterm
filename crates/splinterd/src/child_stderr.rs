//! A bounded tail of a trusted dialog child's standard error.
//!
//! A dialog child (the consent prompt, the upgrade and restart questions)
//! answers on a private socket, so a child that fails before answering
//! leaves its asker only an end-of-file. Its standard error is a pipe that a
//! background thread drains to its end, keeping the last
//! [`MAX_CHILD_STDERR_BYTES`], so the asker can say why the dialog failed
//! once the child is reaped. The child is this installation's own client;
//! what it prints is its error chain, never terminal content.

use std::{
    io::{self, Read},
    process::{Child, ExitStatus},
    sync::mpsc,
    thread,
    time::Duration,
};

/// The most standard-error bytes kept from one child: the last ones.
pub const MAX_CHILD_STDERR_BYTES: usize = 4096;

/// How long [`StderrTail::finish`] waits for a reaped child's pipe to close.
/// A descendant that inherited the pipe can hold it open; then nothing is
/// reported rather than the asker waiting on it.
pub const STDERR_TAIL_WAIT: Duration = Duration::from_millis(500);

/// The standard error of one child, drained in the background.
#[derive(Debug)]
pub struct StderrTail {
    tail: Option<mpsc::Receiver<Vec<u8>>>,
}

impl StderrTail {
    /// Starts draining `child`'s piped standard error. A child spawned
    /// without `Stdio::piped()` for it captures nothing.
    pub fn capture(child: &mut Child) -> Self {
        let Some(mut stderr) = child.stderr.take() else {
            return Self { tail: None };
        };
        let (sender, receiver) = mpsc::channel();
        // The thread lives until the pipe reaches end-of-file: when the
        // child and every descendant that inherited its standard error have
        // exited. A descendant that keeps the pipe open keeps the thread,
        // and its bounded buffer, until then; `finish` stops waiting at its
        // bound, and the send to a dropped receiver is ignored.
        let spawned = thread::Builder::new()
            .name("dialog-stderr".to_owned())
            .spawn(move || {
                let _ = sender.send(read_tail(&mut stderr, MAX_CHILD_STDERR_BYTES));
            });
        Self {
            tail: spawned.ok().map(|_| receiver),
        }
    }

    /// What the child reported ([`report`]) once it has exited: `None` when
    /// it printed nothing, or its pipe stayed open past `wait`.
    #[must_use]
    pub fn finish(self, wait: Duration) -> Option<String> {
        let bytes = self.tail?.recv_timeout(wait).ok()?;
        report(&bytes)
    }
}

/// How a failed Splinterm graphical client's last line on standard error
/// begins; its error chain follows ([`client_failure_line`]).
pub const CLIENT_FAILURE_PREFIX: &str = "splinterm graphical client failed";

/// The longest [`client_failure_line`]: with the newline it is printed
/// with, it fits the [`MAX_CHILD_STDERR_BYTES`] tail an asker keeps, so the
/// tail still begins with [`CLIENT_FAILURE_PREFIX`].
pub const MAX_CLIENT_FAILURE_LINE_BYTES: usize = MAX_CHILD_STDERR_BYTES - 1;

/// The line every failed Splinterm graphical client, a trusted dialog
/// child or not, prints last on standard error: [`CLIENT_FAILURE_PREFIX`],
/// then its error chain as one printable line ([`one_line`]), at most
/// [`MAX_CLIENT_FAILURE_LINE_BYTES`] bytes long. The chain is the client's
/// own errors; a configuration it cannot load is named only by category
/// before it gets here, so no configuration value is printed.
#[must_use]
pub fn client_failure_line(error: &anyhow::Error) -> String {
    let Some(reason) = one_line(format!("{error:#}").as_bytes()) else {
        return CLIENT_FAILURE_PREFIX.to_owned();
    };
    let mut line = format!("{CLIENT_FAILURE_PREFIX}: {reason}");
    if line.len() > MAX_CLIENT_FAILURE_LINE_BYTES {
        let mut end = MAX_CLIENT_FAILURE_LINE_BYTES - '…'.len_utf8();
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    line
}

/// What a dialog child reported in `bytes`, its standard error's tail: its
/// last [`CLIENT_FAILURE_PREFIX`] line when there is one, so the lines a
/// client logs as it starts (the fonts it resolved) do not bury the reason;
/// otherwise the whole tail. On one line, as [`one_line`] makes it.
#[must_use]
pub fn report(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .rev()
        .find(|line| line.starts_with(CLIENT_FAILURE_PREFIX))
        .map_or_else(|| one_line(bytes), |line| one_line(line.as_bytes()))
}

/// The error for a dialog child, named by `what`, that gave no usable
/// answer, on one line so a log that shows only the outermost message keeps
/// it all: what the child printed when it did, how it ended, and `error`,
/// the asker's own failure.
#[must_use]
pub fn child_failure(
    what: &str,
    error: &anyhow::Error,
    status: Option<ExitStatus>,
    printed: Option<&str>,
) -> anyhow::Error {
    let ended = status.map_or_else(|| "not reaped".to_owned(), |status| status.to_string());
    match printed {
        Some(printed) => anyhow::anyhow!("{what} failed: {printed} ({ended}; {error:#})"),
        None => anyhow::anyhow!("{what} gave no answer ({ended}): {error:#}"),
    }
}

/// Reads `reader` to its end and returns its last `maximum` bytes. A read
/// error ends it early with what was read.
pub fn read_tail(reader: &mut impl Read, maximum: usize) -> Vec<u8> {
    let mut tail = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                tail.extend_from_slice(&buffer[..read]);
                if tail.len() > maximum {
                    tail.drain(..tail.len() - maximum);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    tail
}

/// `bytes` as one printable line: its non-blank lines, trimmed, joined by
/// "; ", with every other [`unprintable`] character replaced. `None` when
/// blank.
#[must_use]
pub fn one_line(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let line = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
        .chars()
        .map(|character| {
            if unprintable(character) {
                char::REPLACEMENT_CHARACTER
            } else {
                character
            }
        })
        .collect::<String>();
    (!line.is_empty()).then_some(line)
}

/// Whether `character` could change how a log line reads rather than show
/// as itself: a control character (`Cc`), a format character (`Cf`), which
/// includes the bidirectional embeddings, overrides, isolates, and marks, the
/// zero-width characters, and the byte order mark, or a line or paragraph
/// separator.
#[must_use]
pub fn unprintable(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061c}'
                | '\u{06dd}'
                | '\u{070f}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    #[test]
    fn a_failing_child_reports_its_last_bytes_on_one_line() {
        let mut child = Command::new("sh")
            .args([
                "-c",
                "printf 'first\\n\\n  second\\033[1m  \\n' >&2; exit 1",
            ])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let tail = StderrTail::capture(&mut child);
        assert!(!child.wait().unwrap().success());
        assert_eq!(
            tail.finish(Duration::from_secs(10)).as_deref(),
            Some("first; second\u{fffd}[1m")
        );
    }

    #[test]
    fn a_failure_names_what_the_child_printed_and_how_it_ended() {
        let status = Command::new("sh").args(["-c", "exit 1"]).status().unwrap();
        let error = anyhow::anyhow!("failed to fill whole buffer");
        assert_eq!(
            child_failure("the dialog", &error, Some(status), Some("no font")).to_string(),
            "the dialog failed: no font (exit status: 1; failed to fill whole buffer)"
        );
        assert_eq!(
            child_failure("the dialog", &error, None, None).to_string(),
            "the dialog gave no answer (not reaped): failed to fill whole buffer"
        );
    }

    #[test]
    fn a_client_failure_line_carries_the_chain_on_one_bounded_printable_line() {
        let error = anyhow::anyhow!("no compositor").context("connect to Wayland");
        assert_eq!(
            client_failure_line(&error),
            "splinterm graphical client failed: connect to Wayland: no compositor"
        );
        let hostile = anyhow::anyhow!("bad\u{1b}[31m\u{202e}name\nsecond line");
        assert_eq!(
            client_failure_line(&hostile),
            "splinterm graphical client failed: bad\u{fffd}[31m\u{fffd}name; second line"
        );
        assert_eq!(
            client_failure_line(&anyhow::anyhow!(" \n ")),
            "splinterm graphical client failed"
        );
        // ASCII fills the bound exactly; wide characters stop short of it.
        for filler in ["a", "日"] {
            let long = client_failure_line(&anyhow::anyhow!(
                "{}",
                filler.repeat(MAX_CHILD_STDERR_BYTES)
            ));
            assert!(
                long.len() <= MAX_CLIENT_FAILURE_LINE_BYTES,
                "{}",
                long.len()
            );
            assert!(long.ends_with('…'));
            if filler == "a" {
                assert_eq!(long.len(), MAX_CLIENT_FAILURE_LINE_BYTES);
            }
            // Printed with its newline, the whole line survives the bounded
            // tail an asker keeps, so it reports the line, prefix and all.
            let printed = format!("{long}\n");
            let tail = read_tail(&mut printed.as_bytes(), MAX_CHILD_STDERR_BYTES);
            assert_eq!(tail, printed.as_bytes(), "{filler}");
            let reported = report(&tail).unwrap();
            assert!(reported.starts_with(CLIENT_FAILURE_PREFIX), "{reported}");
            assert_eq!(reported, long, "{filler}");
        }
    }

    #[test]
    fn a_client_failure_line_is_reported_over_startup_noise() {
        let tail = b"Resolved primary regular: Hack (face 0, /x.ttf)\n\
            splinterm graphical client failed: no snapshot\n\
            Resolved 208 ordered Fontconfig fallback faces\n";
        assert_eq!(
            report(tail).as_deref(),
            Some("splinterm graphical client failed: no snapshot")
        );
        assert_eq!(
            report(b"thread panicked\nnote: backtrace\n").as_deref(),
            Some("thread panicked; note: backtrace")
        );
    }

    #[test]
    fn bidi_format_and_separator_characters_are_replaced() {
        let replaced = "\u{202a}\u{202e}\u{2066}\u{2069}\u{200b}\u{200f}\u{2028}\u{2029}\u{061c}\u{feff}\u{00ad}\u{e0041}";
        for character in replaced.chars() {
            assert!(unprintable(character), "{:04x}", u32::from(character));
            assert_eq!(
                one_line(format!("bad{character}name").as_bytes()).as_deref(),
                Some("bad\u{fffd}name")
            );
        }
        for character in ['a', 'é', '—', '•', '…', '日', ' '] {
            assert!(!unprintable(character), "{character}");
        }
    }

    #[test]
    fn only_the_last_bytes_are_kept() {
        let mut long = vec![b'a'; 3 * MAX_CHILD_STDERR_BYTES];
        long.extend_from_slice(b"the end");
        let tail = read_tail(&mut long.as_slice(), MAX_CHILD_STDERR_BYTES);
        assert_eq!(tail.len(), MAX_CHILD_STDERR_BYTES);
        assert!(tail.ends_with(b"the end"));
    }

    #[test]
    fn a_silent_or_unpiped_child_reports_nothing() {
        let mut silent = Command::new("true").stderr(Stdio::piped()).spawn().unwrap();
        let tail = StderrTail::capture(&mut silent);
        silent.wait().unwrap();
        assert_eq!(tail.finish(Duration::from_secs(10)), None);

        let mut unpiped = Command::new("true").stderr(Stdio::null()).spawn().unwrap();
        let tail = StderrTail::capture(&mut unpiped);
        unpiped.wait().unwrap();
        assert_eq!(tail.finish(Duration::from_secs(10)), None);
        assert_eq!(one_line(b" \n\t\n"), None);
    }

    #[test]
    fn a_pipe_held_open_stops_waiting_at_the_bound() {
        // The grandchild keeps the pipe open after its parent exits.
        let mut child = Command::new("sh")
            .args(["-c", "sleep 5 & echo started >&2"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let tail = StderrTail::capture(&mut child);
        child.wait().unwrap();
        let started = std::time::Instant::now();
        assert_eq!(tail.finish(Duration::from_millis(100)), None);
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
