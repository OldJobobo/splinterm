# Splinterm 0.1.0 Beta 3 — Room to Work

Beta 3 removes task ceilings that were too small for ordinary applications,
polishes the Dojo tab strip, and gives legacy unnamed Dojos a stable readable
label.

## Workloads inherit the user manager's task policy

Beta 2 introduced the correct nested systemd hierarchy but shipped provisional
fixed ceilings of 512 tasks per Splint, 1024 per Dojo, and 2048 across all
terminal workloads. Because Linux counts both processes and threads, ordinary
Chromium, Node, editor, build, and coding-agent workloads can reach those limits.
Rejected task creation may then appear as an application failure even though the
workload is not runaway.

Beta 3 keeps the containment hierarchy and exact pre-exec PTY placement:

```text
splinterd.service
app-splinterm.slice
└── app-splinterm-dojo<ID>.slice
    └── splinterm-splint<ID>-<incarnation>.scope
```

The corrected policy is:

- `splinterd.service` retains `TasksMax=2048` for the small control plane;
- the aggregate workload slice does not set `TasksMax`;
- transient Dojo slices do not set `TasksMax`; and
- transient Splint scopes do not set `TasksMax`.

Terminal workloads therefore inherit the systemd user manager's
`DefaultTasksMax`, subject to any stricter administrator or ancestor policy.
`EffectiveTasksMax` is the authoritative runtime value. The existing
`MemoryHigh` pressure boundaries remain 75% aggregate, 50% per Dojo, and 25%
per Splint; Beta 3 still sets no `MemoryMax`.

## Clearer Dojo tabs

The New Dojo `+` control now sits immediately after the final visible tab instead
of being pinned to the far-right edge of the Window. When the strip is full, the
existing bounded active-tab visibility and hit targets remain unchanged.

Inactive tabs now use the theme's semantic pane-border color for a narrow exact
divider. The active tab retains its exact theme-provided body, contrasting
foreground, and accent underline.

Older persisted unnamed Lairs may still carry historical generated identities
such as `terminal-324324573264238-4132`. Reopening one from the Dojo picker now
presents its initial tab as `Dojo 1`. This is a presentation-only compatibility
rule: explicit Dojo names remain unchanged and daemon-owned topology is not
renamed.

## Upgrade boundary

The corrected task policy applies when the daemon and transient workload
hierarchy are recreated. Existing Beta 2 scopes retain their old explicit limits
until then.

Splinterm 0.1 does not support live daemon upgrade handoff. Upgrading Beta 2 to
Beta 3 therefore ends active Dojos. From Foot or another terminal not owned by
`splinterd`, use this lifecycle around the package upgrade:

```bash
systemctl --user stop splinterd.service
# upgrade the splinterm package here
systemctl --user daemon-reload
systemctl --user start splinterd.service
```

Then reopen Splinterm Windows. Package installation does not silently reload or
restart the user service.

## Compatibility

- Persistent topology, restore, history, remote, automation, MCP, preset, and
  terminal protocol contracts are unchanged.
- Exact cgroup placement and workload cleanup remain mandatory in packaged mode.
- Explicit Dojo names remain authoritative; only the strict historical
  `terminal-<timestamp>[-<pid>]` compatibility shape is presented as unnamed.
- Beta 2 tags and packages remain immutable.
- Beta 3 continues to target x86_64 Omarchy/Arch Linux with native Wayland.
