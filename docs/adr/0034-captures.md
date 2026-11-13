# 0034. Captures: PNG at the logical size

- Status: Accepted
- Date: 2026-11-13

## Context

An agent checks what it did by capture ([ADR-0015](0015-agents-workspaces.md)). MCP returns an
image as base64 PNG, JPEG, GIF or WebP. Claude's API refuses an image over 5 MB and scales one
whose long edge passes 2,576 pixels; a model answers in the coordinates of the image it sees
([survey](../survey.md#agents-and-mcp)). A PNG of a 4K output takes 10 to 46 ms to encode on one
core (estimated from the fastest encoders' figures), about three to eleven frames at 240 Hz.

## Decision

The owner chose, on 2026-11-13:

- A capture is a compressed **PNG**, written with the `png` crate.
- By default at the output's **logical size**, 2,560 × 1,440 for the default 4K output at scale
  1.5: a pixel of the image is a unit where the agent clicks. A region, or the physical size, is
  asked for.
- The pixels are copied on the main thread when the frame they show is drawn, and delivered at
  its boundary ([ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md)); the encoding runs on a
  thread of its own, so that no frame waits for it. One capture of a seat is in flight at a time.

## Alternatives considered

- **An uncompressed PNG written by Yuna**: no dependency, but 14.7 MB at 2,560 × 1,440, over the
  5 MB limit; scaled down, a click would land off.
- **Raw pixels in a file**: fast and without dependency, but the model sees nothing.

## Consequences

- Dependency: `png` (image-rs, MIT OR Apache-2.0), with `crc32fast`, `flate2`, `miniz_oxide`,
  `fdeflate`, `adler2`, `simd-adler32`. Base64 is Yuna's own, a few lines.
- A capture's size is checked against the 5 MB limit before it is sent.
