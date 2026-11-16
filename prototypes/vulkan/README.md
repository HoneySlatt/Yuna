# The Vulkan prototype

A throwaway program that answers, on the machine it runs on, the questions of
[ADR-0073](../../docs/adr/0073-the-vulkan-prototype-answers-only-what-no-rival-measures.md):
which client buffers Vulkan imports, how it synchronises with them, how a `wl_shm` buffer reaches
the GPU, what a composition costs at the least, and whether a high-priority GPU queue is granted.
It lives on the branch `prototype/vulkan`, which is never merged, and is deleted when Stage 2's
renderer lands.

It runs headless. It opens no window, no display and no surface: only the GPU's render node,
through Vulkan. It changes no setting and needs no privilege. It writes one file, its JSON report.

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
imports a client's buffer; one file a part after that. `src/pattern.rs` and
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
