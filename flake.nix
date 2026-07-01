{
    # Tremendous thanks to @oati for her help
    inputs = {
        nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
        flake-utils.url = "github:numtide/flake-utils";
        rust-overlay.url = "github:oxalica/rust-overlay";
    };
    outputs = { self, nixpkgs, rust-overlay, flake-utils }:
        flake-utils.lib.eachDefaultSystem (system:
            let
                pkgs = import nixpkgs {
                    inherit system;
                    overlays = [ rust-overlay.overlays.default ];
                };
                rustVersion = pkgs.rust-bin.stable.latest.default;

                rustPlatform = pkgs.makeRustPlatform {
                    cargo = rustVersion;
                    rustc = rustVersion;
                };

                # GUI + audio runtime libs (X11, Wayland, Vulkan, ALSA, PulseAudio)
                runtimeLibs = with pkgs; [
                    # Graphics — Vulkan / OpenGL (eframe supports both backends)
                    vulkan-loader
                    vulkan-validation-layers
                    libGL

                    # X11 backend
                    xorg.libX11
                    xorg.libXcursor
                    xorg.libXi
                    xorg.libXrandr
                    xorg.libxcb

                    # Wayland backend
                    wayland
                    libxkbcommon

                    # Audio — ALSA (cpal default on Linux) + PulseAudio / PipeWire shims
                    alsa-lib
                    pipewire
                    pulseaudio

                    # udev (cpal device enumeration)
                    udev

                    # Tauri (webview + system integration)
                    webkitgtk_4_1
                    gtk3
                    libsoup_3
                    dbus
                    glib
                    cairo
                    pango
                    gdk-pixbuf
                    atk
                    harfbuzz
                    openssl
                    librsvg
                ];

                localRustBuild = rustPlatform.buildRustPackage rec {
                    pname = "audirs";
                    version = "0.1.0";
                    src = ./.;
                    cargoBuildFlags = "";

                    cargoLock = {
                        lockFile = ./src-tauri/Cargo.lock;
                    };

                    buildAndTestSubdir = "src-tauri";

                    nativeBuildInputs = [
                        (rustVersion.override { extensions = [ "rust-src" ]; })
                        pkgs.pkg-config
                        pkgs.cargo
                        pkgs.gcc
                        pkgs.rustfmt
                        pkgs.clippy
                    ];

                    buildInputs = runtimeLibs;

                    RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
                    LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;

                    # Tell winit/eframe where to find Wayland/EGL at runtime
                    WINIT_UNIX_BACKEND = "x11";
                };
            in
            {
                packages.default = localRustBuild;
                packages.audirs  = localRustBuild;

                devShells.default = pkgs.mkShell rec {
                    inputsFrom = [ localRustBuild ];

                    buildInputs = [
                        (rustVersion.override { extensions = [ "rust-src" "rust-analyzer" ]; })
                        pkgs.pkg-config
                        pkgs.cargo
                        pkgs.gcc
                        pkgs.rustfmt
                        pkgs.clippy
                    ] ++ runtimeLibs;

                    RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
                    LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
                    WINIT_UNIX_BACKEND = "x11";
                    GDK_BACKEND = "x11";

                    shellHook = ''
                        echo "audirs dev shell — egui + cpal ready"
                        echo "  WINIT_UNIX_BACKEND=x11 (override to 'wayland' if preferred)"
                    '';
                };
            }
        );
}
