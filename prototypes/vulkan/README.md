# The Vulkan prototype

A throwaway program that answers, on the machine it runs on, the questions of
[ADR-0073](../../docs/adr/0073-the-vulkan-prototype-answers-only-what-no-rival-measures.md):
which client buffers Vulkan imports, how it synchronises with them, how a `wl_shm` buffer reaches
the GPU, what a composition costs at the least, and whether a high-priority GPU queue is granted.
Its `scene` command draws the scene the processor tier is measured on (ADR-0139), below. It
lives on the branch `prototype/vulkan`, which is never merged, and is deleted when Stage 2's
renderer lands.

It runs headless. It opens no window, no display and no surface: only the GPU's render node,
through Vulkan. It changes no setting and needs no privilege. It writes one file, its JSON report,
and for `scene --capture` an image.

## Running it on the laptop or the Steam Deck

It needs Nix with flakes; add `--extra-experimental-features 'nix-command flakes'` to each `nix`
command if they are not on. The first run fetches the Rust toolchain and compiles the prototype,
in about 7 seconds on the desktop. The build needs at most 1.2 GiB of store paths unpacked, most
of it the toolchain; a machine holds part of them already. `nix-collect-garbage` frees them
afterwards.

The commands name the branch `github:HoneySlatt/Yuna/prototype/vulkan`. The same branch is also
`'github:HoneySlatt/Yuna?ref=prototype/vulkan#vulkan-prototype'`, for a Nix that would read the
branch's `/` as part of a path.

Close games and builds first if you can. The report marks each timed part that other programs
disturbed (part 7); such a part's figures are worth measuring again.

### The HP laptop (NixOS)

```sh
# The panel's own size: the first mode listed, such as 1920x1080.
head -1 /sys/class/drm/card*-eDP-*/modes

# The Vulkan devices: the AMD GPU (RADV) should be device 0.
nix run 'github:HoneySlatt/Yuna/prototype/vulkan#vulkan-prototype' -- devices

# The report, at the panel's size (replace 1920x1080 with what the first command printed).
nix run 'github:HoneySlatt/Yuna/prototype/vulkan#vulkan-prototype' -- \
  report --device 0 --size 1920x1080 --json laptop.json > laptop.md
```

### The Steam Deck (NixOS)

Its panel is 1280x800. In a terminal of its desktop session:

```sh
nix run 'github:HoneySlatt/Yuna/prototype/vulkan#vulkan-prototype' -- devices
nix run 'github:HoneySlatt/Yuna/prototype/vulkan#vulkan-prototype' -- \
  report --device 0 --size 1280x800 --json deck.json > deck.md
```

### If `devices` lists no AMD GPU

The system's driver did not load into a program Nix built. On either machine, run the report on
the flake's own Mesa instead; part 1 of the report then names that Mesa's version. Fetch it once:

```sh
mesa=$(nix build --no-link --print-out-paths \
  --inputs-from 'github:HoneySlatt/Yuna/prototype/vulkan' nixpkgs#mesa)
```

