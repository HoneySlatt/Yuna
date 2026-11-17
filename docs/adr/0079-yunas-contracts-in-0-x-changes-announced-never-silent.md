# 0079. Yuna's contracts in 0.x: changes announced, never silent

- Status: Accepted
- Date: 2026-11-17

## Context

Agents already drive Yuna through `yuna mcp`. From M1.10, the owner's other projects drive it too,
without changing their repositories: uwui's `measure.py` and Pluto's bench start `yuna
--headless` as they start a headless sway
([ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)). What they rely on is a
contract, whether written or not:

- the command line: `yuna --headless`, `yuna mcp`, their options (`--output`, `--gpu`,
  `--manual-clock`), the `WAYLAND_DISPLAY=<socket>` line printed at start, the exit codes;
- the environment Yuna reads (`XDG_RUNTIME_DIR`, `YUNA_LOG`) and gives its programs (`YUNA_SEAT`);
- the MCP server: its revisions, its tools' names, arguments and results
  ([ADR-0033](0033-the-mcp-server.md)), whose arguments refuse unknown fields;
- from M1.10, the Nix package's outputs.

Nothing says today whether these may change: the tools' schemas are written in the code, the
version Yuna reports is the workspace's `0.0.0`, and a renamed argument would break a consumer
without a word. [ADR-0074](0074-the-requirements-of-stage-2s-core.md) lists the contracts Stage
2's core owes; this ADR covers those that exist now.

## Decision

The owner chose, on 2026-11-17, at an audit of the architecture:

- **The contracts are the four above**, listed in [`CONTRIBUTING.md`](../../CONTRIBUTING.md).
  Anything else (the logs' text, the internal library API, the tests' helpers) is not one.
- **In 0.x a contract may change, and every change is announced.** A change that breaks a
  consumer comes with a changelog fragment, `changed-…` or `removed-…`
  ([`changelog.d/`](../../changelog.d/README.md)), which says what to do instead, in one line. An
  addition gets an `added-…` fragment.
- **Two tests hold the contracts' text**: one compares `tools/list` with a file kept in the
  tests, one compares `yuna --help`. A change to either is an edit of that file, seen in the
  diff, never an accident.
- **Versions pin both ways.** Yuna reports the workspace's version, raised at each release.
  Consumers pin a revision of Yuna; Yuna's tests pin the revision of each consumer they run, as
  SlimeEngine's job does.
- **Unknown arguments stay refused.** An agent learns of a misspelt argument at once, rather than
  seeing it ignored.

## Alternatives considered

- **No promise before 1.0**: no work, but a break is found only when a consumer fails, and
  nothing makes a change's author see that it breaks one.
- **Stability from now on**, semantic versions, old forms kept a minor version before removal,
  unknown arguments ignored: consumers never break, but an interface a few days old is frozen,
  old forms must be kept, an agent's typos go unseen, and Stage 2's core rewrites part of it
  anyway.

## Consequences

- M1.10 delivers the two tests and the pinned consumers; the list is in `CONTRIBUTING.md` from
  this decision.
- A change to a contract costs a fragment and the edit of a file the tests hold.
- From 1.0, or earlier for a contract the owner names (Hollow's interface, `yuna-ipc` under
  semantic versions, [ADR-0074](0074-the-requirements-of-stage-2s-core.md)), a stronger promise
  is a new decision.

## Validation and reconsideration

Evidence at the time: the code read at the audit (`mcp/tools.rs`, `mcp/protocol.rs`,
`main.rs`) and M1.10's plan. The check is the two tests and the fragments a contract's change
carries. Proposed again when a consumer outside the owner's projects depends on Yuna, or before
1.0.
