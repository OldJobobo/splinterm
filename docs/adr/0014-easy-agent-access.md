# ADR 0014: easy agent access

- **Status:** Accepted for `0.2.0`; implementation pending
- **Date:** 2026-10-20
- **Amends:** [ADR 0005](0005-trusted-consent-broker.md) (consent outcomes,
  prompt input, and persistence), [ADR 0007](0007-supported-automation-policy.md)
  (a second persistent authority source beside policy), and
  [ADR 0011](0011-guarded-in-place-daemon-reexec.md) (what an upgrade reloads)

## Context

Today an agent gets access to a live workspace in one of two ways. It can ask
through `RequestLairAccess`, which opens the trusted consent window. That
window offers only Deny and Grant once, expires after 20 seconds, and lists
individual scopes. The resulting grant lasts five minutes, lives in daemon
memory, and is bound to the requesting process, so a long agent session is
asked again and again and a restarted agent starts over. Or the person writes a
`policy.json` rule by hand, naming the adapter's path and SHA-256 digest, the
scopes, and exact resource IDs. The rule stops matching after every package
upgrade, covers only the resources present when it was loaded, and every reload
disconnects all automation clients. Nothing lists which agent can reach what.

Granting access must be easy for the person and still deliberate. The design
also has to be honest about what it protects. A program running as the user
can already read the user's files and keys and run any command without
Splinterm. Consent and policy cannot stop that program. What they can do is
keep cooperating agents inside what the person agreed to, show the person which
agent can reach what, and let the person take access away.

## Decision

### What this protects, and what it does not

Access levels, durations, remembered grants, and the access list bound
cooperating agents and make their access visible and revocable. They are not a
sandbox. A hostile program running as the account can edit any file the
account owns, including the remembered-grant store, can start the graphical
relay or client itself, and can reach terminals without asking Splinterm.
Documentation must say this wherever remembered access is described, in the
same terms as `FR-REMOTE-02`.

### Access levels

The consent window offers three levels. Each level is a fixed bundle of scopes,
and each larger level contains the smaller ones. Every level covers the named
workspace (Lair) and all of its current and future Dojos and Splints, as the
existing Lair grant does. No level reaches another Lair or a daemon-level
resource, so no level can create a new Lair. A grant carries no numeric limits
of its own; the protocol ceilings apply.

| Policy scope (ADR 0007) | Consent scope | Look | Look and type | Manage |
| --- | --- | :-: | :-: | :-: |
| `topology_metadata_read` | `topology_observe` | yes | yes | yes |
| `topology_subscribe` | `topology_observe` | yes | yes | yes |
| `terminal_visible_read` | `observe` | yes | yes | yes |
| `terminal_subscribe` | `observe` | yes | yes | yes |
| `scrollback_read` | `scrollback` | yes | yes | yes |
| `scrollback_search` | `scrollback` | yes | yes | yes |
| `agent_status_report` | `agent_status_report` | yes | yes | yes |
| `controller_acquire` | implied by `input` or `resize` | | yes | yes |
| `input` | `input` | | yes | yes |
| `resize` | `resize` | | yes | yes |
| `process_spawn` | `process_spawn` | | | yes |
| `topology_layout_mutate` | `topology_layout` | | | yes |
| `topology_name_mutate` | `topology_name` | | | yes |
| `process_terminate` | `terminate` | | | yes |
| `process_restore` | `process_restore` | | | yes |
| `controller_transfer` | `control_takeover` | | | |
| `authorization_inspect` | none | | | |
| `authorization_revoke` | none | | | |
| `audit_inspect` | none | | | |

In words:

- **Look** reads the workspace's layout, the visible terminal, and scrollback,
  including search.
- **Look and type** adds sending input and resizing. It acquires control only
  of a Splint that no other connection controls. It cannot take control from
  the person's Window.
- **Manage** adds starting processes, changing layout and names, closing and
  terminating, and restoring.
- **Every level** includes reporting agent status.
- **No level** includes clipboard access, taking over control
  (`controller_transfer`), authorization inspection or revocation, audit
  inspection, or any handoff or trusted-UI authority. Forced takeover remains
  trusted-UI-only, and these other scopes stay available only through the
  policy file.

