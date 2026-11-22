#!/usr/bin/env python3
"""Checks Yuna's crate map (ADR-0151) against the code: the map file, crates/map.toml, against
`cargo metadata` for the crates and their dependencies, rustc for the `unsafe` code, and a scan
of the sources' text for the rest.

It fails, each problem naming the crate, the file or the entry and what to do, when:

- a crate of the workspace is neither in the map nor outside it;
- an edge between the workspace's crates breaks the map, by a normal or a build dependency:
  level 0 depends on no crate of Yuna's; level 1 on level 0 and the level-1 crates its
  `level-1-edges` names; level 2 on levels 0 and 1; level 3 on levels 0 to 2; level 4 on level 3
  and the crates of `level-4-edges`. A transitional crate is checked at its transitional level.
  Development dependencies do not count, but a crate outside the map depends on no crate of
  Yuna's by any kind, and a crate of the map depends on a crate outside the map only as a
  development dependency;
- a crate of the map depends, by a normal dependency, on an outside library that another crate
  owns, below the library's lowest level, or that the map does not list;
- a file names `unsafe_code` in an attribute other than an `expect` (an `allow`, a `warn`), the
  `forbid` at a pure crate's root apart; holds an `expect` of it without its entry; holds
  `unsafe` code without its own `expect`; or an entry's file holds none;
- a file holds another count of `unsafe` than its entry gives: more needs an ADR first, fewer
  lowers the entry, so that the list stays exact (review, not the script, keeps it shrinking: an
  entry added needs an ADR); or an entry names a file in
  the `src/` of a crate not allowed `unsafe` code there;
- a pure crate's root (levels 0 and 1) lacks `#![forbid(unsafe_code)]` among its file's own
  attributes: one inside an inline module covers that module alone;
- a `static` in the `src/` of a crate of the map, of any type (`static mut`, statics inside
  functions or a `macro_rules!` body, and those a `thread_local!` declares included), is not
  listed by file and name, two in one file share a name (statics that a `cfg` makes
  alternatives of take distinct names too), or a global's entry matches nothing.
  A static that a macro's body names by an argument, `static $name`, cannot be listed, since its
  entry would cover every call of the macro: it is declared by name. Tests, examples and the
  crates outside the map are left out; unit tests inside `src/` are not: a static there is listed
  too;
- a file of a transitional crate's `src/` names Smithay (any identifier `smithay` in code: a path, a
  `use`, an `extern crate`, or a local of that name) without its entry, or an entry names a file that no longer does.

What it reads:

- The crates and their dependencies: `cargo metadata --format-version 1 --no-deps --offline`,
  which reads the manifests alone and never writes Cargo.lock. It trusts what cargo says.
- The `unsafe` code: rustc's lint `unsafe_code`, over every target of the workspace, through
  `cargo clippy --workspace --all-targets --locked --offline --message-format=json -- --force-warn
  unsafe_code`. `--force-warn` overrides each `expect` and reports every `unsafe` as a warning;
  clippy passes it to the workspace's crates alone, so the dependencies' builds are reused. A
  file's count is of every `unsafe` the lint names: blocks, `unsafe fn`, `unsafe extern` blocks,
  `unsafe impl` and `unsafe trait`, and the unsafe attributes such as `#[unsafe(no_mangle)]`. An
  `unsafe` that a macro of the crate makes counts in the file that calls the macro, whose
  `expect` rustc reads; one in a file compiled into several targets, a library and its tests or a
  module the tests share, counts once. Every line clippy prints must be JSON: a procedural macro
  that printed to standard output would fail the step, the line named.
- The rest: a scan of the text of every `.rs` file in the crates' folders, compiled or not, its
  comments, string and character literals (raw ones included) and lifetimes (`'static`) left out.

What it cannot see:

- a static made inside a procedural macro or a dependency's macro, such as `tracing`'s callsites;
- `unsafe` code that a dependency's macro makes: rustc does not report it in the crate that calls
  the macro;
- each copy of an `unsafe` that a macro's repetition makes: rustc reports identical copies once,
  so an argument added to such a call adds no count;
- `unsafe` code under a `cfg` this build leaves out: another target, or the parity canaries'
  feature, `parity-canary`, which it does not turn on. The canaries' code today is under `cfg!`,
  compiled in every build, so it is counted; `unsafe` under `#[cfg(feature = "parity-canary")]`
  would not be, though check.sh's `lints` step, which builds with the feature, refuses it
  without its `expect`;
- `unsafe` code in doctests, which no clippy run builds;
- how many statics a `macro_rules!` body makes: a static it names, `static NAME`, is one entry,
  however many times the macro is called;
- a module that reaches Smithay's types without naming the crate, through the fields or the
  results of a type that names it: the list of files naming Smithay is a list of names, and review
  keeps new modules off Smithay's types.

    scripts/crate-map.py    prints each problem and exits 1 when there is one
"""

