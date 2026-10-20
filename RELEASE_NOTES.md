# Splinterm 0.2.0

Splinterm 0.2.0 is the first feature release after 0.1.1 for the documented
x86_64 Omarchy/Arch Linux target. Its defining change is live daemon upgrade:
a newly installed build can take over running terminals without stopping them.
It also adds explicitly reported agent status, a startup picker, remote launch
from inside the app, and full Omarchy 4 theme and shell-settings following.

0.2.0 changes the private daemon protocol. A 0.2.0 client cannot talk to a 0.1.x
daemon, and the reverse, so read "Upgrading from 0.1.x" before installing.

<!-- PENDING: live upgrade enablement PR -->
## Live daemon upgrade

- `splinterm upgrade` reports whether the running daemon could hand its
  terminals to the installed build, and changes nothing. It shows the daemon's
  generation, build, session counts, whether it kept a rollback copy, and every
  reason a live upgrade cannot run now.
- `splinterm upgrade --now` starts the live upgrade. Shells, scrollback, and
  running programs keep going; the daemon keeps its process ID.
- Open Windows come with it. Each Window shows "Preparing upgrade…", then
  "Upgrading — input paused", relaunches as the new client, and restores its
  tabs, active tab, focused pane, and control, so typing resumes without a click.
  Keys typed while paused are dropped, not queued.
- If some Windows cannot take part, the command lists them and asks whether to
  cancel or proceed without them. In a terminal it asks `[c]ancel / [p]roceed`.
  Without one, or from inside a Splinterm terminal, a small trusted "Splinterm —
  Upgrade" dialog asks instead; only `G` or its green area proceeds, and keys
  count only after the dialog has had focus briefly. Proceeding closes those
  Windows; their terminals keep running and reopen from Recent Dojos.
- If the new build fails before it takes over, the daemon rolls back to the
  previous build in the same process and keeps every terminal. Windows show
  "Rolled back".
- One live upgrade per daemon: a daemon started by a live upgrade or a rollback
  keeps no rollback copy, so restart it before the next live upgrade.
- Only the installed `splinterm` next to the running `splinterd` may start an
  upgrade. The command has no machine output.
- Exit statuses: 0 report shown or upgrade completed; 1 error; 2 usage error;
  3 cancelled, nothing changed; 4 live upgrade unavailable now, nothing changed;
  5 another upgrade in progress; 6 did not complete, previous build still
  serving with terminals kept; 7 result unconfirmed, run `splinterm upgrade` to
  check.
- Other commands are never retried after a mutation is interrupted. A command
  refused during an upgrade says it did not run; one cut off by the upgrade says
  it may or may not have taken effect. Plain reads (`ping`, `list`, `topology`,
  `inspect`, `focus`) are retried once.
- Nix installations refuse live upgrade in 0.2.0; `splinterm upgrade` says so,
  and restarting the daemon remains the Nix upgrade path.

## Agent status

- An agent can report `working`, `needs_input`, `done`, `error`, or `idle` for
  its Splint with `splinterm agent-status set|clear|get`, or through the new MCP
  tool `splinterm.report_agent_status`. An optional short detail and TTL are
  supported; `working` and `needs_input` turn stale after two minutes without a
  fresh report by default.
- Splinterm shows only what was reported. It never guesses from terminal text,
  titles, prompts, process names, or output, and a status cannot grant access,
  approve, type, or take control.
- A process inside the Splint reports without a grant. Anything else, including
  an agent that fully detaches from the Splint, needs the new
  `agent_status_report` policy scope.
- The Explorer shows a static icon and the state in words for each Splint, and
  the most urgent state for each Dojo and Lair. Dojo tabs show one badge with a
  count. States differ by icon shape and text, never by colour or motion alone.
- Explorer search matches state words such as `needs input` or `stale`. The
  palette command **Clear agent status** clears the focused Splint's status.
