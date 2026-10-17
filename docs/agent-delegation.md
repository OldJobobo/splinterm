# Remote agent delegation

Claude Code and Pi can offload bounded work through the same CLI:
`tools/agents/remote`. The parent remains the coordinator; the remote process is
an explicitly assigned delegate, not another autonomous coordinator.

## Current boundary

The runner supports **read-only scouts/reviewers** and opt-in **Cargo validators**
on a Linux host with Python 3.10+, Git, SSH, Pi 1.3.0-compatible JSON events, and
a lingering systemd user manager. Validators additionally require a deliberately
provisioned restricted account, system Cargo/Rust, and working Bubblewrap user
namespaces. Writing agents remain disabled. Jobs inspect/build an exact published
authority commit; unpublished task branches and dirty local diffs are not yet
transferred. Do not mistake validation of main for validation of local edits.

This is a headless job runner, not Splinterm's terminal automation or graphical
remote-access feature. It creates no Windows and uses no production daemon.
The existing [agent guardrails](../AGENTS.md) apply regardless of execution host.

## One-time operator setup

1. Configure an SSH alias and independently verify its host key. The runner uses
   `BatchMode=yes` and `StrictHostKeyChecking=yes`; it never accepts a new key or
   disables host-key checking automatically.
2. Install/authenticate Pi on the worker host through its normal login flow.
   Enable user-service lingering deliberately if it is not already configured.
   The runner does not install packages, enable lingering, or copy credentials.
3. Copy [hosts.example.json](../tools/agents/hosts.example.json) to
   `~/.config/splinterm-agents/hosts.json`. Replace every placeholder. Use an
   absolute pinned Pi executable, its exact version, and an authenticated
   provider/model. Keep this host configuration outside Git.
4. Choose a new private state directory, outside existing repository checkouts.
   It must be owned by the remote user, mode `0700`, with no symlink components.
   Never configure an existing checkout as the state directory.
5. Run the passive prerequisite check:

```sh
tools/agents/remote --host wintermute doctor
tools/agents/remote --json --host wintermute doctor
```

`doctor` checks the Pi version, authentication readiness without refreshing or
printing credentials, the user manager, lingering, and Git. It does **not** make
a model request or prove model availability, network connectivity to the model,
service startup, or checkout/test compatibility. An unrelated failed user unit
does not by itself disqualify an otherwise reachable manager.

Both the calling machine and worker need `python3` on their noninteractive PATH.
On a Nix workstation, use the repository's development shell, which includes
Python, rather than installing a global interpreter:

```sh
nix develop --command tools/agents/remote --host wintermute doctor
```

The worker service uses `/usr/local/bin:/usr/bin:/bin`; configure a Pi executable
whose runtime is available there rather than relying on interactive shell setup.

## Submit, observe, retrieve

Copy [task.example.json](../tools/agents/task.example.json) into private task
storage. Set an unambiguous parent/session identity, a new job ID, exact reviewed
commit SHA, objective, bounded read list, acceptance criteria, and deadline.
`authority` selects `main` or `maint/0.1`; it does not authorize maintenance
changes. Task files contain no credentials.

```sh
tools/agents/remote --host wintermute submit --task /private/path/task.json
tools/agents/remote --host wintermute status remote-access-scout-01
tools/agents/remote --host wintermute result remote-access-scout-01
tools/agents/remote --host wintermute logs remote-access-scout-01
tools/agents/remote --host wintermute cancel remote-access-scout-01
```

Use `--json` for automation; stdout contains one JSON object. Errors go to stderr
and exit 2. Failed/interrupted/unknown jobs and an unready doctor exit 1. Successful
queries of queued/running jobs exit 0: **a zero CLI exit does not mean the task is
finished**. Inspect `state`. Cancellation of an already completed job does not
change its outcome.

Submission transfers this reviewed runner and task over SSH stdin; there is no
remote installation or shell interpolation of task text. The worker stores a
private copy of the runner with the job and starts it as a transient **user
service**, not an SSH-bound process or scope. The service owns its process tree,
with a 4 GiB memory ceiling, 64-task ceiling (256 for validators), two-CPU quota,
task deadline plus 180 seconds for setup, and bounded stdout/stderr logs. The task
deadline covers the model run and, for validators, dependency fetching and checks;
checkout setup has its own bounded subprocess timeouts.

Each job initializes its own Git checkout, fetches only the requested authority
branch from the configured HTTPS repository, verifies the exact source commit is
an ancestor of the fetched authority tip, and checks it out detached. It never
uses, updates, cleans, or synchronizes an existing user checkout. A missing commit
or read path fails closed. This version does not transfer unpublished commits,
untracked files, task worktrees, or dirty local changes.

