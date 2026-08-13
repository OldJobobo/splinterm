# Plan 0036: Alpha3 Wayland file-drop path insertion

- **Status:** Planned for `0.1.0-alpha3`
- **Date:** 2026-08-12
- **Product authority:** Dropping local files is an explicit request to insert
  bounded shell-escaped paths into one exact Splint; it never reads file bodies,
  submits a command, or grants filesystem authority
- **Depends on:** accepted native Wayland data-device handling, bounded clipboard
  pipe I/O, exact pane targeting, controller ownership, and bracketed paste

## Decision

Before publishing `0.1.0-alpha3`, support dropping one or more local files from a
Wayland file manager onto a Splinterm pane. A successful drop inserts a
space-separated sequence of POSIX-shell-escaped local paths into the exact live
Splint under the accepted drop target.

The drop must not append Return or newline, execute the resulting text, inspect
file contents, upload files, persist paths, or infer that the foreground process
is a shell. The feature is a bounded input convenience, not a file-transfer or
application-integration protocol.

Clipboard image saving and insertion remain a separate post-alpha3 feature. This
plan does not require decoding image clipboard MIME types or creating files.

## Confirmed baseline

The native client already binds `wl_data_device_manager`, handles clipboard and
primary selection offers, provides bounded deadline-driven pipe reads, tracks
pane geometry and controller ownership, and encodes safe bracketed paste.

Drag offers are currently rejected in
`crates/splinterm/src/wayland/dispatch/clipboard.rs`: the client accepts no MIME
type, advertises no drag action, and performs no operation on drop. Alpha3 adds
one narrow receive-only path without changing clipboard text behavior.

## Alpha3 behavior contract

### Accepted offer

- Accept only `text/uri-list` for this feature.
- Advertise and select only the Wayland copy action; never move, delete, or claim
  ownership of the source.
- Bound the received offer by byte count, URI count, worker count, and read
  deadline before parsing or insertion.
- Parse the URI-list format strictly: ignore comment lines, accept CRLF or LF
  separators, and reject malformed percent encoding, embedded control
  characters, NUL, empty entries, and overlong paths.
- Reject a URI when its percent-decoded path is not valid UTF-8; never use lossy
  decoding that could insert a path different from the source offer.
- Accept only absolute local `file:` URIs with an empty authority or `localhost`.
  Reject remote hosts, non-file schemes, relative paths, and unsupported portal
  or file-manager-private formats with clear non-terminal feedback.
- Require every decoded target to identify an existing local regular file at the
  time of insertion. Directories, devices, sockets, FIFOs, and missing paths are
  outside this alpha3 contract.

### Exact target and authority

- Resolve the pane under the drop coordinate when the offer enters and capture
  its Dojo, Splint, incarnation, tab, and input generation.
- Accept the drop only while that exact pane remains present, live, visible, and
  controlled by the current graphical connection.
- Reject stale offers after focus/tab changes, topology replacement, controller
  loss, modal entry, disconnect, or target process reincarnation. Never retarget
  a drop to whichever Splint is focused when asynchronous reading finishes.
- A trusted modal, picker, prompt, consent surface, tab strip, divider, or other
  application chrome is not a terminal drop target.

### Inserted text

- Encode each decoded path with one documented, deterministic POSIX shell-quote
  function that preserves spaces, quotes, Unicode, leading dashes, and other
  printable filename bytes representable as UTF-8.
- Join multiple quoted paths with one ASCII space in source order.
- Send the result through the same bounded terminal-input and bracketed-paste
  policy used by ordinary safe paste.
- Insert no trailing space, newline, carriage return, NUL, or command separator.
- If bracketed paste mode is active, wrap the complete joined payload once.
- Emit either the complete validated payload exactly once or no PTY bytes.

### Feedback and privacy

- Show bounded local feedback for rejected MIME types, malformed or remote URIs,
  stale targets, missing control, read timeout, oversized offers, and failed
  insertion.
- Do not echo full paths in logs, audit records, diagnostics, notifications, or
  error text. Tests may use synthetic fixtures.
- Permit only bounded transient in-memory path handling required to validate and
  send the all-or-nothing payload; clear it after completion or cancellation.
- Do not open, stat beyond required type/existence validation, hash, thumbnail,
  copy, upload, persist, or retain dropped file contents or paths after the
  operation.
- Terminal output cannot initiate or approve a drop.

## Explicitly outside alpha3

- clipboard image decoding, saving, naming, cleanup, or path insertion;
- dropping directories, remote URLs, portal document handles, or virtual files;
- file upload to remote Splints or path translation across hosts/containers;
- reading, previewing, hashing, scanning, or attaching file contents;
- shell detection or shell-specific escaping selected from terminal output;
- automatically appending Return, executing a command, or confirming a command;
- drag-out/file-export support; and
- support for private Nautilus, KDE, browser, or application-specific MIME types.

## Validation milestones

### Milestone 1 — pure URI and quoting policy

- extract bounded URI-list parsing and deterministic POSIX path quoting into pure
  helpers;
- test LF/CRLF, comments, percent decoding, spaces, apostrophes, Unicode,
  leading dashes, multiple files, malformed encodings, controls, remote hosts,
  wrong schemes, missing files, directories, special files, size/count limits,
  and all-or-nothing behavior;
- prove generated payloads contain no implicit submission bytes;
- run focused tests, formatting, strict affected-crate Clippy, and
  `git diff --check`.

### Milestone 2 — Wayland receive and exact targeting

- accept only `text/uri-list` with copy semantics;
- reuse bounded worker/deadline infrastructure without weakening clipboard
  limits;
- capture and revalidate the exact pane/Splint/incarnation/input generation;
- prove modal, tab-strip, divider, hidden-tab, stale-target, controller-loss,
  repeat-drop, cancellation, timeout, and disconnect isolation;
- prove bracketed and ordinary insertion send the complete payload exactly once;
- record focused and serial workspace evidence and obtain fresh read-only input
  and privacy review.

### Milestone 3 — packaged graphical acceptance

After separate approval under the repository graphical-testing rules, use the
installed adjacent trusted client and daemon in one isolated Window to:

1. drop one harmless local file whose path contains spaces and an apostrophe;
2. prove the exact shell-escaped path appears without executing anything;
3. drop multiple harmless files and prove ordering and spacing;
4. repeat with bracketed paste enabled;
5. prove a drop over application chrome or a non-controlled pane is rejected;
6. prove a remote or unsupported URI produces bounded feedback and zero PTY
   input; and
7. remove fixtures and restore topology, Window, focus, workspace, monitor,
   geometry, configuration, and package state.

Abort on wrong-pane input, command execution, any file-content read or move,
unrelated topology mutation, unexpected focus movement, or incomplete cleanup.

## Alpha3 acceptance

Plan 0036 is complete only when:

- local regular-file drops insert deterministic shell-escaped paths into the
  exact accepted Splint;
- malformed, remote, unsupported, stale, and unauthorized drops send zero PTY
  bytes;
- the complete multi-file payload is all-or-nothing, bounded, and never includes
  submission bytes;
- bracketed-paste behavior and modal/input isolation pass;
- focused and serial validation evidence is recorded;
- a fresh read-only review has no unresolved blockers; and
- separately approved packaged graphical acceptance is recorded.

This plan does not authorize installation, graphical testing, pushing, candidate
dispatch, promotion approval, AUR publication, or release publication.
