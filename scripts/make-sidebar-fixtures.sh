#!/bin/sh
set -eu

root=${1:-target/round4-fixtures}
mkdir "$root"
root=$(cd "$root" && pwd -P)
primary="$root/checkout/review-sample"
linked="$root/linked/review-sample"
docs="$root/docs-lab"

for var in $(env | sed -n 's/^\(GIT_[A-Za-z_]*\)=.*/\1/p'); do unset "$var"; done
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.com
export GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.com

"$(dirname "$0")/make-fixture-repo.sh" "$primary"
git clone -q --bare "$primary" "$root/remote.git"
git -C "$primary" remote add origin "$root/remote.git"
git -C "$primary" fetch -q origin
git -C "$primary" remote set-head origin main
git -C "$primary" branch --set-upstream-to=origin/feature feature >/dev/null

mkdir -p "$root/linked"
git -C "$primary" worktree add -q -b linked-review "$linked" feature
printf '\n// Retry instrumentation (committed in the linked worktree).\n' >> "$linked/src/lib.rs"
git -C "$linked" commit -qam 'Instrument retries'
git -C "$linked" branch parked-review
git -C "$linked" tag -a review-demo -m 'Review demo'
printf '\n// Local timing probe (not committed).\n' >> "$linked/src/lib.rs"
printf 'This file exists only in the linked worktree.\n' > "$linked/linked-notes.md"

git clone -q --branch main "$root/remote.git" "$docs"
git -C "$docs" remote set-head origin main
git -C "$docs" switch -qc docs-update
printf '# Documentation update\n\nThe HTTP client retries transient errors.\n' > "$docs/guide.md"
git -C "$docs" add guide.md
git -C "$docs" commit -qm 'Document retry behavior'
