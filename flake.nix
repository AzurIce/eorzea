{
  description = "rust";

  # nixConfig = {
  #   extra-substituters = [
  #     "https://mirrors.ustc.edu.cn/nix-channels/store"
  #   ];
  #   trusted-substituters = [
  #     "https://mirrors.ustc.edu.cn/nix-channels/store"
  #   ];
  # };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      crane,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        inherit (pkgs) lib;
        craneLib = (crane.mkLib pkgs).overrideToolchain (
          p:
          p.rust-bin.nightly."2026-01-01".default.override {
            targets = [ "wasm32-unknown-unknown" ];
            extensions = [ "rust-src" ];
          }
        );

        # Tauri v2 system libraries: needed for pkg-config (compile), the linker
        # (via cc-wrapper -L/-rpath) and at runtime (via rpath).
        tauriSystemLibs = with pkgs; [
          openssl # auth crate's reqwest uses default TLS (openssl-sys)
          webkitgtk_4_1 # provides both webkit2gtk-4.1.pc and javascriptcoregtk-4.1.pc
          gtk3
          libsoup_3
          librsvg
          libayatana-appindicator
          glib-networking
          libunwind # wine (ubuntu build) dlopens libunwind.so.8 at runtime
        ];
        # Wine (ubuntu build) 运行所需的系统库：dlopen 加载，不在 ldd 里
        wineLibs = with pkgs; [
          freetype # TrueType 字体渲染
          fontconfig
          gnutls # 加密/pfx
          vulkan-loader # DXVK
          mesa # libGL
          libx11
          libxext
          libxrender
          libxrandr
          libxi
          libxcursor
          libxinerama
          libxxf86vm
          libxcb
          libpulseaudio # wine 声音（pulse/pipewire）
          alsa-lib # wine 声音（ALSA）
        ];
      in
      {
        packages = { };
        devShells.default = craneLib.devShell {
          packages =
            with pkgs; [
              cargo-tauri
              gh
              p7zip # Dalamud release 解压（.7z）
              bun # frontend (bun.lock, `bun run dev` per tauri.conf.json)
              pkg-config
            ];
          buildInputs = tauriSystemLibs ++ wineLibs;
          shellHook = ''
            # Let webkit/GTK find glib-networking's TLS modules at runtime
            export GIO_EXTRA_MODULES="${pkgs.glib-networking}/lib/gio/modules"
            # Make system libs discoverable at runtime (incl. wine's dlopen deps
            # like libunwind.so.8, and the GTK stack for the tauri app)
            export LD_LIBRARY_PATH="${lib.makeLibraryPath (tauriSystemLibs ++ wineLibs)}:$LD_LIBRARY_PATH"
            # WebKitGTK 2.52's EGL accelerated compositing trips Mutter 50's strict
            # wp_linux_drm_syncobj_surface_v1 check ("Missing acquire timeline"
            # protocol error -> app killed at startup). Software rendering avoids
            # the dmabuf/syncobj path entirely; verified stable. If Mutter is
            # upgraded and the bug is fixed upstream, this line can be removed.
            export WEBKIT_DISABLE_COMPOSITING_MODE=1
            # Fallback if the app ever crashes with
            #   "Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display"
            # for another reason:
            #   export GDK_BACKEND=x11
          '';
        };
      }
    );
}
