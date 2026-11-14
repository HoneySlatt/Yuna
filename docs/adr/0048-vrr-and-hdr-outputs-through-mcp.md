# 0048. VRR and HDR outputs through MCP, and surfaces' colours read

- Status: Accepted; amends [0033](0033-the-mcp-server.md) (two tools take more)
- Date: 2026-11-14

## Context

An agent tests a program on an output like the owner's monitor
([ADR-0045](0045-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md),
[ADR-0047](0047-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)), and reads what colours the
program's surfaces declared. Aerith's video lies in a subsurface of its window. The `windows`
reply already passes 25,000 tokens past a few hundred windows ([debt](../debt.md#the-mcp-server)).

## Decision

The owner chose, on 2026-11-14:

- `outputs create` and `seat create` take `vrr_min`, in Hz: VRR on over [vrr_min, rate], refused
  unless the rate is at least twice it; and `hdr`, a boolean: the PG32UCDM's colour volume,
  the one monitor Yuna knows. `outputs list` shows both, the volume by the monitor's name.
  `--output` takes the same: `3840x2160@240,1.5,vrr=48,hdr`. Without them an output is SDR at a
  fixed rate, as before.
- `windows` takes `colour: true`, and then gives for each window every surface of its tree that
  declared a description, the window's own, its subsurfaces' and popups': the transfer function,
  primaries, luminances, target volume, max_cll and max_fall, and intent.
  `Compositor::window_colours` gives the same to tests.

## Alternatives considered

- **Named monitor profiles** (`"monitor": "PG32UCDM"`: size, rate, range and volume in one word):
  a list to keep for one monitor today.
- **Colours always in `windows`**: a heavier reply for every agent that does not ask.

## Consequences

- A second monitor's volume, the laptop's, adds a name to `hdr` then.
