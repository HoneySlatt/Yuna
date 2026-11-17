- A Nix package, `packages.x86_64-linux.yuna`, also the flake's default, built in CI: `yuna`
  draws with the package's own Mesa and finds Xwayland, so that it runs the same on NixOS, on
  another distribution with Nix and in a CI, at about 1.3 GB to fetch.
