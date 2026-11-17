# 0085. Outside contributions: issues now, pull requests from 0.1

- Status: Accepted
- Date: 2026-11-17

## Context

Yuna is public under GPL-3.0-or-later ([ADR-0036](0036-yuna-is-public.md)). Its work is done by
agents in the owner's sessions, milestone by milestone, the owner deciding. Nothing told a
visitor whether code from outside is taken, nor how. M1.12 already brings `SECURITY.md`, issue
and pull request templates holding the definition of done, and a newcomer test.

Two facts weigh on it. Stage 2a replaces the core step by step
([ADR-0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md)): a change to that
code now would soon be redone. And the owner's history carries no AI attribution, nor personal
data: a pull request whose commits carry an AI's `Co-Authored-By` cannot enter as it is.

## Decision

The owner chose, on 2026-11-17, at an audit of the architecture:

- **Issues are welcome now**: bugs, protocols an application needs, ideas.
- **Pull requests are taken from 0.1 on**, under a light process, written in
  [`CONTRIBUTING.md`](../../CONTRIBUTING.md):
  - an issue comes first for a change that touches an ADR, a contract
    ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)) or a dependency;
  - a pull request passes `scripts/check.sh` and meets the definition of done, its tests and
    documents with it;
  - a reviewing agent prepares the review; the owner decides;
  - a proposed ADR is numbered `NNNN`, and takes its number when merged;
  - pull requests are squashed, their message written as the repository's are, with no AI
    attribution;
  - while Stage 2a replaces the core, the code it is replacing takes fixes only.
- **Contributions come under GPL-3.0-or-later, as the rest, with no licence agreement.** Each
  contributor keeps the rights to their code; Yuna's licence can then change only with every
  contributor's consent.

## Alternatives considered

- **No code from outside before 0.2**, issues only: no review during the core's rewrite, but the
  door closed through all of Stage 2, perhaps months.
- **Everything taken now**, under the same process: the door open at once, but changes to code
  about to be replaced, reviews amid the decisions that move most, and a process whose templates
  and newcomer test M1.12 has not made yet.
- **A contributor licence agreement**, or a sign-off that assigns rights: the licence could change
  later, but it puts contributors off, and nothing asks for another licence.

## Consequences

- The README and `CONTRIBUTING.md` say so from this decision; M1.12's templates and newcomer test
  follow its process.
- Reviews of outside pull requests fall to the owner, from 0.1.
- A squashed merge loses a contribution's own commits; its author stays named in the pull request
  and in the commit's author field.

## Validation and reconsideration

Evidence at the time: the repository's state (public, no outside pull request so far) and the
plan of Stage 2a. Proposed again if outside pull requests come faster than the owner can review
them, or if none comes after 0.2.
