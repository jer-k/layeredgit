# Git Viewer — agent guide

A local, native app for reviewing a branch against its base the way a GitHub PR
reads, plus uncommitted working-tree changes layered on top of committed ones.

## Layout

Cargo workspace. The root package is the app binary; libraries live in `crates/`.

| Crate | Role | May depend on |
|---|---|---|
| `crates/diff-stack` | Pure: align a chain of text versions, label each line with the versions it lives in, group into hunks. | `imara-diff`, std. No git, no UI, no I/O. |
| `crates/repo` (planned) | Talks to git: merge-base, changed files, file contents at B / HEAD / index / worktree, untracked files. | std, process |
| `crates/highlight` (planned) | Syntax highlighting of whole file versions into styled spans. | `syntect` |
| root (`src/`) | Rune UI and glue. | everything above, `rune` |

Keep these boundaries. Pure crates stay pure so they can be tested without a repo or a window.

## Rules

- `just check` (fmt-check, clippy with `-D warnings` incl. `clippy::pedantic`, tests) must pass before you report done. A pre-commit hook runs it too.
- Lints live in `[workspace.lints]` in the root `Cargo.toml`. Do not weaken them; if one must be allowed, use a scoped `#[allow(..., reason = "...")]` or `#[expect(...)]` at the smallest item.
- No `unwrap`, `expect`, `panic!`, or indexing that can panic in library code unless the invariant is local and documented. Tests may unwrap.
- Every public item has a doc comment (`missing_docs` is on).
- New dependencies: add with `cargo add -p <crate>`, prefer well-maintained crates, and say why in your report.
- Do not change the Rune git pin or its `[patch.crates-io]` entries.
- Do not commit or push unless asked.
