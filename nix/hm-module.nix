# Home Manager module for wica.
#
# Tags are deliberately not configurable here: `wica tag` writes to
# ~/.config/wica/tags, which must stay a normal, writable file. This module
# never touches it.
self: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.wica;
  toml = pkgs.formats.toml {};

  managerType = lib.types.submodule {
    # Everything wica accepts (try, lookup, search, ...) passes through as is;
    # see src/packages/builtin.toml for the format.
    freeformType = toml.type;
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        description = "Display name. A built-in manager with the same name is replaced.";
      };
      detect = lib.mkOption {
        type = lib.types.str;
        description = "Executable whose presence on PATH means this manager is available.";
      };
      install = lib.mkOption {
        type = lib.types.str;
        description = "Install command; `{package}` is replaced by the package name.";
      };
    };
  };
in {
  options.programs.wica = {
    enable = lib.mkEnableOption "wica, a search for CLI tools by what they do";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "wica.packages.\${system}.default";
      description = "The wica package to use.";
    };

    model = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "minishlab/potion-base-8M";
      description = ''
        Hugging Face repository of the embedding model (sets `WICA_MODEL`).
        `null` uses the built-in default (BAAI/bge-small-en-v1.5). A static
        model like minishlab/potion-base-8M is smaller and indexes much faster,
        but understands queries less well. Run `wica update` after changing it.
      '';
    };

    packageManagers = lib.mkOption {
      type = lib.types.listOf managerType;
      default = [];
      example = lib.literalExpression ''
        [
          # Show declarative instructions instead of `nix profile install`.
          {
            name = "nix";
            detect = "nix";
            install = "add pkgs.{package} to home.packages";
            try = "nix shell nixpkgs#{package}";
            lookup = [{
              command = ["nix-locate" "--minimal" "--top-level" "--whole-name" "--at-root" "/bin/{command}"];
              pattern = "^(\\S+?)(?:\\.(?:out|bin))?$";
            }];
          }
          { name = "paru"; detect = "paru"; install = "paru -S {package}"; }
        ]
      '';
      description = ''
        Extra package managers, written to
        {file}`$XDG_CONFIG_HOME/wica/package-managers.toml`. They are tried
        before the built-in ones and replace built-ins with the same name.
        Check the result with `wica managers`.
      '';
    };

    autoUpdate = {
      enable = lib.mkEnableOption "a systemd user timer that runs `wica update` (downloads the latest tldr pages)";
      frequency = lib.mkOption {
        type = lib.types.str;
        default = "weekly";
        description = "How often to update, as a systemd calendar expression.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.autoUpdate.enable -> pkgs.stdenv.hostPlatform.isLinux;
        message = "programs.wica.autoUpdate needs systemd (Linux).";
      }
    ];

    home.packages = [cfg.package];

    home.sessionVariables = lib.mkIf (cfg.model != null) {WICA_MODEL = cfg.model;};

    xdg.configFile."wica/package-managers.toml" = lib.mkIf (cfg.packageManagers != []) {
      source = toml.generate "wica-package-managers.toml" {manager = cfg.packageManagers;};
    };

    systemd.user.services.wica-update = lib.mkIf cfg.autoUpdate.enable {
      Unit = {
        Description = "Download the latest tldr pages and rebuild the wica index";
        After = ["network-online.target"];
        Wants = ["network-online.target"];
      };
      Service = {
        Type = "oneshot";
        ExecStart = "${lib.getExe cfg.package} update";
        # The update rebuilds the index from PATH, which is nearly empty in a
        # systemd service; without this every tool would count as not installed.
        Environment =
          [
            "PATH=${lib.concatStringsSep ":" [
              "${config.home.profileDirectory}/bin"
              "/etc/profiles/per-user/%u/bin"
              "/run/wrappers/bin"
              "/run/current-system/sw/bin"
              "/nix/var/nix/profiles/default/bin"
              "/usr/local/bin"
              "/usr/bin"
              "/bin"
            ]}"
          ]
          ++ lib.optional (cfg.model != null) "WICA_MODEL=${cfg.model}";
        # Embedding is CPU heavy; don't get in the way.
        Nice = 19;
        IOSchedulingClass = "idle";
      };
    };

    systemd.user.timers.wica-update = lib.mkIf cfg.autoUpdate.enable {
      Unit.Description = "Update wica regularly";
      Timer = {
        OnCalendar = cfg.autoUpdate.frequency;
        Persistent = true;
        RandomizedDelaySec = "1h";
      };
      Install.WantedBy = ["timers.target"];
    };
  };
}
