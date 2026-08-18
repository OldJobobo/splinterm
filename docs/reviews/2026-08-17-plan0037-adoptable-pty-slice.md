# Plan 0037 adoptable PTY slice review

- **Base:** `4cc927a796f3dd0651772d72f199a7cd73c0ae9b`
- **Implementation:** `886c3de4ec661c4207f8345e6873b678b4b6e496`
- **Timing correction:** `26b334cb913c297b17767ec32a772795b24836fe`
- **Evidence correction:** `69b9063`
- **Decision:** Approved for this partial Milestone 1 slice

## Reviewed boundary

The review covered the `splinterm-pty` transition from
`std::process::Child` to validated PID/PGID/SID plus direct `waitpid`, the
recoverable single-master adoption API, the bounded PTY-helper readiness
acknowledgement, the sealed forward/rollback descriptor-exec spike, and the
associated Plan 0037 evidence.

The reviewer confirmed:

- direct `waitpid` reaping preserves repeated `wait` and `try_wait` behavior;
- PID, process group, session, controlling terminal, and direct-child wait
  authority are validated;
- the helper cannot exec the target before parent identity validation;
- fast target-exec failures retain their established error precedence;
- failed pre-exec conversion returns the unchanged old session;
- the spike proves descriptor execution, source replacement/deletion immunity,
  required seal failures, descriptor cleanup, ordered PTY output, resize,
  process-group signaling, and reaping; and
- the evidence does not claim completion of Milestone 1.

## Finding and correction

The initial review found one moderate evidence defect: the first timing version
started its no-reader clock inside the exec helper after reader teardown and
snapshot preparation had already begun. The implementation itself had no
identified correctness or security blocker.

The correction moved complete forward/rollback snapshot copy, rehash, sealing,
and mutation checks before quiescence. The clock now starts immediately after
the old reader is dropped, crosses descriptor exec unchanged, and stops only
after the adopting generation creates its replacement reader. Fifty repeated
runs then recorded:

- forward: 1,089,377–1,829,271 ns; 1,312,181 ns mean;
- rollback: 1,330,664–2,461,159 ns; 1,676,193 ns mean;
- combined: 1,089,377–2,461,159 ns; 1,494,187 ns mean.

The same reviewer verified commits `26b334c` and `69b9063`, confirmed the
machine-readable and prose evidence agree, and approved with no blocker.

## Validation

The final reviewed tree passed:

```text
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features -- --test-threads=1
cargo fmt --all --check
git diff --check origin/main...HEAD
(cd docs/plans/artifacts/0037-milestone1-adoptable-pty && sha256sum -c SHA256SUMS)
```

## Residual scope

Milestone 1 remains open. Production actor quiescence, enforced reader teardown,
descriptor allowlisting, recoverable post-exec adoption, complete daemon/client
snapshot pairs, adjacent-helper compatibility, mutation-race and writable-map
proofs, rollback compatibility rules, package identity, and broader output/load
budgets remain explicitly deferred to later reviewed slices.
