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
          p.rust-bin.nightly.latest.default.override {
            targets = [ "wasm32-unknown-unknown" ];
            extensions = [ "rust-src" ];
          }
        );

        # 锁定 nixpkgs 的 dioxus-cli 是 0.7.9，与项目 dioxus 0.7.10 不兼容
        # （dx serve 版本检查会报错），固定覆盖到 crates.io 的 0.7.10。
        # 注：不能用 overrideAttrs 改 cargoHash——buildRustPackage 的
        # cargoDeps 在其定义时即被急切求值，overrideAttrs 不会重算。
        dioxusCli = pkgs.rustPlatform.buildRustPackage {
          pname = "dioxus-cli";
          version = "0.7.10";

          src = pkgs.fetchCrate {
            pname = "dioxus-cli";
            version = "0.7.10";
            hash = "sha256-kPzo5zRSVs46SjiDRKpKxca8kPcWUgqc/LMKQsk0sC8=";
          };

          cargoHash = "sha256-cvBVIkIqBjXFifYNpL2DqZpQcBaX/59Xw0ZJKUvUcIs=";

          buildFeatures = [
            "no-downloads"
            "disable-telemetry"
          ];

          env = {
            OPENSSL_NO_VENDOR = 1;
          };

          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.cacert
            pkgs.installShellFiles
            pkgs.makeWrapper
          ];

          buildInputs = [
            pkgs.openssl
          ];

          nativeCheckInputs = [
            pkgs.rustfmt
          ];

          checkFlags = [
            # 与原 nixpkgs 包一致：requires network access
            "--skip=serve::proxy::test"
            # requires monorepo structure and mobile toolchains
            "--skip=test_harnesses::run_harness"
          ];

          # 与原 nixpkgs 包一致：shell 补全 + 把 esbuild / wasm-bindgen-cli 挂进 PATH
          postInstall = ''
            installShellCompletion --cmd dx \
              --bash <($out/bin/dx completions bash) \
              --fish <($out/bin/dx completions fish) \
              --zsh <($out/bin/dx completions zsh)
          '';

          postFixup = ''
            wrapProgram $out/bin/dx \
              --suffix PATH : ${
                pkgs.lib.makeBinPath [
                  pkgs.esbuild
                  pkgs.wasm-bindgen-cli_0_2_118
                ]
              }
          '';
        };

        # dioxus-native (winit + blitz/vello) 运行时库：wayland 客户端与键盘处理
        # 均为 dlopen 加载，需出现在 LD_LIBRARY_PATH（版本需 >= 1.24，
        # 否则系统 mesa vulkan ICD 缺 wl_fixes_interface 符号无法加载）。
        guiSystemLibs = with pkgs; [
          openssl # auth crate's reqwest uses default TLS (openssl-sys)
          wayland # winit wayland backend
          libxkbcommon # 键盘输入
          libunwind # wine (ubuntu build) dlopens libunwind.so.8 at runtime
          dbus # rfd 的 xdg-portal 后端运行时 dlopen libdbus-1.so
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
              dioxusCli # dx 命令行（覆盖到 0.7.10，匹配 dioxus 0.7.10）
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
