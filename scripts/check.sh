#!/usr/bin/env bash
# Runs every automated check of Yuna's definition of done (AGENTS.md), in one command:
#
#   nix develop -c scripts/check.sh
#
# CI runs it unchanged, in the same shell (.github/workflows/ci.yml); a change of Markdown files
# alone runs `scripts/check.sh docs` instead (.github/workflows/docs.yml). The shell (flake.nix) gives the
# pinned toolchain and every tool used here. `cargo deny` needs the network to read the advisories;
# every other step works offline once the dependencies are fetched.
#
# Each check is a step with a name; `scripts/check.sh only <step>` runs one, which
# scripts/test-check.sh uses to prove that each stops what it guards against.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n==> %s\n' "$*"; }

# `rust-version` in Cargo.toml names the toolchain of rust-toolchain.toml (ADR-0016); Cargo cannot
# read one from the other, so this compares them. The workspace's lints (the `// SAFETY:` comment,
# documentation) reach only the crates that take them; a crate without these lines would escape
# them silently.
check_rules() {
    step "The workspace's own rules"
    local channel rust_version manifest
    channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
    rust_version=$(sed -n 's/^rust-version = "\(.*\)"$/\1/p' Cargo.toml)
    if [ -z "$channel" ] || [ "$channel" != "$rust_version" ]; then
        echo "rust-toolchain.toml names \"$channel\" but Cargo.toml's rust-version is \"$rust_version\""
        return 1
    fi
    for manifest in crates/*/Cargo.toml; do
        if ! grep -Pzq '\n\[lints\]\nworkspace = true\n' "$manifest"; then
            echo "$manifest lacks the lines \"[lints]\" and \"workspace = true\""
            return 1
        fi
    done
}

check_format() {
    step "Formatting"
    cargo fmt --all --check
}

check_lints() {
    step "Lints, warnings as errors"
    cargo clippy --workspace --all-targets --locked -- -D warnings
}

check_tests() {
    step "Build and tests"
    cargo test --workspace --locked
}

check_rustdoc() {
    step "Documentation, warnings as errors"
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
}

check_dependencies() {
    step "Dependencies: licences compatible with GPL-3.0-or-later, advisories, sources (deny.toml)"
    cargo deny --locked check
}

check_links() {
    step "Links between the Markdown files"
    git ls-files -z '*.md' | xargs -0 lychee --offline --include-fragments --no-progress
}

# Every agent reads these files at each session's start, so their length is paid every time. The
# limits are the owner's project method's: AGENTS.md under about 1,500 words, the PLAN about 2,000.
check_lengths() {
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

# The ADRs' index is written from the ADRs, and each changelog fragment has its form
# (docs/adr/README.md, changelog.d/README.md). The scripts and their tests run with python3, on
# its standard library alone, isolated from the environment (-I) and writing no bytecode into the
# tree (-B).
check_indexes() {
    step "ADRs' index and changelog fragments"
    python3 -I scripts/adr-index.py
    python3 -I scripts/changelog.py
}

# The documents' scripts' tests, and the test of SlimeEngine's job's path filter
# (scripts/test_workflows.py), which needs git alone.
check_script_tests() {
    step "Tests of the scripts: the documents' and the workflows' path filter"
    python3 -I -B -m unittest discover --start-directory scripts --pattern 'test_*.py'
}

DOCUMENT_STEPS=(links lengths indexes script-tests)
CODE_STEPS=(rules format lints tests rustdoc dependencies)

run_step() {
    case "$1" in
    rules) check_rules ;;
    format) check_format ;;
    lints) check_lints ;;
    tests) check_tests ;;
    rustdoc) check_rustdoc ;;
    dependencies) check_dependencies ;;
    links) check_links ;;
    lengths) check_lengths ;;
    indexes) check_indexes ;;
    script-tests) check_script_tests ;;
    *)
        echo "no step $1: ${CODE_STEPS[*]} ${DOCUMENT_STEPS[*]}" >&2
        return 2
        ;;
    esac
}

require_shell() {
    # Outside the shell, a wrong toolchain or a missing tool would fail late, or pass for the
    # wrong reason.
    : "${YUNA_DEV_SHELL:?run this script inside the development shell: nix develop -c scripts/check.sh}"
}

case "${1:-}" in
# The documents' checks alone, for a change of Markdown files only: what the Docs workflow runs
# (.github/workflows/docs.yml), with lychee and git from Nix.
docs)
    for name in "${DOCUMENT_STEPS[@]}"; do run_step "$name"; done
    step "The documents' checks passed"
    ;;
only)
    [ $# -eq 2 ] || { echo "usage: scripts/check.sh only <step>" >&2; exit 2; }
    case " ${DOCUMENT_STEPS[*]} " in
    *" $2 "*) ;;
    *) require_shell ;;
    esac
    run_step "$2"
    ;;
"")
    require_shell
    for name in "${CODE_STEPS[@]}" "${DOCUMENT_STEPS[@]}"; do run_step "$name"; done
    step "All checks passed"
    ;;
*)
    echo "usage: scripts/check.sh [docs | only <step>]" >&2
    exit 2
    ;;
esac
