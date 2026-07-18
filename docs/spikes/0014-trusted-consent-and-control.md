# Spike 0014: trusted consent and control

- **Status:** Implemented and validated on workspace 8
- **Decision:** [ADR 0005](../adr/0005-trusted-consent-broker.md)
- **Plan:** [Plan 0002, Phase 7](../plans/0002-omarchy-terminal-mvp.md)

## Implementation

Protocol v7 defines seven closed access scopes: observe, scrollback, input,
resize, clipboard read, clipboard write, and terminate. Grants are in-memory,
grant-once capabilities bound to the requester's peer UID, PID, executable
identity, Splint ID, and process incarnation. They expire after five minutes
and never survive a daemon restart.

When no matching grant exists, `splinterd` launches its sibling `splinterm
consent` executable. A private Unix socket pair is inherited as the child's
standard input/output transport. A 32-byte one-use capability generated with
Linux `getrandom` is sent only over that channel. The exchange uses 16 KiB
length-prefixed frames and a 20-second deadline; launch, framing, timeout,
identity, or child-exit failures deny safely. Capability material is absent
from argv, environment, logs, protocol errors, and terminal content.

The consent client has a fixed trusted mode selected only by the private
launcher path. It shows requester executable, UID/PID, Splint/incarnation, and
human-readable scopes, with grant-once and deny keyboard/pointer actions.
Closing the window denies. Normal windows show development bypass, active
grant, and controller state in application-owned title chrome. Ctrl+Shift+R
revokes active grants and Ctrl+Shift+L releases the controller.

Revocation removes the grant, releases a controller tied to it, broadcasts an
`AccessRevoked` event, and closes affected subscriptions. Process exit and
explicit termination revoke every grant for that incarnation. Audit storage is
a 256-record in-memory ring containing only ordering/time, peer identity,
Splint identity, scopes, decision, and reason—never terminal, clipboard, or
input bodies.

## Automated validation

Run from the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Coverage includes grant identity/scope binding, explicit revocation, bounded
private frames and audit metadata, default-deny terminal operations, controller
exclusivity, protocol framing, and the existing daemon/PTY detach/reattach
lifecycle.

## Workspace 8 graphical validation

Validation procedure:

1. Build `splinterd` and `splinterm` as sibling binaries.
2. Start an isolated daemon without `SPLINTERM_ENABLE_DEV_ATTACH`.
3. Create the live Splint, switch Hyprland to workspace 8, and launch
   `splinterm window`.
4. Confirm `hyprctl clients` reports app ID `com.oldjobobo.splinterm` on workspace
   8 and title `Splinterm — Trusted Access Request`.
5. Capture the prompt and verify requester, process identity, Splint identity,
   scopes, fixed amber border, red deny area, and green grant-once area.
6. Exercise deny and grant-once. After grant, verify the normal window title
   shows local controller and active access.
7. Exercise Ctrl+Shift+L and Ctrl+Shift+R, verify controller/grant indication is
   removed, and verify the daemon-owned shell survives window closure.

## Recorded result

Validated on workspace 8 under the installed Hyprland session on 2026-07-18.
`hyprctl clients -j` reported class `com.oldjobobo.splinterm`, title
`Splinterm — Trusted Access Request`, workspace ID 8, and a distinct consent
client PID. The prompt visibly showed the requester executable, UID/PID,
Splint/incarnation, observe/input/resize scopes, amber trusted border, red deny
area, and green grant-once area. The full-window capture is
`/tmp/splinterm-phase7/consent-prompt.png` (1820×972; ephemeral validation
artifact, intentionally not committed because it contains local process
metadata).

Grant-once transitioned to the normal window on workspace 8 with title
`oldjobobo@wintermute:/tmp — local controller — EXTERNAL ACCESS ACTIVE`.
Controller release and grant revocation were then exercised after correcting
the release path to rely on connection ownership rather than development mode.
The isolated daemon remained alive and `splinterm list` still reported the
`phase7` Dojo and its live Splint after graphical-client closure.
