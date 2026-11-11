# Smithay's patches

The changes Yuna's fork of Smithay makes, each with its reason. The fork is the private repository
`HoneySlatt/smithay`, branch `yuna`, one commit per patch on top of the upstream revision Yuna pins
([ADR-0021](adr/0021-smithays-fork.md)). Yuna never contributes them upstream
([ADR-0011](adr/0011-built-on-smithay.md)).

Base: upstream `8fd9fe0c74e5c13a1fc9b6797d3960c8381bda55` ([ADR-0017](adr/0017-smithays-revision.md)).

Main does not depend on the fork yet: only the HDR prototype's branch, `m1.3-hdr-prototype`, uses it.

| # | Commit | What | Why | Used by |
|---|---|---|---|---|
| 1 | `69edf122` | `DrmSurface::set_connector_property(connector, name, raw value)`: an atomic surface keeps the value in its pending state, tests it with a modeset, sends it with every later test and modeset commit, never with page flips; changing it makes `commit_pending` true. Legacy DRM refuses it. Two unit tests. | Smithay set only `CRTC_ID` on connectors; HDR needs `max bpc`, `Colorspace` and `HDR_OUTPUT_METADATA` (M1.3). | The HDR prototype (M1.3) |

## When Smithay's revision is raised

Rebase the branch `yuna` on the new revision, run Smithay's tests with the features Yuna uses, then
Yuna's checks, and update the base and the commits above.
