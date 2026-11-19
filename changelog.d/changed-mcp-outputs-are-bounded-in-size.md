- `yuna mcp` refuses an output whose side passes 8,192 pixels, and one that would bring all its
  outputs past the pixels of 16 outputs at 3840 × 2160, whether `seat create`, `outputs create`
  or `outputs configure` asks for it: before, 16 outputs at the renderer's limit could take
  16 GiB. The tools' schemas give the side's maximum.
