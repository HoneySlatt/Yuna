# 0021. Smithay's fork: a private repository, generic patches

- Status: Accepted; amends [0017](0017-smithays-revision.md) (when main moves to the fork); amended
  by [0036](0036-yuna-is-public.md) (the fork is public), [0042](0042-main-on-smithays-fork.md)
  (main moves to the fork at M1.6), [0044](0044-the-fork-over-https-a-branch-per-base.md) (HTTPS, a
  branch per base)
- Date: 2026-11-11

## Context

[ADR-0011](0011-built-on-smithay.md) keeps what Smithay lacks in a fork of Smithay under Yuna's
control, and [ADR-0017](0017-smithays-revision.md) moves the dependency to that fork with its first
patch. M1.3 needed the first one: at `8fd9fe0`, Smithay's atomic DRM surface sets only `CRTC_ID` on
its connectors, so a compositor cannot send `max bpc`, `Colorspace` or `HDR_OUTPUT_METADATA`, which
HDR output needs. The same code already sends `VRR_ENABLED` on the CRTC: a value kept in the
surface's state, sent with every modeset, whose change makes a commit pending.

GitHub's own forks of a public repository are public, and Yuna is private
([ADR-0009](0009-gpl-private-until-ready.md)). Smithay asks that AI-written code be flagged, which
the owner's rules forbid; Yuna never contributes upstream.

## Decision

- The fork is the private repository `HoneySlatt/smithay`: Smithay's history up to the revision
  Yuna pins, then the branch `yuna`, its default, with one commit per patch. GitHub Actions are off
  there: Yuna's CI checks what Yuna uses.
- A patch is as generic as it can be and leaves the meaning to Yuna. The first one adds
  `DrmSurface::set_connector_property(connector, name, raw value)`: Smithay sends the value with
  every modeset and knows nothing of HDR; Yuna builds the metadata blob and chooses the values, so
  the colour pipeline stays in Yuna ([ADR-0012](0012-opengl-first.md)).
- Each patch is listed with its reason in [`smithay-patches.md`](../smithay-patches.md).
- A branch of Yuna that needs a patch depends on the fork by its SSH URL and an exact commit, and
  fetches it with git's own client (`net.git-fetch-with-cli`), which has the owner's credentials.
  Main moves to the fork when code on main first needs a patch; CI then needs a read-only key to
  the fork, decided at that moment.

## Alternatives considered

- **A public GitHub fork**: CI reads it without a key, but it shows Yuna's direction, and
  unflagged AI-written code, in Smithay's network of forks.
- **A local repository only**: nothing to set up on GitHub, but the patches have no copy elsewhere,
  and Yuna builds on one machine only.
- **A patch that knows HDR** (`set_hdr_output(metadata, colorspace, bits)`): a clearer call, but a
  larger fork, and colour decisions inside Smithay.
- **No patch, a commit beside Smithay**: Yuna sets the properties in its own atomic commit and the
  kernel keeps them; no fork, but Smithay's tests and its choice between a modeset and a page flip
  ignore them, and it answers nothing about Smithay with Yuna's patches.

## Consequences

- Building a branch that uses the fork needs access to `HoneySlatt/smithay`.
- Raising Smithay's revision (ADR-0017) now also means rebasing the branch `yuna` and checking each
  patch again.
- The patch's value is raw: Yuna reads each property's type (enum, range, blob) from the kernel to
  build it. A blob it names stays Yuna's to keep alive until a commit replaced it.
