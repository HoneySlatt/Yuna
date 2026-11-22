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
expect_stopped rules "a test naming Smithay" "name Smithay" \
    "printf 'use smithay::utils::Rectangle;\\n' >> crates/yuna/tests/headless/commands.rs"
expect_stopped rules "a test naming Smithay through an alias" "name Smithay" \
    "printf 'use smithay as sm;\\n' >> crates/yuna/tests/headless/commands.rs"
expect_stopped rules "WLCS's shim naming Smithay" "name Smithay" \
    "printf 'use smithay::utils::Rectangle;\\n' >> crates/yuna-wlcs/src/lib.rs"
expect_stopped wlcs "a WLCS test expected to pass that fails" "\\[pass\\] BadBufferTest" \
    "sed -i 's/^fail BadBufferTest.test_truncated_shm_file$/pass BadBufferTest.test_truncated_shm_file/' crates/yuna-wlcs/expected.txt"
expect_stopped wlcs "WLCS's outcomes recorded for another version" "\\[version\\]" \
    "sed -i 's/^wlcs 1.8.1$/wlcs 1.8.0/' crates/yuna-wlcs/expected.txt"
expect_stopped wlcs "WLCS's module not linked NODELETE" "not linked NODELETE" \
    "sed -i 's/-z,nodelete/-z,now/' crates/yuna-wlcs/build.rs"
expect_stopped format "code not formatted" "Diff in" \
    "printf 'fn  badly_formatted( ){}\n' >> crates/yuna-layout/src/lib.rs"
expect_stopped lints "a warning" "unused variable" \
    "printf '\n/// Unused.\npub fn warns() { let unused = 1; }\n' >> crates/yuna-layout/src/lib.rs"
expect_stopped lints "a warning in code only the parity canaries' feature compiles" "unused variable" \
    "printf '\n#[cfg(feature = \"parity-canary\")]\n/// Unused.\npub fn warns() { let unused = 1; }\n' >> crates/yuna-layout/src/lib.rs"
# The function expects `unsafe_code`, so that the lint this case proves is the one that stops it.
expect_stopped lints "an unsafe block without its SAFETY comment" "undocumented_unsafe_blocks|SAFETY" \
    "printf '\n/// Unsafe.\n#[expect(unsafe_code, reason = \"a case of test-check.sh\")]\npub fn reads() -> u8 { unsafe { *(&1u8 as *const u8) } }\n' >> crates/yuna/src/lib.rs"
expect_stopped lints "a module expecting unsafe code it does not hold (ADR-0151)" "lint expectation is unfulfilled" \
    "sed -i '1i #![expect(unsafe_code, reason = \"a case of test-check.sh\")]' crates/yuna/src/frames.rs"
expect_stopped lints "an expect of a lint without its reason" "allow_attributes_without_reason|without a reason" \
    "printf '\\n/// Nothing.\\n#[expect(dead_code)]\\nfn unused() {}\\n' >> crates/yuna/src/lib.rs"
expect_stopped lints "a thread_local! under another name (ADR-0151)" "disallowed macro .std::thread_local." \
    "printf '\nuse std::thread_local as per_thread;\nper_thread! {\n    /// A mark.\n    pub static MARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };\n}\n' >> crates/yuna/src/lib.rs"
# The crate map (ADR-0151). A manifest edited here leaves Cargo.lock behind: `cargo metadata
# --no-deps` reads the manifests alone, so the map's reason is found and printed, and clippy then
# refuses the stale lock, which the step reports too.
expect_stopped map "a pure crate depending on another without its edge" "yuna-layout \\(level 1\\) depends on yuna-ipc \\(level 1\\)" \
    "sed -i 's|^yuna-types = { path = \"../yuna-types\" }$|&\nyuna-ipc = { path = \"../yuna-ipc\" }|' crates/yuna-layout/Cargo.toml"
expect_stopped map "a second crate depending on a library yuna owns" "yuna-wlcs depends on calloop, which yuna owns" \
    "sed -i 's|^tracing.workspace = true$|&\ncalloop.workspace = true|' crates/yuna-wlcs/Cargo.toml"
expect_stopped map "rustix in a pure crate" "yuna-motion \\(level 1\\) depends on rustix, allowed from level 2" \
    "sed -i 's|^yuna-types = { path = \"../yuna-types\" }$|&\nrustix.workspace = true|' crates/yuna-motion/Cargo.toml"
expect_stopped map "one more unsafe block in a module listed" "dmabuf.rs holds 3 .unsafe., its entry 2" \
    "printf '\n/// Reads a byte.\npub(crate) fn reads() -> u8 {\n    // SAFETY: the pointer comes from a reference, alive for the read.\n    unsafe { *(&1u8 as *const u8) }\n}\n' >> crates/yuna/src/dmabuf.rs"
expect_stopped map "an unsafe block in a module not listed, with its expect" "frames.rs holds 1 .unsafe. without an entry" \
    "printf '\n/// Reads a byte.\n#[expect(unsafe_code, reason = \"a case of test-check.sh\")]\npub(crate) fn reads() -> u8 {\n    // SAFETY: the pointer comes from a reference, alive for the read.\n    unsafe { *(&1u8 as *const u8) }\n}\n' >> crates/yuna/src/frames.rs"
