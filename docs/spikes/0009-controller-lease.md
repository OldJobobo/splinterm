# Spike 0009: exclusive graphical controller lease

- **Status:** Implemented and validated
- **Date:** 2026-07-18
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Protocol:** version 3

## Question

Can one authenticated connection exclusively own input and terminal size for
the live Splint while observers remain attachable and disconnect never ends the
shell?

## Mechanism

Protocol version 3 adds `AcquireControl`, `ReleaseControl`, and
`ControlGranted`. Every `Input` and `Resize` request carries the granted
controller ID in addition to the Splint ID and process incarnation.

The daemon stores at most one bounded `ControllerLease`. Acquisition verifies
the current live identity and rejects a second owner with
`ControllerUnavailable`. Input and resize recheck:

- development authorization;
- connection ownership of the controller ID;
- controller token, Splint ID, and incarnation;
- current live process identity; and
- existing byte and terminal-size limits.

Explicit release, owner disconnect, stale incarnation, and shell exit clear the
lease. Releasing control never terminates the shell. Observer subscriptions do
not require control.

The graphical client acquires the lease on its independent control connection
before accepting Wayland input or resize commands, prints the granted ID for
development diagnostics, and releases best-effort when the window closes.
Standalone development `send` and `resize` commands acquire a short-lived lease
and release it after acknowledgment.

## Validation

Unit tests cover exclusive acquisition, token/identity authorization, wrong
controller rejection, identity-specific release, and reacquisition. The
headless end-to-end lifecycle drops the original controller connection,
reacquires from a new connection, resizes, releases explicitly, and continues
the same daemon-owned shell. Protocol serialization and limits tests pass at
version 3.

The workspace-safe demo then validated the live boundary on workspace 8 / DP-2:
a second `send` connection was rejected while the window owned control,
`CONTROLLER_LEASE_OK` entered through the Wayland keyboard reached the PTY, and
a new CLI controller successfully wrote `LEASE_RELEASED_OK` after the window
closed. Workspace 8 was empty after cleanup.

## Remaining authorization work

This is the ownership primitive, not trusted user consent UI. Development
terminal access must still be explicitly enabled. Phase 3/7 work remains for
visible controller indication, consent grant/revocation UI, audit metadata,
complete Foot key mapping, application modes, compose/IME, and clipboard
control distinctions.
