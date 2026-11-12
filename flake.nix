{
  description = "Eine einfache Nix dev shell für Rust";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    jailed-agents.url = "github:andersonjoseph/jailed-agents";
  };

  outputs = {
    self,
    nixpkgs,
    jailed-agents,
  }: let
    system = "x86_64-linux";
    lib = nixpkgs.lib;
    pkgs = nixpkgs.legacyPackages.${system};
  in {
    devShells.${system}.default = pkgs.mkShell {
      name = "rust-dev-shell";

      packages = with pkgs; [
        rustc
        cargo
        clippy
        rustfmt
        rust-analyzer
        rustfmt
        just
        gcc
        clang
        clang-tools
        boehmgc
        cargo-generate
        llvmPackages.libllvm
        llvmPackages.libllvm.lib
        libclang
        libllvm
        pkg-config
        libffi
        libxml2
        # luajit
        # lua
        openssl
        rustPlatform.bindgenHook
        # (self.packages.${system}.default)

        (jailed-agents.lib.${system}.makeJailedClaudeCode {
          extraPkgs = with pkgs; [
            gcc # linker for cargo build / check
            cargo
            rustc
            rustfmt
            python3
            gcc
            llvmPackages.lld
            pkg-config
            llvmPackages.libllvm
            llvmPackages.libllvm.lib
            llvmPackages.libllvm.dev
            libclang
            libffi
            libxml2
            zlib
          ];
          fwdEnv = [
            "PKG_CONFIG_PATH"
            "PKG_CONFIG_PATH_FOR_TARGET"
            "LD_LIBRARY_PATH"
            "LLVM_SYS_211_PREFIX"
          ];
          extraReadwriteDirs = ["~/.claude"];
        })
      ];

      hardeningDisable = ["fortify"];
      shellHook = ''
        export LLVM_SYS_211_PREFIX=${pkgs.llvmPackages.libllvm.dev}
        export PATH=${pkgs.llvmPackages.libllvm.lib}/bin:$PATH
        export LIBCLANG_PATH=${pkgs.llvmPackages.libclang.lib}/lib
      '';
      env.RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
      env.CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG = true;
    };
  };
}
