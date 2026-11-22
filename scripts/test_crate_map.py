"""Tests of scripts/crate-map.py, on map files, cargo's metadata, sources and clippy's messages
written here: each rule's failures, and a workspace that follows the map."""

import contextlib
import importlib.util
import io
import json
import subprocess
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parent / "crate-map.py"
spec = importlib.util.spec_from_file_location("crate_map", SCRIPT)
crate_map = importlib.util.module_from_spec(spec)
spec.loader.exec_module(crate_map)

ROOT = "/work/yuna"

MAP = """
outside = ["yuna-bench"]
level-4-edges = ["yuna-types", "yuna-ipc"]

[crates]
yuna-types = { level = 0 }
yuna-layout = { level = 1 }
yuna-motion = { level = 1, level-1-edges = ["yuna-layout"] }
yuna-ipc = { level = 1 }
yuna-render = { level = 2, unsafe-code = true }
yuna-core = { level = 3 }
yuna = { level = 4, transitional = { until = "M2.19", level = 3 }, unsafe-code = true }
yuna-mcp = { level = 4 }
yuna-wlcs = { level = 4, unsafe-code = true }

[libraries]
rustix = { from-level = 2 }
serde = { from-level = 0 }
calloop = { owner = "yuna" }

[unsafe-code]
"crates/yuna/src/fatal.rs" = { count = 2, reason = "`_exit`, declared and called" }
"crates/yuna/tests/common/dies_with_test.rs" = { count = 1, reason = "a closure between fork and exec" }

[globals."crates/yuna/src/fatal.rs"]
STATE_THREAD = { kind = "fatal-or-signal", reason = "the panic hook's mark of the state thread" }

[smithay]
files = ["crates/yuna/src/state.rs"]
"""

FATAL = """//! The fatal path.

#![expect(unsafe_code, reason = "`_exit`")]

thread_local! {
    static STATE_THREAD: Cell<bool> = const { Cell::new(false) };
}

// SAFETY: POSIX's own declaration.
unsafe extern "C" {
    fn _exit(status: c_int) -> !;
}

pub fn end() -> ! {
    // SAFETY: `_exit` takes any status.
    unsafe { _exit(101) }
}
"""

SOURCES = {
    "crates/yuna-types/src/lib.rs": "//! Ids.\n\n#![forbid(unsafe_code)]\n\npub struct WindowId(pub u64);\n",
    "crates/yuna-layout/src/lib.rs": "#![forbid(unsafe_code)]\n",
    "crates/yuna-motion/src/lib.rs": "#![forbid(unsafe_code)]\n",
    "crates/yuna-ipc/src/lib.rs": "#![forbid(unsafe_code)]\n",
    "crates/yuna/src/lib.rs": "mod fatal;\nmod state;\n",
    "crates/yuna/src/fatal.rs": FATAL,
    "crates/yuna/src/state.rs": "use smithay::wayland::compositor::CompositorState;\n",
    "crates/yuna/tests/common/dies_with_test.rs": '#![expect(unsafe_code, reason = "a closure")]\n',
    "crates/yuna/tests/headless/main.rs": "static RUNS: AtomicU32 = AtomicU32::new(0);\n",
    "crates/yuna-wlcs/src/lib.rs": "",
    "crates/yuna-bench/src/lib.rs": "static MADE: AtomicUsize = AtomicUsize::new(0);\n",
}

WORKSPACE = {
    "yuna-types": [],
    "yuna-layout": [("yuna-types", "normal"), ("proptest", "dev")],
    "yuna-motion": [("yuna-layout", "normal"), ("yuna-types", "normal")],
    "yuna-ipc": [("yuna-types", "normal"), ("serde", "normal")],
    "yuna": [
        ("yuna-ipc", "normal"),
        ("yuna-motion", "normal"),
        ("calloop", "normal"),
        ("rustix", "normal"),
        ("yuna-bench", "dev"),
        ("wayland-client", "dev"),
    ],
    "yuna-wlcs": [("yuna", "normal"), ("yuna-types", "normal"), ("serde", "normal")],
    "yuna-bench": [("rustix", "normal"), ("wayland-client", "normal")],
}

