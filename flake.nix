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
          # The applications that crates/yuna/tests/applications.rs runs in `yuna --headless`
          # (ADR-0025): a terminal, GTK 4's demo, Qt's QML runner and Vulkan's cube.
          pkgs.alacritty
          (pkgs.lib.getBin pkgs.gtk4.dev)
          pkgs.qt6.qtdeclarative
          pkgs.vulkan-tools
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

        # What the applications' tests give each application, so that it draws the same on every
        # machine, on the CPU (crates/yuna/tests/applications.rs). Mesa's Vulkan driver for the
        # CPU, lavapipe, the only one Vulkan's loader is let load.
        YUNA_LAVAPIPE_ICD = "${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.x86_64.json";
        # One font family, so that no font the machine has changes what is drawn.
        YUNA_FONTS_CONF = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
        # Where Qt finds its Wayland platform, and QML its modules: qml is not wrapped.
        YUNA_QT_PLUGINS = "${pkgs.qt6.qtbase}/${pkgs.qt6.qtbase.qtPluginPrefix}";
        YUNA_QML_IMPORTS = "${pkgs.qt6.qtdeclarative}/${pkgs.qt6.qtbase.qtQmlPrefix}";
        # The libraries that winit and wgpu load at run time, for SlimeEngine's `window` example,
        # which the weekly job runs (.github/workflows/slimeengine.yml).
        YUNA_CLIENT_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
          pkgs.wayland
          pkgs.libxkbcommon
          pkgs.vulkan-loader
        ];
      };
    };
}
