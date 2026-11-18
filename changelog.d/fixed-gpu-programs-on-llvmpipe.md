- On llvmpipe, the default, a program drawing with the machine's GPU no longer kills Yuna. Yuna
  refuses a dmabuf whose file llvmpipe cannot map, as a GPU's buffer in its video memory, and
  one whose stride is shorter than a row of its pixels: the program is told `failed`, or ended
  by `invalid_wl_buffer` for `create_immed`, and every other program goes on (ADR-0080,
  ADR-0098). Mesa's llvmpipe took such buffers, then read address zero, or past the buffer's
  end, when it drew them. A GPU program that is not Mesa's, Chromium for one, may still send
  such a buffer, and is then ended alone; which ones do is not verified.