Then, on the laptop (at its panel's size):

```sh
VK_DRIVER_FILES=$mesa/share/vulkan/icd.d/radeon_icd.x86_64.json \
  nix run 'github:HoneySlatt/Yuna/prototype/vulkan#vulkan-prototype' -- \
  report --device 0 --size 1920x1080 --json laptop.json > laptop.md
```

On the Steam Deck:

```sh
VK_DRIVER_FILES=$mesa/share/vulkan/icd.d/radeon_icd.x86_64.json \
  nix run 'github:HoneySlatt/Yuna/prototype/vulkan#vulkan-prototype' -- \
  report --device 0 --size 1280x800 --json deck.json > deck.md
```

### What the run takes, and where the report goes

On the desktop's RX 6900 XT, a run takes about 5 seconds at 3840x2160 and about 120 MB of memory
at 1920x1080, besides what the GPU holds; on lavapipe, the CPU driver, about 80 seconds at
3840x2160 on 16 threads. The slower GPUs take longer, mostly in part 5.

- The report in Markdown goes to standard output: `> laptop.md` keeps it.
- The same report in JSON goes to the file `--json` names, `vulkan-prototype.json` by default,
  written whole or not at all. Its `"format"` is 2.
- Progress goes to standard error.
- The JSON file is written first, then the Markdown printed.
- The exit code is 0 when the report was written, whatever it found; 1 when it could not run or
  write (no Vulkan, no such device, a file or a standard output it could not write); 2 for a
  wrong argument; 3 when `--validate` found a problem.

Send both files back; the figures go to `docs/measurements.md` with the commit the report names.

## What the report holds

1. **The driver**: the device, the driver and Mesa's version, Vulkan's version, the PCI ids, the
   render node, and whether each extension Stage 2's renderer would lean on is offered.
2. **Formats and modifiers**: for `B8G8R8A8_UNORM`, `A2B10G10R10_UNORM_PACK32` and
   `R16G16B16A16_SFLOAT`, every modifier the driver lists, its planes and features, the largest
   image it allows, and how far a 1920x1080 image with it went. The image is drawn on one device,
   exported as a dma-buf, imported into a second device with its modifier and planes, and sampled
   there; every texel is checked against what was drawn. The furthest stage reached is offered,
   created, exported, imported or sampled-correctly; a failure names its step and Vulkan's answer.
3. **Explicit sync**: a semaphore and a fence, each signalled behind work that keeps the client's
   GPU busy, exported as sync files, imported into the second device and waited on there. The
   report says whether the sync file was still pending when imported, and checks that it had
   signalled once the second device's wait ended. Then a timeline semaphore signalled and
   waited, a wait submitted before its signal.
4. **`wl_shm`'s path**: a client's memfd at 1920x1080 and 3840x2160 uploaded 200 times, through a
   staging buffer and, when offered, through its pages imported with
   `VK_EXT_external_memory_host`; CPU, GPU and wall time; the image checked against the client's
   pixels; the memory type and heap the GPU copied from. A pointer or a size not aligned to the
   driver's `minImportedHostPointerAlignment` is a failure, never imported. When the memfd's
   pages cannot be imported, anonymous memory of the same size is tried, to tell which the driver
   refuses.
5. **The composition's floor**: 1, 6 and 15 windows at `--size`, each sampling its own client's
   dma-buf with rounded corners and a soft shadow, into a 16-bit float and an 8-bit output; 1,000
   frames after 100 of warm-up; the CPU time to record and submit, and the GPU time.
6. **Priority**: whether a queue of high and of realtime global priority is granted.
7. **The machine's load** during each timed part, an estimate, and the machine's kernel, CPU and
   memory. Other programs' CPU comes from the kernel's counters in 10 ms ticks; their rounding
   may put it off by up to 0.08 s of CPU time over the part, shown as ± in logical CPUs (0.4
   over a 0.2 s part). Other programs' GPU is `gpu_busy_percent`, the graphics engine's
   activity, read at the part's end and once a second during it (one or two readings for a
   part shorter than a second), less the prototype's own time on the graphics engine
   (`drm-engine-gfx`).

## The processor tier's scene (`scene`)

ADR-0139 judges the processor tier on its own budget, from one scene measured on both of its
paths: today's Yuna, GLES on Mesa's llvmpipe, and Stage 2's Vulkan on lavapipe. `scene` draws
that scene on lavapipe; [`../llvmpipe-scene`](../llvmpipe-scene/README.md), a `wl_shm` client,
has today's Yuna draw it. Both run on the processor alone and never open the GPU.

The scene is defined once, in `src/scene_definition.rs`, which the client includes:

- Yuna's default output, 3840 × 2160 at 240 Hz and scale 1.5, 8 bits a channel, and its
  background colour.
- Two windows where Yuna draws them, in its two columns of half the width: 1884 × 2112 pixels
  at (24, 24) and at (1932, 24). Yuna draws nothing else: no border, no shadow, no title bar.
- Each window is an opaque `XRGB8888` `wl_shm` buffer of that size, its viewport at its logical
  size, so one texel lands on one pixel. Each texel's colour tells where it lies, so a capture
  shows where every texel was drawn.
- **Full damage**: each frame, both windows commit a buffer, damaged whole. **Partial damage**:
  each frame, a 64 × 64 region of window 0 changes and is damaged, alone.

`scene` does what Yuna's renderer, Smithay's GLES, does for the same commits. It uploads what
was damaged, from the clients' memfds, imported once with `VK_EXT_external_memory_host`: one
copy, as `glTexSubImage2D` makes. It then draws what the frame damaged over what the output
holds, without blending, since the windows are opaque. Smithay draws the 64 × 64 region as the
65 × 65 pixels at (922, 1024): it rounds the damage through logical units. `scene` draws the
same rectangle.

Both sides time each frame from its start, uploads included, to its drawing done. On Yuna's
side that is `took_us`, which also spans the frame callbacks it sends, rounded down to whole
microseconds. Here it runs from recording to the fence signalled. Both count the CPU time of the
drawing process's threads over the measured frames, and check the output after the last frame
against the definition, pixel for pixel. A run is quiet when other programs used at most 2
logical CPUs and 10 % of the GPU, on average and in the busiest second, beyond the counters'
rounding (`src/load.rs`).

The two sides do not draw at the same cadence unless told to:

- Yuna's client commits at each frame callback by default. Yuna's frame loop then aims each
  frame at the first boundary after the one its last frame was presented at, and starts it a
  lead earlier, at most one period (`crates/yuna/src/frames.rs`). A full-damage frame a little
  over two periods long (8.4 ms at 240 Hz) therefore comes every second or third boundary, every
  11.2 to 11.6 ms.
- `scene` starts each frame at the first boundary of `--rate` (240 Hz by default) after the frame
  before ended: every second boundary for a 7.8 ms frame.
