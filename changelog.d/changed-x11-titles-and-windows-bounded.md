- An X11 window's title and class are kept to 4 KiB, cut at a character's edge, where 64 KiB
  were; MCP's `windows` gives them so. An X11 program's windows past its thousandth, mapped or
  not, are followed without their properties and never show, as windows past a program's limit
  in the layout: one that asks to be mapped lies below every X11 window Yuna stacks, so that it
  takes no click from one Yuna shows, and stays out of `_NET_CLIENT_LIST`; an override-redirect
  one, which maps itself, is not lowered (ADR-0131).
