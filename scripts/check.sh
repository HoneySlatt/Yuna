#!/usr/bin/env bash
# Runs every automated check of Yuna's definition of done (AGENTS.md), in one command:
#
#   nix develop -c scripts/check.sh
#
# CI runs it unchanged, in the same shell (.github/workflows/ci.yml). The shell (flake.nix) gives the
# pinned toolchain and every tool used here. `cargo deny` needs the network to read the advisories;
# every other step works offline once the dependencies are fetched.
set -euo pipefail
cd "$(dirname "$0")/.."

# Outside the shell, a wrong toolchain or a missing tool would fail late, or pass for the wrong reason.
: "${YUNA_DEV_SHELL:?run this script inside the development shell: nix develop -c scripts/check.sh}"

step() { printf '\n==> %s\n' "$*"; }

step "Formatting"
cargo fmt --all --check

step "Lints, warnings as errors"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "Build and tests"
cargo test --workspace --locked

step "Documentation, warnings as errors"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

step "Dependencies: licences compatible with GPL-3.0-or-later, advisories, sources (deny.toml)"
cargo deny --locked check

step "Links between the Markdown files"
git ls-files -z '*.md' | xargs -0 lychee --offline --include-fragments --no-progress

# Every agent reads these files at each session's start: their length is paid every time
# (AGENTS.md, "Documentation").
step "Lengths of the files read at every session"
too_long=0
for file_and_limit in "AGENTS.md 1500" "docs/PLAN.md 2000"; do
    read -r file limit <<<"$file_and_limit"
    words=$(wc -w <"$file")
    echo "$file: $words words, limit $limit"
    if [ "$words" -gt "$limit" ]; then
        echo "$file is too long: move what is not a rule or a status to its home"
        too_long=1
    fi
done
[ "$too_long" -eq 0 ]

step "All checks passed"