# Each `unsafe` clippy reports: its span's chain, (file, start, end), outermost invocation last.
# The shared test module is reported by two targets, under two spellings of its path.
UNSAFE = [
    [("crates/yuna/src/fatal.rs", 120, 180)],
    [("crates/yuna/src/fatal.rs", 260, 280)],
    [("crates/yuna/tests/common/dies_with_test.rs", 40, 60)],
    [("crates/yuna/tests/headless/../common/dies_with_test.rs", 40, 60)],
]


def metadata(dependencies):
    """cargo metadata's output, without dependencies, for the crates of `dependencies`, each with
    its own as (name, kind)."""
    packages = [
        {
            "name": name,
            "id": f"path+file://{ROOT}/crates/{name}#0.1.0",
            "manifest_path": f"{ROOT}/crates/{name}/Cargo.toml",
            "targets": [{"kind": ["lib"], "src_path": f"{ROOT}/crates/{name}/src/lib.rs"}],
            "dependencies": [{"name": d, "kind": None if kind == "normal" else kind} for d, kind in own],
        }
        for name, own in dependencies.items()
    ]
    return {"workspace_root": ROOT, "workspace_members": [p["id"] for p in packages], "packages": packages}


def clippy_lines(chains):
    """clippy's JSON lines for `chains`, among the lines of other kinds cargo prints."""
    lines = ['{"reason":"compiler-artifact","package_id":"yuna"}']
    for chain in chains:
        span = None
        for file, start, end in reversed(chain):
            span = {
                "file_name": file,
                "byte_start": start,
                "byte_end": end,
                "is_primary": True,
                "expansion": {"span": span} if span else None,
            }
        message = {"code": {"code": "unsafe_code"}, "level": "warning", "message": "usage of an `unsafe` block", "spans": [span]}
        lines.append(json.dumps({"reason": "compiler-message", "message": message}))
    other = {"code": {"code": "dead_code"}, "level": "warning", "message": "never used", "spans": []}
    lines.append(json.dumps({"reason": "compiler-message", "message": other}))
    lines.append('{"reason":"build-finished","success":true}')
    return lines


class CrateMapTest(unittest.TestCase):
    def setUp(self):
        self.map = MAP
        self.dependencies = {name: list(own) for name, own in WORKSPACE.items()}
        self.sources = dict(SOURCES)
        self.unsafe = list(UNSAFE)

    def problems(self):
        return crate_map.check(self.map, metadata(self.dependencies), self.sources, clippy_lines(self.unsafe))

    def assertFound(self, fragment):
        problems = self.problems()
        self.assertTrue(any(fragment in problem for problem in problems), f"no problem holds {fragment!r}: {problems}")

    def assertNoProblem(self):
        self.assertEqual(self.problems(), [])


class AWorkspaceThatFollowsTheMap(CrateMapTest):
    def test_passes(self):
        self.assertNoProblem()

    def test_without_clippys_lines_its_counts_go_unchecked(self):
        self.unsafe = []
        problems = crate_map.check(self.map, metadata(self.dependencies), self.sources, None)
        self.assertEqual(problems, [])


class Crates(CrateMapTest):
    """Rule: a crate of the workspace is in the map or outside it."""

    def test_a_crate_in_neither_is_named(self):
        self.dependencies["yuna-extra"] = []
        self.assertFound("the crate yuna-extra is in the workspace but not in crates/map.toml")

    def test_a_crate_of_the_map_not_yet_made_is_no_problem(self):
        del self.dependencies["yuna-motion"]
        self.dependencies["yuna"].remove(("yuna-motion", "normal"))
        del self.sources["crates/yuna-motion/src/lib.rs"]
        self.assertNoProblem()