- `--rate HZ` on both sides paces them alike: each frame starts, or is committed, at the first
  boundary of HZ after the frame before ended. Yuna then draws the commit at its next refresh,
  up to a period later, at a phase fixed for the run, where `scene` starts on the boundary. The
  rate must leave room for that: at 80 Hz an 8.4 ms frame and that delay can miss the next
  12.5 ms boundary, and Yuna's frames then skip one. `--rate 60` gives every fourth boundary on
  both sides.

Each run reports its cadence: the measured window's length over its frames ("ms a frame"), and
the output's periods between two frames ("steps"), from the boundaries Yuna's frames aimed at,
or from the prototype's frame starts.

Run both from Yuna's development shell, on `main`, so that both use the same Mesa, and inside
`agent-headless`; build each first:

```sh
# Here: the prototype; in ../llvmpipe-scene: the client; in Yuna's checkout: yuna, release.
nix develop -c cargo build --release
# From Yuna's checkout:
nix develop -c agent-headless -- bash -c 'VK_ICD_FILENAMES=$YUNA_LAVAPIPE_ICD \
  <prototype>/target/release/vulkan-prototype scene --damage partial --json partial.json'
nix develop -c agent-headless -- <client>/target/release/llvmpipe-scene \
  --yuna <yuna>/target/release/yuna --damage partial --json partial.json --log yuna.log
# Full damage at the same cadence on both sides, every fourth boundary:
nix develop -c agent-headless -- bash -c 'VK_ICD_FILENAMES=$YUNA_LAVAPIPE_ICD \
  <prototype>/target/release/vulkan-prototype scene --damage full --rate 60 --json full.json'
nix develop -c agent-headless -- <client>/target/release/llvmpipe-scene \
  --yuna <yuna>/target/release/yuna --damage full --rate 60 --json full.json --log yuna.log
```

`scene` takes `--damage full|partial` (full by default), `--warm-up N` (300 frames),
`--frames N` (1200 measured), `--rate HZ` (240; 0 draws frames back to back), `--json FILE`
(`scene.json`) and `--capture FILE`, which writes the output after the last frame as a PAM
image. It prints its figures in Markdown. Set `LP_NUM_THREADS` to choose the rasterizer's
threads; Mesa's default is one a logical CPU. Its exit code is 0 when it measured, 1 when it
could not, 2 for a wrong argument.

## For a developer

Everything runs in the repository's Nix shell, which has glslang for the shaders, Vulkan's loader
and Khronos' validation layer:

```sh
cd prototypes/vulkan
nix develop -c cargo build --release
nix develop -c cargo test --release
nix develop -c cargo clippy --all-targets -- -D warnings
nix develop -c target/release/vulkan-prototype report --validate
# lavapipe alone:
nix develop -c bash -c 'VK_ICD_FILENAMES=$YUNA_LAVAPIPE_ICD target/release/vulkan-prototype report'
```

`--validate` turns on `VK_LAYER_KHRONOS_validation`, which the shell provides; outside it the
layer is not found and the run stops. Its CPU times include the layer's own work: take figures
from a run without it.

The code: `src/main.rs` runs the parts in order; `src/gpu.rs` holds the instance, the devices, the
scope that destroys what a test made, and the recorder of commands; `src/dmabuf.rs` exports and
imports a client's buffer; one file a part after that. `src/scene.rs` is the `scene` command;
`src/scene_definition.rs`, `src/scene_report.rs`, `src/threads.rs`, `src/load.rs`,
`src/stats.rs` and `src/json.rs` hold no Vulkan, and `../llvmpipe-scene` includes them, so that
both sides of the scene sum up their figures with the same code. `src/pattern.rs` and
`shaders/pattern.frag` compute the same test pattern and must change together. ash 0.38 is the
only dependency (ADR-0073); the few Linux calls the standard library lacks are declared in
`src/sys.rs`.

## Limits

- The client and the compositor are two `VkDevice`s in one process, not two processes: the kernel
  may share their buffers more directly than it would a real client's.
- A modifier that cannot be drawn into is listed but not tried: the prototype writes its pattern
  by drawing, as a GPU client does.
- Each upload and each frame waits for the GPU before the next, so the figures are each one's own
  cost, not a pipelined compositor's throughput.
- Other programs' load is an estimate (part 7). Their GPU load rests on one or two readings in a
  part shorter than a second, as most of RADV's parts are. A CPU driver such as lavapipe has no
  GPU reading.
- On lavapipe, in the runs so far, the sync files had signalled before the import, so part 3
  checks there only the import of signalled sync files.
- If the GPU never finishes a test's work, the prototype waits 20 seconds at most for it, then
  leaves that test's objects alive, and the device and instance with them, rather than hang.
- AMD modifiers are named from drm_fourcc.h's fields; other vendors' are shown in hexadecimal.
- `scene` follows what Yuna does on its scene, not every case: one output, opaque windows, no
  motion. Its output is `B8G8R8A8_UNORM`; Yuna's GLES draws into `ABGR8888`, the same bytes in
  another order. Its "client" is in its own process, where Yuna's is another: the client's work,
  writing the 64 × 64 region, counts in `scene`'s CPU time, a few microseconds a frame.
