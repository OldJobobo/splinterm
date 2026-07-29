# Attempt 2 diagnosis

Attempt 2 aborted before any graphical client launch. The pre-feature private
daemon rejected the configured Unix socket with `path must be shorter than
SUN_LEN`; no smoke or matrix window mapped. Workspace, monitor, focus, and
process cleanup passed.

The corrected pre-feature build did contain its required PTY helper. A separate
headless diagnostic then used the 12-byte socket path `/tmp/sbd/r/s`, passed
`ping`, created a daemon-owned Splint, spawned `/usr/bin/sleep 1` through the
pre-feature PTY helper, and observed its normal restorable exit in `topology`.
This proves both the daemon and helper before another graphical proposal.

The next proposed harness uses `/tmp/sbr3` so every generated socket path is far
below `SUN_LEN`. It was not launched automatically.