class Edges(CrateMapTest):
    """Rule: an edge between the workspace's crates follows the map."""

    def test_a_level_1_crate_on_another_without_its_edge(self):
        self.dependencies["yuna-layout"].append(("yuna-ipc", "normal"))
        self.assertFound("yuna-layout (level 1) depends on yuna-ipc (level 1), which the map does not allow")

    def test_level_0_on_any_crate_by_a_build_dependency(self):
        self.dependencies["yuna-types"].append(("yuna-ipc", "build"))
        self.assertFound("yuna-types (level 0) depends on yuna-ipc (level 1) by a build dependency")

    def test_a_development_dependency_does_not_count(self):
        self.dependencies["yuna-types"].append(("yuna-layout", "dev"))
        self.assertNoProblem()

    def test_level_4_on_a_crate_of_level_1_beside_the_interfaces_types(self):
        self.dependencies["yuna-wlcs"].append(("yuna-layout", "normal"))
        self.assertFound("yuna-wlcs (level 4) depends on yuna-layout (level 1)")

    def test_a_transitional_crate_is_checked_at_its_transitional_level(self):
        self.dependencies["yuna"].append(("yuna-wlcs", "normal"))
        self.assertFound("yuna (transitional, level 3 for its edges) depends on yuna-wlcs (level 4)")

    def test_level_3_on_level_3(self):
        self.dependencies["yuna-core"] = [("yuna", "normal")]
        self.assertFound("yuna-core (level 3) depends on yuna (transitional, level 3 for its edges)")

    def test_the_bench_on_a_crate_of_yunas_by_any_kind(self):
        self.dependencies["yuna-bench"].append(("yuna-types", "dev"))
        self.assertFound("yuna-bench, outside the map, depends on yuna-types by a dev dependency")

    def test_a_crate_of_the_map_on_the_bench_beyond_its_tests(self):
        self.dependencies["yuna"].append(("yuna-bench", "normal"))
        self.assertFound("yuna depends on yuna-bench, a crate outside the map, by a normal dependency")


class Libraries(CrateMapTest):
    """Rule: an outside library is used by its owner, from its lowest level, and is listed."""

    def test_a_second_owner(self):
        self.dependencies["yuna-wlcs"].append(("calloop", "normal"))
        self.assertFound("yuna-wlcs depends on calloop, which yuna owns")

    def test_below_its_lowest_level(self):
        self.dependencies["yuna-motion"].append(("rustix", "normal"))
        self.assertFound("yuna-motion (level 1) depends on rustix, allowed from level 2")

    def test_not_listed(self):
        self.dependencies["yuna"].append(("libloading", "normal"))
        self.assertFound("yuna depends on libloading, an outside library crates/map.toml does not list")

    def test_development_and_build_dependencies_and_the_bench_do_not_count(self):
        self.dependencies["yuna-layout"].append(("rustix", "dev"))
        self.dependencies["yuna-wlcs"].append(("cc", "build"))
        self.dependencies["yuna-bench"].append(("calloop", "normal"))
        self.assertNoProblem()


class UnsafeCodeAttributes(CrateMapTest):
    """Rule: only an `expect` with its entry, or a pure crate's `forbid`, names `unsafe_code`."""

    def test_an_allow(self):
        self.sources["crates/yuna/src/shell.rs"] = "#[allow(unsafe_code)]\nfn reads() {}\n"
        self.assertFound("crates/yuna/src/shell.rs:1: `allow` names `unsafe_code`")

    def test_a_warn_inside_a_cfg_attr(self):
        self.sources["crates/yuna/src/shell.rs"] = "\n#![cfg_attr(test, warn(unsafe_code))]\n"
        self.assertFound("crates/yuna/src/shell.rs:2: `cfg_attr` names `unsafe_code`")

    def test_a_forbid_outside_a_pure_crates_root(self):
        self.sources["crates/yuna/src/shell.rs"] = "#![forbid(unsafe_code)]\n"
        self.assertFound("crates/yuna/src/shell.rs:1: `forbid` names `unsafe_code`")

    def test_an_expect_without_its_entry(self):
        self.sources["crates/yuna/src/shell.rs"] = '#![expect(unsafe_code, reason = "none")]\n'
        self.assertFound("crates/yuna/src/shell.rs:1 expects `unsafe_code`, but crates/map.toml has no entry")

    def test_the_lint_named_in_a_comment_or_a_string_is_no_attribute(self):
        self.sources["crates/yuna/src/shell.rs"] = '// #[allow(unsafe_code)]\nconst A: &str = "#[allow(unsafe_code)]";\n'
        self.assertNoProblem()

    def test_a_pure_crates_root_without_its_forbid(self):
        self.sources["crates/yuna-layout/src/lib.rs"] = "//! The layout: #![forbid(unsafe_code)]\n"
        self.assertFound("crates/yuna-layout/src/lib.rs, the root of yuna-layout, a pure crate, lacks `#![forbid(unsafe_code)]`")

    def test_a_forbid_inside_an_inline_module_of_a_pure_root_covers_that_module_alone(self):
        self.sources["crates/yuna-layout/src/lib.rs"] = "//! The layout.\n\nmod tree {\n    #![forbid(unsafe_code)]\n}\n"
        self.assertFound("crates/yuna-layout/src/lib.rs, the root of yuna-layout, a pure crate, lacks `#![forbid(unsafe_code)]`")
        self.assertFound("crates/yuna-layout/src/lib.rs:4: `forbid` names `unsafe_code`")

    def test_the_files_own_attributes_are_those_outside_every_bracket(self):
        code = crate_map.code_only(
            "#![forbid(unsafe_code)]\nconst C: char = '{';\nfn f() { #![allow(unsafe_code)] }\n"
            "m!( #![warn(unsafe_code)] );\n#![expect(unsafe_code, reason = \"{\")]\n"
        )
        self.assertEqual(
            crate_map.unsafe_code_attributes(code),
            [("forbid", True, 1), ("allow", False, 3), ("warn", False, 4), ("expect", True, 5)],
        )


