# Oracle patches

Test-only patches for the pinned Foot revision will live here.

They may add semantic state-dump constructors or accessors, but must not alter
terminal behavior. The oracle workflow applies them only to a disposable
worktree or build copy, never directly to `~/Playground/foot`.

`0001-semantic-state-dump.patch` adds the smallest machine-readable state dump
needed by the initial fixtures. It also accepts an oracle-only logical grid size
so compositor tiling cannot change wrapping behavior during comparison.
