# Spike 0026 — Kitty external application-input security

- **Status:** Not accepted for production; external inputs remain disabled
- **Date:** 2026-07-24
- **Scope:** Kitty `t=f`, `t=t`, and `t=s` application-to-terminal inputs only
- **Distinct from:** the accepted daemon-to-trusted-UI content socket and sealed memfd transport

## Decision

Do not implement Kitty file, temporary-file, or POSIX shared-memory input in
Phase 5. Keep all three media fail-closed with bounded `ENOTSUP` replies. Direct
Kitty bytes and bounded iTerm2 inline base64 remain the only application image
inputs.

The spike is complete, but its production gate is deliberately **rejected**:
there is no single namespace and ownership policy that is both compatible with
representative local clients and safe across local, detached, headless, remote,
container, and automation-driven sessions. In particular, terminal output is
attacker-controlled data while the daemon retains the user's filesystem
credentials. Treating an application-supplied path or SHM name as daemon
instructions would expand PTY output into ambient read/delete authority.

## Threat model

An untrusted child, remote shell, replayed scroll stream, or compromised process
can emit arbitrary graphics escapes. It may race path replacement, name a
symlink, FIFO, device, procfs entry, socket, sparse/growing file, another user's
object, or a large SHM segment. It may disconnect, cancel, reset, or exit while
an input is opening or reading. Trusted UI content retrieval and automation
inspection must not expose the named source bytes or grant a read oracle.

The daemon and terminal actor run with authority broader than a remote child.
`SO_PEERCRED`, trusted-UI executable verification, and content-transfer tokens
authorize daemon-to-UI delivery; they do not authenticate a pathname embedded
in PTY output.

## Considered policies

### Ambient paths

Opening absolute paths or resolving relative paths against the daemon working
directory is rejected. It grants unrelated filesystem authority and has no
stable relationship to the emitting process.

### Child-working-directory sandbox

Resolving beneath a captured child cwd with `openat2(RESOLVE_BENEATH |
RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV)` narrows lookup,
but does not establish that the emitting process owns the object. The child cwd
can change, detached sessions outlive an initiating client, remote/container
namespaces may differ, and existing clients commonly use absolute temporary
paths. This policy is therefore not accepted as compatible or sufficient.

### Descriptor-first regular-file reads

`O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK`, regular-file/type/owner/mode
checks, exact descriptor-based size limits, bounded reads, and post-read inode
checks can prevent several symlink/FIFO/growth attacks. They still do not supply
an authorization boundary for which regular files PTY output may name.

### Temporary-file deletion

Even after descriptor validation, pathname unlink can delete a replacement
installed after open. Comparing inode metadata before `unlinkat` is not atomic
with deletion. Linux does not provide an unlink-by-open-descriptor primitive
with the required semantics. Automatic deletion is rejected without a private,
daemon-created namespace and capability-style handle.

### POSIX shared memory

A strict name grammar plus owner, mode, type, and exact-size checks narrows POSIX
SHM, but the global namespace, replacement/unlink lifetime, remote behavior, and
lack of capability binding remain unresolved. Reusing the trusted UI's sealed
memfd channel is not equivalent: that descriptor is daemon-created and passed
over an authenticated socket, while Kitty `t=s` supplies only a name.

## Required future acceptance design

External media may be reconsidered only with all of the following:

1. A capability is created through an authenticated local control operation,
   bound to one Splint incarnation, PTY actor, object identity, byte limit, and
   short deadline.
2. PTY graphics bytes carry the opaque capability rather than ambient paths or
   SHM names.
3. Files are opened descriptor-first with no symlinks/magic links/mount escape;
   only owner-matching regular files are accepted; FIFO/device/socket/procfs and
   sparse/growing inputs are rejected.
4. Temporary cleanup targets a private daemon-owned directory and cannot unlink
   a replacement.
5. Reads reserve process-wide inbound bytes before allocation, have exact size
   and deadline/cancellation bounds, and release all leases on every exit.
6. Source bytes and names never enter snapshots, replies, audit records,
   automation operations, or untrusted logs.
7. Local, detached, headless, remote, container, cancellation, daemon restart,
   and actor-drop semantics are specified and tested.

A capability design would intentionally diverge from ambient-path Kitty clients;
that compatibility/security tradeoff requires a separate product decision.

## Current enforcement and evidence

`crates/splinterm-terminal/src/image/kitty.rs` parses non-direct `t` media as
unsupported. `Terminal::transmit_kitty` rejects them before image decoding or
any filesystem/SHM operation. The adversarial terminal test
`external_media_never_open_unlink_or_consume_application_named_objects` sends a
regular path, symlink path, and SHM name through `t=f`, `t=t`, and `t=s`; each
returns bounded `ENOTSUP`, commits no image state, and leaves the regular file
and symlink unchanged.

Existing daemon tests `image_metadata_and_retrieval_require_the_matching_trusted_ui`,
`automation_role_never_receives_trusted_ui_bypass`,
`binary_image_content_channel_is_raw_windowed_and_acknowledged`, and
`sealed_image_content_channel_passes_one_exact_immutable_descriptor` prove that
metadata/body retrieval remains confined to the executable-verified trusted UI
and exact token-bound content channel. The automation-client test
`untrusted_role_rejects_image_content_source` proves an automation-role
connection cannot request an image body. No request accepts a terminal-derived
source path or returns the original external-object bytes.

## Gate result

- Path/SHM/replacement/oversize/cancellation risk analysis: complete.
- Ambient external input implementation: rejected.
- Unauthorized automation readback: absent by construction and existing trust tests.
- Configured byte/time escape: absent because external objects are never opened.
- Production status: `t=f`, `t=t`, and `t=s` remain disabled and unadvertised.