class UnsafeCodeCounts(CrateMapTest):
    """Rules: each file's `unsafe`, as rustc counts it, matches its entry and its own `expect`."""

    def test_an_entry_that_matches_nothing(self):
        self.unsafe = [chain for chain in UNSAFE if "dies_with_test" not in chain[0][0]]
        self.assertFound('the entry [unsafe-code] "crates/yuna/tests/common/dies_with_test.rs" matches nothing')

    def test_unsafe_in_a_module_without_its_entry(self):
        self.sources["crates/yuna/src/shell.rs"] = '#![expect(unsafe_code, reason = "a read")]\n'
        self.unsafe.append([("crates/yuna/src/shell.rs", 50, 60)])
        self.assertFound("crates/yuna/src/shell.rs holds 1 `unsafe` without an entry under [unsafe-code]")

    def test_unsafe_in_a_module_without_its_own_expect(self):
        self.sources["crates/yuna/src/fatal.rs"] = FATAL.replace('#![expect(unsafe_code, reason = "`_exit`")]', "")
        self.assertFound("crates/yuna/src/fatal.rs holds 2 `unsafe` without its own `expect(unsafe_code")

    def test_more_than_its_entry(self):
        self.unsafe.append([("crates/yuna/src/fatal.rs", 300, 310)])
        self.assertFound("crates/yuna/src/fatal.rs holds 3 `unsafe`, its entry 2: a module's `unsafe` grows only by an ADR")

    def test_fewer_than_its_entry(self):
        self.unsafe.remove([("crates/yuna/src/fatal.rs", 260, 280)])
        self.assertFound("crates/yuna/src/fatal.rs holds 1 `unsafe`, fewer than its entry's 2: lower the entry to 1")

    def test_an_entry_in_the_src_of_a_crate_not_allowed_unsafe_code(self):
        self.dependencies["yuna-mcp"] = []
        self.map = self.map.replace(
            "[unsafe-code]\n", '[unsafe-code]\n"crates/yuna-mcp/src/server.rs" = { count = 1, reason = "a read" }\n'
        )
        self.sources["crates/yuna-mcp/src/server.rs"] = '#![expect(unsafe_code, reason = "a read")]\n'
        self.unsafe.append([("crates/yuna-mcp/src/server.rs", 50, 60)])
        self.assertFound('[unsafe-code] "crates/yuna-mcp/src/server.rs" is in the `src/` of yuna-mcp, which may not hold')

    def test_an_unsafe_a_macro_makes_counts_where_the_macro_is_called(self):
        self.sources["crates/yuna/src/macros.rs"] = "macro_rules! reads { () => { unsafe { 1 } } }\n"
        self.unsafe.append([("crates/yuna/src/macros.rs", 30, 44), ("crates/yuna/src/fatal.rs", 400, 408)])
        self.assertFound("crates/yuna/src/fatal.rs holds 3 `unsafe`, its entry 2")
        counts, problems = crate_map.unsafe_counts(clippy_lines(self.unsafe), Path(ROOT))
        self.assertEqual(problems, [])
        self.assertEqual(counts["crates/yuna/src/fatal.rs"], 3)
        self.assertNotIn("crates/yuna/src/macros.rs", counts)

    def test_the_same_span_reported_twice_counts_once(self):
        self.unsafe.append([("crates/yuna/src/fatal.rs", 120, 180)])
        self.unsafe.append([(f"{ROOT}/crates/yuna/src/fatal.rs", 260, 280)])
        counts, _ = crate_map.unsafe_counts(clippy_lines(self.unsafe), Path(ROOT))
        self.assertEqual(counts["crates/yuna/src/fatal.rs"], 2)
        self.assertEqual(counts["crates/yuna/tests/common/dies_with_test.rs"], 1)
        self.assertNoProblem()

    def test_a_line_that_is_not_json_or_a_message_with_no_place(self):
        placeless = {"code": {"code": "unsafe_code"}, "message": "usage of an `unsafe` block", "spans": []}
        lines = clippy_lines(self.unsafe) + ["{not json", json.dumps({"reason": "compiler-message", "message": placeless})]
        counts, problems = crate_map.unsafe_counts(lines, Path(ROOT))
        self.assertEqual(len(problems), 2, problems)
        self.assertIn("is not JSON", problems[0])
        self.assertIn("reports `unsafe_code` at no place", problems[1])
        self.assertEqual(sum(counts.values()), 3)

    def test_lines_and_fields_of_the_wrong_type_are_problems_not_crashes(self):
        unsafe_code = {"code": "unsafe_code"}
        wrong = [
            "[1, 2]",
            '"text"',
            "null",
            json.dumps({"reason": "compiler-message", "message": "text"}),
            json.dumps({"reason": "compiler-message", "message": {"code": "unsafe_code", "spans": []}}),
            json.dumps({"reason": "compiler-message", "message": {"code": unsafe_code, "spans": "here"}}),
            json.dumps({"reason": "compiler-message", "message": {"code": unsafe_code, "spans": ["here"]}}),
            json.dumps({"reason": "compiler-message", "message": {"code": unsafe_code, "spans": [
                {"is_primary": True, "file_name": "crates/yuna/src/fatal.rs", "byte_start": [1], "byte_end": 2}
            ]}}),
            json.dumps({"reason": "compiler-message", "message": {"code": unsafe_code, "spans": [
                {"is_primary": True, "file_name": "crates/yuna/src/fatal.rs", "byte_start": 1, "byte_end": 2,
                 "expansion": {"span": "the macro"}}
            ]}}),
        ]
        counts, problems = crate_map.unsafe_counts(clippy_lines(self.unsafe) + wrong, Path(ROOT))
        self.assertEqual(sum(counts.values()), 3)
        self.assertEqual([problem.split(", ", 2)[2] for problem in problems], [
            "is not a JSON object",
            "is not a JSON object",
            "is not a JSON object",
            "is a compiler message without its diagnostic",
            "reports `unsafe_code` at no place",
            "reports `unsafe_code` at no place",
            "reports `unsafe_code` at no place",
            "reports `unsafe_code` at no place",
        ])


