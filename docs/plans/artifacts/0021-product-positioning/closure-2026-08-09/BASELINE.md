# Plan 0021 closure baseline

- Baseline HEAD: `a65a740` (`Close bounded Dojo tabs plan`)
- Worktree owner: parent single writer for Plan 0021 repository documentation
- Graphical testing: not required or authorized

Before Plan 0021 implementation began, the following uncommitted website paths
were already present in the worktree:

- `site/astro.config.mjs`
- `site/src/content/docs/docs/concepts.md`
- `site/src/content/docs/docs/configure/configuration.md`
- `site/src/content/docs/docs/index.md`
- `site/src/content/docs/docs/status.md`
- `site/src/pages/index.astro`
- `site/src/styles/site.css`
- untracked `site/src/content/docs/docs/wayland.md`

They add native-Wayland explanation and presentation material. During Plan 0021
review, the independent concurrent website writer also added or changed:

- `site/src/content/docs/docs/automation.md`
- `site/src/content/docs/docs/mcp.md`
- `site/src/content/docs/docs/install.md`
- further changes to the already-listed site index/status/configuration
  surfaces.

Plan 0021 did not write any `site/` source path and excludes the entire live site
diff from its implementation scope and future staged commit. The site validation
necessarily exercises the combined moving worktree and is reported as such; it
is not evidence that Plan 0021 owns those website edits.
