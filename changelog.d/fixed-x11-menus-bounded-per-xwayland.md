- One Xwayland shows at most 256 menus and tooltips at once, all its X11 connections together:
  one program opening many connections no longer passes the 64 of each. A menu or tooltip past
  the 256 waits, and shows once another of its Xwayland goes, lowest in X11's stacking first: a program
  that floods its Xwayland over several connections delays another's tooltip until its own menus
  go, no longer for good.
