# 0042. Main on Smithay's fork: seats and outputs filtered per client

- Status: Accepted; amends [0021](0021-smithays-fork.md) (main moves to the fork now),
  [0035](0035-agents-seats.md) (the seat filter comes at M1.6); supersedes
  [0027](0027-smithay-without-overflow-checks.md); amended by
  [0044](0044-the-fork-over-https-a-branch-per-base.md) (HTTPS, a branch per base)
- Date: 2026-11-13

## Context

[ADR-0035](0035-agents-seats.md) left a gap until Stage 3: with two seats in one Yuna, a program
of the second may bind the first's `wl_seat`. It kept main on upstream Smithay for the cost of a
fork to rebase and a read key in CI. Yuna and its fork are public since
[ADR-0036](0036-yuna-is-public.md): CI needs no key.

The gap is Smithay's, not wayland-server's ([ADR-0040](0040-the-wire-protocol-stays-wayland-rs.md)):
Smithay hands each global's `can_view` to its data, and the data of `wl_seat` and `wl_output`
keep the default, which shows them to every client. Twenty of Smithay's protocol files already
override it, and Yuna filters `ext-data-control` to the owner's seat that way. Smithay leaves
Yuna by the end of Stage 2 ([ADR-0039](0039-leaving-smithay-step-by-step.md)).

## Decision

The owner chose, on 2026-11-13: **M1.6 opens by moving main onto Yuna's fork of Smithay**, with
two generic patches, each listed in [`smithay-patches.md`](../smithay-patches.md):

- a filter of the `wl_seat` and `wl_output` globals per client, given by Yuna, so that a client
  sees only the seat it belongs to and that seat's outputs;
- saturating sums in a popup positioner's arithmetic, which replace the build setting of
  [ADR-0027](0027-smithay-without-overflow-checks.md).

## Alternatives considered

- **Waiting until Yuna's own core**, at Stage 2: nothing to rebase meanwhile, but the gap stays
  open through M1.6 to M1.10.

## Consequences

- ADR-0035's gap closes, with a story that a client of one seat sees no other seat's `wl_seat` or
  `wl_output`.
- ADR-0027 is superseded once the saturating patch is on main.
- Each raise of Smithay's revision rebases the fork's branch, as ADR-0021 says, until Smithay is
  gone.
