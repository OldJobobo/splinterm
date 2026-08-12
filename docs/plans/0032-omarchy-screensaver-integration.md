# Plan 0032: Omarchy screensaver integration

- **Status:** Splinterm-owned integration redesign in progress; app-ID transport and
  profile are implemented, while managed activation and guarded graphical acceptance remain open
- **Date:** 2026-08-11
- **Product authority:** Splinterm remains a standalone terminal and implements generic XDG launch metadata rather than an Omarchy-only window mode
- **Integration authority:** Splinterm's packaged desktop adapter, profile, launcher helper, and explicit user-level activation link
- **Depends on:** implemented transient XDG command launches from Plan 0029

## Decision

Support Splinterm as Omarchy's default terminal for the fullscreen screensaver by combining three independently owned changes:

1. Splinterm accepts and validates an XDG app-ID override for its private `xdg-launch` path.
2. The Splinterm package installs a screensaver presentation profile owned by Splinterm.
3. Splinterm offers an explicit, reversible user-level launcher link that delegates to Omarchy for other terminals and invokes the standard XDG adapter with `org.omarchy.screensaver` only when Splinterm is selected.

The integration must preserve the transient lifecycle already established for command-bearing XDG launches:

```text
omarchy-launch-screensaver
  -> xdg-terminal-exec --app-id=org.omarchy.screensaver -- omarchy-screensaver
  -> com.oldjobobo.splinterm.desktop
  -> splinterm-xdg-terminal-exec
  -> splinterm xdg-launch --app-id=org.omarchy.screensaver -- omarchy-screensaver
  -> transient client-bound Lair
```

Splinterm's package must not overwrite Omarchy-owned system files or user-owned launcher overrides.

## Baseline blockers

At plan approval, the following blockers defined the implementation. The local Splinterm source changes address app-ID transport, desktop metadata,
and the owned profile. The remaining integration is owned entirely by Splinterm;
no Omarchy source modification or upstream submission is part of this plan.

### Omarchy rejects Splinterm

`xdg-terminal-exec --print-id` reports:

```text
com.oldjobobo.splinterm.desktop
```

The upstream `omarchy-launch-screensaver` allowlist and launch branches currently recognize only Alacritty, Ghostty, Foot, and Kitty. Splinterm is rejected before a Window is launched.

### XDG app-ID propagation is not advertised

`dist/applications/com.oldjobobo.splinterm.desktop` does not declare `X-TerminalArgAppId`. Consequently, `xdg-terminal-exec` drops:

```text
--app-id=org.omarchy.screensaver
```

and invokes only:

```text
splinterm-xdg-terminal-exec -- omarchy-screensaver
```

### Splinterm fixes every Wayland app ID

`crates/splinterm/src/wayland.rs` currently applies `config::APP_ID` to every Window. Omarchy relies on the exact app ID `org.omarchy.screensaver` for:

- fullscreen, floating, and animation rules;
- detecting when each monitor's screensaver Window has mapped;
- determining whether the screensaver still has focus; and
- terminating screensaver client processes during cleanup.

Changing the Omarchy allowlist alone would therefore launch an ordinary Splinterm Window that does not satisfy the screensaver lifecycle.

### A user-local launcher can shadow Omarchy

On the reviewed installation, `~/.local/bin/omarchy-launch-screensaver` shadows the Omarchy-owned `/usr/bin/omarchy-launch-screensaver`. Package installation cannot safely replace or remove that user file. The integration must detect or document this case and require an explicit user decision to update or remove the override.

## Product contract

### Ordinary Splinterm windows

Native launches, Dojo pickers, consent windows, and XDG launches without an app-ID override retain:

```text
com.oldjobobo.splinterm
```

No user configuration option may globally impersonate another application's identity.

### XDG app-ID override

Only the private `xdg-launch` integration boundary accepts the desktop-standard app-ID argument. It must:

- validate the value before Window creation;
- carry the exact validated value into the resulting Wayland Window;
- preserve exact cwd and argv transport;
- remain unavailable for remote endpoints;
- preserve persistent behavior for commandless XDG launches; and
- preserve transient owner-bound behavior for command-bearing XDG launches.

`org.omarchy.screensaver` is the motivating value, not a hard-coded special case in the renderer.

### Screensaver presentation

The Omarchy screensaver Window uses a dedicated profile with:

- an 18-point Nerd Font;
- zero padding;
- opaque background; and
- background blur disabled.

The profile must not replace or rewrite the user's normal Splinterm configuration.

## Design

### 1. App-ID transport