import json
import os
import re
import subprocess
import sys
import tomllib
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAP_FILE = "crates/map.toml"

METADATA = ["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"]
CLIPPY = [
    "cargo", "clippy", "--workspace", "--all-targets", "--locked", "--offline",
    "--message-format=json", "--", "--force-warn", "unsafe_code",
]

LEVELS = range(5)
PURE_LEVELS = (0, 1)
# ADR-0151's kinds of global: the fatal path and the signal handlers; state the operating system
# keeps for the whole process; what a library's interface forces until it leaves; a constant.
GLOBAL_KINDS = ("fatal-or-signal", "process", "library", "constant")
# What cargo names a library target: `lib`, or the crate types a manifest asks for.
LIBRARY_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}

# The tokens of Rust's text that matter here, raw strings before identifiers so that `r"…"` is
# not read as the identifier `r`; a raw identifier, `r#static`, is one identifier.
TOKEN = re.compile(
    r'(?P<raw>(?:br|cr|r)(?P<hashes>#*)")|(?P<identifier>(?:r#)?\w+)'
    r"|(?P<line_comment>//)|(?P<block_comment>/\*)|(?P<string>\")|(?P<quote>')"
)
STRING_REST = re.compile(r'(?:[^"\\]|\\.)*"', re.DOTALL)
CHARACTER_REST = re.compile(r"\\.[^']*'", re.DOTALL)
IDENTIFIER = re.compile(r"(?:r#)?\w+")
BLANKED = re.compile(r"[^\n]")

STATIC = re.compile(r"\bstatic\s+(?:(mut)\s+)?(\$?(?:r#)?\w+)")
ATTRIBUTE = re.compile(r"#\s*(!?)\s*\[")
ATTRIBUTE_NAME = re.compile(r"\s*(\w+(?:\s*::\s*\w+)*)")
NAMES_UNSAFE_CODE = re.compile(r"\bunsafe_code\b")
# As a word, so that `use smithay as sm;` and `extern crate smithay;` count, `smithay_output` not.
SMITHAY = re.compile(r"\bsmithay\b")


class Malformed(Exception):
    """The map file cannot be read as one: the message names the entry."""


# Reading Rust's text.


def code_only(source):
    """`source` with its comments, string and character literals and lifetimes made spaces, its
    newlines kept, so that what is left is code, at the source's own offsets and lines."""
    pieces, kept_from, position = [], 0, 0
    while match := TOKEN.search(source, position):
        if match.group("identifier"):
            position = match.end()
            continue
        start, end = match.start(), literal_end(source, match)
        pieces.append(source[kept_from:start])
        pieces.append(BLANKED.sub(" ", source[start:end]))
        kept_from = position = end
    pieces.append(source[kept_from:])
    return "".join(pieces)


def literal_end(source, match):
    """Where the comment, literal or lifetime that `match` starts ends; the source's end when it
    is never closed, which the compiler refuses anyway."""
    if match.group("raw") is not None:
        closing = '"' + match.group("hashes")
        end = source.find(closing, match.end())
        return len(source) if end == -1 else end + len(closing)
    if match.group("line_comment"):
        end = source.find("\n", match.end())
        return len(source) if end == -1 else end
    if match.group("block_comment"):
        return block_comment_end(source, match.start())
    if match.group("string"):
        rest = STRING_REST.match(source, match.end())
        return rest.end() if rest else len(source)
    return quote_end(source, match.start())


def block_comment_end(source, start):
    """The end of the block comment at `start`, whose own comments nest, as Rust's do."""
    depth, position = 0, start
    while position < len(source):
        if source.startswith("/*", position):
            depth, position = depth + 1, position + 2
        elif source.startswith("*/", position):
            depth, position = depth - 1, position + 2
            if depth == 0:
                return position
        else:
            position += 1
    return len(source)


def quote_end(source, start):
    """The end of what starts with the quote at `start`: a character literal, `'x'` or `'\\n'`,
    or a lifetime or a label, `'static`."""
    if source.startswith("\\", start + 1):
        rest = CHARACTER_REST.match(source, start + 1)
        return rest.end() if rest else len(source)
    if source.startswith("'", start + 2):
        return start + 3
    lifetime = IDENTIFIER.match(source, start + 1)
    return lifetime.end() if lifetime else start + 1