- `inspect` and `topology` machine output carry an optional `agent_status`.
- `[explorer] agent-status=no` hides all of it. `docs/automation.md` includes
  ready-made hooks for Claude Code and Codex CLI.

<!-- PENDING: easy agent access (Plan 0053) -->

## Launching and remote Windows

- `main.startup=dojo-picker` makes the commandless desktop launch open the
  Recent Dojos picker instead of starting a shell. `--picker` and `--new` choose
  for one launch, and the **New Terminal** desktop action always creates a
  terminal.
- Open a remote without a terminal: `splinterm remotes`, the **Remote Dojos**
  desktop action, **Open Remote…** in the command palette, or the **Remote
  Dojos…** row in Recent Dojos all list your `remotes.toml` profiles. Each
  remote opens in its own Window.
- Every graphical remote launch first shows a connection-status Window. On
  failure it names the cause, suggests a fix, and shows recent OpenSSH output;
  press R or Enter to retry, or Escape to close.
- When OpenSSH needs a password, passphrase, or PIN and no ssh-agent key or
  `SSH_ASKPASS` can answer, the status Window asks in a masked field. The answer
  is never echoed, logged, or put on the clipboard, and an unanswered prompt
  ends the attempt after 120 seconds. Host keys are still never accepted
  through it.

## Dojos, tabs, and the Explorer

- `multiplexer.close-unused=yes` makes Close Tab and closing a Window end a
  Dojo that was never used, instead of leaving it running detached. Typing, a
  split, a rename, or a program in the foreground all count as use. Default
  `no`; remote Windows get the same decision from the remote daemon.
- New Dojos and Lairs created inside a Window now start in your home directory;
  splits still start in the focused Splint's directory. Change either with
  `multiplexer.new-dojo-directory` and `multiplexer.new-splint-directory`
  (`inherit`, `home`, or a path).
- **Reload configuration** now also applies these settings, plus
  `persist-on-tab-organization` and `explorer.agent-status`.
- `Ctrl+Shift+1` to `Ctrl+Shift+9` select the first nine tabs by default, also
  on layouts where Shift+digit produces a symbol.
- Inactive tab labels are muted but keep at least 4.5:1 contrast against the
  tab strip.
- Duplicate tab names from launcher-created Lairs read "Dojo 1 (1)",
  "Dojo 1 (2)" instead of showing raw generated Lair names; pickers use the same
  labels.
- Keybinding help (`Prefix+?`) is searchable. Save, pin toggle, Preview, and
  Restore of the current Lair are bindable actions; the `omarchy-tmux` profile
  binds them to `Prefix Shift+S/F/V/O`.
- Copy mode gains `w`/`b`/`e` word motions and `0`/`$`.
- The Explorer docks beside a half-width terminal instead of covering it, and
  tabs, the Explorer, menus, and dialogs stay readable at maximum zoom.

## Omarchy 4

- Splinterm reads Omarchy 4's theme location and colour names, and follows
  theme switches without a restart. Non-Omarchy desktops use the built-in
  palette quietly.
- Glassy Windows: the terminal background, padding, and tab strip turn
  translucent with blur when glass is on, while text stays opaque and crisp.
  Toggling applies live.
- Splinterm follows the Omarchy shell's `shell.toml`: panel alpha for menus and
  popups (raised as needed to keep text readable), text size from
  `omarchy-display-text-size`, applied live to open Windows, and the top bar's
  height.
- The tab strip is now as tall as Omarchy's top bar (26 px by default) and
  scales with the text size.
- Without Omarchy's `terminal` tag, Omarchy's `Super+C` sends an interrupt. The
  install messages and a one-time desktop notification now recommend
  `splinterm integration omarchy enable`; set
  `main.omarchy-integration-hint=no` to silence the notification.
- The screensaver integration works with Omarchy 4's `PATH` and opens on
  Omarchy's special screensaver workspace.

## MCP

