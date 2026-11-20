# llvmpipe-scene

A throwaway `wl_shm` client that has today's Yuna draw the processor tier's scene on Mesa's
llvmpipe, and measures it as the Vulkan prototype's `scene` measures its own drawing on lavapipe
(ADR-0139). It lives on the branch `prototype/vulkan`, which is never merged, and goes with the
prototype. The scene, how both sides measure it and how to run them are in the prototype's
README, [*The processor tier's scene*](../vulkan/README.md#the-processor-tiers-scene-scene).

What it does, in order:

1. Starts `yuna --headless` from the path `--yuna` gives, on llvmpipe and its default output,
   logging at `debug` to the file `--log` names, with nothing of its own environment but the Nix
   shell's EGL and Mesa, the keyboard's data and `LP_NUM_THREADS` (`src/yuna.rs`).
2. Opens the scene's two windows, one after the other, and checks that Yuna configures them at
   the scene's sizes and scale (`src/client.rs`).
3. Commits a frame at each of window 0's frame callbacks, or with `--rate HZ` at the first
   boundary of HZ after it: both windows whole, or window 0's 64 × 64 region. Over the measured
   frames, it reads each frame's `took_us` from Yuna's log and the boundary it aimed at, the CPU
   time of Yuna's threads, and other programs' load, Yuna's and its own processor time apart.
4. Captures the output with `ext-image-copy-capture-v1` and checks it against the scene's
   definition, pixel for pixel; `--capture` writes it as a PAM image.

It includes the prototype's `scene_definition.rs`, `scene_report.rs`, `threads.rs`, `load.rs`,
`stats.rs` and `json.rs`, so that both sides use one definition and one way of summing up.
Its only dependencies are `wayland-client` and `wayland-protocols`, at the versions of Yuna's
`Cargo.lock`, of which its own is a copy.

```sh
nix develop -c cargo build --release --offline
nix develop -c cargo clippy --release --offline --all-targets -- -D warnings
nix develop -c cargo test --release --offline
```

Its exit code is 0 when it measured, 1 when it could not, 2 for a wrong argument.
