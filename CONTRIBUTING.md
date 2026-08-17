# Contributing

Splinterm is a **public beta**. Keep changes small, preserve
crate and authority boundaries, and add focused tests for domain, protocol,
terminal, renderer, or lifecycle behavior.

Read [`docs/status.md`](docs/status.md) for current product scope and
[`docs/architecture.md`](docs/architecture.md) before changing ownership or
protocol boundaries. Plans, spikes, benchmarks, and retained artifacts are
historical evidence; do not rewrite them merely to match current marketing.

## Branch and worktree workflow

Keep the repository-root `main` worktree coordination-only. Create a short-lived
branch and dedicated sibling worktree before editing:

```bash
root=$PWD
worktree=../splinterm-worktrees/0039-binding-help
git fetch origin
git worktree add --no-track -b feat/0039-binding-help-search \
  "$worktree" origin/main
cd "$worktree"
```

Use `plan/…`, `feat/…`, `fix/…`, `docs/…`, or `release/…` names with one coherent
milestone per branch. Base 0.2 and general work on `origin/main`; base an
explicitly authorized 0.1 maintenance patch on `origin/maint/0.1`. Merge the
maintenance patch back to `maint/0.1`, then forward-port any applicable fix to
`main` through a separate reviewed branch rather than merging the long-lived
branches together. One writer owns each branch/worktree; dependent or overlapping
milestones remain serial. Read-only review may inspect the same worktree, while
intentionally concurrent writers require separate worktrees and explicit
approval under [`AGENTS.md`](AGENTS.md).

Open a pull request only after focused validation, actual-diff inspection,
`git diff --check`, and the independent review required by the owning plan.
Publish the exact task branch with an upstream of the same name, then open the
pull request:

```bash
git push --set-upstream origin HEAD
```

Prefer squash merge. After the merge is verified:

```bash
cd "$root"
git worktree remove "$worktree"
# A verified squash merge does not make the branch tip an ancestor of main.
git branch -D feat/0039-binding-help-search
```

Do not publish releases from task branches. Release candidates and promotion
remain bound to reviewed commits on `main` for the active 0.2 line or
`maint/0.1` for the 0.1 maintenance line. Candidate construction and promotion
must be dispatched from the same authority branch.

## Standard validation

Use the narrowest tier that proves the current boundary; do not run every tier
after every edit.

During implementation, run exact affected tests and cheap hygiene checks:

```bash
cargo test -p PACKAGE TEST_FILTER -- --exact
cargo fmt --all --check
git diff --check
```

At a coherent milestone, run the affected crate or integration targets plus
strict workspace linting:

```bash
cargo test -p PACKAGE
cargo clippy --workspace --all-targets -- -D warnings
```

At an integration or release boundary, run the complete workspace once:

```bash
cargo test --workspace -- --test-threads=1
```

Serialized execution is required for suites that own process, socket, signal,
or service state. A clean package build after this complete pass should normally
use `tools/package/build-local-package.sh --no-check`; omit `--no-check` only
when the package build itself is the selected complete test boundary. This
avoids compiling and running the same workspace suite twice.

Documentation or packaging changes also use:

```bash
desktop-file-validate dist/applications/com.oldjobobo.splinterm.desktop
appstreamcli validate --no-net dist/metainfo/com.oldjobobo.splinterm.metainfo.xml
(cd packaging && makepkg --printsrcinfo -p PKGBUILD >/dev/null)
(
  cd site
  npm run validate
)
```

## Isolated daemon and client development

Never debug routine source changes against the packaged production daemon or its
state. The repository helper builds development binaries and uses its own socket,
state, and configuration:

```bash
./splinterm-test          # build, start/reuse the isolated daemon, open a client
./splinterm-test restart  # rebuild and restart after daemon/protocol changes
./splinterm-test ping     # build and verify the isolated daemon
./splinterm-test stop     # stop the exact isolated daemon
```

For custom harnesses, create a unique short temporary root and set at least
`SPLINTERM_SOCKET`, `XDG_STATE_HOME`, and `SPLINTERM_CONFIG` for every daemon and
client process. Record exact PIDs/paths and clean only the namespace the harness
created. Do not point development clients at production topology.

The daemon owns child shells. Stopping it ends those processes. Never run
`splinterm reset`, terminate a production daemon, replace `/usr/bin` binaries, or
install a package as an incidental test step.

## Focused test areas

- Rust package/unit/integration tests: `cargo test -p PACKAGE ...`
- Serialized daemon end-to-end:
  `cargo test -p splinterd --test end_to_end -- --test-threads=1`
