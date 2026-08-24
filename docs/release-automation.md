# Release automation

This document defines the authority boundaries and state transitions for Splinterm releases. It is intentionally separate from product status: [`status.md`](status.md) states what is released, while this document states how a candidate may become a release.

## Authorities

| Concern | Authority |
| --- | --- |
| Product version | `[workspace.package].version` in `Cargo.toml` |
| Release authority branches | `main` for the active 0.2 line; `maint/0.1` for 0.1 maintenance |
| Local Arch package layout | `packaging/PKGBUILD` |
| Source-built AUR recipe | `packaging/aur/PKGBUILD` and generated `.SRCINFO` |
| Prebuilt AUR recipe | `packaging/aur-bin/PKGBUILD` and generated `.SRCINFO` |
| Deterministic validation and artifact construction | GitHub Actions and checked-in scripts under `tools/release/` and `tools/package/` |
| Permission to publish | The protected GitHub `release` environment |
| Notifications and monitoring | n8n on Neuromancer; advisory only |
| Current public release truth | [`status.md`](status.md) |

n8n is not trusted with release authority. Its unavailability may delay a notification, but it cannot publish, approve, replace, or mutate a release.

## State machine

1. **Source** — an exact reviewed commit on `main` or `maint/0.1` contains a consistent workspace version, package recipes, documentation, and tests.
2. **Candidate** — a manually dispatched, read-only workflow builds that commit once and emits a closed manifest, source archive, packages, checksums, release-notes draft, and AUR recipe drafts. Candidate artifacts are private GitHub workflow artifacts and are explicitly marked non-published.
3. **Approved** — a maintainer starts `.github/workflows/promote-release.yml` with the exact candidate workflow run ID and manifest SHA-256, reviews the verified summary, and approves the protected GitHub `release` environment. Creating or selecting a candidate never implies approval.
4. **Published** — the protected job creates the versioned tag and GitHub prerelease from the approved candidate artifacts without rebuilding them, downloads every published asset, verifies the tag target, title, prerelease state, release-notes digest, and exact asset set, and retains a publication receipt. An unsuccessful publication run may advance only through the bounded recovery path below, including receipt-only recovery when public state completed before durable receipt retention.
5. **Distributed** — `.github/workflows/distribute-aur.yml`, protected independently by `aur-release`, consumes the exact candidate plus a successful publication/recovery receipt, pushes the closed AUR drafts without force, reverifies their public commits and recipe hashes, and retains a distribution receipt.
6. **Recorded** — release URLs, hashes, workflow run, AUR versions, and resulting status-document changes are retained as release evidence.

A failure never advances the state. Retrying candidate construction creates a new workflow run. A changed commit or version is a different candidate and requires a new approval.

## Candidate contract

`.github/workflows/release-candidate.yml` is deliberately non-publishing:

- manual `workflow_dispatch` only from `main` or `maint/0.1`;
- candidate construction queries the Actions API with `actions: read` and proves
  that the exact candidate commit has a completed successful `CI / check` push
  run on the same authority branch; pending, failed, cancelled, skipped,
  pull-request-only, stale-SHA, other-branch, and missing runs are rejected;
- repository permission limited to `actions: read` and `contents: read`;
- no GitHub Environment, release token, AUR credential, tag creation, push, or release API call;
- full Git history is fetched so the exact commit and previous version tag can be recorded;
- the requested version must exactly match Cargo and all three Arch recipes;
- repository-owned Foot oracle metadata must match the current `Cargo.lock` before
  candidate construction;
- website sources and deployment automation are absent from the source archive;
- package validation runs against extracted package contents;
- all output is uploaded as one private, retention-bounded workflow artifact.

The candidate manifest binds the repository, commit, version, tag, architecture, prior version tag, candidate workflow identity, exact CI run ID/URL/branch/SHA/event and successful `check` job, and SHA-256 digest of every proposed release asset. A later publisher validates and preserves this CI provenance and must consume this manifest and these exact artifacts rather than rebuilding. Package construction may use `--no-check` only after the exact CI attestation succeeds.

## Deterministic preflight and provenance refresh

Run the same cheap release checks locally and in CI before Rust compilation:

```bash
python tools/release/release-doctor.py
python tools/release/release-doctor.py --version 0.2.0-alpha1
```

