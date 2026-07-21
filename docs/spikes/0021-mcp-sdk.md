# Spike 0021: rmcp 2.2.0 stdio server

- **Date:** 2026-07-21
- **Plan:** [Plan 0007, Slice 0](../plans/0007-phase4-mcp-adapter.md)
- **Result:** Pass
- **Scope:** Non-shipping SDK/protocol proof only; no daemon client, production
  tools, packaging, network transport, OAuth, or client implementation.

## Decision and pin

The workspace MSRV is raised from Rust 1.85 to **Rust 1.88**. The exact
`rmcp-macros 2.2.0` dependency uses language/library support unavailable to the
old toolchain; the user approved raising the project MSRV rather than replacing
the official SDK or avoiding its reviewed macros. CI now runs 1.88.0 so the
MSRV remains an exercised contract.

The workspace dependency is exactly:

```toml
rmcp = { version = "=2.2.0", default-features = false, features = ["macros", "schemars", "server", "transport-io"] }
```

These are the only explicitly enabled rmcp features. `server` enables the
SDK's async-read/write transport and schema dependency, `macros` enables the
official handler/tool macros, `schemars` makes schema intent explicit, and
`transport-io` supplies stdio support. `cargo tree -e features` confirmed that
rmcp's default `base64` feature is absent. The `client`, `auth`, every reqwest
and streamable-HTTP feature, server-side HTTP, SSE, OAuth/JWT, child-process,
worker, tower, UUID, elicitation, and `which-command` features are absent.

## Spike design and protocol evidence

`crates/splinterm-mcp` is a workspace member with `publish = false`. Its
executable uses a custom `BoundedLineReader` before rmcp's async read/write
transport. The wrapper allocates one 256 KiB buffer at construction, waits for
a newline before releasing a frame to rmcp, counts the newline in the limit,
and returns an error when the buffer is full without a newline. It therefore
never reads or allocates a 256 KiB-plus-one frame. An incomplete trailing frame
is discarded at EOF. stdout is owned only by the SDK transport; bounded startup
or transport errors go to stderr.

The daemon-free server deliberately exposes only three `splinterm.spike.*`
tools, one static resource, and two resource templates. They are test probes,
not the Plan 0007 production catalog. Initialization accepts exactly MCP
`2025-11-25`; all four other versions understood by rmcp 2.2.0 and an unknown
version receive JSON-RPC `-32600`, after which the process closes. Tool calls
also require `notifications/initialized`.

The initialization capability object is exactly:

```json
{"resources":{"subscribe":true},"tools":{}}
```

There is no prompt, logging, completion, experimental, extension, or task
capability. Roots, sampling, and elicitation are not server capabilities and
are never requested from the client. Every spike tool declares task support
`forbidden`, has a closed generated input schema, and is read-only/closed-world.
The echo probe returns matching `structuredContent` and one compact JSON text
block. The failure probe returns `isError: true`. The cancellation probe waits
on rmcp's request cancellation token, records observation, and its late result
is suppressed by the SDK.

The black-box subprocess suite proves:

- initialize response, exact version/capabilities, and initialized gating;
- exact static tool/resource/template lists and closed unknown-field rejection;
- reads for the static and both templated resource forms;
- subscribe, update notification, unsubscribe, and no post-unsubscribe update;
- structured success, compact JSON compatibility text, and structured tool error;
- `notifications/cancelled` reaches the handler and suppresses the cancelled response;
- every non-target known version plus an unknown version is rejected;
- every stdout line is valid JSON, initialized stdin EOF exits successfully;
- an exactly 256 KiB framed line is accepted; a larger line exits without output.

rmcp maps a tool-macro parameter deserialization failure to a caller-visible
`isError: true` tool result rather than JSON-RPC `-32602`. The spike verifies
that unknown fields are rejected and no handler body runs. Production schema
work must decide whether Plan 0007's protocol-error classification requires a
thin dispatch override; this does not block the Slice 0 SDK safety gate.

## Exact dependency and license inventory

