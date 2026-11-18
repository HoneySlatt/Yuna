- A program on a seat's accessibility bus can no longer end that bus for the whole seat. Yuna's
  registry records an application as the connection that embeds it, and refuses with
  `InvalidArgs` an `Embed`, `Unembed` or `DeregisterApplication` that names another; Yuna sends
  no message whose names or path the bus would disconnect it for, so that a node a toolkit
  places at the bus's reserved path, `/org/freedesktop/DBus/Local`, goes unread alone
  (ADR-0094). The bus used to disconnect Yuna, which then ended the bus: every program of the
  seat lost its accessibility tree for the rest of its life.
- Yuna no longer spins at full processor once a seat's accessibility bus has ended, when its
  connection to that bus could be watched only in part, as when file descriptors run out.