An agent may ask for a level by name; the daemon expands the name to exactly the
bundle above. An agent may instead ask for an explicit scope set. If that set
equals one bundle exactly, the prompt names that level. Otherwise the prompt
shows **Custom** access, lists every requested scope, and grants exactly that
set. A request is never rounded up to the next level, and the prompt never grants
more than was asked for. Scopes no level contains, such as `control_takeover`,
always make a request Custom. The person can accept or deny the request as
shown. To get less, the person denies and the agent asks again.

### Durations

The person chooses one of three durations. Duration and identity are decided
together: the longer a grant lasts, the more stable, and the more carefully
checked, the identity it is bound to.

| Choice | Bound to | Lasts | Survives | Ends |
| --- | --- | --- | --- | --- |
| **Just this once** | The requesting process: UID, PID, start time, executable device and inode, and executable path and digest (today's grant) | Five minutes | Closing every Window | Expiry, requester exit, revocation, workspace end, daemon restart or upgrade |
| **While this workspace is open** | The verified executable identity of ADR 0007 (UID, canonical path, and SHA-256 digest of the executing file) plus the Lair | While the Lair exists in this daemon generation | Agent restart and reconnect, closing every Window | Workspace end, daemon stop, restart, crash, or upgrade, revocation |
| **Always for this workspace** | A durable identity (below) plus the Lair | Until revoked or the workspace is removed; no automatic expiry | Daemon restart and upgrade, agent restart and upgrade | Revocation, workspace removal |

A **workspace ends** when its Lair leaves daemon topology: it is terminated,
retired by the capacity policy, or its saved metadata is removed. Closing its
last Window, detaching, or the exit of all its Splints does not end it. These
are the same events that revoke a Lair grant today. Lair IDs are never reused,
so a grant can never attach to a different workspace.

"While this workspace is open" means the Lair's lifetime, not Window
visibility. Help text says "until this workspace is closed or Splinterm
restarts". A graphical disconnect never revokes it.

"Always for this workspace" has no inactivity expiry. The access list shows when
each remembered grant was last used. An optional explicit expiry may be added
later; a hidden inactivity rule will not.

A development build qualifies for Just this once and While this workspace is
open, because both bind its exact digest. Rebuilding changes the digest and
therefore the identity. Always is not offered to it.

### Identity for remembered grants

A remembered grant stores a durable identity instead of a digest, so that it
survives upgrades of the agent. The daemon accepts exactly two kinds, checks the
connecting process against the stored identity on **every new connection**, and
caches nothing between connections.

Both kinds start from the same evidence ADR 0007 already requires. The daemon
reads `SO_PEERCRED` and `SO_PEERPIDFD`, opens the peer's `/proc/<pid>/exe`
read-only, and treats that open file as the executing file. Without a pidfd,
remembered authority fails closed.

**Protected install.** The stored identity is an absolute, canonical path `P`.
A connection matches when, at that connection:

- every component of `P`, from `/` to the file, opens without following a
  symbolic link;
- every directory on the path, and the file itself, is owned by root and is
  neither group- nor world-writable, and none carries an access ACL that grants
  write permission to anyone else;
- the file is a regular executable file; and
- the file opened through `P` in the daemon's mount namespace has the same
  device and inode as the executing file.

The last check means an executable running from a different mount namespace,
a copy, or an old, replaced file does not match. After a package upgrade
replaces the file at `P`, newly started agents match without a prompt. An agent
process that is still running the old, deleted file does not match. It must
be restarted, or it is asked again.

**Nix profile.** The stored identity is a profile entry path `P` from this
closed list, where `HOME` and `USER` come from the password database entry for
the daemon's UID, never from any environment:

- `/run/current-system/sw/bin/NAME`
- `/etc/profiles/per-user/USER/bin/NAME`
- `/nix/var/nix/profiles/default/bin/NAME`
- `/nix/var/nix/profiles/per-user/USER/profile/bin/NAME`
- `HOME/.nix-profile/bin/NAME`
- `HOME/.local/state/nix/profile/bin/NAME`

`NAME` is a single path component. A connection matches when, at that
connection:

- the daemon resolves `P` one link at a time, following at most 40 symbolic
  links, in its own mount namespace;
- the final target is an absolute path of the form
  `/nix/store/HASH-NAME/...`, where `HASH-NAME` is a well-formed store object
  name, and names a regular executable file;
- the store object and every directory and file beneath it on that path are
  owned by root and are neither group- nor world-writable; `/nix/store` itself
  must be owned by root and may be the usual sticky, group-writable store
  directory; and
- the resolved file has the same device and inode as the executing file.

Intermediate links may be owned by the user. That is what lets a profile follow
upgrades, and it is the accepted residual risk of this kind: **the profile link
is user-controlled, so any software running as the user can repoint it at a
different store path**, and the next process started from that store path
matches the remembered grant. A single-user Nix installation, whose store is
not owned by root, does not qualify. An executable started through a Nix
wrapper script runs a different file from the one the profile entry names, so
it does not match.

To offer Always, the daemon must find a qualifying identity at prompt time. It
first tests the executing path as a protected install. Otherwise it tests the
profile entries in the closed list whose `NAME` equals the executing file's
name. The name only selects which entries to test; the device and inode check
decides the match.

**Never an identity.** Host name, basename, MCP client name, argv, environment,
parent process, the same UID, or a copied digest never establish or select a
remembered grant. The host-reported agent name is stored and shown only as
display data.

**General-purpose executables.** The identity names whatever executable
connects. If that is an interpreter running a script, every program run by the
same interpreter shares the grant. The prompt and the access list always show
the exact executable path so the person can see this. The `splinterm relay`
executable is never eligible for While this workspace is open or Always,
because it relays for every caller that can reach it over SSH. Remote and
headless automation keeps using the policy file.

### The prompt

The prompt stays a daemon-launched trusted Window, as decided in ADR 0005. It
changes as follows:

- It shows the agent's host-reported name as data, the exact executable path,
  the workspace name and short ID, the level name with one line saying what it
  allows (for example "Read terminals, type commands, and create or close
  sessions" for Manage), and a details control listing the exact scopes. For a
  Custom request, the scope list is always visible.
- It is **default-deny**. Enter, Escape, and `D` deny. Only `G` or a pointer
  click on the grant button grants. This is a deliberate change: the access
  dialog grants on Enter today.
- Key presses and the grant click are ignored until an arming delay of 600 ms
  after the Window first gains keyboard focus. Key repeats are always ignored.
  This matches the upgrade question.
- The person selects a duration. **Just this once** is preselected. Selecting a
  duration never grants.
- The grant button names the exact choice: "Allow Look once", "Allow Look and
  type while 'Workspace' is open", "Always allow Manage for 'Workspace'". This
  one careful prompt is the whole confirmation. No level, including Manage with
  Always, asks a second time.
- Always is shown disabled, with a one-line reason, when the requester has no
  qualifying durable identity.
- When the answer would replace an existing remembered grant for the same
  identity and workspace, the prompt says what it replaces.

The workspace name is user data and can be renamed by any agent holding Manage.
It is always shown together with the short ID.

### Pending requests

A prompt lasts longer than an MCP call. MCP calls use a 5-second default daemon
deadline, configurable up to 30 seconds. So a request that needs a prompt
becomes a bounded, cancellable **pending request**:

- The daemon opens at most one pending request per requesting process and
  workspace, and at most eight in total. Excess requests fail with the retryable
  `resource_limit`.
- A pending request expires 120 seconds after it opens. This replaces the
  20-second prompt deadline. While it waits, Windows show a visible pending
  indicator, and the access list shows it.
- The call that opened it waits up to its own deadline. If the person has not
  answered, the call returns a `pending` result with the request's identifier
  and expiry. This is not a failure.
- The requesting process collects the answer by repeating the same request from
  any of its connections. A repeat with the same workspace and the same access
  joins the pending request and never opens a second prompt. A repeat asking for
  different access while one is pending fails with the existing request's
  identifier. The requester can also cancel it.
- A pending request ends without a grant when it expires, when it is cancelled,
  when the requesting process exits, when the workspace ends, or when the daemon
  fences for an upgrade. The prompt closes. An answer that arrives later grants
  nothing.
- Before committing an answer, the daemon validates again: the request is
  still pending; the requester process is alive with the same identity; the
  workspace still exists; the scopes are still valid; for Always, the durable
  identity still qualifies; no upgrade fence is active; and, for Always, the
  durable write has succeeded.
- Pending requests are never carried across a daemon upgrade or restart.

If no trusted graphical session can show the prompt, the request fails closed
immediately, as today.

### Authorization and composition

Policy rules, Just-this-once grants, While-this-workspace-is-open grants, and
Always grants are independent sources. A request is authorized when any one
source covers every scope it needs. Scopes are not combined across sources.
Controller ownership, operation preconditions, resource checks, and protocol
limits apply unchanged.

Each duration keeps its own definitions. A short answer never widens a longer
grant. Granting Always for the same identity and workspace replaces the earlier
Always definition with exactly what the new prompt showed. Repeating a While
open grant replaces that definition in the same way. The Just-this-once grant
keeps its current behavior. A request already covered by an existing grant is
answered without a prompt.

### The remembered-grant store

Remembered grants live in a daemon-owned store, separate from `policy.json`.
The policy file stays the expert and headless path, and is unchanged.

- The store is `$XDG_STATE_HOME/splinterm/agent-access.json` (by default
  `~/.local/state/splinterm/agent-access.json`). It sits beside the session
  state its Lair IDs refer to. `splinterm reset` moves it to the same backup as
  the session state. The private format is versioned and is not a public
  compatibility surface.
- It holds at most 256 entries and 256 KiB. Each entry holds a stable entry ID,
  the identity kind and path, the Lair ID, the level or Custom scope set, the
  creation time, the last-used time, and the host-reported name as display data.
  It holds no secret, token, digest, or terminal data.
- It is opened like the policy file: no symbolic link in any path component, a
  regular file with one hard link, owned by the daemon UID with mode `0600`,
  within the size bound, in an owner-only directory.
- Writes are atomic. The daemon writes a new `0600` file in the same directory,
  syncs it, renames it over the store, and syncs the directory. Only then does
  it change memory or report success. A failed write grants nothing for that
  answer, not even a downgraded Just-this-once grant. The agent receives an
  error, and the person sees the failure.
- A missing store means no remembered grants. A store that is unsafe, corrupt,
  oversized, or of an unsupported version makes the remembered source deny
  everything. That failure never affects policy or the in-memory grants. The
  daemon reports the failure and offers no Always choice until the store is
  repaired. It never silently overwrites a rejected store; recovery is an
  explicit trusted action that moves the file aside to a reported backup.
- Last-used time is updated in memory on every authorized request and written
  at most once an hour per entry and at clean shutdown. It is display data and
  never extends or ends a grant.
- An entry whose Lair is absent when the store is loaded authorizes nothing. It
  is listed as "workspace missing" until it is revoked. Entries for a Lair are
  removed when the daemon observes that workspace ending.

The store is only as safe as the account. Any process running as the user can
edit it, as it can edit `policy.json`; no key stored under the same account
would change that. This is the same limit as the rest of this ADR.

### Access list and revocation

A daemon-wide, bounded inventory, shown as **Agent access** in the command
palette and through a human CLI command, lists every grant definition and its
active use:

- every Always, While open, and Just-this-once grant, with the agent's display
  name, the executable path, the identity kind (protected install, Nix profile,
  exact build, or process), the workspace, the level or Custom scopes, the
  duration, the creation time, and, for Always, the last-used time;
- the active use of each grant: connected processes, controllers it
  authorized, and pending requests; and
- policy rules that match a connected agent, marked as coming from the policy
  file and read-only here.

Remembered entry IDs and runtime grant IDs are distinct, and the list never
confuses the two. The inventory is available to the trusted UI and the human
CLI. A machine client needs `authorization_inspect` or `authorization_revoke`
from the policy file; no level includes them.

Revoke works per entry. It stops new requests at once, cancels the grant's
subscriptions, and releases the controllers it authorized. For an Always entry
the daemon also removes it durably. If that write fails, the grant stays
revoked in memory and the list says it will return after a restart unless
removal succeeds. **Revoke all access** revokes every consent-derived grant of
every duration and says how many remembered grants it removes. It never
touches policy rules or the trusted UI.

Revoking a remembered grant does not remove a `policy.json` rule that matches
the same agent. When such a rule exists, the list says that access continues
under the policy file, names the rule ID, and explains that the rule must be
edited in the policy file. The Explorer marks workspaces an agent can reach, and
distinguishes them from workspaces an agent is connected to now.

### Agent status

`agent_status_report` is in every level, so a granted agent can show its status
for any Splint in the workspace. Reporting uses the ordinary automation
transport and role. There is no separate reporting role, and the handoff
continuation channel is never used. Self-reports from inside a Splint still
need no grant. A grant is identity-wide: a While open or Always grant authorizes
every connection whose process matches its identity, so a status report on a
fresh connection is covered without asking again. Whether an adapter reuses one
socket is an implementation choice. A status report is display data: it never
creates, extends, or confirms a grant.

### Upgrade and handoff

Remembered definitions are durable user state, not handoff state. They are not
written into the handoff checkpoint. An adopted or rolled-back generation loads
the store fresh from disk, with the same checks, before admission reopens. A
generation that does not support the store's version treats it as unavailable
and never rewrites it. Old connections, controllers, Just-this-once grants,
While open grants, and pending requests all end at the fence, as ADR 0011
already requires. An agent reconnects, its new connection is checked against
the remembered identities again, and it continues without a prompt only if it
still matches. Loading a definition is not restoring an old session, and the
resume ticket never restores an agent's controller.

## Consequences

- An agent the person trusts with a workspace can keep working across its own
  restarts, daemon restarts, and package upgrades after one deliberate answer.
- The person can see every grant, its last use, and its active connections, and
  can revoke each one.
- The policy file keeps its exact-digest semantics and remains the only path for
  headless, remote, daemon-level, and administrative access.
- Remembered access depends on the installation model. Distribution packages and
  multi-user Nix qualify; development builds and user-writable installs get the
  shorter durations only.
- The Nix profile kind accepts a documented residual risk: same-user software
  can repoint the profile link.
- The access dialog's keyboard behavior changes from Enter-grants to
  default-deny.
- A prompt can outlast one MCP call, so the consent exchange gains a pending
  state and typed decisions instead of a yes/no reply.

## Validation requirements

Implementation must proceed through bounded milestones. At minimum, retained
non-graphical evidence must prove:

- each level expands to exactly the table above; an exact bundle is named and
  any other set is Custom; nothing is rounded up;
- every duration has the identity and lifetime above, including workspace end,
  daemon generation change, agent restart, and executable replacement;
- the protected-install check rejects symbolic links, writable or
  non-root-owned components, write ACLs, device or inode mismatch, and a
  deleted executing file;
- the Nix resolver rejects paths outside the closed list, link chains longer
  than 40, targets outside `/nix/store`, malformed store object names,
  non-root-owned or writable store paths, wrapper mismatches, and device or
  inode mismatch, and follows a repointed profile as documented;
- the store rejects unsafe files, corrupt and oversized content, and unknown
  versions by denying only the remembered source; a failed write grants
  nothing; success is never reported before the durable write;
- the prompt denies on Enter, Escape, and `D`, ignores input before arming and
  all key repeats, and grants only on `G` or the grant button;
- pending requests honor their bounds, join on repeat, and grant nothing after
  expiry, cancellation, requester exit, workspace end, or an upgrade fence;
- revocation releases subscriptions and controllers, handles durable-removal
  failure as documented, and leaves matching policy rules in place; and
- after an upgrade, remembered definitions reload fresh while connections,
  controllers, shorter grants, and pending requests end.

The trusted prompt, the access list, and the Explorer marks also require one
separately approved graphical acceptance session.