def line_at(code, offset):
    return code.count("\n", 0, offset) + 1


def statics_in(code):
    """The statics `code` declares, as (name, line, `static mut`): `$name` for one a macro's
    body names by a metavariable."""
    return [
        (match.group(2), line_at(code, match.start()), match.group(1) is not None)
        for match in STATIC.finditer(code)
    ]


def unsafe_code_attributes(code):
    """The attributes of `code` that name `unsafe_code`, as (their name, whether it is one of the
    file's own, line): `expect`, `allow`, `forbid`, `cfg_attr`… A file's own attribute is an inner
    one, `#![…]`, outside every brace, bracket and parenthesis; an inner one inside an inline
    module, a function or a macro's body covers that alone."""
    found = []
    for match in ATTRIBUTE.finditer(code):
        content = code[match.end():bracket_end(code, match.end())]
        if NAMES_UNSAFE_CODE.search(content):
            name = ATTRIBUTE_NAME.match(content)
            name = re.sub(r"\s", "", name.group(1)) if name else ""
            of_file = match.group(1) == "!" and nesting_at(code, match.start()) == 0
            found.append((name, of_file, line_at(code, match.start())))
    return found


def nesting_at(code, offset):
    """How many braces, brackets and parentheses are open at `offset` of `code`."""
    opened = sum(code.count(bracket, 0, offset) for bracket in "{[(")
    return opened - sum(code.count(bracket, 0, offset) for bracket in "}])")


def bracket_end(code, position):
    """Where the attribute's brackets, opened just before `position`, close."""
    depth = 1
    while position < len(code) and depth:
        if code[position] == "[":
            depth += 1
        elif code[position] == "]":
            depth -= 1
        position += 1
    return position - 1 if depth == 0 else position


def expects_unsafe_code(code):
    return any(name == "expect" for name, _, _ in unsafe_code_attributes(code))


def forbids_unsafe_code(code):
    return any(name == "forbid" and of_file for name, of_file, _ in unsafe_code_attributes(code))


def smithay_line(code):
    """The first line on which `code` names Smithay, none when it does not."""
    match = SMITHAY.search(code)
    return line_at(code, match.start()) if match else None


# Reading rustc's messages.


def compiler_diagnostics(lines):
    """The compiler's diagnostics in cargo's JSON lines, as (line number, diagnostic), and a
    problem for each line that is not a JSON object, or a compiler message without its diagnostic.
    Cargo's other messages, its artifacts and the build's end, are left out."""
    diagnostics, problems = [], []
    for number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError as error:
            problems.append(f"clippy's output, line {number}, is not JSON: {error}")
            continue
        if not isinstance(message, dict):
            problems.append(f"clippy's output, line {number}, is not a JSON object")
        elif message.get("reason") == "compiler-message":
            if isinstance(message.get("message"), dict):
                diagnostics.append((number, message["message"]))
            else:
                problems.append(f"clippy's output, line {number}, is a compiler message without its diagnostic")
    return diagnostics, problems


def unsafe_counts(lines, root):
    """Each file's count of `unsafe`, from clippy's JSON lines, and the problems met reading
    them. An `unsafe` a macro makes is counted at its outermost invocation, the last span of its
    expansion's chain; the same chain reported again, by another target that compiles the file,
    is counted once."""
    diagnostics, problems = compiler_diagnostics(lines)
    counts, seen = Counter(), set()
    for number, diagnostic in diagnostics:
        if lint_code(diagnostic) != "unsafe_code":
            continue
        chain = span_chain(primary_span(diagnostic), root)
        if chain is None:
            problems.append(f"clippy's output, line {number}, reports `unsafe_code` at no place")
            continue
        if chain not in seen:
            seen.add(chain)
            counts[chain[-1][0]] += 1
    return counts, problems


def lint_code(diagnostic):
    """The name of the lint a diagnostic reports, none when it reports none."""
    code = diagnostic.get("code")
    return code.get("code") if isinstance(code, dict) else None


def primary_span(diagnostic):
    spans = diagnostic.get("spans")
    if not isinstance(spans, list):
        return None
    return next((span for span in spans if isinstance(span, dict) and span.get("is_primary")), None)


def span_chain(span, root):
    """A span and the spans of the macros' invocations it came from, outermost last, each as
    (file from the repository's root, start, end); none when a span lacks its place."""
    chain = []
    while span is not None:
        place = span_place(span, root)
        if place is None:
            return None
        chain.append(place)
        expansion = span.get("expansion")
        span = expansion.get("span") if isinstance(expansion, dict) else None
    return tuple(chain) or None


