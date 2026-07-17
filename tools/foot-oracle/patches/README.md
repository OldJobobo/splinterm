# Oracle patches

Test-only patches for the pinned Foot revision will live here.

They may add semantic state-dump constructors or accessors, but must not alter
terminal behavior. The oracle workflow applies them only to a disposable
worktree or build copy, never directly to `~/Playground/foot`.

No patch has been accepted yet. The first patch should introduce the smallest
machine-readable state dump needed to promote the initial fixtures from
`source_reviewed` to `oracle_verified`.
