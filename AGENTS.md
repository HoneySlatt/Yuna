# AGENTS.md

Rules for AI agents working in this repository. Read this file fully at the start of every session.

## Project

Yuna is a Wayland compositor in Rust, for Linux, in two modes: headless, where agents and tests
drive it, and the desktop, its owner's daily compositor ([ADR-0001](docs/adr/0001-what-yuna-is.md)).
It is built on Smithay ([ADR-0011](docs/adr/0011-built-on-smithay.md)), which it leaves step by
step ([ADR-0039](docs/adr/0039-leaving-smithay-step-by-step.md)), headless first
([ADR-0002](docs/adr/0002-the-headless-mode-first.md)). It is under GPL-3.0-or-later, and public
([ADR-0036](docs/adr/0036-yuna-is-public.md)). Today it has its checks, its layout model, a
headless compositor and its MCP server.

Agents develop it; the owner sets its direction and validates its decisions. Anyone must be able to
understand it from the repository alone. Documentation is part of every deliverable.

## Read before working

- `docs/PLAN.md`, its *Current* section and the current milestone: what to do now.
- When planning, or when the task touches a decision: `docs/ROADMAP.md` (direction, stages, the
  quality bar), `docs/ARCHITECTURE.md`, `docs/adr/`, and `docs/survey.md`, the facts
  behind the decisions.

You remember nothing from earlier sessions. What is not written in these files is lost.

## Scope

- Work on the current milestone of `docs/PLAN.md`, or on what the owner asks. A request that changes
  the plan is written into the PLAN.
- Do not start later stages, nor prepare abstractions for them.
- When experience contradicts the roadmap, propose a change; once accepted, record it in an ADR.

## Commands

Everything runs in the Nix development shell, with the toolchain pinned in `rust-toolchain.toml`.
`CONTRIBUTING.md` gives the detail.

```sh
nix develop -c scripts/check.sh    # every check of the definition of done; what CI runs
nix develop -c cargo run -- --help
```

Never here:

- Never run Yuna on the owner's display, nor take the owner's DRM devices or input: agents run it
  headless only. What needs the owner's monitor (M1.3, the desktop's stages) is run by the owner, by
  hand, from steps written in the milestone.
- Never measure on the owner's desktop without the owner's consent; timed measures run one at a
  time across the owner's projects ([ADR-0003](docs/adr/0003-a-parallel-project-uwui-first.md)).
- `--gpu` opens the GPU's render node, never a display device; an agent runs it only for a
  measure the owner consented to, one at a time, never beside a game, and never in tests.

## Definition of done

- `scripts/check.sh` passes.
- The program was run and behaved as expected.
- New behaviour is tested where a test is practical; a layout behaviour always is.
- The budgets the milestone touches are measured and met; the figures go to
  `docs/measurements.md`.
- `docs/PLAN.md` gives the new status.
- Every document the change affects is updated and reread as a newcomer would.
- Every decision a newcomer would question is an ADR.

Never report as working what was not verified. Say exactly what was not, and why. Never weaken a
check to make it pass (a skipped test, a raised budget, a silenced lint) without the owner's go.

## Architecture rules

- The layout model holds no Wayland or Smithay object; it is tested alone
  ([ADR-0005](docs/adr/0005-the-layouts.md)).
- The colour and HDR pipeline, the layout and the animations do not depend on the renderer; only
  the final drawing is OpenGL ([ADR-0012](docs/adr/0012-opengl-first.md)).
- Everything runs headless; the DRM backend is one backend among others.
- An agent's seat never reaches the owner's seat ([ADR-0015](docs/adr/0015-agents-workspaces.md)).
- One client's error never takes Yuna down; a configuration error never stops it
  ([ADR-0007](docs/adr/0007-surviving-a-crash.md), [ADR-0008](docs/adr/0008-configuration-in-kdl.md)).

## Rules the code depends on

- Yuna writes its own code. niri and jay are read, never copied; jay's code is GPL-3.0-only and
  would change Yuna's licence ([ADR-0013](docs/adr/0013-our-own-code-niri-and-jay-as-references.md)).
- Smithay is pinned to one revision. A patch to it lives in Yuna's fork, listed with its reason in
  `docs/smithay-patches.md`; Yuna never contributes upstream
  ([ADR-0011](docs/adr/0011-built-on-smithay.md)).
- Games and films come first: nothing may cost a game a frame or a film its cadence.
- At rest, nothing runs, polls or draws.

## Code rules

- No abstraction without a second concrete use.
- No reimplementation of the standard library without a measurement.
- A new dependency needs a reason, an alternative weighed and a licence compatible with
  GPL-3.0-or-later; ask the owner.
- Every `unsafe` block has a `// SAFETY:` comment.
- Code reads plainly to someone who has never seen it: names that say what things are, one job per
  function, the obvious solution over the clever one.
- Comments say why, not what. No commented-out code.
- A bug is fixed at its cause, with the test that failed first; look for the same bug elsewhere.
- Edge cases are thought through and tested: empty, huge, malformed, concurrent, interrupted.
- The user's data is never lost: versioned formats, atomic writes, a backup before a destructive
  step.
- Search for an existing helper before writing one. One way to do a thing.
- Delete dead code and unused parameters.
- Report drift from `docs/ARCHITECTURE.md` or an ADR; do not work around it.

## Documentation

- It is written for a newcomer: plain words, short sentences, the essential first, as short as
  complete.
- It describes the code as it is now. Wrong documentation is worse than none.
- No reference to conversations the reader cannot see. Coined terms go in `docs/glossary.md`.
- Limits and debt are stated plainly.
- Each fact has one home; other documents link to it:
  - the code's documentation: contracts, limits, figures;
  - `docs/ARCHITECTURE.md`: flows across parts;
  - ADRs: why a decision was taken, what was rejected; only their status changes later;
  - `docs/survey.md`: facts of the field, with their sources and dates;
  - `docs/PLAN.md`: status; `docs/debt.md`: known debt;
  - `CHANGELOG.md`: what changed for users.
- A change that leaves a document wrong is not finished.

## Commits

- One logical change per commit, in English, its subject in the imperative.
- Never commit code that fails the checks. A check that fails on the main branch is fixed before
  any new work.
- `.delegation/` holds delegated tasks' briefs and proofs; it stays local.

## When to stop and ask

Before a decision no ADR covers, a new dependency, a request against the roadmap or an ADR, a plan
that turns out wrong, or deleting or rewriting a large part of the code. Give the options, their
trade-offs in plain terms and a recommendation for the long term, one decision at a time.

## Reporting

A stage ends with a stage audit, and a release is preceded by a release audit; their findings go to
`docs/audits/`. Ideas outside the current work go to the *Ideas* of `docs/PLAN.md`.

At the end of each milestone, after a reviewing agent on the strongest model has checked it against
this file, the ADRs and the quality bar, report: what was done; the choices, why, and the
alternatives rejected; what to read first; the figures measured; shortcuts and debt; the next step,
on the owner's go. The next milestone starts in a new session, unless the owner says to continue.

## Working with several agents

- One milestone per agent, as `docs/PLAN.md` assigns it.
- Shared files (`docs/PLAN.md`, `docs/ARCHITECTURE.md`, the build manifests): one agent at a time.
- Commit your files by their paths, never with `git add -A`.
- Delegate broad searches to subagents; keep their conclusions, not the files they read.

## Language

Everything in the repository is in English.
