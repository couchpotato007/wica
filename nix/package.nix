{
  lib,
  rustPlatform,
}: let
  cargo = (lib.importTOML ../Cargo.toml).package;
in
  rustPlatform.buildRustPackage {
    pname = cargo.name;
    inherit (cargo) version;

    # Only what the build needs, so editing the flake or eval/ doesn't rebuild.
    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ../Cargo.toml
        ../Cargo.lock
        ../src
      ];
    };

    cargoLock.lockFile = ../Cargo.lock;

    meta = {
      description = "Find the CLI tool you forgot the name of by describing what it does";
      longDescription = ''
        wica ("what is it called?") searches the tools on your PATH, their man
        pages and the tldr pages by meaning: `wica render markdown` finds glow,
        `wica name deer` finds tealdeer. Run `wica update` once to download
        the tldr pages and the embedding model.
      '';
      license = lib.licenses.mit;
      mainProgram = "wica";
      platforms = lib.platforms.unix;
    };
  }