def span_place(span, root):
    """A span's (file from the repository's root, start, end), none when it lacks one."""
    if not isinstance(span, dict):
        return None
    file, start, end = span.get("file_name"), span.get("byte_start"), span.get("byte_end")
    # `type(…) is int`, not `isinstance`: JSON's `true` reads as a bool, which `isinstance` takes
    # for an int.
    if not isinstance(file, str) or type(start) is not int or type(end) is not int:
        return None
    return repository_path(file, root), start, end


def repository_path(name, root):
    """A file's path from the repository's root, `..` resolved: `tests/headless/../common/a.rs`
    is `tests/common/a.rs`."""
    path = Path(name)
    if path.is_absolute():
        try:
            path = path.relative_to(root)
        except ValueError:
            pass
    return Path(os.path.normpath(path)).as_posix()


# Reading the map file.


@dataclass
class Crate:
    """A crate of the map."""

    name: str
    level: int
    # The level-1 crates a level-1 crate may depend on.
    level_1_edges: tuple
    # Whether its `src/` may hold `unsafe` code.
    unsafe_code: bool
    # The level its edges and libraries are checked at while it is transitional, else none.
    transitional_level: int | None

    @property
    def checked_level(self):
        return self.level if self.transitional_level is None else self.transitional_level

    @property
    def described(self):
        if self.transitional_level is None:
            return f"{self.name} (level {self.level})"
        return f"{self.name} (transitional, level {self.transitional_level} for its edges)"


@dataclass
class Library:
    """An outside library: the one crate that may use it, or the lowest level that may."""

    owner: str | None
    from_level: int | None


@dataclass
class CrateMap:
    crates: dict
    outside: set
    level_4_edges: set
    libraries: dict
    # Each module's count of `unsafe`, by its file.
    unsafe_code: dict
    # (file, name) of each global.
    globals: set
    smithay_files: list


def read_map(text):
    """The map file's text as a `CrateMap`; raises `Malformed`, naming the entry, when it is not
    one."""
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        raise Malformed(f"{MAP_FILE} is not TOML: {error}") from None
    keys = ("crates", "outside", "level-4-edges", "libraries", "unsafe-code", "globals", "smithay")
    table_at(data, MAP_FILE, required=keys)
    crates = {name: read_crate(name, value) for name, value in mapping_at(data["crates"], "[crates]").items()}
    crate_map = CrateMap(
        crates=crates,
        outside=set(texts_at(data["outside"], "`outside`")),
        level_4_edges=set(texts_at(data["level-4-edges"], "`level-4-edges`")),
        libraries={
            name: read_library(name, value) for name, value in mapping_at(data["libraries"], "[libraries]").items()
        },
        unsafe_code={
            path: read_unsafe_entry(path, value)
            for path, value in mapping_at(data["unsafe-code"], "[unsafe-code]").items()
        },
        globals=read_globals(mapping_at(data["globals"], "[globals]")),
        smithay_files=texts_at(table_at(data["smithay"], "[smithay]", required=("files",))["files"], "[smithay] files"),
    )
    check_references(crate_map)
    return crate_map


def read_crate(name, value):
    where = f"[crates] {name}"
    table_at(value, where, required=("level",), optional=("level-1-edges", "unsafe-code", "transitional"))
    level = level_at(value["level"], f"{where}: `level`")
    edges = tuple(texts_at(value.get("level-1-edges", []), f"{where}: `level-1-edges`"))
    if edges and level != 1:
        raise Malformed(f"{where}: `level-1-edges` belong to a crate of level 1")
    unsafe_code = value.get("unsafe-code", False)
    if not isinstance(unsafe_code, bool):
        raise Malformed(f"{where}: `unsafe-code` is true or false")
    if unsafe_code and level in PURE_LEVELS:
        raise Malformed(f"{where}: a pure crate (level 0 or 1) forbids `unsafe` code")
    transitional_level = None
    if "transitional" in value:
        transitional = table_at(value["transitional"], f"{where}: `transitional`", required=("until", "level"))
        text_at(transitional["until"], f"{where}: `transitional.until`")
        transitional_level = level_at(transitional["level"], f"{where}: `transitional.level`")
    return Crate(name, level, edges, unsafe_code, transitional_level)


def read_library(name, value):
    where = f"[libraries] {name}"
    table_at(value, where, optional=("owner", "from-level"))
    if ("owner" in value) == ("from-level" in value):
        raise Malformed(f"{where}: give it either an `owner` or a `from-level`")
    if "owner" in value:
        return Library(text_at(value["owner"], f"{where}: `owner`"), None)
    return Library(None, level_at(value["from-level"], f"{where}: `from-level`"))


