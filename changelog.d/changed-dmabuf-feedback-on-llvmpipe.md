- On llvmpipe, `linux-dmabuf` is version 6, with feedback whose main device is no device
  (`dev_t` 0), rather than version 3: a Mesa Vulkan program on a GPU, GTK 4's among them, then
  shares linear copies of its frames outside its video memory, which llvmpipe reads, and its
  windows show (ADR-0098). It works for programs that compare that device with their GPU as
  Mesa does; Chromium, Firefox, VA-API's video decoders and NVIDIA's driver are not verified.
- On llvmpipe, `linux-dmabuf` offers only the formats of one plane whose pixel size Yuna knows,
  23 with Mesa 26.2 (red, red and green, and RGB), each linear or with no modifier given, rather
  than all 57 llvmpipe imports: its YUV formats, packed or of several planes, are no longer
  offered there.