- Dojo picker/reference client:
  `python -m pytest -q tools/automation/test_dojo_picker.py`
- Public contract fixtures:
  `python tools/automation/validate-contract-fixtures.py`
- Benchmark harness tests:
  `python -m pytest -q tools/benchmark/test_benchmark.py`

Use the exact commands named by the accepted plan that owns a changed subsystem.
Do not broaden tolerances or regenerate accepted references silently.

## Foot oracle and provenance

Foot 1.27.0 commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e` is the terminal-behavior oracle.
The canonical checkout and accepted comparison images are read-only authorities.
Do not modify the checkout, translate comparison images, broadly widen pixel or
semantic tolerances, or regenerate references without an explicit reviewed plan.

Oracle tooling and provenance live under [`tools/foot-oracle/`](tools/foot-oracle/).
Start with its README and verify provenance before comparison work:

```bash
python tools/foot-oracle/check-provenance.py
python -m pytest -q tools/foot-oracle/test_*.py
```

Run only the bounded oracle command appropriate to the changed behavior. Keep
exact environment, font, scale, source commit, command, and result metadata with
new evidence.

When adapting code from Foot or another project, verify license compatibility,
preserve required notices, annotate translated source where the project pattern
requires it, and update [`THIRD_PARTY.md`](THIRD_PARTY.md) with exact provenance.

## Benchmarks

Benchmark runners and schemas live under [`tools/benchmark/`](tools/benchmark/);
performance analysis helpers live under `tools/performance/`. Validate harnesses
before trusting product results:

```bash
python -m pytest -q tools/benchmark/test_benchmark.py
python tools/benchmark/run.py --help
```

Use release binaries for product comparisons, preserve host/software manifests,
and keep workload, warmup, sample count, cleanup, and schema fixed. A benchmark
failure is evidence to diagnose, not permission to shrink the workload, widen a
gate, or delete an unfavorable result. Graphical benchmark runners are subject to
the graphical-test rules below.

## Fuzzing

Fuzz targets require the `cargo-fuzz` toolchain and belong to explicit accepted
boundaries. List targets before running one:

```bash
cargo +nightly fuzz list
```

Use a bounded duration and retain target, source identity, sanitizer/toolchain,
execution count, and findings. A typical accepted-plan invocation is:

```bash
cargo +nightly fuzz run terminal-advance -- -max_total_time=60
```

Do not claim fuzz coverage for code paths the target does not reach. Diagnose a
crash or timeout before retrying, minimize retained reproducers without changing
the failure, and never discard a corpus/finding to make a gate pass.

## Graphical test guardrails

Graphical testing requires separate explicit approval for the complete bounded
sequence. Non-graphical implementation authorization does not imply permission
to map, focus, move, resize, capture, or send input to a Window.

Approved tests must:

- use an isolated test Window on workspace 8 / DP-2 unless the user explicitly
  names an existing active Window;
- record the target address, PID, workspace, monitor, geometry, original focus,
  cursor, scale, and transform before input;
- target the exact fresh address and abort if identity, focus, placement, or
  cleanup differs;
- use private daemon/socket/state/config paths and development binaries;
- run one guarded smoke before any approved matrix;
- preserve unrelated Windows and user processes; and
- restore focus, cursor, monitor state, an empty workspace 8, processes, and
  private paths after every case.

Do not close a user Window, terminate its shell/daemon, enter commands into it,
or manipulate production topology unless that exact action was separately
approved. A failed expensive or graphical command must be diagnosed before a
bounded retry.

## Packaging and installation

`./install.sh` packages a clean committed `HEAD`; it cannot install uncommitted
work. Building is not authorization to replace a Pacman-owned binary. Read
[`docs/packaging.md`](docs/packaging.md) before package work.

For local source packaging without installation:

```bash
tools/package/build-local-package.sh
```

Publishing, pushing, release creation, system installation, and `/usr/bin`
replacement require the authority appropriate to those actions.

## Review and evidence

At a coherent milestone:

1. run focused tests and the required non-graphical boundary;
2. inspect the actual diff and cleanup/error paths;
3. retain exact commands and outputs required by the owning plan;
4. run `git diff --check`; and
5. obtain the independent reviews required by that plan.

Do not claim a milestone complete without recorded validation and recorded
review. Keep generated evidence body-free where security contracts require it;
terminal content must not leak into audit, trace, or machine-contract artifacts.