- `splinterm-mcp` also accepts MCP protocol `2025-06-18`, so Codex can connect.
- New tool `splinterm.report_agent_status`; the catalog now has 34 tools.

## Security and robustness

- A failed `accept()` no longer stops the daemon and every terminal with it; the
  daemon pauses briefly and keeps serving.
- A crashed policy reload installs deny-all and keeps the daemon running.
- Closing, killing, or terminating a stubborn process no longer blocks every
  other topology change for up to a minute.
- Daemon shutdown no longer waits a full minute on a terminal whose process
  group is already gone.
- Hostile terminal output is bounded: a multibyte character can no longer cut an
  OSC title short, huge tab-movement counts no longer stall the terminal, and
  synchronized-update damage is capped. A malformed colour in configuration is
  an error instead of a crash.
- Fonts without horizontal metrics are rejected before measuring.
- Typing over SSH and in remote tmux no longer stalls: a throttled synchronized
  frame is now flushed, and queued keystrokes go to the daemon in one batch.
- The documentation now states plainly what agent permissions do not protect:
  policy and consent bound a cooperating agent, but they are not a sandbox
  against a hostile program running as your account.

## Behaviour changes

- Private protocol 42 (0.1.1 used 36). Old and new clients, daemons, and relays
  do not interoperate. Upgrade both ends of a remote connection, and keep
  `splinterm-mcp` at the same version.
- New Dojos and Lairs start in your home directory, not the focused Splint's
  directory.
- On Omarchy 4 with no font size in `config.ini`, the terminal follows
  Omarchy's text size (9 pt on a stock desktop) instead of Splinterm's 14 px.
  Set a size in `config.ini` to keep your own.
- `Ctrl+Shift+1`–`9` are now tab shortcuts: on US layouts with tabs,
  `Ctrl+Shift+2` and `Ctrl+Shift+6` no longer send `Ctrl+@` and `Ctrl+^`
  (`Ctrl+2` and `Ctrl+6` still do). A keymap overlay that binds these chords
  fails at startup until it adds the `[[unbind]]` the error names.
- The tab strip is shorter on a stock Omarchy desktop.
- The repository moved to `ministry-of-taste/splinterm`; old URLs redirect.

## Known limitations

- Upgrading from 0.1.x to 0.2.0 cannot be live, because the running 0.1.x daemon
  has no live-upgrade support.
- Live upgrade is unavailable while any transient Lair is open, for daemons not
  started from an installed package build, on Nix, and on systems that forbid
  executable memory files (`vm.memfd_noexec=2`), where no rollback copy is kept.
- Agent status lives only in daemon memory. It is dropped when a Splint
  relaunches and after a daemon restart or live upgrade, until the agent reports
  again. Topology subscriptions do not carry it.
- Dojo tab badges have no accessible description yet; the Explorer carries the
  same status. Splinterm does not read the desktop's reduce-animation setting.
- Splinterm's SSH prompt needs Linux pidfds (kernel 5.3 or later). Remote
  graphical sessions still exclude terminal image transfer.
- Persistence still follows the daemon lifetime: a crash, logout without
  lingering, or reboot ends running processes, and saved layouts do not
  checkpoint applications.

## Upgrading from 0.1.x

Save your work and upgrade from a terminal Splinterm does not own, such as Foot
or a text console. The running 0.1.x daemon must be restarted, which ends its
terminals; the desktop launcher restarts it by itself the first time it finds
the old daemon. Then reopen every Splinterm Window so it runs the new client,
and upgrade `splinterm-mcp` and any remote hosts to 0.2.0 too.

From 0.2.0 on, install the new package and run `splinterm upgrade --now` to
keep terminals running. Package scripts never restart or upgrade a running
daemon by themselves.

See `docs/cli.md`, `docs/packaging.md`, `docs/configuration.md`,
`docs/remote.md`, and `docs/automation.md` for details.
