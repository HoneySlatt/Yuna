- The activation tokens of clients that have left are bounded, 256 together, the oldest
  forgotten first: a client that connects again and again to ask tokens no longer grows Yuna's
  memory, and a launcher's token still outlives its launcher (ADR-0131).
