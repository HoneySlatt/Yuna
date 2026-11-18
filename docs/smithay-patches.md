# Smithay's patches

The changes Yuna's fork of Smithay makes, each with its reason. The fork is the public repository
`HoneySlatt/smithay`; each upstream base has its own branch, one commit per patch on top of it,
never rewritten ([ADR-0021](adr/0021-smithays-fork.md),
[ADR-0045](adr/0045-the-fork-over-https-a-branch-per-base.md)). Yuna never contributes them
upstream ([ADR-0011](adr/0011-built-on-smithay.md)).

Base: upstream `7ff0ede4df2af1ce3500da9ef1ee190d48b49103` ([ADR-0017](adr/0017-smithays-revision.md)),
branch `yuna-7ff0ede4`; main pins its head, `4b54d2f1`, since M1.13. Before it, the branch `yuna`,
on upstream `8fd9fe0c`: main pinned `67ec6ed0` from M1.11, `f819b3f0` from M1.10 and `e229954a`
from M1.6 ([ADR-0042](adr/0042-main-on-smithays-fork.md)); the table gives each patch's commit on
`yuna` after its commit on `yuna-7ff0ede4`.

| # | Commit | What | Why | Used by |
|---|---|---|---|---|
| 1 | `49239e91`, `9ee8e509` (`69edf122`, `f1e7f88c`) | `DrmSurface::set_connector_property(connector, name, raw value)`: an atomic surface keeps the value in its pending state, tests it with a modeset, sends it with every later test and modeset commit, never with page flips; changing it makes `commit_pending` true. Legacy DRM refuses it. Two unit tests. Its second commit, a fix found in review, checks the name before allocating the test buffer. | Smithay set only `CRTC_ID` on connectors; HDR needs `max bpc`, `Colorspace` and `HDR_OUTPUT_METADATA` (M1.3). | The HDR prototype (M1.3, deleted); Stage 5. Behind the `backend_drm` feature, which Yuna does not build yet. |
| 2 | `26add891` (`47cef682`) | `SeatState::new_wl_seat_with_filter`: a closure of the client, kept in the global's data and called by its `can_view`, decides which clients see the global and may bind it. The old constructor shows it to every client. Its `wl_output` half, `Output::create_global_with_filter`, is upstream's since `7ff0ede4`, with the same name and meaning. | Upstream shows each `wl_seat` and `wl_output` to every client: a program of one agent's seat saw and could bind another seat's ([ADR-0042](adr/0042-main-on-smithays-fork.md)). | `seats.rs`, `output.rs`; tested by Yuna's stories (`a_client_sees_its_own_seat_and_its_outputs_alone`): Smithay has no client to test globals with. |
| 3 | `eac14e9a` (`e229954a`) | Saturating sums in `PositionerState`: `get_anchor_point`, `get_geometry`, `get_unconstrained_geometry` and `compute_offsets`. | A client's positioner values near `i32::MAX` overflowed there: a panic in a debug build, a wrap in release. It replaces [ADR-0027](adr/0027-smithay-without-overflow-checks.md)'s build setting. | Every popup; tested by Yuna's `a_positioner_past_i32_max_is_dismissed_not_a_panic`. |
| 4 | `df37ec66` (`57d42361`) | `VirtualPointerDevice::client()`, as `VirtualKeyboardDevice` has. | A `zwlr_virtual_pointer_v1` created without a seat is for its client's seat, which Yuna knows by its client alone; without it, Yuna could not tell which seat's pointer a request moves. | `virtual_input.rs`; tested by Yuna's stories of virtual input (M1.10). |
| 5 | `9beeec6b` (`f819b3f0`) | A virtual keyboard's keymap read with `pread`, its size checked against the file and refused past 1 MiB, never mapped; its `unsafe` block gone. | Smithay mapped the size the client told: a size larger than the file made libxkbcommon read past its end, and the kernel ended the compositor with SIGBUS, every seat with it. | `virtual_input.rs`; tested by Yuna's `a_keymap_told_larger_than_its_file_is_refused`. |
| 6 | `4b54d2f1` (`67ec6ed0`) | When a `wl_data_source` or a primary selection's source that is its seat's selection is destroyed, by its client or with it, Smithay clears the selection, then calls `SelectionHandler::new_selection` with none, as when a client clears it. A data-control source destroyed still clears it silently. | Smithay told no handler: to release the X11 selection of a Wayland copy whose program had quit, Yuna would have to ask Smithay after every dispatch whether the selection was still there. | `selection.rs`, `xwayland/selections.rs`; tested by Yuna's `a_wayland_source_destroyed_or_its_client_gone_releases_the_x11_selection`. |

Patch 1 gives the private `build_request` an eighth parameter and allows clippy's
`too_many_arguments` there, as Smithay does on `AtomicDrmSurface::new`; the owner accepted it on
2026-11-12, to keep the patch small.

Its debt, before Yuna calls it: the tests do not cover a value set twice, `reset_state` and page
flips; it accepts `CRTC_ID`, which Smithay sets itself, and the request then differs between debug
and release builds.

## When Smithay's revision is raised

Copy the patches onto a new branch `yuna-<upstream commit's first 8 characters>` from the new
revision, make it the fork's default, run Smithay's tests with the features Yuna uses, then Yuna's
checks, and update the base and the commits above. A new pin of the fork, a patch added
included, changes the hash `flake.nix` gives Smithay's sources for the package: `nix build .#yuna`
names the new one. The old branch stays, so that every commit a
Yuna pinned stays reachable.
