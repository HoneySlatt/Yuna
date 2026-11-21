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
    # Umbriel, a rival the bench measures, at the revision docs/survey.md read. Its source alone:
    # the bench shell builds Umbriel's own package definition (nix/package.nix) with this nixpkgs,
    # as its flake's `packages.default` does with its own, so that it draws with the bench's Mesa
    # and adds no input of its own to the lock. Fetched only by the bench shell.
    umbriel = {
      url = "github:noctalia-dev/umbriel/d171ccb31628da65b1beee3221b37a2758d553b3";
      flake = false;
    };
    # The owner's projects whose drivers Yuna's tests run against `yuna --headless --agent-seat`,
    # each pinned to the revision read, raised in a commit of its own (ADR-0079, ADR-0089): uwui's
    # scripts/measure.py, and Pluto's pluto-input. Their sources alone.
    uwui = {
      url = "github:HoneySlatt/uwui/fb77f0d90c06fb6ef89aef3529239e5d4b70eefd";
      flake = false;
    };
    pluto = {
      url = "github:HoneySlatt/Pluto/4624243566575387ec80a9a41e164f6d1112711c";
      flake = false;
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      umbriel,
      uwui,
      pluto,
      ...
    }:
    let
      # Linux only (ADR-0006); the machines the owner develops on are x86_64.
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      # The default shell's settings, which the bench's shell extends.
      shell = {
        packages = [
          toolchain
          # Licences, advisories and sources of the dependencies.
          pkgs.cargo-deny
          # Links between the Markdown files.
          pkgs.lychee
          # Writes the ADRs' index and checks the changelog's fragments (scripts/).
          pkgs.python3
          # Lists the Markdown files the link check reads.
          pkgs.git
          # The X server Yuna starts for X11 programs, found on PATH (ADR-0067).
          pkgs.xwayland
        ];

        # Linked by Smithay's keyboard (xkbcommon).
        buildInputs = [ pkgs.libxkbcommon ];

        # Tells scripts/check.sh that it runs in this shell.
        YUNA_DEV_SHELL = "1";

        # The dbus-daemon Yuna runs for each agent's seat's accessibility bus, by its absolute
        # path, nixpkgs' own, never the host's (crates/yuna/src/dbus/mod.rs); the package's
        # wrapper sets the same.
        YUNA_DBUS_DAEMON = "${pkgs.dbus}/bin/dbus-daemon";

        # The `unshare` that puts Opium in a network namespace of its own in the weekly job's test
        # (crates/yuna/tests/opium.rs), and that makes each run's namespaces in the parity script
        # (crates/yuna-bench/src/parity/launch.rs): util-linux's, pinned here, never the host's.
        YUNA_UNSHARE = "${pkgs.util-linux}/bin/unshare";

        # Yuna draws with EGL, loaded at run time (ADR-0022): libglvnd's dispatcher, which this
        # file points at the shell's own Mesa only, so that Yuna never loads the host's drivers.
        LD_LIBRARY_PATH = "${pkgs.libglvnd}/lib";
        __EGL_VENDOR_LIBRARY_FILENAMES = "${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json";
        # The keyboard's default keymap.
        XKB_CONFIG_ROOT = "${pkgs.xkeyboard_config}/share/X11/xkb";

        # WLCS, the Wayland conformance suite, which runs Yuna through crates/yuna-wlcs
        # (ADR-0149): nixpkgs' plain runner by its store path (the package installs no bin/, and
        # its sanitizer builds are not used), and its headers, which the shim's ABI test compiles
        # against. Taken from this nixpkgs, never another: the shim is loaded into WLCS's process,
        # so both must load the same glibc and libstdc++.
        YUNA_WLCS_RUNNER = "${pkgs.wlcs}/libexec/wlcs/wlcs";
        YUNA_WLCS_INCLUDE = "${pkgs.wlcs}/include";
        # Its version, which the list of expected outcomes names (crates/yuna-wlcs/expected.txt).
        YUNA_WLCS_VERSION = pkgs.wlcs.version;

        # What the applications' tests give each application, so that it draws the same on every
        # machine, on the CPU (crates/yuna/tests/applications.rs, ADR-0025). Its PATH, of the
        # store alone: a terminal, GTK 4's demo (in GTK's development output), Qt's QML runner,
        # Vulkan's cube, mpv, whose surface declares its colours (ADR-0050), Wine, whose own
        # notepad runs on X11, and `sleep` and `env` for the terminal and the tests. Wine is the
        # 64-bit build, all notepad needs: the WoW64 build, for 32-bit programs too, is 287 MB
        # larger. Kept off the shell's own PATH, so that GTK's development files never reach the
        # shell's builds.
        YUNA_APPLICATIONS_PATH = pkgs.lib.makeBinPath [
          pkgs.alacritty
          pkgs.gtk4.dev
          pkgs.qt6.qtdeclarative
          pkgs.vulkan-tools
          pkgs.mpv
          pkgs.wine64Packages.stable
          pkgs.coreutils
        ];
        # The consumers' sources, which crates/yuna/tests/consumers.rs runs (ADR-0089).
        YUNA_UWUI_SOURCE = "${uwui}";
        YUNA_PLUTO_SOURCE = "${pluto}";
        # Yuna's own PATH in those tests: Xwayland alone, which it looks for there (ADR-0067).
        YUNA_XWAYLAND_PATH = pkgs.lib.makeBinPath [ pkgs.xwayland ];
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
      toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      rustPlatform = pkgs.makeRustPlatform {
        cargo = toolchain;
        rustc = toolchain;
      };

      # Yuna's package (ADR-0058, ADR-0089): `yuna` alone, built with the pinned toolchain from the
      # workspace's sources, and wrapped to draw with the package's own Mesa through libglvnd, as
      # the shell does, and to find Xwayland: it runs the same on NixOS, on another distribution
      # with Nix and in a CI on Ubuntu. Its outputs are a contract (CONTRIBUTING.md). The tests
      # run in scripts/check.sh, not here: they need the shell's applications.
      yuna = rustPlatform.buildRustPackage {
        pname = "yuna";
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
        src = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./crates
          ];
        };
        cargoLock = {
          lockFile = ./Cargo.lock;
          # Yuna's fork of Smithay, a git dependency: its hash changes with the revision
          # Cargo.toml pins (docs/smithay-patches.md).
          outputHashes = {
            "smithay-0.7.0" = "sha256-6phGfjIFcCeuiXp87MBYVDcNmqC7Nxv7g/PCVldUvtw=";
          };
        };
        cargoBuildFlags = [
          "--package"
          "yuna"
          "--bin"
          "yuna"
        ];
        doCheck = false;
        # The standard library's sources are named in the binary's panic messages: remapped,
        # so that the toolchain, 1.4 GB, stays out of the package's closure, and refused if a
        # reference to it comes back.
        env.RUSTFLAGS = "--remap-path-prefix=${toolchain}=/rust";
        disallowedReferences = [ toolchain ];
        nativeBuildInputs = [ pkgs.makeWrapper ];
        buildInputs = [ pkgs.libxkbcommon ];
        # The variables set here are Yuna's alone: the programs `yuna mcp` starts get the caller's
        # own values back, kept in YUNA_CALLER_<name> (crates/yuna/src/mcp/programs/environment.rs).
        postInstall = ''
          wrapProgram $out/bin/yuna \
            --run 'if [ -n "''${PATH+x}" ]; then export YUNA_CALLER_PATH="$PATH"; fi' \
            --run 'if [ -n "''${LD_LIBRARY_PATH+x}" ]; then export YUNA_CALLER_LD_LIBRARY_PATH="$LD_LIBRARY_PATH"; fi' \
            --run 'if [ -n "''${__EGL_VENDOR_LIBRARY_FILENAMES+x}" ]; then export YUNA_CALLER___EGL_VENDOR_LIBRARY_FILENAMES="$__EGL_VENDOR_LIBRARY_FILENAMES"; fi' \
            --set YUNA_WRAPPED_VARIABLES "PATH LD_LIBRARY_PATH __EGL_VENDOR_LIBRARY_FILENAMES" \
            --prefix LD_LIBRARY_PATH : ${pkgs.libglvnd}/lib \
            --set __EGL_VENDOR_LIBRARY_FILENAMES ${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json \
            --set YUNA_DBUS_DAEMON ${pkgs.dbus}/bin/dbus-daemon \
            --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.xwayland ]}
        '';
        meta = {
          description = "A Wayland compositor for Linux";
          homepage = "https://github.com/HoneySlatt/Yuna";
          license = pkgs.lib.licenses.gpl3Plus;
          mainProgram = "yuna";
          platforms = [ system ];
        };
      };
    in
    {
      packages.${system} = {
        inherit yuna;
        default = yuna;
      };

      devShells.${system} = {
        default = pkgs.mkShell shell;

        # The parity script (`yuna-bench parity`, docs/ARCHITECTURE.md, ADR-0142): the default
        # shell and valgrind, whose callgrind counts a frame's instructions and whose DHAT counts
        # its allocations (ADR-0143). Kept out of the default shell, which every check fetches;
        # CI's parity job alone uses it. The revisions it compares are built in their own default
        # shells.
        parity = pkgs.mkShell (
          shell
          // {
            packages = shell.packages ++ [ pkgs.valgrind ];
          }
        );

        # The rivals' bench (crates/yuna-bench, CONTRIBUTING.md): the default shell, and the
        # compositors and clients it runs, by their store paths. Kept out of the default shell, so
        # that CI, which fetches that shell at every run, never fetches KWin and its Qt.
        bench = pkgs.mkShell (
          shell
          // {
            # sway's own binary, not nixpkgs' wrapper, which would add its own environment. Its
            # shipped configuration, in etc/sway, draws no wallpaper.
            YUNA_BENCH_SWAY = "${pkgs.sway-unwrapped}";
            YUNA_BENCH_KWIN = "${pkgs.kdePackages.kwin}";
            # dwl and mango as nixpkgs ships them, with XWayland, and built without it for the
            # equal-work configuration: neither has a switch to turn it off at run time.
            YUNA_BENCH_DWL = "${pkgs.dwl}";
            YUNA_BENCH_DWL_NO_XWAYLAND = "${pkgs.dwl.override { enableXWayland = false; }}";
            YUNA_BENCH_MANGO = "${pkgs.mango}";
            YUNA_BENCH_MANGO_NO_XWAYLAND = "${pkgs.mango.override { enableXWayland = false; }}";
            YUNA_BENCH_UMBRIEL = "${pkgs.callPackage "${umbriel}/nix/package.nix" {
              rev = umbriel.shortRev;
            }}";
            YUNA_BENCH_HYPRLAND = "${pkgs.hyprland}";
            YUNA_BENCH_MUTTER = "${pkgs.mutter}";
            # Sets dwl's output to the bench's mode, through wlr-output-management, and reads the
            # outputs of the compositors on wlroots.
            YUNA_BENCH_WLR_RANDR = "${pkgs.wlr-randr}";
            # weston-presentation-shm, the pacing scenario's client.
            YUNA_BENCH_WESTON = "${pkgs.weston}";
            # Makes the film scenario's clip.
            YUNA_BENCH_FFMPEG = "${pkgs.ffmpeg-headless.bin}";
            # dbus-run-session and dbus-daemon for each compositor's own session bus, dbus-send to
            # drive KWin.
            YUNA_BENCH_DBUS = "${pkgs.dbus}";
            # The Mesa every compositor and client draws with: its EGL, GBM and Vulkan drivers.
            YUNA_BENCH_MESA = "${pkgs.mesa}";
            # The shell's LD_LIBRARY_PATH, libglvnd's EGL, for the compositors: `cargo run`
            # puts its own folders before it in the bench's.
            YUNA_BENCH_LIBRARY_PATH = shell.LD_LIBRARY_PATH;
            YUNA_BENCH_XWAYLAND = "${pkgs.xwayland}";
            # The shell and the commands a compositor's own children run: sway's bar runs a loop
            # of `date` and `sleep` in `sh`.
            YUNA_BENCH_COREUTILS = "${pkgs.coreutils}";
            YUNA_BENCH_BASH = "${pkgs.bash}";
          }
        );
      };
    };
}
