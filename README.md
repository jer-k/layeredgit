# Git Viewer

A local Git branch review app built with [Rune](https://github.com/anomalyco/rune).
The diff compares the merge base, HEAD, and working tree.

## Install on macOS

Run this command from the checkout with rustup, just, Python 3, and Git 2.42 or
newer installed.

```sh
just install
```

This builds the release app, installs `~/Applications/Git Viewer.app`, and puts
`git-review` in `~/.local/bin`. Add `~/.local/bin` to your shell's `PATH`, then run
`git review` anywhere inside a working tree. To compare against a specific ref,
run `git review BASE`. The default tries `origin/HEAD`, then `main`, then `master`.
Run `git review --help` for the manual or `git review -h` for a short usage line.

Open **Git Viewer** from Finder, Spotlight, or the Dock for the repository picker
and recent working trees. Press **⌘O** in a review to return to the picker. Each
working tree gets its own window. Running `git review` again for that tree focuses
the same window and applies a base when you supply one. Closing the last window
leaves the app in the Dock so you can reopen the picker. Use **⌘Q** to quit.

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
the start window, or `cargo run -- --demo` for the built-in example. Pass
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
