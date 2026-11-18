#!/bin/sh
# Build a throwaway git repo that exercises every kind of change the viewer shows:
# committed vs uncommitted edits (all six line classes), a base branch that moved on
# after the branch point, a rename, an added / deleted / untracked / binary file, a
# trailing-newline-only change, a pure rename and a mode change, several languages,
# and one large file.
#
# Usage: scripts/make-fixture-repo.sh [dir]   (default: target/fixture-repo)
# Then:  cargo run -- --repo target/fixture-repo
set -eu

dir=${1:-target/fixture-repo}
rm -rf "$dir"
mkdir -p "$dir"
cd "$dir"

# Never act on a repo named by an inherited git environment (e.g. when run from a hook).
for var in $(env | sed -n 's/^\(GIT_[A-Za-z_]*\)=.*/\1/p'); do unset "$var"; done

# Isolate from the user's git config so the fixture is identical everywhere.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.com
export GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.com
export GIT_AUTHOR_DATE='2026-01-01T12:00:00Z' GIT_COMMITTER_DATE='2026-01-01T12:00:00Z'

git init -q -b main
mkdir -p src web docs assets

# ---------- base (main) ----------
cat > src/lib.rs <<'EOF'
//! Retry helpers.

use std::time::Duration;

/// How long to wait before trying a request again.
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Backoff {
        Backoff { base, max, attempt: 0 }
    }

    /// The wait before the next attempt.
    pub fn next(&mut self) -> Duration {
        let wait = self.base * 2u32.pow(self.attempt);
        self.attempt += 1;
        wait.min(self.max)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}
EOF

cat > web/client.ts <<'EOF'
export interface Options {
  baseUrl: string;
  timeoutMs: number;
}

export async function get(opts: Options, path: string): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), opts.timeoutMs);
  try {
    return await fetch(opts.baseUrl + path, { signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}
EOF

cat > Cargo.toml <<'EOF'
[package]
name = "fixture"
version = "0.1.0"
edition = "2024"

[dependencies]
serde = "1"
EOF

cat > docs/guide.md <<'EOF'
# Guide

Install the tool, then run it inside a repository.

## Options

- `--base` picks the branch to compare against.
- `--theme` picks light or dark.
EOF

cat > docs/old-name.md <<'EOF'
# Changelog

## 0.1.0

- First release.
- Retry with exponential backoff.
EOF

printf 'legacy settings\nmode = fast\n' > legacy.ini
printf 'no trailing newline yet\n' > notes.txt
printf 'Reference for the web client.\n' > web/old-reference.txt
printf '#!/bin/sh\nprintf "Ready\\n"\n' > web/setup.sh

# A large file with scattered edits later on.
i=1
while [ "$i" -le 5000 ]; do
  printf 'pub const VALUE_%04d: u32 = %d;\n' "$i" "$i"
  i=$((i + 1))
done > src/generated.rs

printf '\211PNG\r\n\032\n\000\000\000\rIHDR\000\000\000\001' > assets/logo.png

git add -A
git commit -q -m "Initial project"

# ---------- feature branch: committed changes ----------
git checkout -q -b feature

# lib.rs: committed edits (adds, deletes, and a line that will be removed again later)
cat > src/lib.rs <<'EOF'
//! Retry helpers.

use std::time::Duration;

/// How long to wait before trying a request again, with jitter so clients spread out.
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
    jitter: f32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Backoff {
        Backoff { base, max, attempt: 0, jitter: 0.2 }
    }

    /// The wait before the next attempt, give or take `jitter` of it.
    pub fn next(&mut self, roll: f32) -> Duration {
        let wait = self.base.saturating_mul(2u32.saturating_pow(self.attempt));
        self.attempt += 1;
        let spread = 1.0 + self.jitter * (roll * 2.0 - 1.0);
        wait.mul_f32(spread).min(self.max)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}
EOF

# client.ts: committed only
cat > web/client.ts <<'EOF'
export interface Options {
  baseUrl: string;
  timeoutMs: number;
  retries: number;
}

export async function get(opts: Options, path: string): Promise<Response> {
  let lastError: unknown;
  for (let attempt = 0; attempt <= opts.retries; attempt++) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), opts.timeoutMs);
    try {
      return await fetch(opts.baseUrl + path, { signal: controller.signal });
    } catch (error) {
      lastError = error;
    } finally {
      clearTimeout(timer);
    }
  }
  throw lastError;
}
EOF

git mv docs/old-name.md docs/CHANGELOG.md
printf '\n## 0.2.0\n\n- Jitter in backoff.\n' >> docs/CHANGELOG.md
git mv web/old-reference.txt web/reference.txt
chmod +x web/setup.sh
git update-index --chmod=+x web/setup.sh
git rm -q legacy.ini
printf '\000\000\000\002IDAT\001' >> assets/logo.png
cat > src/config.rs <<'EOF'
/// Settings read from the environment.
pub struct Config {
    pub retries: u32,
}
EOF
# generated.rs: a few committed edits
sed -i.bak -e 's/VALUE_0100: u32 = 100;/VALUE_0100: u32 = 1000; \/\/ bumped/' \
           -e 's/VALUE_2500: u32 = 2500;/VALUE_2500: u32 = 25000; \/\/ bumped/' src/generated.rs
rm src/generated.rs.bak
git add -A
git commit -q -m "Add jitter, retries, and config"

# ---------- base moves on after the branch point (must NOT show up) ----------
git checkout -q main
printf '\n## Troubleshooting\n\nThis section landed on main after the branch point.\n' >> docs/guide.md
git commit -q -am "Docs on main after branching"
git checkout -q feature

# ---------- uncommitted changes on top ----------
# lib.rs: uncommitted add, uncommitted delete of a base line, and remove a committed line
cat > src/lib.rs <<'EOF'
//! Retry helpers.

use std::time::Duration;

/// How long to wait before trying a request again, with jitter so clients spread out.
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Backoff {
        Backoff { base, max, attempt: 0 }
    }

    /// The wait before the next attempt, give or take a fifth of it.
    pub fn next(&mut self, roll: f32) -> Duration {
        let wait = self.base.saturating_mul(2u32.saturating_pow(self.attempt));
        self.attempt += 1;
        let spread = 1.0 + 0.2 * (roll * 2.0 - 1.0);
        wait.mul_f32(spread).min(self.max)
    }

    /// Whether another attempt is allowed.
    pub fn can_retry(&self, limit: u32) -> bool {
        self.attempt < limit
    }
}
EOF

# Cargo.toml: uncommitted only
cat > Cargo.toml <<'EOF'
[package]
name = "fixture"
version = "0.2.0"
edition = "2024"

[dependencies]
serde = { version = "1", features = ["derive"] }
rand = "0.9"
EOF

# notes.txt: trailing-newline-only change
printf 'no trailing newline yet' > notes.txt

# generated.rs: an uncommitted edit deep in the file
sed -i.bak -e 's/VALUE_4200: u32 = 4200;/VALUE_4200: u32 = 4242; \/\/ uncommitted/' src/generated.rs
rm src/generated.rs.bak

# a staged new file and an untracked file
cat > src/metrics.rs <<'EOF'
/// Counts retries per endpoint.
pub struct Metrics {
    pub retries: u64,
}
EOF
git add src/metrics.rs
printf 'scratch notes, not tracked yet\n' > TODO.md

printf 'Fixture repo ready at %s (base main, head feature).\n' "$(pwd)"
