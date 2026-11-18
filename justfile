# Run every gate: formatting, lints, tests.
default: check

# Format all code in place.
fmt:
    cargo fmt --all

# Fail if any file is not formatted.
fmt-check:
    cargo fmt --all -- --check

# Clippy across the workspace, warnings are errors.
lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy --workspace --all-targets --features dev -- -D warnings

# Run all tests.
test:
    cargo test --workspace

# The gate every change must pass.
check: fmt-check lint test

# Point git at the versioned hooks in .githooks.
install-hooks:
    git config core.hooksPath .githooks

# Build the release binary and install layeredgit.app and the layeredgit command.
install:
    python3 scripts/install-mac-app.py install

# Remove layeredgit and its legacy installation from the selected directories.
uninstall:
    python3 scripts/install-mac-app.py uninstall
