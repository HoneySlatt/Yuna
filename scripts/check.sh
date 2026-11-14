#!/usr/bin/env bash
# Runs every automated check of Yuna's definition of done (AGENTS.md), in one command:
#
#   nix develop -c scripts/check.sh
#
# CI runs it unchanged, in the same shell (.github/workflows/ci.yml); a change of Markdown files
# alone runs `scripts/check.sh docs` instead (.github/workflows/docs.yml). The shell (flake.nix) gives the
# pinned toolchain and every tool used here. `cargo deny` needs the network to read the advisories;
# every other step works offline once the dependencies are fetched.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n==> %s\n' "$*"; }

# The checks of the Markdown files alone: their links, and the length of those read at every
# session.
check_documents() {
    step "Links between the Markdown files"
    git ls-files -z '*.md' | xargs -0 lychee --offline --include-fragments --no-progress

    # Every agent reads these files at each session's start, so their length is paid every time.
    # The limits are the owner's project method's: AGENTS.md under about 1,500 words, the PLAN
    # about 2,000.
    step "Lengths of the files read at every session"
    local too_long=0 file_and_limit file limit words
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
}

# `scripts/check.sh docs` runs those alone, for a change of Markdown files only: what the Docs
# workflow runs (.github/workflows/docs.yml), with lychee and git from Nix.
if [ "${1:-}" = docs ]; then
    check_documents
    step "The documents' checks passed"
    exit 0
fi

# Outside the shell, a wrong toolchain or a missing tool would fail late, or pass for the wrong reason.
: "${YUNA_DEV_SHELL:?run this script inside the development shell: nix develop -c scripts/check.sh}"

step "The workspace's own rules"
# `rust-version` in Cargo.toml names the toolchain of rust-toolchain.toml (ADR-0016); Cargo cannot
# read one from the other, so this compares them.
channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
rust_version=$(sed -n 's/^rust-version = "\(.*\)"$/\1/p' Cargo.toml)
if [ -z "$channel" ] || [ "$channel" != "$rust_version" ]; then
    echo "rust-toolchain.toml names \"$channel\" but Cargo.toml's rust-version is \"$rust_version\""
    exit 1
fi
# The workspace's lints (the `// SAFETY:` comment, documentation) reach only the crates that take
# them; a crate without these lines would escape them silently.
for manifest in crates/*/Cargo.toml; do
    if ! grep -Pzq '\n\[lints\]\nworkspace = true\n' "$manifest"; then
        echo "$manifest lacks the lines \"[lints]\" and \"workspace = true\""
        exit 1
    fi
done

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

check_documents

step "All checks passed"