Add an optional app ID to the hidden XDG command in:

- `crates/splinterm/src/app/commands.rs`;
- `crates/splinterm/src/app/cli.rs`; and
- `crates/splinterm/src/app/sessions.rs`.

Carry it as client-local presentation state through the live Window construction path. Add the presentation value to `WindowOptions` in `crates/splinterm/src/frontend/options.rs`, defaulting to `config::APP_ID`, and apply it in `crates/splinterm/src/wayland.rs`.

Do not add the override to daemon topology, protocol messages, persistent launch metadata, automation JSON, remote transport, or user configuration. It is a property of one graphical client Window, not of a Lair, Dojo, or Splint.

Validate app IDs at the CLI boundary. Reject empty, oversized, malformed, or control-character-bearing values before connecting to the daemon or creating a Window. The accepted grammar must cover valid reverse-DNS desktop identities including `org.omarchy.screensaver` without accepting arbitrary shell syntax.

### 2. Desktop metadata and adapter

Add to `dist/applications/com.oldjobobo.splinterm.desktop`:

```ini
X-TerminalArgAppId=--app-id=
```

The POSIX adapter remains a pure argv-preserving forwarder. It must not parse, quote-rebuild, or special-case Omarchy. `xdg-terminal-exec` expands the desktop metadata and the wrapper forwards the result to `splinterm xdg-launch`.

Extend `tools/package/validate-package.py` to prove that:

- the desktop metadata advertises the app-ID argument;
- the packaged wrapper preserves `--app-id=org.omarchy.screensaver` exactly;
- cwd and structured command arguments remain unchanged; and
- commandless versus command-bearing routing retains Plan 0029 semantics.

### 3. Splinterm-owned profile

Add a source profile such as:

```text
dist/omarchy/screensaver.ini
```

Install it from `packaging/PKGBUILD` at:

```text
/usr/share/splinterm/omarchy/screensaver.ini
```

The same asset must be included by the repository's local installer/package layout. Package validation must assert its presence and the required presentation values.

The profile is intentionally owned by Splinterm rather than copied into `/usr/share/omarchy`. Omarchy may consume the stable path when Splinterm is selected, but neither package claims files owned by the other.

### 4. Splinterm-owned launcher and activation

Install an executable helper at:

```text
/usr/lib/splinterm/integrations/omarchy-launch-screensaver
```

When Splinterm is selected by `xdg-terminal-exec --print-id`, the helper launches
one screensaver Window per monitor through the XDG adapter, uses the packaged
profile and `org.omarchy.screensaver`, retains Omarchy's event-socket wait, and
restores the original monitor focus. When another terminal is selected, it
executes Omarchy's canonical `/usr/share/omarchy/bin/omarchy-launch-screensaver`
directly without recursion.

Activation is explicit and user-scoped:

```text
~/.local/bin/omarchy-launch-screensaver
  -> /usr/lib/splinterm/integrations/omarchy-launch-screensaver
```

`splinterm integration omarchy-screensaver enable` creates only that exact link.
It refuses every existing file or different symlink, verifies login-shell command
resolution, and moves the link to a dedicated non-PATH disabled location if the
managed launcher does not win. `disable` atomically moves only the exact managed
link to that location and never unlinks it; `enable` moves it back. `status` is
read-only.

### 5. Installation and upgrades

`packaging/PKGBUILD` and `install.sh` may install only Splinterm-owned files. They must not:

- patch `/usr/bin/omarchy-launch-screensaver`;
- write under `/usr/share/omarchy`;
- create or overwrite `~/.local/bin/omarchy-launch-screensaver`;
- alter the user's XDG terminal preference; or
- remove an existing user-local override.

The user-mode installer may report that a user-local screensaver launcher shadows Omarchy's packaged command. Pacman hooks cannot reliably inspect or mutate each user's home and should provide only a concise compatibility note if one is warranted.

Omarchy support becomes available when both compatible package versions are installed. Omarchy should not depend on Splinterm; Splinterm should not depend on Omarchy.

## Dependency-ordered milestones

### Milestone 1 — app-ID model and parser

Implement validated XDG-only app-ID transport through `WindowOptions` and Wayland creation.

Focused acceptance:

- ordinary Windows retain `com.oldjobobo.splinterm`;
- `org.omarchy.screensaver` reaches `window.set_app_id` unchanged;
- malformed values fail before launch;
- no app ID enters daemon or persistence models; and
- commandless and command-bearing XDG lifecycle behavior remains unchanged.

