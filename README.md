# Git Viewer

A local Git branch review app built with [Rune](https://github.com/anomalyco/rune).
The diff compares the merge base, HEAD, and working tree.

With [rustup](https://rustup.rs/) and Git 2.42 or newer installed, run `cargo run` from a
working tree. Pass `--repo PATH` to review another repository or `--base REF` to
override the default base (origin/HEAD, main, or master). Pass `--demo` for the
built-in example.

The review follows filesystem changes and checks Git status periodically. Press
`r` to refresh immediately.

To render the fixture headlessly, run:

```sh
scripts/make-fixture-repo.sh target/fixture-repo
cargo run -- --repo target/fixture-repo --snap target/fixture.png --chrome --size 1400x900 --scale 2
cargo run -- --repo target/fixture-repo --script scripts/ui/large-file-fold.json --chrome --out target/fold-frames
```

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
