- An X11 window's title and class are kept to 4 KiB, cut at a character's edge, where 64 KiB
  were; MCP's `windows` gives them so. An X11 program's windows past its thousandth, mapped or
  not, are followed without their properties and never show, as windows past a program's limit
  in the layout (ADR-0070).
