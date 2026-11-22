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
    # The tests, the examples and WLCS's shim drive Yuna through its interface, which names no
    # type of Smithay's (ADR-0146, ADR-0149): one that names Smithay would tie them to what
    # Stage 2a replaces.
    local named
    if named=$(grep -rn --include='*.rs' 'smithay::' crates/yuna/tests crates/yuna/examples \
        crates/yuna-wlcs); then
        echo "the tests, the examples or WLCS's shim name Smithay:"
        echo "$named"
        return 1
    fi
}

check_format() {
    step "Formatting"
    cargo fmt --all --check
}

# Twice: the second with the parity script's canaries, code that only their feature compiles
# (ADR-0142's controls), so that it is linted too.
check_lints() {
    step "Lints, warnings as errors"
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo clippy --workspace --all-targets --locked \
        --features yuna/parity-canary,yuna-layout/parity-canary,yuna-motion/parity-canary \
        -- -D warnings
}

# The crate map (ADR-0151): scripts/crate-map.py compares crates/map.toml with the code. It guards
# the crates' levels and the edges between them, the outside libraries' owners and lowest levels,
# each module's `unsafe` code, as rustc counts it, against its entry, every global of the map's
# crates, and the files of `yuna` that name Smithay; its header says what it cannot see. It runs
# clippy once more, its own flag (`--force-warn unsafe_code`) reaching the workspace's crates
# alone, so that the dependencies' builds are reused.
check_map() {
    step "The crate map: levels, libraries, unsafe code, globals (crates/map.toml)"
    python3 -I -B scripts/crate-map.py
}

check_tests() {
    step "Build and tests"
    cargo test --workspace --locked
}

# WLCS's tests that crates/yuna-wlcs/expected.txt expects to pass, against the module built here
# (ADR-0149): one that no longer passes, or a crash, a hang or a fault of the module in any of
# them, stops the check. CI's `wlcs` job runs every test, a new pass included (ci.yml).
# The module must be linked NODELETE (crates/yuna-wlcs/build.rs): unmapped as WLCS exits, it
# crashed the process under a thread it had started.
check_wlcs() {
    step "WLCS: the tests expected to pass"
    cargo build --locked -p yuna-wlcs
    local module="${CARGO_TARGET_DIR:-target}/debug/libyuna_wlcs.so"
    if ! readelf --dynamic "$module" | grep -q 'FLAGS_1.*NODELETE'; then
        echo "WLCS's module is not linked NODELETE: $module"
        return 1
    fi
    cargo run --locked -p yuna-bench -- wlcs check --listed
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

# The documents' scripts' tests, and the test of the path filters of SlimeEngine's and Opium's jobs
# (scripts/test_workflows.py), which needs git alone.
check_script_tests() {
    step "Tests of the scripts: the documents' and the workflows' path filter"
    python3 -I -B -m unittest discover --start-directory scripts --pattern 'test_*.py'
}

DOCUMENT_STEPS=(links lengths indexes script-tests)
CODE_STEPS=(rules format lints map tests wlcs rustdoc dependencies)

run_step() {
    case "$1" in
    rules) check_rules ;;
    format) check_format ;;
    lints) check_lints ;;
    map) check_map ;;
    tests) check_tests ;;
    wlcs) check_wlcs ;;
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
