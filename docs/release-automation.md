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
3. **Approved** — a maintainer starts `.github/workflows/promote-release.yml` with the exact candidate workflow run ID and manifest SHA-256, reviews the verified summary, and approves the protected GitHub `release` environment. Creating or selecting a candidate never implies approval.
4. **Published** — the protected job creates the immutable tag and GitHub prerelease from the approved candidate artifacts without rebuilding them, downloads every published asset, verifies the tag target and exact asset set, and retains a publication receipt.
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

The publishing job uses `environment: release`. Configure at least one required reviewer and exactly one custom deployment-branch policy named `main` in the GitHub repository settings before running it. The protected job independently queries the Environment and fails closed unless both controls are present; merely referencing an automatically created unprotected Environment cannot publish. The read-only verification job has only `actions: read` and `contents: read`; it accepts a candidate run ID and candidate-manifest SHA-256, proves that the source was one successful manual candidate run from `main`, paginates the run artifact inventory, requires exactly one unexpired artifact, binds its workflow commit to the candidate manifest commit, and closes over the exact file set and hashes before the protected job becomes approvable. The write-authorized job executes release tooling only from the reviewed promotion-workflow dispatch commit, never from candidate-controlled source, and checkout credentials are not persisted.

Approval authorizes only the exact candidate identified by commit, version, manifest digest, and workflow run. After approval, publication refuses an existing tag or release and never clobbers, force-updates, or deletes remote state. If a later step fails after tag or release creation, the workflow stops and reports the partial immutable state for maintainer diagnosis. It does not retry by replacing that state.

The protected `release` environment supplies `SPLINTERM_RELEASE_TOKEN`, a
fine-grained token scoped only to this repository with Contents and Workflows
read/write permission. The read-only verifier cannot access it. The protected
publisher needs Workflows permission because the immutable tag may point to a
candidate that changes files under `.github/workflows/`; GitHub rejects that
ref creation when only the job's ordinary `GITHUB_TOKEN` is used.

The publisher uploads only the source archive, main/MCP packages, candidate
manifest, and checksums. AUR drafts remain private inputs for the separately
gated distribution milestone. It downloads the public assets again, verifies
the tag resolves to the candidate commit, requires an exact asset set, and
retains a 90-day receipt. AUR publication remains a distinct job after GitHub
release verification so partial distribution can be diagnosed without
recreating the release.

## n8n boundary

A future n8n workflow may:

- receive GitHub `workflow_run`, deployment, release, and failure webhooks;
- send a concise candidate summary and direct GitHub approval link;
- remind on a pending approval;
- verify that GitHub and AUR expose the expected versions and hashes; and
- report a bounded recovery action when a stage fails.

It must not hold a GitHub release token, AUR SSH key, or an approval-bypass secret. Webhook payloads must be signature-verified, deduplicated by delivery ID, and treated as notifications rather than release commands.
