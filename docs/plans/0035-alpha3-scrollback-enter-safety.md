# Plan 0035: Alpha3 scrollback Enter safety

- **Status:** Complete for `0.1.0-alpha3`; implementation, review, and installed-package graphical acceptance recorded
- **Date:** 2026-08-12
- **Product authority:** Enter submits terminal input only while the focused
  Splint is following live output
- **Depends on:** accepted bounded graphical scrollback and the resolved keymap
  `history.return-live` action

## Decision

When the focused Splint's viewport is above the live/current input line, plain
Enter and keypad Enter must perform the same viewport transition as
`history.return-live` (`Shift+End` in the built-in profiles). They must not send
carriage return, newline, or any other bytes to the PTY.

Enter reaches the PTY normally only when the focused Splint was already following
live output before that physical key press.

## Confirmed baseline defect

The current history-key path consumes configured Page Up, Page Down, and Return
to Live actions. Plain Enter is not classified as history navigation, so it
falls through to terminal input encoding and sends `\r` even when the user is
reading historical output.

This can accidentally execute a partially typed command that is not visible in
the historical viewport.

## Alpha3 behavior contract

1. On a non-repeated press of `Return` or `KP_Enter`, inspect the focused
   Splint's authoritative client-local viewport before terminal encoding.
2. If the viewport is historical:
   - consume the event;
   - return that focused Splint to live output using the existing
     `HistoryNavigation::ReturnToLive` path;
   - schedule the required redraw and cursor/IME reconciliation;
   - send zero input bytes and zero terminal mouse events; and
   - mark that physical key as consumed until release.
3. Repeated events for an Enter key that initiated return-to-live remain
   consumed even though the first event has already made the viewport live.
   Holding one key must never turn the same physical press into PTY input.
4. Release clears only the matching consumed-key state.
5. When the viewport was already live before the press, Enter retains normal
   terminal behavior and sends the existing carriage-return encoding.
6. The rule is focused-Splint-local. It must not alter hidden tabs, other panes,
   daemon topology, controller ownership, or another client's viewport.
7. Existing trusted modal behavior remains authoritative:
   - Enter activates palette/menu choices or submits owned prompts/search where
     already documented;
   - copy mode retains its own local Enter semantics, if any;
   - consent and confirmation surfaces retain their exact rules; and
   - no modal Enter leaks to either history navigation or the PTY.
8. Configurable bindings may still invoke `history.return-live`; plain Enter's
   historical-viewport safety behavior is an invariant and cannot be unbound.

Returning live should preserve the existing `Shift+End` semantics for viewport,
search highlighting, selection, follow-live, and damage. This plan must not
invent a second subtly different jump-to-bottom implementation.

## Validation

### Pure and focused tests

Add tests proving:

- Return and keypad Enter on a historical viewport dispatch Return to Live and
  encode no PTY bytes;
- Return and keypad Enter on a live viewport retain existing PTY encoding;
- press → repeat → release while initially historical sends no PTY bytes;
- the next distinct Enter press after release, while live, sends exactly one
  normal carriage return;
- Page Up followed by Enter returns live without terminal input;
- pointer-wheel and configured history navigation reach the same state;
- active-pane changes use the newly focused pane's viewport;
- hidden tabs and inactive panes remain unchanged;
- modal palette, picker, prompt, search, help, copy-mode, and consent Enter paths
  retain precedence and isolation; and
- redraw scheduling and focus/IME cursor state are correct after return-to-live.

Run focused client tests, formatting, strict affected-crate Clippy, and
`git diff --check`. Include the behavior in the coherent alpha3 serial workspace
validation and independent review.

### Packaged graphical acceptance

After separate approval under the repository graphical-testing rules:

1. open one isolated installed Splinterm Window with a harmless shell fixture;
2. type but do not submit a visible marker command;
3. move into scrollback and press Enter once;
4. prove the viewport returns live and the marker command did not execute;
5. press Enter again while live and prove normal submission occurs exactly once;
6. repeat with keypad Enter if the guarded input harness can identify it
   reliably; and
7. close the exact test Window, end its test Splint, and restore focus,
   workspace, monitor, geometry, and input state.

Abort on wrong-window input, any submission from the historical Enter press,
unrelated viewport/topology mutation, or incomplete cleanup.

## Implementation evidence (2026-08-13)

- Plain Return and keypad Enter inspect the focused Splint's authoritative
  viewport before terminal encoding. A historical viewport uses the existing
  Return-to-Live path and emits zero PTY bytes.
- The initiating raw key is retained in bounded consumed-key state through
  repeat and cleared only by its matching release, so one held press cannot
  become terminal input after the viewport reaches live output.
- Enter pressed while already live retains ordinary carriage-return behavior.
  Palette, picker, prompt, search, help, copy-mode, consent, and confirmation
  handling remain earlier authoritative modal routes.
- Focused tests cover Return and keypad Enter, press/repeat/release, subsequent
  live submission, pane-local viewport choice, redraw, and modal isolation.
- On the coherent pre-release worktree, `cargo fmt --all --check`, strict
  workspace Clippy, `cargo test --workspace`, release/package tooling tests,
  site check/build, shell validation, and `git diff --check` pass.
- A fresh read-only correctness review confirmed modal precedence, historical
  plain/keypad Enter routing, exact raw-key consumption through matching release,
  and the evidence scope, with no blockers or fixes worth doing now.

## Installed-package graphical evidence (2026-08-13)

- In the optimized installed client, Return on historical output returned the
  exact focused Splint to live state without executing a prepared command.
- The initiating press remained consumed through release. A subsequent Return
  from the live viewport submitted exactly once, while palette and confirmation
  Return retained their earlier modal authority.

## Alpha3 acceptance

Plan 0035 is complete only when:

- historical Enter and keypad Enter return the focused Splint to live output;
- the initiating physical key is consumed through release;
- zero bytes reach the PTY for that press;
- live Enter retains ordinary terminal behavior;
- modal and multi-pane/tab isolation regressions pass;
- focused and serial evidence plus fresh read-only review are recorded; and
- separately approved packaged graphical acceptance is recorded.

This plan does not authorize installation, graphical testing, pushing, candidate
dispatch, promotion approval, AUR publication, or release publication.
