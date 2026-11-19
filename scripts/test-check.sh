#!/usr/bin/env bash
# Proves that each check of scripts/check.sh stops what it guards against
# ([ADR-0058](../docs/adr/0058-what-yuna-takes-from-the-owners-other-projects.md)):
#
#   nix develop -c scripts/test-check.sh
#
# It clones the last commit into a scratch folder, breaks the clone in one way at a time, runs the
# step that guards against it (`scripts/check.sh only <step>`) and expects that step to fail; the
# clone is put back after each. A step that passes on a broken clone is a check that stops
# nothing: the script then fails, naming it. It builds the clone in target/test-check, so that
# its cases share their builds; the first case of the tests takes minutes, as check.sh does.
# Run it after changing check.sh or a check, and before a release.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
: "${YUNA_DEV_SHELL:?run this script inside the development shell: nix develop -c scripts/test-check.sh}"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
scratch="$work/yuna"
git clone --quiet --no-hardlinks "$root" "$scratch"
export CARGO_TARGET_DIR="$root/target/test-check"

failures=()

# Every step check.sh knows is in one of its lists: a step left out of them would no longer run
# with the whole check, in CI, though `only` still runs it.
known=$(sed -n 's/^    \([a-z-]*\)) check_.*/\1/p' "$root/scripts/check.sh" | sort)
listed=$(sed -n 's/^[A-Z]*_STEPS=(\(.*\))$/\1/p' "$root/scripts/check.sh" | tr ' ' '\n' | sort)
if [ -z "$known" ] || [ "$known" != "$listed" ]; then
    echo "NOT RUN: check.sh's steps and the lists its whole run takes differ"
    failures+=("a step outside check.sh's lists")
fi

# expect_stopped <step> <what is broken> <a pattern its failure prints> <command that breaks the
# clone>: the step must fail, and for this reason, its output holding the pattern (grep -E), so
# that a step failing for another, a network down for instance, is not taken for a stop.
expect_stopped() {
    local step=$1 what=$2 reason=$3 breaking=$4
    if ! (cd "$scratch" && eval "$breaking"); then
        echo "NOT BROKEN: the command that breaks the clone for $what failed"
        failures+=("$what (not broken)")
    elif (cd "$scratch" && scripts/check.sh only "$step") >"$work/log" 2>&1; then
        echo "NOT STOPPED: $what passes the step $step"
        failures+=("$what ($step)")
    elif ! grep -Eq -- "$reason" "$work/log"; then
        echo "STOPPED FOR ANOTHER REASON: $what, by $step; its output lacks /$reason/:"
        tail -n 20 "$work/log"
        failures+=("$what ($step, another reason)")
    else
        echo "stopped by $step: $what"
    fi
    git -C "$scratch" checkout --quiet -- .
    git -C "$scratch" clean -fdq
}

expect_stopped rules "a rust-version other than the toolchain's" "rust-version is" \
    "sed -i 's/^rust-version = .*/rust-version = \"1.0.0\"/' Cargo.toml"
expect_stopped rules "a crate without the workspace's lints" "lacks the lines" \
    "sed -i '/^\[lints\]/,+1d' crates/yuna-motion/Cargo.toml"
expect_stopped format "code not formatted" "Diff in" \
    "printf 'fn  badly_formatted( ){}\n' >> crates/yuna-layout/src/lib.rs"
expect_stopped lints "a warning" "unused variable" \
    "printf '\n/// Unused.\npub fn warns() { let unused = 1; }\n' >> crates/yuna-layout/src/lib.rs"
expect_stopped lints "an unsafe block without its SAFETY comment" "undocumented_unsafe_blocks|SAFETY" \
    "printf '\n/// Unsafe.\npub fn reads() -> u8 { unsafe { *(&1u8 as *const u8) } }\n' >> crates/yuna/src/lib.rs"
expect_stopped tests "a test that fails" "panicked.*broken|broken.*FAILED|fails \\.\\.\\. FAILED" \
    "printf '\n#[cfg(test)]\nmod broken { #[test] fn fails() { panic!(\"broken\") } }\n' >> crates/yuna/src/lib.rs"
expect_stopped tests "the help changed without its contract's file (ADR-0079)" "differs from the contract" \
    "sed -i 's/Yuna, a Wayland compositor for Linux\./Yuna, a compositor./' crates/yuna/src/main.rs"
expect_stopped tests "a protocol offered that docs/protocols.md does not say (ADR-0058)" "docs/protocols.md" \
    "sed -i 's/^| viewporter | \`wp_viewporter\` | Offered, every client |/| viewporter | — | Not decided |/' docs/protocols.md"
expect_stopped rustdoc "a broken link in the code's documentation" "Nowhere" \
    "printf '\n/// See [Nowhere].\npub fn documented() {}\n' >> crates/yuna-layout/src/lib.rs"
expect_stopped dependencies "a crate under a licence GPL-3.0-or-later cannot take" "BUSL" \
    "sed -i 's/^license.workspace = true/license = \"BUSL-1.1\"/' crates/yuna-layout/Cargo.toml"
expect_stopped links "a broken link between Markdown files" "nowhere.md" \
    "printf '\n[Nowhere](docs/nowhere.md)\n' >> README.md"
expect_stopped lengths "AGENTS.md past its length" "too long" \
    "printf 'word %.0s' \$(seq 2000) >> AGENTS.md"
expect_stopped indexes "an ADR missing from the index" "index is not the one the ADRs give" \
    "printf '# 9999. A decision\n\n- Status: Accepted\n- Date: 2026-11-17\n' > docs/adr/9999-a-decision.md"
expect_stopped indexes "a changelog fragment badly named" "something.md" \
    "printf -- '- Something.\n' > changelog.d/something.md"
expect_stopped script-tests "SlimeEngine's job leaving out the drawing path" "files the job needs" \
    "sed -i 's|      - \"docs/\*\*\"|      - \"docs/**\"\n      - \"crates/yuna/src/render.rs\"|' .github/workflows/slimeengine.yml"
expect_stopped script-tests "Opium's job leaving out the drawing path" "files the job needs" \
    "sed -i 's|      - \"docs/\*\*\"|      - \"docs/**\"\n      - \"crates/yuna/src/render.rs\"|' .github/workflows/opium.yml"

if [ ${#failures[@]} -gt 0 ]; then
    printf '\nChecks that stopped nothing:\n'
    printf -- '- %s\n' "${failures[@]}"
    exit 1
fi
printf '\nEvery check stopped what it guards against.\n'
