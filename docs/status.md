# Current product status

This document is the repository authority for Splinterm's current maturity,
validated product scope, availability, and release gates. Historical plans and
evidence explain how a capability was accepted; the roadmap owns future work;
this page owns what the product is today.

## Maturity

**Splinterm is a public alpha.**

Source, documentation, the versioned AUR package, and commit-bound Arch edge
packages are publicly available. Core terminal emulation, daemon-owned persistence, multiplexing,
native Wayland presentation, Arch packaging, and bounded automation workflows
are implemented and validated in the scopes named below. Public availability is
not a stable-support promise: alpha interfaces may change, the validated target
remains narrow, and broader compatibility guarantees have not been released.

Splinterm is **security-conscious**, not absolutely secure. Automation is
constrained by exact executable identity, explicit scopes, resource and message
bounds, exclusive controller ownership, consent, revocation, and body-free audit
metadata. Terminal output remains untrusted data and cannot grant authority or
become an automatic instruction.

## Validated environment

The current product target is:

- x86_64 Omarchy/Arch Linux;
- native Wayland under the documented Hyprland environment;
- Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
  as the terminal-behavior oracle; and
- a public alpha `0.1.0alpha1` Arch package built from clean committed source.

Other Linux distributions, compositors, architectures, and package formats are
not current compatibility promises. Headless `splinterd` does not require a
graphical environment, but its packaged and remote workflows remain alpha
interfaces on the documented platform.

## Capability truth table

