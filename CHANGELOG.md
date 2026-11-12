# Changelog

What changed for users, newest first. Yuna has no release yet: everything below comes with 0.1
([ROADMAP](docs/ROADMAP.md)).

## Unreleased

- `yuna --headless` runs Yuna without a screen, on virtual outputs drawn offscreen, and prints
  `WAYLAND_DISPLAY=<socket>`; SIGTERM or Ctrl-C stops it and removes its socket.
- `--output WIDTHxHEIGHT@RATE,SCALE` adds a virtual output; the default is one output of
  3840 × 2160 at 240 Hz and scale 1.5.
- Yuna draws on Mesa's llvmpipe by default; `--gpu` draws on the GPU's render node instead.
- `YUNA_LOG` sets the level of its logs: `error`, `warn`, `info`, `debug` or `trace`.
- Clients get `xdg-shell` windows placed in scrolling columns, with server-side decorations and
  no title bar; popups, `xdg-activation`, `xdg-dialog`, `wl_shm` and `linux-dmabuf` buffers,
  viewporter and fractional scale, presentation time, a keyboard, the clipboard, the primary
  selection and `ext-data-control`.
- Alacritty, GTK 4, Qt 6 and Vulkan applications run in it, and it draws nothing while they rest.
