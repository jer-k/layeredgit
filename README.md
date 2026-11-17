# Git Viewer

A local Git branch review app built with [Rune](https://github.com/anomalyco/rune).
The diff compares the merge base, HEAD, and working tree.

With [rustup](https://rustup.rs/) and Git 2.42 or newer installed, run `cargo run` from a
working tree. Pass `--repo PATH` to review another repository or `--base REF` to
override the default base (origin/HEAD, main, or master). Pass `--demo` for the
built-in example.

The review refreshes when Git status changes. Press `r` to force a refresh, including
after further edits to a file already listed as modified or untracked.

To render the fixture headlessly, run:

```sh
scripts/make-fixture-repo.sh target/fixture-repo
cargo run -- --repo target/fixture-repo --snap target/fixture.png --size 1400x900 --scale 2
cargo run -- --repo target/fixture-repo --script scripts/ui/large-file-fold.json --out target/fold-frames
```

Rune is pinned to a Git revision in `Cargo.toml`. Update the dependency and its matching patches together when changing that pin.
