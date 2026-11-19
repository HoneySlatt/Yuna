# The owner's runs

What the owner runs by hand, on the owner's own machines, after the owner's go
([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md),
[ADR-0106](adr/0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md)). Agents never run
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

At Stage 2's exit, with Yuna among the rivals: one command that runs every rival on the real
display in turn ([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)).
Its steps are written here once the bench can run it.