class RunningClippy(unittest.TestCase):
    """When clippy fails, its errors are printed and its lines' problems reported."""

    def run_clippy(self, lines):
        failed = subprocess.CompletedProcess(crate_map.CLIPPY, returncode=101, stdout="\n".join(lines) + "\n")
        printed = io.StringIO()
        with mock.patch.object(crate_map.subprocess, "run", return_value=failed), contextlib.redirect_stderr(printed):
            result = crate_map.run_clippy(Path(ROOT))
        return result, printed.getvalue()

    def test_a_json_line_that_is_not_an_object_is_a_problem(self):
        error = {"reason": "compiler-message", "message": {"level": "error", "rendered": "error: it broke"}}
        (lines, problems), printed = self.run_clippy(["[1]", "7", json.dumps(error), "{not json"])
        self.assertIsNone(lines)
        self.assertEqual(printed, "error: it broke\n")
        self.assertEqual(len(problems), 4, problems)
        self.assertIn("line 1, is not a JSON object", problems[0])
        self.assertIn("line 2, is not a JSON object", problems[1])
        self.assertIn("line 4, is not JSON", problems[2])
        self.assertIn("cargo clippy failed (exit 101)", problems[3])


class Globals(CrateMapTest):
    """Rule: every static of the map's crates' `src/` is listed by file and name."""

    def assertStatics(self, source, expected):
        self.assertEqual([(name, mutable) for name, _, mutable in crate_map.statics_in(crate_map.code_only(source))], expected)

    def test_an_unlisted_static(self):
        self.sources["crates/yuna/src/shell.rs"] = "\n\npub(crate) static NEXT: AtomicU64 = AtomicU64::new(1);\n"
        self.assertFound("crates/yuna/src/shell.rs:3: `static NEXT` is a global crates/map.toml does not list")

    def test_an_unlisted_static_mut_used_through_raw(self):
        self.sources["crates/yuna/src/shell.rs"] = "static mut HITS: u32 = 0;\nfn hit() { unsafe { (&raw mut HITS).write(1) } }\n"
        self.assertFound("crates/yuna/src/shell.rs:1: `static mut HITS` is a global")

    def test_a_static_behind_a_newtype_or_an_alias(self):
        source = "struct Count(AtomicU32);\ntype Shared = Mutex<u32>;\nstatic COUNT: Count = Count(AtomicU32::new(0));\nstatic SHARED: Shared = Mutex::new(0);\n"
        self.assertStatics(source, [("COUNT", False), ("SHARED", False)])

    def test_statics_inside_a_function_a_macros_body_and_a_thread_local(self):
        source = (
            "fn once() { static DONE: Once = Once::new(); }\n"
            "macro_rules! counter { ($name:ident) => { static $name: AtomicU32 = AtomicU32::new(0); static MADE: u8 = 0; } }\n"
            "thread_local! { static MARK: Cell<bool> = const { Cell::new(false) }; }\n"
        )
        self.assertStatics(source, [("DONE", False), ("$name", False), ("MADE", False), ("MARK", False)])
        self.sources["crates/yuna/src/shell.rs"] = source
        self.assertFound("crates/yuna/src/shell.rs:2: `static MADE` is a global")

    def test_a_static_named_by_a_raw_identifier(self):
        self.assertStatics("static r#type: u8 = 0;\nstatic r#ref: u8 = 0;\n", [("r#type", False), ("r#ref", False)])

    def test_static_in_a_lifetime_a_comment_or_a_raw_string_is_no_static(self):
        source = (
            "fn name() -> &'static str { r#\"static X: u8 = 0; \"quoted\"\"# }\n"
            "// static Y: u8 = 0;\n"
            "/* static Z: u8 = 0; /* nested */ static W: u8 = 0; */\n"
            "const QUOTE: char = '\"'; const ESCAPED: char = '\\''; const S: &str = \"static V: u8\";\n"
            "fn bound<T: 'static>(r#static: T) {}\n"
        )
        self.assertStatics(source, [])

    def test_two_statics_of_one_name_in_one_file(self):
        self.sources["crates/yuna/src/fatal.rs"] = FATAL + "fn mark() { static STATE_THREAD: AtomicBool = AtomicBool::new(false); }\n"
        self.assertFound("two statics named STATE_THREAD in crates/yuna/src/fatal.rs (lines 6, 18): rename one")

    def test_the_same_name_in_two_files_is_two_globals(self):
        self.sources["crates/yuna/src/shell.rs"] = "static STATE_THREAD: AtomicBool = AtomicBool::new(false);\n"
        problems = self.problems()
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("crates/yuna/src/shell.rs:1: `static STATE_THREAD` is a global crates/map.toml does not list", problems[0])

    def test_a_static_named_by_a_macros_argument(self):
        self.sources["crates/yuna/src/shell.rs"] = "macro_rules! counter { ($name:ident) => { static $name: AtomicU32 = AtomicU32::new(0); } }\n"
        self.assertFound("crates/yuna/src/shell.rs:1: `static $name` is named by a macro's argument")
        self.assertFound("declare the static by name in the macro's body")

    def test_an_entry_named_by_a_macros_argument_is_refused(self):
        self.map = self.map.replace("STATE_THREAD = ", '"$name" = { kind = "process", reason = "a counter" }\nSTATE_THREAD = ')
        self.assertFound(
            "[globals.\"crates/yuna/src/fatal.rs\"] $name: a static named by a macro's argument would cover every "
            "call of the macro: declare the static by name"
        )

    def test_a_global_entry_that_matches_nothing(self):
        self.map = self.map.replace("STATE_THREAD = ", 'GONE = { kind = "process", reason = "gone" }\nSTATE_THREAD = ')
        self.assertFound('the global GONE of [globals."crates/yuna/src/fatal.rs"] matches nothing')

    def test_tests_examples_and_the_bench_are_not_scanned(self):
        self.sources["crates/yuna/examples/frame_time.rs"] = "static RUNS: AtomicU32 = AtomicU32::new(0);\n"
        self.assertNoProblem()


