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

        # What the applications' tests give each application, so that it draws the same on every
        # machine, on the CPU (crates/yuna/tests/applications.rs, ADR-0025). Its PATH, of the
        # store alone: a terminal, GTK 4's demo (in GTK's development output), Qt's QML runner,
        # Vulkan's cube, and `sleep` for the terminal. Kept off the shell's own PATH, so that
        # GTK's development files never reach the shell's builds.
        YUNA_APPLICATIONS_PATH = pkgs.lib.makeBinPath [
          pkgs.alacritty
          pkgs.gtk4.dev
          pkgs.qt6.qtdeclarative
          pkgs.vulkan-tools
          pkgs.coreutils
        ];
        # Mesa's Vulkan driver for the CPU, lavapipe, the only one Vulkan's loader is let load.
        YUNA_LAVAPIPE_ICD = "${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.x86_64.json";
        # DejaVu alone, and none of the machine's fonts or rules: nixpkgs' makeFontsConf would
        # include /etc/fonts and /usr/share/fonts. The cache goes to the application's own
        # XDG_CACHE_HOME.
        YUNA_FONTS_CONF = pkgs.writeText "yuna-fonts.conf" ''
          <?xml version="1.0"?>
          <!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
          <fontconfig>
            <dir>${pkgs.dejavu_fonts}/share/fonts</dir>
            <cachedir prefix="xdg">fontconfig</cachedir>
          </fontconfig>
        '';
        # Where Qt finds its Wayland platform, and QML its modules: qml is not wrapped.
        YUNA_QT_PLUGINS = "${pkgs.qt6.qtbase}/${pkgs.qt6.qtbase.qtPluginPrefix}";
        YUNA_QML_IMPORTS = "${pkgs.qt6.qtdeclarative}/${pkgs.qt6.qtbase.qtQmlPrefix}";
        # The libraries that winit, wgpu and uwui load at run time, for SlimeEngine's examples,
        # which the weekly job runs (.github/workflows/slimeengine.yml): uwui finds the system's
        # fonts through fontconfig, the shell's DejaVu alone (YUNA_FONTS_CONF).
        YUNA_CLIENT_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
          pkgs.wayland
          pkgs.libxkbcommon
          pkgs.vulkan-loader
          pkgs.fontconfig
        ];
      };
    };
}
