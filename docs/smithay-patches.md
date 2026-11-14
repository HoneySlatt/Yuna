# Smithay's patches

The changes Yuna's fork of Smithay makes, each with its reason. The fork is the public repository
`HoneySlatt/smithay`; each upstream base has its own branch, one commit per patch on top of it,
never rewritten ([ADR-0021](adr/0021-smithays-fork.md),
[ADR-0044](adr/0044-the-fork-over-https-a-branch-per-base.md)). Yuna never contributes them
upstream ([ADR-0011](adr/0011-built-on-smithay.md)).

Base: upstream `8fd9fe0c74e5c13a1fc9b6797d3960c8381bda55` ([ADR-0017](adr/0017-smithays-revision.md)),
branch `yuna`; main pins its head, `e229954a`, since M1.6
([ADR-0042](adr/0042-main-on-smithays-fork.md)).

| # | Commit | What | Why | Used by |
|---|---|---|---|---|
| 1 | `69edf122`, `f1e7f88c` | `DrmSurface::set_connector_property(connector, name, raw value)`: an atomic surface keeps the value in its pending state, tests it with a modeset, sends it with every later test and modeset commit, never with page flips; changing it makes `commit_pending` true. Legacy DRM refuses it. Two unit tests. Its second commit, a fix found in review, checks the name before allocating the test buffer. | Smithay set only `CRTC_ID` on connectors; HDR needs `max bpc`, `Colorspace` and `HDR_OUTPUT_METADATA` (M1.3). | The HDR prototype (M1.3, deleted); Stage 5. Behind the `backend_drm` feature, which Yuna does not build yet. |
| 2 | `47cef682` | `SeatState::new_wl_seat_with_filter` and `Output::create_global_with_filter`: a closure of the client, kept in the global's data and called by its `can_view`, decides which clients see the global and may bind it. The old constructors show it to every client. | Upstream shows each `wl_seat` and `wl_output` to every client: a program of one agent's seat saw and could bind another seat's ([ADR-0042](adr/0042-main-on-smithays-fork.md)). | `seats.rs`, `output.rs`; tested by Yuna's stories (`a_client_sees_its_own_seat_and_its_outputs_alone`): Smithay has no client to test globals with. |
| 3 | `e229954a` | Saturating sums in `PositionerState`: `get_anchor_point`, `get_geometry`, `get_unconstrained_geometry` and `compute_offsets`. | A client's positioner values near `i32::MAX` overflowed there: a panic in a debug build, a wrap in release. It replaces [ADR-0027](adr/0027-smithay-without-overflow-checks.md)'s build setting. | Every popup; tested by Yuna's `a_positioner_past_i32_max_is_dismissed_not_a_panic`. |

Patch 1 gives the private `build_request` an eighth parameter and allows clippy's
`too_many_arguments` there, as Smithay does on `AtomicDrmSurface::new`; the owner accepted it on
2026-11-12, to keep the patch small.

Its debt, before Yuna calls it: the tests do not cover a value set twice, `reset_state` and page
flips; it accepts `CRTC_ID`, which Smithay sets itself, and the request then differs between debug
and release builds.

## When Smithay's revision is raised

Copy the patches onto a new branch `yuna-<upstream commit's first 8 characters>` from the new
revision, make it the fork's default, run Smithay's tests with the features Yuna uses, then Yuna's
checks, and update the base and the commits above. The old branch stays, so that every commit a
Yuna pinned stays reachable.