No agent extensions, MCP servers, skills, or prompt templates are loaded. Project
resource trust is explicitly declined; repository context files still load.
The model receives only `read,grep,find,ls`, with no Bash/edit/write tools. The
runner verifies the checkout is unchanged before recording successful execution.

## Compile, test, and lint on the worker

Use a separate private host profile such as `wintermute-build`, pointing at the
restricted worker SSH identity and a fresh job-state root. Add `"build_uid": 1001`
with the **actual provisioned UID**, not a guessed/copied value. This is deliberate
operator opt-in, not automatic account provisioning or proof of a user's sudo
policy. Keep the Pi provider/model aligned with that account's verified login.
`doctor` reports build prerequisites as ready/not_ready/disabled; a configured
build profile fails readiness if its UID or namespace probe fails.

Copy `tools/agents/build-task.example.json`, set the reviewed source SHA and job
identity, and choose 1–8 argument arrays. Supported commands are `cargo build`,
`check`, `test`, `clippy`, and `fmt --check`; no shell strings, package installation,
or generic Bash tool is enabled.

```sh
tools/agents/remote --host wintermute-build doctor
tools/agents/remote --host wintermute-build submit --task /private/build-task.json
tools/agents/remote --host wintermute-build status protocol-check-01
tools/agents/remote --host wintermute-build check-logs protocol-check-01
tools/agents/remote --host wintermute-build result protocol-check-01
```

The runner—not the model—executes the declared commands. It first runs
`cargo fetch --locked` in a credential-free, network-enabled namespace when
needed. Fetching dependencies does **not** install host packages. It then runs
Cargo checks with enforced `--locked --offline` flags and a disconnected network
namespace. Formatting runs skip dependency fetching and require `--check`.

Build processes see a read-only source checkout and system toolchain/libraries,
public CA trust and font configuration, fresh `/proc`, minimal `/dev`, and
job-local writable Cargo cache/output. They cannot see either user's home,
provider credentials, SSH keys, host runtime/desktop sockets, KVM, or uinput.
The environment is cleared; Cargo uses two build jobs. There is no writable host
root mount. A sandbox setup failure aborts; there is no unsandboxed fallback.
This is tested with the system Arch toolchain; user-local Rustup/mise toolchains
and Nix store toolchains are not exposed automatically.

The existing two-CPU/4-GiB service limit remains; validators allow 256 tasks instead
of 64 for compiler threads. The task deadline covers dependency fetching, all
checks, and Sol's analysis together. Each output stream remains capped at 16 MiB.
Caches and outputs are per-job, not shared. They are retained and have **no disk
quota or automatic pruning**; account for disk use before large/repeated builds.

`checks.json` records every started command, network mode, exit code or runtime
error, log paths/hashes, planned check count, and aggregate result. Checks stop
at the first failure. Raw logs live in `checks/NN/{stdout,stderr}.log`;
`check-logs` retrieves bounded tails. Artifacts live in `build-output/` and
`build-cache/`. Build outputs are retained remotely; binary downloading is not
implemented by this CLI yet.

After normal command completion, Sol gets **only read tools** to inspect receipts,
logs, and the specified source files. A positive model report cannot turn a failed
command into success. Failed validations retain a report when analysis succeeds;
`result` returns it with state `failed` and exit 1. Setup failures/timeouts may
have only partial check logs and no model report. An interrupted in-progress
check is not a passing check.

## Wintermute VM access is a separate boundary

Do not expose the operator's VM SSH key, QMP socket, or host desktop to the build
sandbox. Existing Splinterm VM configurations may live in earlier task worktrees
rather than the root checkout; identify the established guest from those configs
instead of assuming that an arbitrary listening SSH port is the testbed.

The coordinator retains `tools/testbed/omarchy-vm.sh` and its private configuration
under the VM owner's identity. The restricted worker can report that a graphical
check is needed, but its Cargo profile cannot launch it. Before execution the
coordinator must obtain Section 3 approval for the exact guest Window, smoke case,
permitted input, gated matrix, and cleanup, then invoke the existing testbed tool.
Do not hand an unbounded guest shell or raw QMP endpoint to the remote model.

The current capability milestone does **not** yet implement unattended VM job
submission, cancellation/cleanup supervision, capture transfer, or a remote-model
visual loop. Confirming the existing guest and having SSH connectivity is not
visual acceptance. Those are a separate VM-interface milestone; no new VM should
be created merely because the root checkout lacks a testbed config.

## Identity, retries, and recovery

