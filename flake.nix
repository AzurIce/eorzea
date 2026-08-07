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

        # dioxus-native (winit + blitz/vello) 运行时库：wayland 客户端与键盘处理
        # 均为 dlopen 加载，需出现在 LD_LIBRARY_PATH（版本需 >= 1.24，
        # 否则系统 mesa vulkan ICD 缺 wl_fixes_interface 符号无法加载）。
        guiSystemLibs = with pkgs; [
          openssl # auth crate's reqwest uses default TLS (openssl-sys)
          wayland # winit wayland backend
          libxkbcommon # 键盘输入
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
              gh
              p7zip # Dalamud release 解压（.7z）
              pkg-config
            ];
          buildInputs = guiSystemLibs ++ wineLibs;
          shellHook = ''
            # Make system libs discoverable at runtime (winit dlopens
            # libwayland-client/libxkbcommon; wine dlopens libunwind etc.)
            export LD_LIBRARY_PATH="${lib.makeLibraryPath (guiSystemLibs ++ wineLibs)}:$LD_LIBRARY_PATH"
          '';
        };
      }
    );
}