expect_stopped map "an allow of unsafe code" ".allow. names .unsafe_code." \
    "printf '\n/// Nothing.\n#[allow(unsafe_code)]\npub fn nothing() {}\n' >> crates/yuna/src/lib.rs"
expect_stopped map "a static not listed" ".static NEXT_RUN. is a global" \
    "printf '\n/// A count.\npub static NEXT_RUN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);\n' >> crates/yuna/src/lib.rs"
expect_stopped map "a static mut not listed" ".static mut HITS. is a global" \
    "printf '\n/// A count.\npub static mut HITS: u32 = 0;\n' >> crates/yuna/src/lib.rs"
# The program's root expects clippy's lint for fatal.rs's `thread_local!` (crates/yuna/src/main.rs).
expect_stopped map "a second thread_local! in yuna's program" "stop_signal.rs:[0-9]+: .static SECOND_MARK. is a global" \
    "printf '\nthread_local! {\n    /// A second mark.\n    static SECOND_MARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };\n}\n' >> crates/yuna/src/stop_signal.rs"
expect_stopped map "a global's entry that matches nothing" "the global GONE .* matches nothing" \
    "sed -i 's|^\\[globals.\"crates/yuna/src/process.rs\"\\]$|&\nGONE = { kind = \"process\", reason = \"a case of test-check.sh\" }|' crates/map.toml"
expect_stopped map "a new file of yuna naming Smithay" "named.rs:1 names Smithay without its entry" \
    "printf 'use smithay::utils::Rectangle;\n' > crates/yuna/src/named.rs"
# The map's other failures, each on the real workspace (its unit tests prove them on fixtures).
expect_stopped map "a crate of the workspace left out of the map" "the crate yuna-motion is in the workspace but not in crates/map.toml" \
    "sed -i '/^yuna-motion = /d' crates/map.toml"
expect_stopped map "the bench depending on a crate of Yuna's" "yuna-bench, outside the map, depends on yuna-types" \
    "sed -i 's|^serde_json.workspace = true$|&\nyuna-types = { path = \"../yuna-types\" }|' crates/yuna-bench/Cargo.toml"
expect_stopped map "a crate of the map depending on the bench beyond its tests" "yuna-wlcs depends on yuna-bench, a crate outside the map" \
    "sed -i 's|^tracing.workspace = true$|&\nyuna-bench = { path = \"../yuna-bench\" }|' crates/yuna-wlcs/Cargo.toml"
expect_stopped map "an edge the map refuses, by a build dependency" "yuna-layout \\(level 1\\) depends on yuna-ipc \\(level 1\\) by a build dependency" \
    "printf '\n[build-dependencies]\nyuna-ipc = { path = \"../yuna-ipc\" }\n' >> crates/yuna-layout/Cargo.toml"
expect_stopped map "an outside library the map does not list" "yuna-ipc depends on proptest, an outside library" \
    "sed -i 's|^yuna-types = { path = \"../yuna-types\" }$|&\nproptest.workspace = true|' crates/yuna-ipc/Cargo.toml"
expect_stopped map "a pure crate's root without its forbid" "the root of yuna-motion, a pure crate, lacks" \
    "sed -i '/^#!\\[forbid(unsafe_code)\\]$/d' crates/yuna-motion/src/lib.rs"
expect_stopped map "a module with fewer unsafe than its entry" "dmabuf.rs holds 2 .unsafe., fewer than its entry's 3" \
    "sed -i 's|^\"crates/yuna/src/dmabuf.rs\" = { count = 2 }$|\"crates/yuna/src/dmabuf.rs\" = { count = 3 }|' crates/map.toml"
expect_stopped map "an unsafe entry that matches nothing" "\\[unsafe-code\\] .crates/yuna/src/frames.rs. matches nothing" \
    "sed -i 's|^\\[unsafe-code\\]$|&\n\"crates/yuna/src/frames.rs\" = { count = 1 }|' crates/map.toml"
expect_stopped map "an unsafe entry in a pure crate" "in the .src/. of yuna-layout, which may not" \
    "sed -i 's|^\\[unsafe-code\\]$|&\n\"crates/yuna-layout/src/area.rs\" = { count = 1 }|' crates/map.toml"
expect_stopped map "two statics of one name in a file" "two statics named STOP_PIPE in crates/yuna/src/stop_signal.rs" \
    "printf '\n/// Nothing.\npub(crate) fn again() {\n    static STOP_PIPE: u8 = 0;\n    let _ = STOP_PIPE;\n}\n' >> crates/yuna/src/stop_signal.rs"
expect_stopped map "a static named by a macro's argument" "is named by a macro's argument" \
    "printf '\nmacro_rules! counted {\n    (\$name:ident) => {\n        static \$name: u8 = 0;\n    };\n}\n' >> crates/yuna/src/lib.rs"
expect_stopped map "a Smithay entry for a file that no longer names it" "command.rs is listed under \\[smithay\\] but no longer names Smithay" \
    "sed -i 's|^files = \\[$|&\n    \"crates/yuna/src/command.rs\",|' crates/map.toml"
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