The same job ID plus identical task, host profile, and runner content returns the
existing receipt without launching again. A differing request with the same ID
is rejected. Do not change IDs merely because SSH disconnected: query the
original ID first. Updating the runner does not prevent querying old jobs, but
resubmitting their ID with changed runner content is a conflict.

States distinguish `claimed`, `queued`, `running`, `dispatch_unknown`,
`cancelling`, `completed`, `failed`, `cancelled`, and `interrupted`.

- `dispatch_unknown`: service dispatch did not return an unambiguous result.
  Inspect status; never blindly resubmit under another ID.
- `interrupted`: the service is no longer active and no terminal receipt exists.
  This includes reboot/timeout/crash recovery. It is not a successful result.
- An unreachable SSH connection or user manager means **unknown**, not failed or
  cancelled. Reconnect and query the original ID.
- `cancel` stops only the exact job service/control group. A failed stop leaves
  cancellation unconfirmed. The runner never uses process-name kills.
- `completed` means the final Pi message succeeded and automatic work settled,
  the report exists, and the checkout stayed clean. For validators, all planned
  command exit codes must also be zero. It is **not** coordinator acceptance,
  independent review, or proof of tests outside the declared command list.

Pi JSON mode may exit zero after model failure. The runner requires a successful
final assistant message plus `agent_settled`, and rejects errors, aborts,
truncation, malformed events, missing settlement, nonzero exits, and empty
reports. Raw model output is evidence, never authority to execute another action.

## Artifacts and ownership

Each job directory retains `request.json`, `status.json`, `runner.py`, `repo/`,
`prompt.txt`, `sessions/`, `events.jsonl`, `stderr.log`, and `report.md` as available.
Some files do not exist when setup fails. Artifacts are private and may contain
repository text, prompts, or sensitive diagnostics; do not publish them blindly.
`logs` returns only a bounded stderr tail, not the full transcript.

There is no automatic retry, cleanup, pruning, service reset, or deletion command.
Retain artifacts until the coordinator has inspected the report and recorded the
outcome. Cleanup is a separate, explicitly authorized operation on exact job
paths after confirming the service is stopped. Failed jobs retain evidence too.

Start with one remote job at a time. The runner does not provide a distributed
scheduler or global concurrency quota across independent parent sessions. Parents
must coordinate admission and honor repository fanout limits. Independent roots
have independent service namespaces; copying a job ID between roots does not
provide cross-root deduplication.

## Security limits

**Pi's worktree, tool allowlist, and systemd limits are not an OS sandbox.**
Only validation commands receive the additional build namespace described above;
the model process itself still uses the remote user's identity and Pi credentials. Its
read tools can access files that user can read; the task's read list is guidance,
not an enforced filesystem boundary. User-level Pi configuration and provider
credential resolution remain trusted operator inputs. Do not use it for hostile
repositories or tasks requiring protection from a compromised model/runtime.
Use a separately configured restricted account/container/VM before enabling
unattended writers or requiring isolation of the model process. Do not use the
build profile's namespace boundary as justification for granting Pi a Bash tool.

Publication, installation, graphical activity, additional concurrent writers,
and recursive delegation retain their explicit approval boundaries. Do not
preapprove unrestricted SSH or shell access in Claude merely to use this runner.
Grant any command permission narrowly through the caller's normal permission
mechanism. The runner intentionally has no `--force` or permission-bypass mode.

## Parent integration

Either parent can discover this workflow through `AGENTS.md` and execute the
same commands. A local skill may make that more convenient, but must not duplicate
policy or invent another execution protocol. A Pi `external-cli` adapter must
wait for and verify the job's terminal receipt before returning successful
completion; returning the submit acknowledgement would falsely finish the child.
A bare SSH job is not a native Pi subagent and has no automatic parent wakeup:
use explicit status polling or a separately implemented notification adapter.

Keep model IDs, host paths, and SSH routing in user-local configuration. Never
copy the coordinator's authentication files, entire session, or private unrelated
working files to a worker. Send only the bounded task and its approved context.

## Validation and next milestones

```sh
python3 -m unittest discover -s tools/agents -p 'test_*.py' -v
python3 tools/release/release-doctor.py
git diff --check
```

Tests use fake process/service responses and real disposable local subprocesses,
not remote agents or a production user manager. A live read-only pilot remains a
separate acceptance boundary: verify receipt delivery, SSH disconnect/reconnect,
source identity, cancellation, and artifact retention with disposable jobs.
Build acceptance additionally needs a live namespace-isolation probe, verified
TLS dependency fetching, and a focused real compile with checked exit receipts.
Isolated writers, unpublished source transfer, supervised VM jobs, scheduling,
and native Claude/Pi convenience adapters remain separate capabilities.
