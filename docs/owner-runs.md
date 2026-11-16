# The owner's runs

What M1.9 asks the owner to run by hand, on the owner's own machines, after the owner's go
([ADR-0071](adr/0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md),
[ADR-0073](adr/0073-the-vulkan-prototype-answers-only-what-no-rival-measures.md)). Agents never run
these: they take a display. Each run's files go back to the agent, which puts their figures in
[`measurements.md`](measurements.md).

Every command uses Nix with flakes and the package versions Yuna pins. Close games and builds first
if you can: the reports mark what other programs disturbed.

## 1. The Vulkan prototype, on the laptop and the Steam Deck

Headless: it opens no display and takes about 5 seconds on a GPU. Its README, on its branch, says
what it measures and its fallbacks
([`prototypes/vulkan/README.md`](https://github.com/HoneySlatt/Yuna/blob/prototype/vulkan/prototypes/vulkan/README.md)).

```sh
p='github:HoneySlatt/Yuna/prototype/vulkan#vulkan-prototype'
nix run "$p" -- devices                     # the AMD GPU should be device 0
head -1 /sys/class/drm/card*-eDP-*/modes    # the panel's size, such as 1920x1080
nix run "$p" -- report --device 0 --size 1920x1080 --json prototype.json > prototype.md
```

On the Steam Deck the size is `1280x800`. Bring back `prototype.json` and `prototype.md`.

## 2. The laptop's display, through a Vulkan compositor

Whether the laptop's display engine shows a Vulkan compositor's buffers, with and without
modifiers (ADR-0073). From a virtual terminal where you are logged in and no compositor runs
(Ctrl+Alt+F3, for example, while niri stays on its own):

```sh
mkdir -p ~/yuna-display-check && cd ~/yuna-display-check
nix build --inputs-from github:HoneySlatt/Yuna nixpkgs#drm_info nixpkgs#vulkan-tools \
  nixpkgs#sway nixpkgs#mpv -o tool
# One link per package: tool is drm_info, tool-1 vulkan-tools, tool-2 sway, tool-3 mpv.
tool/bin/drm_info > drm_info.txt
tool-1/bin/vulkaninfo > vulkaninfo.txt
cat > sway.conf <<EOF
exec $PWD/tool-1/bin/vkcube
exec $PWD/tool-3/bin/mpv --loop av://lavfi:testsrc2=size=1920x1080:rate=60
EOF
WLR_RENDERER=vulkan tool-2/bin/sway -d -c sway.conf 2> sway-vulkan.log
WLR_RENDERER=vulkan WLR_DRM_NO_MODIFIERS=1 tool-2/bin/sway -d -c sway.conf 2> sway-vulkan-no-modifiers.log
```

Let each sway run about two minutes, look at the cube and the test pattern, then leave sway with
Super+Shift+E. Note what you saw (smooth, frozen, black, garbled). Bring back the four files and
your notes.

## 3. The tours

One command runs each rival in turn on the real display, the same rounds with the same clients,
and writes what it measured: frames missed on the screen's refresh, input to present, and watts
([ADR-0071](adr/0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md)).
It runs from a virtual terminal of its own, so that it measures no shell and no window of yours.
Your own session stays on its own terminal: the tour reaches nothing of it.

| Machine | Rivals | About |
|---|---|---|
| Desktop | sway, dwl, mango, Umbriel, Hyprland, niri, KWin, Mutter, cosmic-comp, gamescope | 25 min |
| Laptop | dwl, mango, Umbriel, Hyprland, niri, KWin | 16 min |
| Steam Deck | the laptop's and gamescope | 17 min |

Those times are the rounds'. The first `nix run` on a machine also downloads about 4 GB (the
rivals, Qt for KWin, Mesa) and builds Umbriel and the bench, a few minutes more. The tour tells
the machine by itself (`--machine desktop`, `laptop` or `deck` to say it).

### Before

- Close games and builds: the run file marks what other programs disturbed.
- The laptop and the Deck: run on battery, charged above half, so that the battery gives the whole
  machine's watts. On mains only the GPU's watts are read, which a laptop's APU may not give.
- The Deck: plug a keyboard in, for the virtual terminal.
- Every GPU's performance level must be `auto`:

  ```sh
  cat /sys/class/drm/card*/device/power_dpm_force_performance_level
  ```

  If one is not, the tour refuses to start, and prints the command that sets it and the one that
  puts yours back after. It never writes it itself.

### Run it

Switch to a spare virtual terminal (Ctrl+Alt+F3, for example) and log in. No compositor may run
in that session: the tour refuses to start if `WAYLAND_DISPLAY` or `DISPLAY` is set there. Then,
the same on each machine:

```sh
mkdir -p ~/yuna-tour && cd ~/yuna-tour
nix run 'github:HoneySlatt/Yuna/m1.9#yuna-bench' -- tour
```

It prints the rivals, the time it will take, and waits 5 s: Ctrl+C then stops it before it takes
the screen. Then, for each case, it prints the case, its time, and the most it may keep the screen.

### What you see

For each case the screen goes black a moment, and the rival takes it:

- `rest`: 15 windows of plain colours, still, for a minute.
- `game`: first a dark grey screen for a second, then a moving test pattern, fullscreen.
- `motion`: five windows; the focus moves between them and the workspace switches, eight times.
- `latency`: a dark grey screen whose shade changes faintly at each key the tour types, about 16
  a second, for half a minute. Mutter takes no typed keys: its `latency` shows for a moment and
  fails, saying so. gamescope runs `game` and `latency` alone.

Between cases the terminal comes back for a moment. Leave the keyboard and the mouse alone while a
rival shows: they reach the rival, and would change what it draws.

### If a rival hangs

Each case says the most it may keep the screen: at that time systemd stops it, whatever the tour
does, and the terminal comes back. The tour then goes on with the next rival, its case failed.

If you cannot wait: Ctrl+Alt+F4 still switches terminals while the rival answers. Log in there and
stop the rival's sandbox; the tour waits up to 10 minutes for its terminal to come back:

```sh
systemctl --user kill --signal=KILL 'yuna-bench-*.scope'
```

Then switch back to the tour's terminal. If the screen and the keys are frozen, only the time
limit, or that command from another machine over SSH, stops it.

### What to bring back

The two files the tour writes in `~/yuna-tour`, `tour-<time>.json` and `tour-<time>.md`, with your
notes of anything that looked wrong. When a case failed, the tour prints the folder of its logs,
`/tmp/yuna-bench-<pid>-<time>`: bring it back too,
`tar czf ~/yuna-tour/logs.tgz -C /tmp yuna-bench-<pid>-<time>`.

What agents could not check without a display, and so what a first tour may show: that each rival
takes the screen through the tour's session as their sources say; that KWin and gamescope take
the tour's keys on the real display as they do headless; and that SteamOS gives a login prompt on
a virtual terminal.
