# ADR 0002: Use rustix with an exec-first PTY child helper

- **Status:** Accepted
- **Date:** 2026-07-17

## Context

Phase 5 requires a Linux PTY/process boundary with Foot-compatible session,
controlling-terminal, environment, resize, exit, and process-group behavior.
It must remain usable after Tokio has started without running Splinterm
allocator or runtime code in a callback between `fork` and `exec`.
First-party Rust code keeps `unsafe_code = "forbid"`.

Foot 1.27.0 allocates a PTY master, forks, changes directory, resets selected
signal state, calls `setsid`, opens and claims the slave with `TIOCSCTTY`, maps
it to standard input/output/error, and executes the target. The parent makes
the master nonblocking and close-on-exec, resizes through `TIOCSWINSZ`, polls
child exit, and signals the original process group during shutdown.

Evaluated options were:

- direct `rustix` or `nix` fork/pre-exec code;
- `portable-pty`;
- a temporary Foot C bridge; and
- safe PTY allocation plus a separately executed setup helper.

Direct fork callbacks conflict with the safety boundary. `portable-pty` hides
important Linux process-group and descriptor details and would require its
post-fork path to be re-audited on upgrades. A C bridge is unnecessary because
rustix exposes the required Linux operations through safe owned-descriptor
APIs.

## Decision

Create a Linux-only `splinterm-pty` crate backed by `rustix 1.1.x` and a small
`splinterm-pty-child` executable.

The daemon-side backend:

1. opens the master with read/write, no-controlling-terminal, and close-on-exec
   flags;
2. grants and unlocks the slave;
3. sets the initial cell and pixel size;
4. opens the slave close-on-exec, enables `IUTF8`, and supplies duplicates as
   the helper's standard streams;
5. binds a random-capability Linux abstract Unix socket for exec status;
6. uses `std::process::Command` to start the helper with explicit cwd and
   environment policy; and
7. makes the retained master nonblocking.

The helper is already a newly executed, single-threaded process before it calls
`setsid` and `TIOCSCTTY`. It connects to the capability-named abstract socket,
verifies all standard streams are terminals, and writes a fixed readiness
marker. Its status stream is close-on-exec: replacing itself with the target via
`CommandExt::exec` gives the parent EOF, while setup or exec failure writes a
bounded failure marker before status 126. The backend returns a session only
after readiness and successful exec are both proven. Login-shell mode changes
only `argv[0]` by prefixing the supplied value with a hyphen, matching Foot.

The public API exposes project-owned command, size, signal, session, and error
types. Dependency types do not become the Splinterm contract. Read integration
uses a cloned owned file descriptor so Phase 6 can register it with Tokio
without coupling this crate to an async runtime.

## Foot compatibility and intentional differences

The backend preserves Foot's `COLORTERM=truecolor`, `PWD`, foreign-terminal
environment cleanup, last-wins environment overrides, valid-shell `SHELL`
assignment, `IUTF8`, session/process-group creation, controlling terminal,
close-on-exec on every PTY-owned non-stdio descriptor, master-side resize,
nonblocking I/O, exit polling, and process-group signaling.

The spike deliberately differs in these areas:

- the default is `TERM=xterm-256color`, not Foot's `TERM=foot`. Splinterm does
  not yet implement Foot's complete advertised keyboard contract; claiming the
  Foot terminfo entry causes applications such as Neovim to select input modes
  that Splinterm cannot encode. A project-owned `TERM=splinterm` remains blocked
  on an accurate installed terminfo entry and the compatibility matrix in
  `pre-planning-research.md`;

- shutdown escalation timing and detached reaping belong to the Phase 6 daemon
  actor rather than `Drop` in the PTY crate;
- the original child process group is signaled, matching Foot; descendants that
  create another process group are not implicitly killed;
- explicit signal-mask/disposition normalization beyond what
  `std::process::Command` guarantees remains a follow-up validation item;
- the backend relies on the daemon-wide close-on-exec invariant for unrelated
  descriptors. It does not enumerate and close arbitrary inherited raw file
  descriptors because doing so has no safe owned-descriptor API.

These differences are kept behind the Splinterm-owned interface. The exec
failure handshake remains an internal PTY contract and does not expose
terminal or protocol dependency types.

## Consequences

- No first-party unsafe block or post-fork Rust callback is introduced.
- Linux PTY semantics remain directly testable and are not hidden behind a
  portability abstraction.
- Packaging must install the helper beside the daemon;
  `LinuxPtyBackend::installed` resolves that layout while tests may inject an
  explicit helper path.
- Phase 6 must own child reaping and the Foot-style HUP → TERM → KILL shutdown
  policy; dropping a session is not defined as process cleanup.
- Non-Linux support requires a separate backend decision.

## Validation

Integration probes assert:

- PID equals session ID, original process-group ID, controlling-terminal
  session ID, and initial foreground process-group ID;
- all three standard streams are attached to the slave;
- cwd, `TERM`, `COLORTERM`, `SHELL`, environment cleanup and overrides;
- PTY-owned master/slave descriptors do not survive target exec beyond the
  intended standard streams;
- initial and subsequent row/column/pixel sizes;
- bidirectional byte flow and nonblocking child polling;
- login-shell `argv[0]` behavior;
- process-group signal delivery and observable signal exit status; and
- invalid cwd and synchronously rejected target-exec failures.