class Smithay(CrateMapTest):
    """Rule: the files of the transitional crate's `src/` that name Smithay are listed."""

    def test_a_new_file_naming_smithay(self):
        self.sources["crates/yuna/src/named.rs"] = "//! A module.\nuse smithay::utils::Rectangle;\n"
        self.assertFound("crates/yuna/src/named.rs:2 names Smithay without its entry under [smithay]")

    def test_smithay_in_a_comment_or_a_string_is_not_named(self):
        self.sources["crates/yuna/src/named.rs"] = '// smithay::utils\nconst A: &str = "smithay::utils";\nlet smithay_output = 1;\n'
        self.assertNoProblem()

    def test_smithay_named_as_a_word_without_a_path(self):
        for source in ("use smithay as sm;\n", "use smithay;\n", "extern crate smithay;\n", "pub use ::smithay::{self};\n"):
            with self.subTest(source=source):
                self.sources["crates/yuna/src/named.rs"] = "//! A module.\n\n" + source
                self.assertFound("crates/yuna/src/named.rs:3 names Smithay without its entry under [smithay]")

    def test_a_word_that_holds_smithay_is_not_smithay(self):
        for source in ("let smithay_output = 1;\n", "use crate::yuna_smithay::Glue;\n", "struct Smithay;\n", "let r#smithays = 2;\n"):
            with self.subTest(source=source):
                self.sources["crates/yuna/src/named.rs"] = source
                self.assertNoProblem()

    def test_a_listed_file_that_no_longer_names_it(self):
        self.sources["crates/yuna/src/state.rs"] = "use crate::seat::Seat;\n"
        self.assertFound("crates/yuna/src/state.rs is listed under [smithay] but no longer names Smithay")

    def test_other_crates_are_not_scanned(self):
        self.sources["crates/yuna-wlcs/src/lib.rs"] = "use smithay::utils::Rectangle;\n"
        self.assertNoProblem()