def read_unsafe_entry(path, value):
    where = f'[unsafe-code] "{path}"'
    table_at(value, where, required=("count",))
    count = value["count"]
    if type(count) is not int or count < 1:
        raise Malformed(f"{where}: `count` is a number of `unsafe`, 1 or more")
    return count


def read_globals(files):
    globals_ = set()
    for path, names in files.items():
        for name, value in mapping_at(names, f'[globals."{path}"]').items():
            where = f'[globals."{path}"] {name}'
            if name.startswith("$"):
                raise Malformed(
                    f"{where}: a static named by a macro's argument would cover every call of the macro: "
                    "declare the static by name in the macro's body, and list that name"
                )
            table_at(value, where, required=("kind", "reason"), optional=("until",))
            if value["kind"] not in GLOBAL_KINDS:
                raise Malformed(f"{where}: `kind` is one of {', '.join(GLOBAL_KINDS)} (ADR-0151)")
            if (value["kind"] == "library") != ("until" in value):
                raise Malformed(f"{where}: a global of kind `library`, and only one, says `until` which milestone")
            text_at(value["reason"], f"{where}: `reason`")
            globals_.add((path, name))
    return globals_


def check_references(crate_map):
    """Refuses a map whose entries name crates it does not hold, or one crate twice."""
    for crate in crate_map.crates.values():
        for other in crate.level_1_edges:
            if crate_map.crates.get(other) is None or crate_map.crates[other].level != 1 or other == crate.name:
                raise Malformed(f"[crates] {crate.name}: `level-1-edges` names {other}, which is no other level-1 crate")
    for name in crate_map.outside & set(crate_map.crates):
        raise Malformed(f"{name} is both under [crates] and `outside`")
    for name in crate_map.level_4_edges - set(crate_map.crates):
        raise Malformed(f"`level-4-edges` names {name}, which is not under [crates]")
    for name, library in crate_map.libraries.items():
        if library.owner is not None and library.owner not in crate_map.crates:
            raise Malformed(f"[libraries] {name}: its owner, {library.owner}, is not under [crates]")


def table_at(value, where, required=(), optional=()):
    value = mapping_at(value, where)
    unknown = sorted(set(value) - set(required) - set(optional))
    if unknown:
        raise Malformed(f"{where}: unknown keys {', '.join(unknown)}")
    missing = [key for key in required if key not in value]
    if missing:
        raise Malformed(f"{where} lacks {', '.join(missing)}")
    return value


def mapping_at(value, where):
    if not isinstance(value, dict):
        raise Malformed(f"{where} is not a table")
    return value


def level_at(value, where):
    if type(value) is not int or value not in LEVELS:
        raise Malformed(f"{where} is a level, 0 to 4")
    return value


def text_at(value, where):
    if not isinstance(value, str) or not value.strip():
        raise Malformed(f"{where} is a text, not empty")
    return value


def texts_at(value, where):
    if not isinstance(value, list):
        raise Malformed(f"{where} is a list")
    for item in value:
        text_at(item, f"{where}'s items")
    if len(set(value)) != len(value):
        raise Malformed(f"{where} names one entry twice")
    return value


# Reading cargo's metadata.


@dataclass
class WorkspaceCrate:
    """A crate of the workspace, as cargo gives it."""

    name: str
    # From the repository's root, without a final slash: `crates/yuna`.
    directory: str
    # The library target's root, none for a crate with no library.
    library_root: str | None
    # (name, kind) of each dependency: kind `normal`, `build` or `dev`.
    dependencies: list

    def holds_in_src(self, path):
        return path.startswith(f"{self.directory}/src/")


def workspace_crates(metadata):
    """The crates of `cargo metadata`'s output, by name."""
    root = Path(metadata["workspace_root"])
    members = set(metadata["workspace_members"])
    crates = {}
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        library = next((t["src_path"] for t in package["targets"] if LIBRARY_KINDS & set(t["kind"])), None)
        crates[package["name"]] = WorkspaceCrate(
            name=package["name"],
            directory=repository_path(str(Path(package["manifest_path"]).parent), root),
            library_root=repository_path(library, root) if library else None,
            dependencies=[(d["name"], d["kind"] or "normal") for d in package["dependencies"]],
        )
    return crates


def crate_of(path, crates):
    """The workspace's crate whose directory holds `path`, none when none does."""
    return next((c for c in crates.values() if path.startswith(f"{c.directory}/")), None)


# The checks.