The optional version check additionally refuses an existing candidate tag and
validates the predecessor/release-note commit range. On an Arch host with
`makepkg`, the doctor also compares each checked-in AUR `.SRCINFO` with
`makepkg --printsrcinfo` output.

A `Cargo.lock` change does not authorize reference regeneration. Refresh only
the repository-owned lock identities with an inspect-then-write sequence:

```bash
python tools/foot-oracle/update-provenance.py
python tools/foot-oracle/update-provenance.py --write
python tools/foot-oracle/check-provenance.py --portable
```

The default update command does not write. `--write` replaces the provenance
manifest atomically and updates every duplicate lock identity while preserving
the pinned Foot commit and reference policy.

## CI boundaries and package-check equivalence

`CI / check` remains the required branch-protection context. It runs under
`always()` and fails unless every mandatory dependency result is exactly
`success`, so a failed, cancelled, or skipped boundary cannot turn the required
context green.

| Boundary | Exact responsibility |
| --- | --- |
| `preflight` | `release-doctor.py` before compilation |
| `static` | `cargo fmt`, ShellCheck, and frozen workspace clippy |
| `workspace-tests` | workspace lib/bin/example/doc tests plus every Rust integration target except the separately isolated daemon and MCP targets |
| `daemon-tests` | `splinterd` `end_to_end`, serialized, no retry-to-green |
| `mcp-tests` | `schema_inventory` and `stdio_protocol`, serialized, no retry-to-green |
| `package-automation` | all four `packaging/PKGBUILD::check()` Python suites plus package/release workflow and helper tests |
| `oracle-fixtures` | portable Foot provenance, fixture schemas, public contract fixtures, Foot Python tests, and benchmark contracts |

Together, the split Rust commands are equivalent to
`cargo test --frozen --workspace -- --test-threads=1`: workspace unit, binary,
example, and documentation tests run once, and every checked-in integration
target is named exactly once. The package automation boundary runs each of the
four Python commands from `packaging/PKGBUILD::check()` exactly once. The policy
test in `tools/release/test_ci_workflow.py` keeps this matrix synchronized with
current integration targets and the package recipe.

The preflight also runs actionlint v1.7.7 from a pinned
`rhysd/actionlint` image digest, so executable workflow syntax is checked
without installing a tool on the runner. Policy-oriented Python tests remain in
place for repository-specific authority, immutability, artifact, and credential
contracts; API and partial-state interpretation lives in tested Python helpers
rather than shell response matching.

Failed workspace, daemon, MCP, package/release, and oracle boundaries retain the
focused captured output as 14-day artifacts. Automatic retries are not used.
`.github/workflows/flake-stress.yml` is a non-publishing weekly/manual diagnostic
that runs 1–25 serialized daemon/MCP repetitions and stops on the first failure;
a failure remains a failed run rather than being retried to green. Manual use is
for maintainers investigating flakes and does not grant candidate or publication
authority.

## Human approval boundary

The publishing job uses `environment: release`. Configure at least one required reviewer and exactly two custom deployment-branch policies named `main` and `maint/0.1` in the GitHub repository settings before running it. The protected job independently queries the Environment and fails closed unless both controls are present; merely referencing an automatically created unprotected Environment cannot publish. The read-only verification job has only `actions: read` and `contents: read`; it accepts a candidate run ID and candidate-manifest SHA-256, proves that the source was one successful manual candidate run from the same authority branch as the promotion dispatch, paginates the run artifact inventory, requires exactly one unexpired artifact, binds its workflow commit to the candidate manifest commit, and closes over the exact file set and hashes before the protected job becomes approvable. The write-authorized job executes release tooling only from the reviewed promotion-workflow dispatch commit, never from candidate-controlled source, and checkout credentials are not persisted.

Approval authorizes only the exact candidate identified by authority branch, commit, version, manifest digest, and workflow run. After approval, publication refuses an existing tag or release and never clobbers, force-updates, or deletes remote state. If a later step fails after tag or release creation, the workflow stops and reports the partial published state for maintainer diagnosis. It does not retry by replacing that state.

The protected `release` environment supplies `SPLINTERM_RELEASE_TOKEN`, a
fine-grained token scoped only to this repository with Contents and Workflows
read/write permission. The read-only verifier cannot access it. The protected
publisher needs Workflows permission because the versioned tag may point to a
candidate that changes files under `.github/workflows/`; GitHub rejects that
ref creation when only the job's ordinary `GITHUB_TOKEN` is used.

