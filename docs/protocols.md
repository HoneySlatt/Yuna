# Wayland protocols

Every protocol Yuna offers, plans or has not decided yet, and who needs it
([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)). By Stage 2's end, Yuna
offers every protocol niri or KWin offers that an application the owner uses needs; one it
refuses will say why here. A test, `crates/yuna/tests/headless/protocols.rs`, compares the rows
`Offered` with the globals a client of the owner's seat and one of an agent's seat are shown in
`yuna --headless`: a global added or removed without its row fails it.

- **Global**: the interfaces a client binds, for the protocols Yuna offers.
- **Yuna**: `Offered` and to whom, with its version where clients depend on it; or
  `Planned` and the stage ([ROADMAP](ROADMAP.md)); or `Not decided`, which Stage 2's opening
  settles for each protocol an application needs. `Offered, every client` reads a client of any
  seat; `the owner's seat alone` and `agents' seats alone` read one kind of seat; `Xwayland
  alone`, a seat's Xwayland.
- **Who needs it**: the clients read in their sources ([survey](survey.md#wayland-protocols-the-rivals-and-the-clients))
  and the owner's projects ([client needs](client-needs.md)).
- **niri, KWin, Hyprland**: what each offers, from the [survey](survey.md#wayland-protocols-the-rivals-and-the-clients)
  of 2026-11-14: `Y` offered, `Yn` at version n, `N` not found, `?` unknown, `c` behind an
  option, `*` from wayland.app alone.

| Protocol | Global | Yuna | Who needs it | niri | KWin | Hyprland |
|---|---|---|---|---|---|---|
| wayland | `wl_compositor`, `wl_subcompositor`, `wl_shm`, `wl_seat`, `wl_output`, `wl_data_device_manager` | Offered, every client; a seat's `wl_seat` and `wl_output`s to its own clients ([ADR-0042](adr/0042-main-on-smithays-fork.md)) | Every client | Y | Y | Y |
| linux-dmabuf-v1 | `zwp_linux_dmabuf_v1` | Offered, every client, when the renderer imports a format: v6 with feedback, naming no device on llvmpipe, with its formats of one plane alone, and the render node on the GPU ([ADR-0098](adr/0098-on-llvmpipe-dmabufs-offered-for-no-device-and-checked-as-llvmpipe-maps-them.md)) | Mesa's Vulkan WSI, Aerith | Y | Y5 | Y5 |
| presentation-time | `wp_presentation` | Offered, every client | Mesa's Vulkan WSI, mpv, Aerith, SlimeEngine | Y | Y2 | Y2 |
| tablet-v2 | — | Planned, 2c ([ADR-0061](adr/0061-yunas-uses.md)) | The owner's tablet | Y | Y2 | Y1 |
| viewporter | `wp_viewporter` | Offered, every client | Aerith | Y | Y1 | Y1 |
| xdg-shell | `xdg_wm_base` | Offered, v7, every client | Every application | Y | Y6 | Y7 |
| alpha-modifier-v1 | — | Not decided | — | N | Y1 | Y1 |
| color-management-v1 | `wp_color_manager_v1` | Offered, v3, every client | Mesa's Vulkan WSI, Wine, Chromium, Firefox, mpv, Aerith | N | Y2 | Y3c |
| color-representation-v1 | — | Not decided | Firefox | N | Y1 | N |
| commit-timing-v1 | — | Planned, 2c ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)) | Mesa's Vulkan WSI, SlimeEngine | N | Yc | Yc |
| content-type-v1 | — | Planned, 2c: a hidden game's pace ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)) | Chromium, mpv, Aerith | N | Y1 | Y1 |
| cursor-shape-v1 | `wp_cursor_shape_manager_v1` | Offered, every client | SDL3 | Y | Y2 | Y2 |
| drm-lease-v1 | — | Not decided | Xwayland (VR) | Y | Y1 | Y1 |
| ext-background-effect-v1 | — | Planned, Stage 6 ([client needs](client-needs.md)) | Hollow | Y | Y1 | Y1 |
| ext-data-control-v1 | `ext_data_control_manager_v1` | Offered, the owner's seat alone | Clipboard managers, SlimeEngine | Y | Y1 | Y1 |
| ext-foreign-toplevel-list-v1 | — | Not decided | — | Y1* | N | Y1 |
| ext-idle-notify-v1 | — | Not decided | Chromium | Y | Y2 | Y2 |
| ext-image-capture-source-v1 | `ext_output_image_capture_source_manager_v1` | Offered, outputs alone, every client ([ADR-0038](adr/0038-captures-coordinates-windows-and-wayland.md)) | grim, the portals | N (main: Y) | N | Y |
| ext-image-copy-capture-v1 | `ext_image_copy_capture_manager_v1` | Offered, every client, its own seat's outputs ([ADR-0038](adr/0038-captures-coordinates-windows-and-wayland.md)) | grim, wf-recorder, the portals | N (main: Y) | N | Y1 |
| ext-session-lock-v1 | — | Planned, 2b ([ADR-0086](adr/0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)) | Hollow | Y | N | Y1 |
| ext-transient-seat-v1 | — | Not decided | — | N | N | N |
| ext-workspace-v1 | — | Not decided | — | Y | N | Y1 |
| fifo-v1 | — | Planned, 2c ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)) | Mesa's Vulkan WSI, Aerith, SlimeEngine | N | Y1 | Y1 |
| fractional-scale-v1 | `wp_fractional_scale_manager_v1` | Offered, every client | SDL3, Aerith | Y | Y1 | Y1 |
| linux-drm-syncobj-v1 | — | Planned, 2a, in the queue of commits ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)) | Mesa's Vulkan WSI, Xwayland, Chromium | N | Y1 | Y1 |
| pointer-warp-v1 | — | Planned, 2c ([ADR-0061](adr/0061-yunas-uses.md)) | Wine | N | Y1 | Y1 |
| security-context-v1 | — | Planned, Stage 6: sandboxed programs kept from sensitive protocols | Flatpak | Y | Y1 | Y1 |
| single-pixel-buffer-v1 | — | Not decided | mpv | Y | Y1 | Y1 |
| tearing-control-v1 | — | Planned, Stage 4 | Mesa's Vulkan WSI, Xwayland | N | Y1 | Y1 |
| xdg-activation-v1 | `xdg_activation_v1` | Offered, every client | SDL3, Firefox | Y | Y1 | Y1 |
| xdg-dialog-v1 | `xdg_wm_dialog_v1` | Offered, every client | GTK 4 | N | Y1 | Y1 |
| xdg-system-bell-v1 | — | Planned, 2c: the bell to the shell ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)) | — | N | Y1 | Y1 |
| xdg-toplevel-drag-v1 | — | Not decided | Chromium | N | Y1 | N |
| xdg-toplevel-icon-v1 | — | Not decided | Wine | N | Y1 | N |
| xdg-toplevel-tag-v1 | — | Not decided | — | N | Y1 | Y1 |
| xwayland-shell-v1 | `xwayland_shell_v1` | Offered, Xwayland alone ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)) | Xwayland | N | Y | Y1 |
| fullscreen-shell-unstable-v1 | — | Not decided | — | N | N | N |
| idle-inhibit-unstable-v1 | — | Planned, 2c: inhibition ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)) | SDL3, Chromium, Firefox, mpv | Y | Y1 | Y1 |
| input-method-unstable-v1 | — | Not decided | — | N | Y | N |
| input-timestamps-unstable-v1 | — | Not decided | — | N | N | N |
| keyboard-shortcuts-inhibit-unstable-v1 | — | Planned, 2c ([ADR-0061](adr/0061-yunas-uses.md)) | SDL3, Xwayland | Y | Y1 | Y1 |
| linux-explicit-synchronization-unstable-v1 | — | Not decided: replaced by `linux-drm-syncobj-v1` | — | N | N | N |
| pointer-constraints-unstable-v1 | — | Planned, 2c ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)) | Games, SDL3, Wine, Xwayland, Chromium, Firefox | Y | Y1 | Y1 |
| pointer-gestures-unstable-v1 | — | Not decided | — | Y | Y3 | Y3 |
| primary-selection-unstable-v1 | `zwp_primary_selection_device_manager_v1` | Offered, every client | GTK, Qt, terminals | Y | Y1 | Y1 |
| relative-pointer-unstable-v1 | — | Planned, 2c ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)) | Games, SDL3, Wine, Xwayland, Chromium, Magnolia | Y | Y1 | Y1 |
| text-input-unstable-v3 | `zwp_text_input_manager_v3` | Offered, every client; Yuna is the input method of agents' seats alone ([ADR-0043](adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)) | Wine, SlimeEngine | Y | Y2 | Y1 |
| xdg-decoration-unstable-v1 | `zxdg_decoration_manager_v1` | Offered, every client | Qt, SlimeEngine | Y | Y2 | Y2 |
| xdg-foreign-unstable-v2 | — | Planned, 2c ([ADR-0061](adr/0061-yunas-uses.md)) | Chromium, SlimeEngine, Magnolia | Y | Y | Y |
| xdg-output-unstable-v1 | `zxdg_output_manager_v1` | Offered, v3, every client | uwui's measures | Y | Y3 | Y3 |
| xwayland-keyboard-grab-unstable-v1 | — | Not decided | Xwayland | N | Y | N |
| input-method-unstable-v2 | — | Planned, 2c | Input methods | Y | N | Y1 |
| virtual-keyboard-unstable-v1 | `zwp_virtual_keyboard_manager_v1` | Offered, agents' seats alone ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)) | Pluto's bench, SlimeEngine's tests | Y | N | Y1 |
| wayland-drm | — | Not decided: replaced by `linux-dmabuf-v1` | — | Y2* | N | Y2 |
| wlr-data-control-unstable-v1 | — | Not decided | Wine | Y | N | Y2 |
| wlr-export-dmabuf-unstable-v1 | — | Not decided | — | N | N | N |
| wlr-foreign-toplevel-management-unstable-v1 | — | Not decided | — | Y | N | Y3 |
| wlr-gamma-control-unstable-v1 | — | Not decided: Yuna draws the night light itself ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)) | — | Y | N | Y1 |
| wlr-input-inhibitor-unstable-v1 | — | Not decided: replaced by `ext-session-lock-v1` | — | N | N | N |
| wlr-layer-shell-unstable-v1 | — | Planned, 2b ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)) | Hollow, Amane | Y | Y5 | Y5 |
| wlr-output-management-unstable-v1 | — | Not decided | — | Y | N | Y4 |
| wlr-output-power-management-unstable-v1 | — | Not decided | — | N | N | Y1 |
| wlr-screencopy-unstable-v1 | `zwlr_screencopy_manager_v1` | Offered, v3, every client, its own seat's outputs ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)) | uwui's measures, older grim and wf-recorder | Y | N | Y3 |
| wlr-virtual-pointer-unstable-v1 | `zwlr_virtual_pointer_manager_v1` | Offered, agents' seats alone ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)) | uwui's measures, Pluto's bench, SlimeEngine's tests | Y | N | Y2 |
| xdg-session-management-v1 | — | Not decided | Chromium, Firefox | N | Y | N |
| text-input-unstable-v1 | — | Not decided | — | N | Y | Y1 |
| xdg-foreign-unstable-v1 | — | Not decided: replaced by v2 | — | N | N | N |