def check(map_text, metadata, sources, clippy_lines):
    """Every problem found between the map file's text and the code: cargo's metadata, every
    Rust file of the workspace's crates as {path: text}, and clippy's JSON lines, none when
    clippy could not run, which leaves the counts of `unsafe` unchecked. Empty when they agree."""
    try:
        crate_map = read_map(map_text)
    except Malformed as error:
        return [str(error)]
    crates = workspace_crates(metadata)
    codes = {path: code_only(text) for path, text in sources.items()}
    problems = (
        unmapped_crate_problems(crate_map, crates)
        + edge_problems(crate_map, crates)
        + library_problems(crate_map, crates)
        + unsafe_attribute_problems(crate_map, crates, codes)
        + global_problems(crate_map, crates, codes)
        + smithay_problems(crate_map, crates, codes)
    )
    if clippy_lines is not None:
        counts, reading_problems = unsafe_counts(clippy_lines, Path(metadata["workspace_root"]))
        problems += reading_problems + unsafe_count_problems(crate_map, crates, codes, counts)
    return problems


def unmapped_crate_problems(crate_map, crates):
    return [
        f"the crate {name} is in the workspace but not in {MAP_FILE}: add it under [crates] with "
        "its level (ADR-0151), or to `outside`"
        for name in sorted(crates)
        if name not in crate_map.crates and name not in crate_map.outside
    ]


def edge_problems(crate_map, crates):
    problems = []
    for crate in crates.values():
        for dependency, kind in crate.dependencies:
            if dependency in crates:
                problem = edge_problem(crate_map, crate.name, dependency, kind)
                if problem:
                    problems.append(problem)
    return problems


def edge_problem(crate_map, name, dependency, kind):
    """What is wrong with `name` depending on `dependency`, two crates of the workspace, by a
    dependency of `kind`; none when the map allows it, or when a crate is in no list, which
    `unmapped_crate_problems` reports."""
    if name in crate_map.outside:
        return (
            f"{name}, outside the map, depends on {dependency} by a {kind} dependency: a crate "
            "outside the map depends on no crate of Yuna's, by any kind (ADR-0151)"
        )
    if dependency in crate_map.outside:
        if kind == "dev":
            return None
        return (
            f"{name} depends on {dependency}, a crate outside the map, by a {kind} dependency: the "
            "map's crates use it as a development dependency alone (ADR-0151)"
        )
    if kind == "dev" or name not in crate_map.crates or dependency not in crate_map.crates:
        return None
    crate, other = crate_map.crates[name], crate_map.crates[dependency]
    if allowed_edge(crate_map, crate, other):
        return None
    by = "" if kind == "normal" else f" by a {kind} dependency"
    return (
        f"{crate.described} depends on {other.described}{by}, which the map does not allow: "
        f"{edge_rule(crate_map, crate.checked_level)} (ADR-0151)"
    )


def allowed_edge(crate_map, crate, other):
    level, other_level = crate.checked_level, other.checked_level
    if level == 0:
        return False
    if level == 1:
        return other_level == 0 or (other_level == 1 and other.name in crate.level_1_edges)
    if level == 4:
        return other_level == 3 or other.name in crate_map.level_4_edges
    return other_level < level


def edge_rule(crate_map, level):
    return [
        "level 0 depends on no crate of Yuna's",
        "level 1 depends on level 0, and on a level-1 crate only along an edge its `level-1-edges` names",
        "level 2 depends on levels 0 and 1",
        "level 3 depends on levels 0 to 2",
        f"level 4 depends on level 3 and on {', '.join(sorted(crate_map.level_4_edges)) or 'nothing else'}",
    ][level]


def library_problems(crate_map, crates):
    problems = []
    for crate in crates.values():
        mapped = crate_map.crates.get(crate.name)
        if mapped is None:
            continue
        for library, kind in crate.dependencies:
            if kind == "normal" and library not in crates:
                problem = library_problem(crate_map, mapped, library)
                if problem:
                    problems.append(problem)
    return problems


def library_problem(crate_map, crate, name):
    library = crate_map.libraries.get(name)
    if library is None:
        return (
            f"{crate.name} depends on {name}, an outside library {MAP_FILE} does not list: a new "
            "library goes to the level-2 crate that owns it, with its entry under [libraries] (ADR-0151)"
        )
    if library.owner is not None and library.owner != crate.name:
        return (
            f"{crate.name} depends on {name}, which {library.owner} owns: a library has one owner; "
            f"reach what it does through {library.owner} (ADR-0151)"
        )
    if library.from_level is not None and crate.checked_level < library.from_level:
        return (
            f"{crate.described} depends on {name}, allowed from level {library.from_level}: move "
            f"the code that needs it to a crate of level {library.from_level} or above (ADR-0151)"
        )
    return None


