- An X11 window whose size hints ask a minimum larger than every output of its seat, up to X11's
  32,767 pixels, is given no more than the seat's largest output on each axis, and opens, rather
  than being configured to a size nothing shows.
- One Xwayland shows at most 256 menus and tooltips at once, all its X11 connections together:
  one program opening many connections no longer passes the 64 of each (ADR-0070).