### Milestone 2 — desktop metadata and package adapter

Advertise `X-TerminalArgAppId`, extend parser and package tests, and prove exact argv preservation.

Focused acceptance:

```bash
xdg-terminal-exec --print-cmd \
  --app-id=org.omarchy.screensaver \
  -- omarchy-screensaver
```

must include the Splinterm adapter and the exact app-ID argument.

### Milestone 3 — packaged screensaver profile

Add and install the Splinterm-owned profile. Validate extracted package contents and configuration parsing without modifying the live system.

### Milestone 4 — Splinterm-owned activation

Package the launcher helper and add explicit `enable`, `disable`, and `status`
commands. Prove collision refusal, exact-link-only removal, login-shell resolution
rollback, non-Splinterm delegation, per-monitor waiting, and focus restoration.
The normal installer may offer activation interactively with a default of No;
unattended and direct package installs only print the activation command. Tests
must cover concurrent replacement before and after an atomic move without
unlinking either user object.

### Milestone 5 — guarded graphical acceptance

After explicit graphical-test approval, verify the complete multi-monitor sequence on the approved target environment:

- exactly one Window maps per monitor;
- every Window class is `org.omarchy.screensaver`;
- fullscreen rules apply;
- the profile is opaque, unblurred, and unpadded;
- keyboard or pointer input exits the screensaver;
- every transient Lair and child process is removed; and
- original monitor focus is restored.

Account for any user-local launcher override before testing. Do not remove or replace it without explicit approval and a rollback copy.

## Non-graphical validation

Run focused checks after each coherent milestone, followed by:

```bash
sh -n dist/bin/splinterm-xdg-terminal-exec
cargo test -p splinterm
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python tools/package/validate-package.py --help
git diff --check
```

Run the repository's documented package build and extracted-package validation only after source validation is green and the relevant source is committed, because the package builder archives a clean `HEAD`.

For the packaged helper, run `bash -n`, ShellCheck, and its stubbed non-graphical contract tests. No Omarchy repository checkout or upstream submission is required.

## Required test matrix

| Case | Expected result |
| --- | --- |
| Native Splinterm Window | App ID remains `com.oldjobobo.splinterm` |
| XDG launch without app-ID | App ID remains `com.oldjobobo.splinterm` |
| XDG launch with valid app-ID | Exact value reaches the Wayland Window |
| XDG launch with invalid app-ID | Rejected before Window creation |
| XDG launch without command | Persistent Lair behavior unchanged |
| XDG launch with screensaver command | Transient owner-bound Lair |
| Wrapper with cwd, app-ID, and unusual argv | Every argument preserved exactly |
| Extracted package | Desktop metadata and profile are present and valid |
| Another XDG terminal selected | Delegate directly to Omarchy's canonical launcher |
| Existing user-level launcher | Refuse activation without changing it |
| Managed link loses PATH precedence | Roll back activation |
| Splinterm selected in Omarchy | One matching screensaver Window per monitor |
| Screensaver input or focus loss | All Windows, children, and transient Lairs retire |
| User-local Omarchy override exists | Reported; never overwritten or removed automatically |

## Stop-loss boundaries

Stop and report before continuing if:

- app-ID support requires persisting presentation identity in daemon topology;
- ordinary Splinterm Windows can inherit an untrusted global app-ID override;
- cwd or command argv would be reconstructed through a shell;
- the screensaver launch becomes persistent or appears in Recent Dojos;
- package integration requires claiming Omarchy-owned or user-owned files;
- activation would require modifying Omarchy, Hyprland, shell/menu, PATH, or an existing user launcher;
- testing requires replacing the Pacman-owned client or Omarchy launcher without approval;
- graphical testing has not been explicitly approved; or
- publishing, pushing, installing, or removing the user-local override becomes necessary without explicit approval.

## Completion criteria

The integration is complete only when:

- Splinterm advertises and correctly applies XDG app-ID overrides;
- ordinary Splinterm Window identity remains unchanged;
- the package installs a validated screensaver presentation profile under `/usr/share/splinterm`;
- the Splinterm-owned helper launches `omarchy-screensaver` with `org.omarchy.screensaver` and delegates other terminals to Omarchy;
- command-bearing launch remains transient and cleans up completely;
- no installer or package overwrites Omarchy-owned or user-owned files;
- user-level activation is explicit, collision-safe, precedence-verified, and reversibly moved outside PATH without unlinking;
- recorded non-graphical validation passes;
- guarded graphical acceptance is recorded; and
- independent review has no unresolved blockers.