def unsafe_attribute_problems(crate_map, crates, codes):
    """The rules on attributes naming `unsafe_code`, and the `forbid` at each pure crate's root."""
    pure = [
        crate
        for crate in crates.values()
        if crate.name in crate_map.crates and crate_map.crates[crate.name].level in PURE_LEVELS
    ]
    pure_roots = {crate.library_root for crate in pure}
    problems = []
    for path, code in sorted(codes.items()):
        for name, of_file, line in unsafe_code_attributes(code):
            if name == "expect":
                if path not in crate_map.unsafe_code:
                    problems.append(
                        f"{path}:{line} expects `unsafe_code`, but {MAP_FILE} has no entry for it: "
                        "list it under [unsafe-code] with its count (in `yuna`, once an "
                        "ADR names it, ADR-0151), or remove the `expect`"
                    )
            elif not (name == "forbid" and of_file and path in pure_roots):
                problems.append(
                    f"{path}:{line}: `{name}` names `unsafe_code`: only a module's `#![expect(unsafe_code, "
                    "reason = \"…\")]`, with its entry under [unsafe-code], and the `#![forbid(unsafe_code)]` "
                    "of a pure crate's root may (ADR-0151)"
                )
    for crate in sorted(pure, key=lambda crate: crate.name):
        if not forbids_unsafe_code(codes.get(crate.library_root, "")):
            problems.append(
                f"{crate.library_root or crate.directory}, the root of {crate.name}, a pure crate, "
                "lacks `#![forbid(unsafe_code)]` (ADR-0151)"
            )
    return problems


def unsafe_count_problems(crate_map, crates, codes, counts):
    """The rules on where `unsafe` code is and how much, from rustc's counts."""
    problems = []
    for path, found in sorted(counts.items()):
        if path not in crate_map.unsafe_code:
            problems.append(
                f"{path} holds {found} `unsafe` without an entry under [unsafe-code] in {MAP_FILE}: "
                "remove the code, or list the module with its count (in `yuna`, once an "
                "ADR names it, ADR-0151)"
            )
        if not expects_unsafe_code(codes.get(path, "")):
            problems.append(
                f"{path} holds {found} `unsafe` without its own `expect(unsafe_code, reason = \"…\")`: "
                "add it at the module's top, or on the item in a crate's root"
            )
    for path, count in sorted(crate_map.unsafe_code.items()):
        crate = crate_of(path, crates)
        mapped = crate_map.crates.get(crate.name) if crate else None
        found = counts.get(path, 0)
        if mapped is not None and crate.holds_in_src(path) and not mapped.unsafe_code:
            problems.append(
                f'the entry [unsafe-code] "{path}" is in the `src/` of {crate.name}, which may not '
                "hold `unsafe` code there (ADR-0151): remove the code and its entry"
            )
        if found == 0:
            problems.append(
                f'the entry [unsafe-code] "{path}" matches nothing: rustc finds no `unsafe` in that '
                "file; remove the entry"
            )
        elif found > count:
            problems.append(
                f"{path} holds {found} `unsafe`, its entry {count}: a module's `unsafe` grows only by "
                "an ADR that names it (ADR-0151); then raise its count"
            )
        elif found < count:
            problems.append(
                f"{path} holds {found} `unsafe`, fewer than its entry's {count}: lower the entry to "
                f"{found}, so that the list stays exact"
            )
    return problems


def global_problems(crate_map, crates, codes):
    """Every static of the `src/` of the map's crates against the list of globals."""
    scanned = [crate for crate in crates.values() if crate.name in crate_map.crates]
    found, problems = set(), []
    for path, code in sorted(codes.items()):
        if not any(crate.holds_in_src(path) for crate in scanned):
            continue
        statics = statics_in(code)
        problems += shared_name_problems(path, statics)
        for name, line, mutable in statics:
            declared = f"static mut {name}" if mutable else f"static {name}"
            if name.startswith("$"):
                problems.append(
                    f"{path}:{line}: `{declared}` is named by a macro's argument, which no entry of {MAP_FILE} "
                    "can list, since it would cover every call: declare the static by name in the macro's body"
                )
                continue
            found.add((path, name))
            if (path, name) not in crate_map.globals:
                problems.append(
                    f"{path}:{line}: `{declared}` is a global {MAP_FILE} does not list: pass the state "
                    f'down instead; a global of one of ADR-0151\'s kinds is listed under [globals."{path}"], '
                    "in `yuna` once an ADR names it"
                )
    for path, name in sorted(crate_map.globals - found):
        problems.append(
            f'the global {name} of [globals."{path}"] matches nothing: no static of that name is in '
            "that file; remove the entry"
        )
    return problems


