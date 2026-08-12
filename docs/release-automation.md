# Release automation

This document defines the authority boundaries and state transitions for Splinterm releases. It is intentionally separate from product status: [`status.md`](status.md) states what is released, while this document states how a candidate may become a release.

## Authorities

| Concern | Authority |
| --- | --- |
| Product version | `[workspace.package].version` in `Cargo.toml` |
| Local Arch package layout | `packaging/PKGBUILD` |
| Source-built AUR recipe | `packaging/aur/PKGBUILD` and generated `.SRCINFO` |
| Prebuilt AUR recipe | `packaging/aur-bin/PKGBUILD` and generated `.SRCINFO` |
| Deterministic validation and artifact construction | GitHub Actions and checked-in scripts under `tools/release/` and `tools/package/` |
| Permission to publish | The protected GitHub `release` environment |
| Notifications and monitoring | n8n on Neuromancer; advisory only |
| Current public release truth | [`status.md`](status.md) |

n8n is not trusted with release authority. Its unavailability may delay a notification, but it cannot publish, approve, replace, or mutate a release.

## State machine

1. **Source** — an exact Git commit contains a consistent workspace version, package recipes, documentation, and tests.
2. **Candidate** — a manually dispatched, read-only workflow builds that commit once and emits a closed manifest, source archive, packages, checksums, release-notes draft, and AUR recipe drafts. Candidate artifacts are private GitHub workflow artifacts and are explicitly marked non-published.
3. **Approved** — a maintainer reviews the candidate and approves the protected GitHub `release` environment. This state is reserved for a later publishing workflow; creating a candidate never implies approval.
4. **Published** — automation creates the immutable tag and GitHub release from the approved candidate artifacts without rebuilding them.
5. **Distributed** — separately gated automation updates AUR recipes to the exact published assets and verifies their visible state.
6. **Recorded** — release URLs, hashes, workflow run, AUR versions, and resulting status-document changes are retained as release evidence.

A failure never advances the state. Retrying candidate construction creates a new workflow run. A changed commit or version is a different candidate and requires a new approval.

## Candidate contract

`.github/workflows/release-candidate.yml` is deliberately non-publishing:

- manual `workflow_dispatch` only;
- repository permission limited to `contents: read`;
- no GitHub Environment, release token, AUR credential, tag creation, push, or release API call;
- full Git history is fetched so the exact commit and previous version tag can be recorded;
- the requested version must exactly match Cargo and all three Arch recipes;
- website sources and deployment automation are absent from the source archive;
- package validation runs against extracted package contents;
- all output is uploaded as one private, retention-bounded workflow artifact.

The candidate manifest binds the repository, commit, version, tag, architecture, prior version tag, workflow run identity, and SHA-256 digest of every proposed release asset. A later publisher must consume this manifest and these exact artifacts rather than rebuilding.

## Human approval boundary

The future publishing job must use `environment: release`. Configure required reviewers in the GitHub repository settings before enabling that job. The workflow must display the candidate manifest, checksums, release notes, and validation run before pausing for approval.

Approval authorizes only the exact candidate identified by commit, version, manifest digest, and workflow run. AUR publication remains a distinct job after GitHub release verification so partial distribution can be diagnosed and retried without recreating the release.

## n8n boundary

A future n8n workflow may:

- receive GitHub `workflow_run`, deployment, release, and failure webhooks;
- send a concise candidate summary and direct GitHub approval link;
- remind on a pending approval;
- verify that GitHub and AUR expose the expected versions and hashes; and
- report a bounded recovery action when a stage fails.

It must not hold a GitHub release token, AUR SSH key, or an approval-bypass secret. Webhook payloads must be signature-verified, deduplicated by delivery ID, and treated as notifications rather than release commands.
