# Git Viewer

A local Git branch review app built with [Rune](https://github.com/anomalyco/rune).
The diff compares a chosen head to its merge base and layers in any changes
from the worktree where that head is checked out.

## Install on macOS

Run this command from the checkout with rustup, just, Python 3, and Git 2.42 or
newer installed.

```sh
just install
```

This builds the release app, installs `~/Applications/Git Viewer.app`, and puts
`git-review` in `~/.local/bin`. Add `~/.local/bin` to your shell's `PATH`, then run
`git review` anywhere inside a working tree to add or select it in Git Viewer.
To override its base, run `git review BASE`. The default tries `origin/HEAD`,
then `main`, then `master`.
Run `git review --help` for the manual or `git review -h` for a short usage line.

Open **Git Viewer** from Finder, Spotlight, or the Dock to restore your
repositories in one window. Use **+** or **⌘O** to add a working tree, **⌘1–9**
to jump to a repository, and **⌘[** / **⌘]** to cycle. Right-click a repository
to remove it from the sidebar without changing anything on disk. Collapse the
sidebar to make more room for the diff. Closing the window leaves the app in
the Dock; use **⌘Q** to quit.

Use the **Base** and **HEAD** pickers to search branches or enter a tag, commit
ID, or revision expression. Git Viewer remembers each repository’s base. HEAD
defaults to the checkout on launch; **Reset to current** restores it at any
time. If you choose a branch checked out in another linked worktree, its
uncommitted changes appear in the review. A branch without a checkout, a
remote-tracking ref, a tag, or a commit ID shows committed changes only.

After pulling changes, run `just install` again. Rune's installer swaps the bundle
in place and relaunches a running app with its original arguments. Remove the
bundle and launcher with `just uninstall`. For another location, set the same
`APP_DIR` and `BIN_DIR` on both commands.

```sh
APP_DIR="$PWD/target/install-test/Applications" BIN_DIR="$PWD/target/install-test/bin" just install
APP_DIR="$PWD/target/install-test/Applications" BIN_DIR="$PWD/target/install-test/bin" just uninstall
```

## Development

Run `cargo run -- --repo .` to review the current working tree, `cargo run` for
the workspace, or `cargo run -- --demo` for the built-in example. Pass
`--base REF` to override the comparison base.

The review follows filesystem changes and checks Git status periodically. Press
`r` to refresh immediately. While the fresh-line highlight is active, press `p`
or `n` to jump to the nearest off-screen edit above or below.

To render the fixture headlessly, run:

```sh
scripts/make-fixture-repo.sh target/fixture-repo
cargo run -- --repo target/fixture-repo --snap target/fixture.png --chrome --size 1400x900 --scale 2
cargo run -- --repo target/fixture-repo --script scripts/ui/large-file-fold.json --chrome --out target/fold-frames
```

Add `--scene reduced-motion` to any headless capture to exercise Reduce Motion.
Use `--demo-pulse` to simulate a reload at 0.5 seconds on the fixture.

To exercise the repository sidebar and revision pickers, generate disposable
fixtures and pass repeated `--repo` flags in a debug headless run. The last path
is selected, even if it appeared earlier in the list:

```sh
sh scripts/make-sidebar-fixtures.sh
F=target/round4-fixtures
cargo run -- --repo "$F/checkout/review-sample" --repo "$F/linked/review-sample" \
  --repo "$F/docs-lab" --repo "$F/checkout/review-sample" \
  --script scripts/ui/revision-pickers.json --chrome --size 1400x900 \
  --video target/revision-pickers.mp4 --keys --pointer
```

Use `scripts/ui/repository-workspace.json` with the same arguments to check
repository switching and collapse, or run `scripts/ui/repository-empty.json`
without any `--repo` flags to exercise first launch and the folder-dialog flow.

### Tune the diff colours

Run `cargo run --features dev -- --repo target/fixture-repo` and press **⌘⇧K**.
The panel opens the provenance controls for the current system appearance. To
preview the other palette, switch your system appearance or launch with
`--theme light` or `--theme dark`. The edge, gutter dot, and pulse colours use
OKLCH. Edit a working-tree line to see the pulse with the current settings.

Use **Copy** in the panel header to copy the selected group's values. In
[`src/palette.rs`](src/palette.rs), match each copied label to its row in
`palette! { … }`. Replace the penultimate number for Light or the final number
for Dark. Builds without `dev` use those defaults directly.

Rune is pinned to a Git revision in `Cargo.toml`. Update the dependency and its matching patches together when changing that pin.