def shared_name_problems(path, statics):
    """The names that several statics of one file share: the map lists a global by file and
    name, so one entry would pass them all. A name of a macro's argument, `$name`, is left out:
    `global_problems` refuses each such static."""
    lines = {}
    for name, line, _ in statics:
        if not name.startswith("$"):
            lines.setdefault(name, []).append(str(line))
    return [
        f"two statics named {name} in {path} (lines {', '.join(at)}): rename one, so that each has "
        f"its own entry in {MAP_FILE}"
        for name, at in sorted(lines.items())
        if len(at) > 1
    ]


def smithay_problems(crate_map, crates, codes):
    """The files of the transitional crates' `src/` that name Smithay against their list."""
    transitional = [
        crate
        for crate in crates.values()
        if crate.name in crate_map.crates and crate_map.crates[crate.name].transitional_level is not None
    ]
    listed, naming, problems = set(crate_map.smithay_files), set(), []
    for path, code in sorted(codes.items()):
        line = smithay_line(code) if any(crate.holds_in_src(path) for crate in transitional) else None
        if line is None:
            continue
        naming.add(path)
        if path not in listed:
            problems.append(
                f"{path}:{line} names Smithay without its entry under [smithay] in {MAP_FILE}: no "
                "new module is built on Smithay's types (ADR-0151)"
            )
    for path in sorted(listed - naming):
        problems.append(f"{path} is listed under [smithay] but no longer names Smithay: remove it")
    return problems


# Running it on the repository.


def rust_sources(crates, root):
    """Every Rust file of the workspace's crates, by path from the repository's root, and the
    problems met reading them."""
    sources, problems = {}, []
    for crate in crates.values():
        for path in sorted((root / crate.directory).rglob("*.rs")):
            name = path.relative_to(root).as_posix()
            try:
                sources[name] = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as error:
                problems.append(f"{name} cannot be read: {error}")
    return sources, problems


def run_clippy(root):
    """Clippy's JSON lines, none when it failed, and the problems then. Its own errors are
    printed, so that what stopped the build shows."""
    try:
        result = subprocess.run(CLIPPY, cwd=root, stdout=subprocess.PIPE, text=True, check=False)
    except OSError as error:
        return None, [f"cargo clippy could not run: {error}"]
    lines = result.stdout.splitlines()
    if result.returncode == 0:
        return lines, []
    diagnostics, problems = compiler_diagnostics(lines)
    for _, diagnostic in diagnostics:
        if diagnostic.get("level") == "error":
            print(diagnostic.get("rendered", ""), file=sys.stderr)
    return None, problems + [
        f"cargo clippy failed (exit {result.returncode}), so the counts of `unsafe` were not checked: "
        "fix the build first"
    ]


def run(root):
    """Every problem of the repository at `root` and, when there is none, what the map holds."""
    try:
        metadata = json.loads(subprocess.run(METADATA, cwd=root, stdout=subprocess.PIPE, check=True).stdout)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        return [f"cargo metadata gave no metadata: {error}"], None
    try:
        map_text = (root / MAP_FILE).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        return [f"{MAP_FILE} cannot be read: {error}"], None
    sources, problems = rust_sources(workspace_crates(metadata), root)
    clippy_lines, clippy_problems = run_clippy(root)
    problems += check(map_text, metadata, sources, clippy_lines) + clippy_problems
    return problems, None if problems else summary(read_map(map_text), workspace_crates(metadata))


def summary(crate_map, crates):
    mapped = sum(name in crate_map.crates for name in crates)
    return (
        f"{MAP_FILE} matches the code: {mapped} crates of the workspace in the map and "
        f"{len(crates) - mapped} outside it; {len(crate_map.unsafe_code)} modules with `unsafe` code, "
        f"{sum(crate_map.unsafe_code.values())} `unsafe` in all; {len(crate_map.globals)} globals; "
        f"{len(crate_map.smithay_files)} files naming Smithay"
    )


def main():
    if sys.argv[1:]:
        raise SystemExit("usage: scripts/crate-map.py")
    problems, held = run(ROOT)
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        print(f"\n{len(problems)} problems between {MAP_FILE} and the code (ADR-0151)", file=sys.stderr)
        sys.exit(1)
    print(held)


if __name__ == "__main__":
    main()
