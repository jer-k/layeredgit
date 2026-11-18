# layeredgit — agent guide

A local, native app for reviewing a branch against its base the way a GitHub PR
reads, plus uncommitted working-tree changes layered on top of committed ones.

## Layout

Cargo workspace. The root package is the app binary; libraries live in `crates/`.

| Crate | Role | May depend on |
|---|---|---|
| `crates/diff-stack` | Pure: align a chain of text versions, label each line with the versions it lives in, group into hunks. | `imara-diff`, std. No git, no UI, no I/O. |
| `crates/repo` | Talks to git: merge-base, changed files, file contents at B / HEAD / index / worktree, untracked files. | std, process |
| root (`src/`) | Rune UI and glue. Syntax colours come from `rune_kit::syntax` (no separate highlight crate). | everything above, `rune`, `rune-kit` |

Keep these boundaries. Pure crates stay pure so they can be tested without a repo or a window.

## Rune

Rune is pre-release and pinned to one git rev in the root `Cargo.toml`; its source is in
`~/.cargo/git/checkouts/rune-*/10a3958/`. Read `crates/rune/README.md` and `crates/rune-kit/README.md`
there before UI work, and use the source as the API reference. `rune_kit::diff` (DiffView) is a
two-sided diff viewer; ours is three-way so we build our own view, but borrow its look, its
virtualised rows and its use of `rune_kit::syntax`.

Follow Rune's app rules: no file I/O or git calls on the UI thread (load off-thread, e.g.
`rune::Latest`), build only mounted rows (`virtual_rows`), and an idle window draws nothing.

### Checking UI work headlessly

Debug builds accept Rune's agent tooling flags, so check every UI change by looking at it:

```sh
cargo build
B=target/debug/layeredgit
$B --snap target/snaps/x.png --chrome [--theme dark|light] [--size 1400x900] [--scale 2]
$B --tree target/snaps/x.json        # element tree: key, role, label, text, rects, colours
$B --script scripts/ui/<name>.json --out target/snaps/<name>/   # scripted input -> frames
```

Scripts are timestamped JSON events (`tap`/`move`/`scroll`/`key`/`type`, at `x,y` or an element
`key`); see "Agent tooling" in Rune's README. Always pass `--chrome` to snapshots: it paints the macOS traffic lights onto the image and prints
`rune: chrome: ... overlaps the traffic lights` for any element under them. That warning is a bug
(content sits under a transparent titlebar), never something to ignore.

Give interactive elements stable `.key(..)`s so
scripts and `--tree` can target them. Keep generated images under `target/`.
Commit only the README screenshot at `docs/screenshot.png`, the app icon at
`assets/icon.svg`, `assets/icon.png`, and `assets/icon.icns`, and the SVG sources
under `assets/icon-candidates/`.

## Rules

- `just check` (fmt-check, clippy with `-D warnings` incl. `clippy::pedantic`, tests) must pass before you report done. A pre-commit hook runs it too.
- Lints live in `[workspace.lints]` in the root `Cargo.toml`. Do not weaken them; if one must be allowed, use a scoped `#[allow(..., reason = "...")]` or `#[expect(...)]` at the smallest item.
- No `unwrap`, `expect`, `panic!`, or indexing that can panic in library code unless the invariant is local and documented. Tests may unwrap.
- Every public item has a doc comment (`missing_docs` is on).
- New dependencies: add with `cargo add -p <crate>`, prefer well-maintained crates, and say why in your report.
- Do not change the Rune git pin or its `[patch.crates-io]` entries.
- Keep Rust files under ~800 non-test lines; split by capability.
- Do not commit or push unless asked.
