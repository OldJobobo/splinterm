# Font startup hotfix review

**Verdict: Approve.**

No blocker or fix worth doing now was found in the bounded hotfix.

## Verified behavior

- `resolve_primary_faces` resolves the configured regular pattern first and
  derives every primary style pattern from that effective family; no hardcoded
  JetBrains primary-style pattern remains.
- Failed, foreign-family, duplicate, wrong-weight/slant, unreadable, or
  metric-incompatible style candidates emit a font-specific warning and reuse
  the selected regular identity. An unusable regular face remains fatal.
- Accepted style candidates require the same normalized family, a distinct
  file/index identity, the requested relative weight/slant, and an M advance
  within 0.01 px of regular.
- The application default is `monospace:style=Regular`, while the shipped sample
  leaves `main.font` commented, preserving the distinction between default and
  explicit authority.
- Fontconfig family/style values are escaped and passed through `Command::args`,
  not a shell.
- Process-wide face caching remains immutable and documentation accurately
  limits the hotfix to newly launched clients. Live replacement remains Plan
  0038 work.
- Regression tests cover arbitrary escaped family patterns, absence of a
  JetBrains pattern, foreign/duplicate/wrong-style/metric rejection, regular
  identity fallback, and real effective `monospace` coherence.

## Fixes worth doing now

None.

## Optional later work

- Parse the shipped sample in a unit test to guard its default-versus-explicit
  distinction.
- Add variable-font axis support. Same-file/index named styles currently fall
  back safely to regular, which is within this hotfix's contract.

## Review authority

Fresh read-only review run `40347854`; reviewer did not edit files or perform
graphical testing. Parent-supplied evidence included full workspace format,
Clippy, and serialized tests; isolated Caskaydia and regular-only Fontconfig
fixtures; package integration tests; and `git diff --check`.
