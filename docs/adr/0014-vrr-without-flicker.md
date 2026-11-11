# 0014. VRR without flicker

- Status: Accepted
- Date: 2026-11-11

## Context

OLED panels are tuned for one refresh rate; when the rate changes, dark greys change brightness.
TFTCentral and RTINGS measure it on every OLED; RTINGS notes it on the owner's PG32UCDM, whose
firmware (MCM112) has no anti-flicker setting. Under Linux compositors, a desktop at rest falls to
the panel's minimum (48 Hz) and jumps back to 240 Hz on each cursor move: the owner sees the flicker
mainly on the desktop. No compositor limits how fast the rate changes or holds a high floor; KWin
writes that it "can't properly limit" it without new kernel APIs. amdgpu doubles frames itself below
the panel's minimum. 240 is a whole multiple of 24 ([survey](../survey.md#vrr-flicker)).

## Decision

- **The desktop runs at a fixed rate**, 240 Hz on the owner's monitor. VRR is used only for games
  and video.
- **Films play at a constant, exact multiple of their rate**: 24 fps at 240 Hz, 23.976 at
  239.76 Hz, 25 at 225 Hz.
- **Games stay in a high, narrow band**: Yuna shows each frame two or three times to keep the panel
  above a floor (about 160 Hz on the owner's monitor, set in the configuration), and smooths the
  rate's changes from one frame to the next.
- **The cursor never makes the rate jump.**

## Alternatives considered

- **VRR in fullscreen only**, as other compositors: still flickers in menus and loading screens, and
  in fullscreen video.
- **VRR off**: no flicker, but tearing or stutter in games.

## Consequences

- The desktop's flicker is gone by construction.
- The games' band is a first: a prototype measured with a light sensor shows how much flicker it
  removes on the owner's QD-OLED, whose spikes TFTCentral finds at random rates. Its floor and its
  smoothing are set from that measure.
- A film's rate must be known: from a protocol where one exists, otherwise from the cadence of the
  client's frames.