| Area | Classification | Current boundary and evidence |
| --- | --- | --- |
| Native Wayland presentation | Implemented and validated | Keyboard, pointer, selection, clipboard, IME, scaling, damage-driven SHM rendering, and guarded Hyprland matrices are accepted within the documented target. See [Architecture](architecture.md) and [Plan 0002](plans/0002-omarchy-terminal-mvp.md). |
| Native blur | Implemented and validated | Optional, compositor-capability-gated blur for translucent themes; unsupported protocol capability falls back to ordinary transparency. See [Plan 0013](plans/0013-native-background-blur.md). |
| Persistent sessions and explicit restore | Implemented and validated | `splinterd` owns shells, terminal state, layouts, and metadata. Client detachment does not end them; exited processes restart only through explicit restore. See [Architecture](architecture.md) and [Headless operation](headless.md). |
| Panes and multiple Dojos | Implemented and validated | Persistent split trees, focus, ratios, lifecycle operations, search, and multiple simultaneous clients are accepted. See [Roadmap](roadmap.md). |
| Window-local Dojo tabs | Implemented and validated | Up to 32 client-local tabs may span Lairs; closing a tab detaches the view and does not close daemon topology. See [Plan 0019 closure evidence](plans/artifacts/0019-dojo-tabs/closure-2026-08-09/EVIDENCE.md). |
| Multi-client control | Implemented and validated | Exclusive controller ownership, transfer, denial, trusted forced takeover, disconnect cleanup, and observer fallback are bounded. See [Automation](automation.md). |
| JSON/NDJSON automation | Implemented and validated | Versioned schema-major-2 one-shot and subscription contracts with stable exit categories and checked-in schemas. See [Automation](automation.md) and [CLI reference](cli.md). |
| SSH relay | Implemented and validated | Policy-scoped stdio automation relay and private human graphical relay; no daemon network listener. See [Remote access](remote.md). |
| Native remote graphical client | Implemented and validated | Profile-bound OpenSSH transport, native picker/window workflow, control, reconnect diagnostics, and client-local lifecycle; remote image transfer is not supported. See [Plan 0028](plans/0028-remote-graphical-client.md). |
| MCP adapter | Implemented and validated | Optional, separately packaged, exact-identity adapter over the supported automation surface. See [MCP](mcp.md). |
| Terminal images | Supported documented subset | Sixel, practical static Kitty, and inline iTerm2 PNG subsets are bounded; full Kitty graphics is not claimed. See [Images](images.md). |
| Arch/Omarchy packaging | Public alpha packages validated | Versioned AUR and commit-bound edge split packages, service, desktop metadata, upgrade checks, trusted-client identity, and rollback guidance. See [Packaging](packaging.md). |
| AUR package | Available | [`splinterm` `0.1.0alpha1-1`](https://aur.archlinux.org/packages/splinterm) publishes the main package and optional exact-version `splinterm-mcp` split package from an immutable checksummed source release. |
| Public source and edge channel | Available | The repository, documentation, immutable edge releases, and rolling verified installer channel are public. |
| Stable support | Unreleased | No compatibility window, support duration, or formal support/security-reporting process is promised yet. |
| Nix and broader distribution | Planned | Not current product behavior or support. |

**Classification meanings:** implemented means present in current code; validated
means required recorded evidence exists for the named scope; supported means a
documented compatibility contract exists; proposed and planned mean not current
behavior; deferred means intentionally outside the present product.

## Important limitations

- A graphical Window is a disposable view. Topology commands do not map, focus,
  move, resize, or assign compositor windows.
- Native Wayland does not imply GPU rendering, universal compositor support, or
  automatic performance superiority. The renderer currently uses CPU composition
  and Wayland shared-memory buffers.
- Persistence follows the daemon lifetime. Stopping or upgrading an incompatible
  daemon ends its child processes; saved launch metadata is never executed
  automatically.
- Machine clients do not inherit human graphical authority. Raw daemon frames and
  Rust protocol types are private interfaces.
- Controller leases are exclusive and connection-owned. Observers may read only
  within their granted scope and do not implicitly gain input authority.
- Remote graphical sessions currently exclude terminal image transfer.
- Image compatibility is deliberately narrower than full Kitty graphics; external
  file and shared-memory media are rejected.
- Configuration is focused rather than arbitrary `foot.ini` compatibility.
- Current benchmark gates document remaining performance work. In particular,
  [Plan 0011](plans/0011-burst-output-memory-retention.md) retains its final
  client-performance no-go and [Plan 0012](plans/0012-bounded-compact-publication-frames.md)
  remains blocked on a sparse bounded-frame ownership redesign.

## Stable-release gates

Before Splinterm can graduate from public alpha to a supported stable release,
maintainers must make and validate explicit decisions about:

- release channels, signed/immutable source publication, upgrades, and rollback;
- supported architectures, distributions, compositor versions, and compatibility
  duration;
- support and security-reporting processes;
- completion or explicit disposition of release-blocking performance gates;
- public installation and recovery testing beyond the maintainer workflow; and
- any promised Nix, sandboxed package, or broader Linux support.

None of those unresolved decisions weakens the accepted public-alpha
capabilities above; none may be inferred as a stable-support promise.

## Documentation authority

| Subject | Authority |
| --- | --- |
| Current maturity, availability, validated scope, and release gates | This document |
| Product entry point and first workflow | [`README.md`](../README.md) |
| Human operation and controls | [Usage](usage.md) |
| Human and machine command inventory | [CLI reference](cli.md) |
| Ownership and system boundaries | [Architecture](architecture.md) |
| Configuration and Foot migration | [Configuration](configuration.md) |
| JSON/NDJSON policy, schemas, limits, and exit behavior | [Automation](automation.md) |
| SSH and native remote workflows | [Remote access](remote.md) |
| MCP integration | [MCP](mcp.md) |
| Image compatibility | [Images](images.md) |
| Service, persistence, policy, backup, and reset | [Headless operation](headless.md) |
| Public alpha package installation and upgrades | [Packaging](packaging.md) |
| Completed phases and future work | [Roadmap](roadmap.md) |
| Development workflow and test guardrails | [`CONTRIBUTING.md`](../CONTRIBUTING.md) |

The public website may summarize these sources for readers, but it does not
replace repository authority.
