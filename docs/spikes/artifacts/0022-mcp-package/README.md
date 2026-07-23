# MCP Slice 9 package and interoperability evidence

Date: 2026-07-23. No package was installed, no `sudo` was used, and no graphical
test ran.

## Split package

An isolated current-worktree source archive under
`/tmp/splinterm-slice9-package` produced:

- `splinterm-0.1.0.pre-1-x86_64.pkg.tar.zst`, SHA-256
  `36c3917bb709f1cfc1e44c26ebc7be3c7cc8f88d4c77744ba0ebf58de288df86`;
- `splinterm-mcp-0.1.0.pre-1-x86_64.pkg.tar.zst`, SHA-256
  `0112325375663057296ca0071ab57c34abca855557d284ba78972e3e16452ca9`.

The optional package depends on exact `splinterm=0.1.0.pre-1` and contains only
`/usr/bin/splinterm-mcp`, `mcp.md`, and project/third-party notices. The main
package does not contain the adapter. `validate-package.py` extracted both into
an isolated root and passed layout, mode, linkage, desktop/service, headless,
reference picker, SSH relay, theme/launcher, and MCP runtime checks. The MCP
runtime check used the packaged canonical executable/digest against an isolated
real daemon and proved deny-all discovery, exact 32-tool/one-resource/two-template
inventory, authorized terminal read, topology mutation, controller input/release,
resource update/unsubscribe, under-scoped controller denial, and disconnect/socket
cleanup.

`namcap` reported warnings but no errors. MCP warnings were the intentional
runtime dependency on the main package, implicit libgcc, and conservative
`gcc-libs`; no missing library was reported.

The first `makepkg` check attempt compiled successfully and passed 14 of 16
real-daemon scenarios, but two known 20-second timing-sensitive scenarios timed
out in the constrained build environment. They had passed in normal serialized
validation. The bounded follow-up reused the built release artifacts with
`--nocheck`; acceptance therefore depends on the separately recorded workspace
suite rather than claiming the embedded check passed.

## Host and Inspector evidence

Actually installed host CLIs used for documentation:

- Claude Code `2.1.218` (`claude mcp add ... -- /usr/bin/splinterm-mcp`);
- Visual Studio Code `1.125.0` (`code --add-mcp ...` and `.vscode/mcp.json`).

MCP Inspector was run ephemerally as
`@modelcontextprotocol/inspector@1.0.0` over stdio. Its first strict discovery
run rejected externally referenced schemas and success-only output schemas.
The adapter was corrected to advertise self-contained object schemas whose
output is the union of the frozen success contract and frozen error contract;
runtime validation and checked-in schemas remain unchanged. The final Inspector
runs passed initialization implicitly, 32-tool list, one fixed resource, two
resource templates, a successful real-daemon `splinterm.ping`, and a schema-valid
`invalid_argument` tool error. A stalled-daemon run returned the adapter's bounded
`timeout`; Inspector CLI 1.0.0 exposes no direct cancellation or durable
subscription command suitable for proving its client-side cancellation path.
The repository black-box suite remains the cancellation/subscription authority.

## Official conformance

The official `@modelcontextprotocol/conformance@0.1.16` runner was fetched and
its `2025-11-25` server scenarios were listed. `conformance server` requires
`--url <url>` and has no stdio command option. Running the supported initialize
scenario without a URL failed exactly with `required option '--url <url>' not
specified`. Splinterm did not add HTTP/OAuth merely for this runner. Therefore
no official server scenario was executed and no conformance pass is claimed.

## Closure decision

The package, extracted-tree runtime, two host configurations, Inspector checks,
and repository stdio protocol suite are complete. A reviewed acceptance
amendment approved on 2026-07-23 makes the black-box stdio and extracted-package
suites authoritative for cancellation and durable-subscription lanes absent
from Inspector CLI 1.0.0. Inspector remains the interoperability authority for
initialization, discovery, successful calls, and tool errors.

Official conformance 0.1.16 exposes no stdio server scenario, so the plan's
existing unsupported-runner fallback applies; no HTTP/OAuth transport was added
and no official conformance pass is claimed. Under these explicit evidence rules,
Plan 0007 Slice 9, Plan 0007, and core Phase 4 are complete.
