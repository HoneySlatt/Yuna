- Xwayland, and the `xkbcomp` it runs, get only the variables of Yuna's environment that they
  and their drivers read (the path, the home, the locale, Mesa's, Vulkan's and EGL's), and their
  Wayland connection: nothing else of the program that started Yuna, an agent's harness in
  `yuna mcp` included.
