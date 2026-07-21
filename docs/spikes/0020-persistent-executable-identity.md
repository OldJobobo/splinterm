# Spike 0020: persistent executable identity races

## Question

Can `splinterd` bind a persistent-policy decision to the exact executable that
opened a Unix-socket connection without trusting a reusable PID, a later path
lookup, or client-provided identity?

This spike implements and tests only the file-snapshot portion in
`crates/splinterd/src/executable_identity.rs`. It does not enable persistent
policy or replace the existing daemon-lifetime consent identity.

## File-snapshot result

A bounded open-descriptor algorithm is viable:

1. read `/proc/<pid>/exe` and require a bounded absolute path containing only
   root and normal components;
2. reject the kernel's ` (deleted)` suffix;
3. open `/proc/<pid>/exe` read-only with `O_CLOEXEC`;
4. require a nonempty regular file no larger than 256 MiB;
5. stream SHA-256 through a fixed 64 KiB buffer;
6. compare device, inode, size, owner, mode, mtime, and ctime before and after
   hashing; and
7. re-read the link and require its path lookup to name the opened device and
   inode.

Focused tests prove that this rejects relative paths, directories, oversized
files, in-place mutation during hashing, and path replacement after descriptor
open. Hashing the opened descriptor preserves the exact bytes even when a path
lookup could race.

## PID-reuse result

`SO_PEERCRED` plus `/proc/<pid>/exe` is not sufficient for persistent policy.
The process can exit after credential retrieval and its numeric PID can be
reused before or during the executable snapshot. A socket descriptor can also
be passed to another process. Treating the numeric PID as stable would allow the
snapshot to describe a process other than the peer that created the connection.

Linux 6.5 added `SO_PEERPIDFD`, which returns a pidfd bound by the kernel to the
Unix-socket peer. The Phase 4 persistent-policy path must:

1. obtain both `SO_PEERCRED` and `SO_PEERPIDFD` from the accepted socket;
2. verify that the pidfd describes the same PID reported by the credentials;
3. require the peer pidfd to remain alive before and after the executable
   snapshot;
4. perform bounded hashing in `spawn_blocking`, outside daemon locks; and
5. monitor the pidfd for the connection lifetime, closing the connection and
   releasing subscriptions/controllers if the original peer exits even when a
   passed socket descriptor remains open elsewhere.

If `SO_PEERPIDFD` is unavailable, unsupported, or inconsistent, persistent
policy authorization fails closed. Existing graphical grant-once behavior may
retain its daemon-lifetime device/inode identity until migrated separately; it
must not be promoted into persistent-policy authority.

Holding a peer pidfd and monitoring exit closes the PID-reuse and post-exit
descriptor-passing cases. It does not protect against an authorized executable
that intentionally delegates requests while still alive or a compromised user
account that can rewrite owner-controlled policy. Those remain the explicit
account-compromise boundary from ADR 0007.

## Evidence

```text
cargo test -p splinterd executable_identity -- --nocapture

5 passed; 0 failed
```

The cases are:

- the current process through `/proc/self/exe`;
- exact regular-file path, size, and known SHA-256;
- path replacement after descriptor open;
- in-place mutation after descriptor open; and
- relative, non-regular, and oversized targets.

## Decision

The open-descriptor algorithm is accepted as the file component of canonical
identity. `SO_PEERPIDFD` acquisition and lifetime monitoring are mandatory
prerequisites for the persistent-policy matcher. This raises the supported
persistent-policy kernel floor to Linux 6.5; the daemon must emit a bounded
fail-closed diagnostic rather than silently falling back to PID-only identity.