The publisher uploads only the source archive, main/MCP packages, candidate
manifest, and checksums. Its promotion record also closes over the exact release
title, required prerelease state, release-notes SHA-256, and public asset hashes.
The exact verified candidate bytes and that promotion record are retained for
14 days; this bounded retention window is the maximum recovery window. The
publisher downloads the public assets again, verifies the tag resolves to the
candidate commit, requires exact metadata and an exact asset set, and retains a
90-day receipt. AUR drafts remain private inputs for the separately gated
distribution workflow.

## Immutable publication recovery

`.github/workflows/recover-release.yml` is a separate manual workflow using the
same protected `release` environment and promotion concurrency group. It accepts
only an original promotion run with a `failure`, `cancelled`, or `timed_out`
conclusion plus the exact SHA-256 of that run's `promotion.json`; successful and
all other conclusions are refused. Recovery succeeds only while that run's 14-day
`verified-release-candidate-*` artifact is still unexpired. It validates the
artifact ID and exact run-bound name before download, checks the promotion-record digest, reverifies the
candidate manifest and every candidate byte before and after approval, and then
inspects live GitHub state through tested Python helpers.

Recovery refuses an altered tag target, title, draft/prerelease state,
release-notes body, candidate identity, existing asset hash, duplicate asset, or
extra asset. Recovery also refuses a missing or altered tag and a completely
absent publication (use normal promotion). The helper returns only a bounded
list of absent operations: create the missing release behind the exact existing
tag and/or upload specifically named missing assets. When the exact tag, release,
and asset hashes are already complete, that list is empty and recovery performs
no remote mutation; it only reverifies public state and retains the missing
receipt. The workflow never deletes, replaces, clobbers, force-updates, or
rebuilds. A completed recovery performs the same full public verification and
emits the same 90-day publication receipt contract as normal promotion, bound
back to the original promotion run and the once-lowercased promotion-record
digest.

## Protected AUR distribution

Configure an `aur-release` GitHub Environment separately from `release`. It must
have at least one required reviewer and exactly two custom deployment branch
policies, `main` and `maint/0.1`; the protected job queries live settings and
fails closed on any difference. The environment supplies:

- `SPLINTERM_AUR_POLICY_TOKEN`, a repository-scoped token able to read live
  Environment protection and deployment-branch policy settings; and
- `SPLINTERM_AUR_SSH_PRIVATE_KEY`, an AUR deploy key authorized only for the
  `splinterm` and `splinterm-bin` package bases.

Do not place either credential at repository or verifier-job scope. The SSH key
is exposed only to the two protected push steps, written to a temporary file,
and removed by a step-local trap. SSH is locked to the checked-in
`aur.archlinux.org` Ed25519 host identity with strict host-key checking. Tooling
is checked out from `${{ github.sha }}` with persisted checkout credentials
disabled.

A distribution dispatch provides candidate run ID plus manifest SHA-256 and one
successful promotion/recovery receipt run ID plus receipt SHA-256. Before any
AUR credential is available, the verifier redownloads the candidate and receipt,
reverifies the public tag, exact release metadata, asset set, and downloaded
hashes, and binds all identities. After approval, the protected job first
validates the live `aur-release` Environment policy, then immediately repeats
that exact public tag, release, asset, and hash verification before cloning and
inspecting current AUR state. It permits an already-exact recipe as an idempotent no-op,
refuses a different recipe at the same version, and fails closed unless the candidate's
canonical SemVer release is newer than the current AUR release. Only then does it commit
the candidate's three closed draft files and push normally to `master`. It never rebuilds or
uses a forced push. Fresh anonymous clones must expose the exact versions,
commits, and file hashes before a 90-day `aur-distribution-receipt-*` artifact is
created. Environment and credential configuration, dispatch, and first live
publication remain separate approval-gated operations.

## n8n boundary

A future n8n workflow may:

- receive GitHub `workflow_run`, deployment, release, and failure webhooks;
- send a concise candidate summary and direct GitHub approval link;
- remind on a pending approval;
- verify that GitHub and AUR expose the expected versions and hashes; and
- report a bounded recovery action when a stage fails.

It must not hold a GitHub release token, AUR SSH key, or an approval-bypass secret. Webhook payloads must be signature-verified, deduplicated by delivery ID, and treated as notifications rather than release commands.
