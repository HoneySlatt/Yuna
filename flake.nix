{
  # The development shell that agents, the owner and CI use alike: `nix develop`, then
  # `scripts/check.sh`. It holds every tool the checks use (CONTRIBUTING.md).
  description = "Yuna, a Wayland compositor for Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The Rust toolchain at the exact version rust-toolchain.toml names (ADR-0016).
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      # Linux only (ADR-0006); the machines the owner develops on are x86_64.
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
          # Licences, advisories and sources of the dependencies.
          pkgs.cargo-deny
          # Links between the Markdown files.
          pkgs.lychee
          # Lists the Markdown files the link check reads.
          pkgs.git
        ];

        # Linked by Smithay's keyboard (xkbcommon).
        buildInputs = [ pkgs.libxkbcommon ];

        # Tells scripts/check.sh that it runs in this shell.
        YUNA_DEV_SHELL = "1";

        # Yuna draws with EGL, loaded at run time (ADR-0022): libglvnd's dispatcher, which this
        # file points at the shell's own Mesa only, so that Yuna never loads the host's drivers.
        LD_LIBRARY_PATH = "${pkgs.libglvnd}/lib";
        __EGL_VENDOR_LIBRARY_FILENAMES = "${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json";
        # The keyboard's default keymap.
        XKB_CONFIG_ROOT = "${pkgs.xkeyboard_config}/share/X11/xkb";
      };
    };
}
