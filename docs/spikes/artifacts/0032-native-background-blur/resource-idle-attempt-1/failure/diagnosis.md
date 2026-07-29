# Attempt 1 diagnosis

The guarded RC smoke passed, including placement/focus/cleanup, no effect
creation, stable RSS, and zero idle CPU ticks. The conditional matrix aborted
before its first pre-feature window mapped.

The pre-feature daemon log records `PTY operation spawn PTY helper failed`; the
client reported the daemon's bounded internal error. Non-graphical inspection
confirmed that the isolated release build did not contain the required sibling
`target/release/splinterm-pty-child`. The initial build command compiled only
`splinterm` and `splinterd`, while the current repository target happened to
contain a helper from earlier builds.

No product, protocol, compositor, placement, focus, or cleanup failure was
observed. Workspace 8 was empty, DP-2 remained scale 1.0/transform 0 and
unfocused, the user remained on DP-3/workspace 6, and both private process
groups exited. The matrix was not retried automatically.
