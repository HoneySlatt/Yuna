# Smithay's patches

The changes Yuna's fork of Smithay makes, each with its reason. The fork is the private repository
`HoneySlatt/smithay`, branch `yuna`, one commit per patch on top of the upstream revision Yuna pins
([ADR-0021](adr/0021-smithays-fork.md)). Yuna never contributes them upstream
([ADR-0011](adr/0011-built-on-smithay.md)).

Base: upstream `8fd9fe0c74e5c13a1fc9b6797d3960c8381bda55` ([ADR-0017](adr/0017-smithays-revision.md)); head of `yuna`: `f1e7f88c`.

Main does not depend on the fork yet. The HDR prototype (M1.3, deleted since) used patch 1 and
showed it drives the owner's monitor in HDR; Stage 5 uses it again.

| # | Commit | What | Why | Used by |
|---|---|---|---|---|
| 1 | `69edf122`, `f1e7f88c` | `DrmSurface::set_connector_property(connector, name, raw value)`: an atomic surface keeps the value in its pending state, tests it with a modeset, sends it with every later test and modeset commit, never with page flips; changing it makes `commit_pending` true. Legacy DRM refuses it. Two unit tests. Its second commit, a fix found in review, checks the name before allocating the test buffer; the next rebase squashes it into the first. | Smithay set only `CRTC_ID` on connectors; HDR needs `max bpc`, `Colorspace` and `HDR_OUTPUT_METADATA` (M1.3). | The HDR prototype (M1.3, deleted); Stage 5 |

The patch gives the private `build_request` an eighth parameter and allows clippy's
`too_many_arguments` there, as Smithay does on `AtomicDrmSurface::new`; the owner accepted it on
2026-11-12, to keep the patch small.

Its debt, before main uses it: the tests do not cover a value set twice, `reset_state` and page
flips; it accepts `CRTC_ID`, which Smithay sets itself, and the request then differs between debug
and release builds.

## When Smithay's revision is raised

Rebase the branch `yuna` on the new revision, run Smithay's tests with the features Yuna uses, then
Yuna's checks, and update the base and the commits above.
