- In `yuna mcp`, an Xwayland that crashes once its window manager is in charge counts as a crash
  whichever of Yuna's reapers takes its status, so that a crash loop pauses its X display as in
  `yuna --headless`, rather than start Xwayland again at each X11 connection.