class MapFile(CrateMapTest):
    """A map file that is not one is refused, naming the entry."""

    def test_not_toml(self):
        self.map = "[crates\n"
        self.assertFound("crates/map.toml is not TOML")

    def test_a_pure_crate_allowed_unsafe_code(self):
        self.map = self.map.replace('yuna-layout = { level = 1 }', 'yuna-layout = { level = 1, unsafe-code = true }')
        self.assertFound("[crates] yuna-layout: a pure crate (level 0 or 1) forbids `unsafe` code")

    def test_an_edge_to_no_level_1_crate(self):
        self.map = self.map.replace('level-1-edges = ["yuna-layout"]', 'level-1-edges = ["yuna-render"]')
        self.assertFound("[crates] yuna-motion: `level-1-edges` names yuna-render, which is no other level-1 crate")

    def test_a_global_of_no_kind(self):
        self.map = self.map.replace('kind = "fatal-or-signal"', 'kind = "convenient"')
        self.assertFound("`kind` is one of fatal-or-signal, process, library, constant")

    def test_a_librarys_global_without_its_milestone(self):
        self.map = self.map.replace('kind = "fatal-or-signal"', 'kind = "library"')
        self.assertFound("a global of kind `library`, and only one, says `until` which milestone")

    def test_an_unknown_key(self):
        self.map = self.map.replace('yuna-ipc = { level = 1 }', 'yuna-ipc = { level = 1, owner = "x" }')
        self.assertFound("[crates] yuna-ipc: unknown keys owner")


class CodeOnly(unittest.TestCase):
    """Comments, literals and lifetimes become spaces; the code and its lines stay."""

    def test_keeps_offsets_and_lines(self):
        source = 'let a = "x\ny"; // c\nlet b = \'z\';\n'
        code = crate_map.code_only(source)
        self.assertEqual(len(code), len(source))
        self.assertEqual(code.count("\n"), source.count("\n"))
        self.assertEqual(code.split(), ["let", "a", "=", ";", "let", "b", "=", ";"])

    def test_raw_strings_byte_strings_and_raw_identifiers(self):
        source = 'br##"a "# b"## b"c\\"" r#type cr"d" b\'\\\\\''
        self.assertEqual(crate_map.code_only(source).split(), ["b", "r#type", "b"])

    def test_an_unclosed_comment_ends_with_the_file(self):
        self.assertEqual(crate_map.code_only("a /* b /* c */ d").split(), ["a"])


if __name__ == "__main__":
    unittest.main()