On the Linux reference host, `cargo tree -p splinterm-mcp` resolved the direct
set to `anyhow 1.0.103`, `rmcp 2.2.0`, `serde 1.0.228`, `serde_json 1.0.150`, and
`tokio 1.52.4`. rmcp's immediate set is `async-trait 0.1.91`, `chrono 0.4.45`,
`futures 0.3.32`, `pastey 0.2.3`, `pin-project-lite 0.2.17`, `rmcp-macros
2.2.0`, `schemars 1.2.1`, `serde 1.0.228`, `serde_json 1.0.150`, `thiserror
2.0.18`, `tokio 1.52.4`, `tokio-util 0.7.18`, and `tracing 0.1.44`.

The complete active normal/build dependency inventory, grouped by the license
expression reported by Cargo metadata, is:

- **Apache-2.0:** `rmcp 2.2.0`, `rmcp-macros 2.2.0`.
- **MIT:** `bytes 1.12.1`, `darling 0.23.0`, `darling_core 0.23.0`,
  `darling_macro 0.23.0`, `mio 1.2.2`, `schemars 1.2.1`,
  `schemars_derive 1.2.1`, `slab 0.4.12`, `strsim 0.11.1`, `tokio 1.52.4`,
  `tokio-macros 2.7.0`, `tokio-util 0.7.18`, `tracing 0.1.44`,
  `tracing-attributes 0.1.31`, `tracing-core 0.1.36`, and `zmij 1.0.23`.
- **MIT OR Apache-2.0** (including reversed or slash spelling): `anyhow
  1.0.103`, `async-trait 0.1.91`, `autocfg 1.5.1`, `chrono 0.4.45`,
  `dyn-clone 1.0.20`, `errno 0.3.14`, `futures 0.3.32`, `futures-channel
  0.3.32`, `futures-core 0.3.32`, `futures-executor 0.3.32`, `futures-io
  0.3.33`, `futures-macro 0.3.32`, `futures-sink 0.3.33`, `futures-task
  0.3.32`, `futures-util 0.3.32`, `ident_case 1.0.1`, `itoa 1.0.18`, `libc
  0.2.186`, `num-traits 0.2.19`, `once_cell 1.21.4`, `pastey 0.2.3`,
  `pin-project-lite 0.2.17`, `proc-macro2 1.0.106`, `quote 1.0.46`,
  `ref-cast 1.0.26`, `ref-cast-impl 1.0.26`, `serde 1.0.228`, `serde_core
  1.0.228`, `serde_derive 1.0.228`, `serde_derive_internals 0.29.1`,
  `serde_json 1.0.150`, `signal-hook-registry 1.4.8`, `socket2 0.6.5`, `syn
  2.0.119`, `syn 3.0.2`, `thiserror 2.0.18`, and `thiserror-impl 2.0.18`.
- **Unlicense OR MIT:** `memchr 2.8.3`.
- **(MIT OR Apache-2.0) AND Unicode-3.0:** `unicode-ident 1.0.24`.

`Cargo.lock` records checksums and target-specific transitive packages. No
resolved package is named `reqwest`, `oauth2`, `jsonwebtoken`, `http`,
`http-body`, `hyper`, `axum`, `tower-service`, `sse-stream`, `url`,
`process-wrap`, or `which`.

## Commands

All commands used Rust 1.88.0:

```text
cargo +1.88.0 fmt --all --check
cargo +1.88.0 check --workspace --all-targets
cargo +1.88.0 test --workspace
cargo +1.88.0 clippy --workspace --all-targets -- -D warnings
cargo +1.88.0 check -p splinterm-mcp --all-targets
cargo +1.88.0 test -p splinterm-mcp --all-targets
cargo +1.88.0 clippy -p splinterm-mcp --all-targets -- -D warnings
cargo +1.88.0 tree -p splinterm-mcp
cargo +1.88.0 tree -p splinterm-mcp -e features
git diff --check
```

The final validation results are recorded in Plan 0007 Slice 0 after all gates
pass.
