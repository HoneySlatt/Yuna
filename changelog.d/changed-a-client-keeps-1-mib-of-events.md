- A client that stops reading, as a frozen program does, keeps up to 1 MiB of the events Yuna
  sends it before it is ended, where it kept 4 KiB: a few `type`s of 4,096 characters, or a few
  seconds of a fast pointer, rather than one (ADR-0136).
