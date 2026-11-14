# 0003. A parallel project, uwui first on any conflict

- Status: Accepted; amended by 0060
- Date: 2026-11-11

## Context

uwui, the owner's UI framework, holds back six projects. The owner already runs several projects in
parallel sessions. Yuna's headless mode would help uwui's own milestones (frames at the display's
rate, VRR that only a person can check today) and Aerith's HDR.

## Decision

Yuna starts now, as a parallel project with sessions of its own. On any conflict of the owner's
attention, of the subscription's quota or of the machine for measures, uwui comes first. Timed
measures of all the owner's projects run one at a time.

## Alternatives considered

- **After SlimeEngine's Stage 6d**: uwui alone meanwhile, but agents gain nothing during it.
- **After uwui's 1.0**: Yuna could draw its own interface parts on uwui, but very late.

## Consequences

- A timed measure of Yuna waits when another project's runs; each measure names the machine's state.
