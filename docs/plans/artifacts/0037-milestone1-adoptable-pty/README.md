# Plan 0037 Milestone 1 — adoptable PTY slice

This evidence covers implementation commit
`886c3de4ec661c4207f8345e6873b678b4b6e496`. It is the first Milestone 1
implementation slice, not completion of Plan 0037 or the complete milestone.

## Implemented boundary

`splinterm-pty` no longer depends on an unreconstructable
`std::process::Child` after spawn. `LinuxPtySession` retains validated child PID,
process-group ID, session ID, one canonical PTY master, and cached direct
`waitpid` status. Adoption revalidates:

- PID, process-group, and session identity;
- controlling-terminal session identity;
- ordinary direct-child wait authority; and
- close-on-exec restoration on the adopted master.

The selected reader model carries only the canonical master across exec. The
actor must stop and drop its cloned async reader before conversion; the adopting
generation creates exactly one replacement reader. A failed conversion returns
the unchanged `LinuxPtySession` with its error so the old generation can resume.

The PTY helper now uses a bounded acknowledgement on its existing private exec
status socket. It completes `setsid`, `TIOCSCTTY`, and the PTY readiness marker,
then waits for parent identity validation before executing the target. This
closes the fast-exit race for commands such as `/bin/true` without changing
`TargetExec` error precedence.

## Exec spike

The Linux-only integration spike performs an actual in-place sequence:

```text
old test generation -> sealed forward descriptor exec -> sealed rollback descriptor exec
```

Across both exec boundaries it proves:

- the daemon test PID and shell PID remain unchanged;
- child PID equals the retained process-group and session IDs;
- one canonical PTY master survives while each generation creates one reader;
- bidirectional PTY traffic and resize continue;
- 512 lines emitted while no userspace reader is active arrive in exact order;
- process-group `SIGHUP` still works;
- direct `waitpid` reaping and cached repeated status remain correct; and
- inherited PTY and executable descriptors are closed or restored to
  close-on-exec after adoption.

Forward and rollback executables are copied into `MFD_ALLOW_SEALING | MFD_EXEC`
memfds, independently rehashed, and sealed with
`F_SEAL_WRITE | F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL`. Write, grow,
shrink, and seal-change attempts fail. The forward source is overwritten and
unlinked after sealing; execution still uses the sealed descriptor.

## Repetition result

Fifty consecutive runs completed 100 exec boundaries. Each boundary preserved
512 ordered lines. Measured no-reader intervals:

- minimum: 1,123,531 ns;
- maximum: 2,211,354 ns;
- mean: 1,412,631 ns.

Machine-readable results are in `summary.json`.

## Validation

The coherent tree passed:

```text
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features -- --test-threads=1
cargo fmt --all --check
git diff --check
```

The two daemon fast-exit regressions that exposed the helper race also pass by
exact fully qualified name.

## Still required for Milestone 1

This slice deliberately does not mark Milestone 1 complete. Remaining gates
include production actor quiescence and descriptor allowlisting, complete
forward/rollback daemon-client pair handling, adjacent-helper compatibility,
pre/post-copy mutation-race detection, writable-mapping seal tests, rollback
pair compatibility rules, broader sustained-output budgets, retained package
identity evidence, and fresh independent review of the complete milestone.
