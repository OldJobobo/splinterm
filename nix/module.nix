{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.splinterm;
in
{
  options.programs.splinterm = {
    enable = lib.mkEnableOption "Splinterm's Wayland terminal, CLI, and systemd user daemon";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.splinterm;
      description = "The complete package; client, daemon, relay and PTY helper must remain together.";
    };
    desktopIntegration = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Enable Fontconfig and install terminal/CJK/emoji fonts. The GUI and desktop entry are always in the package; this does not enable a compositor or change the default terminal.";
    };
    startAtLogin = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Start the user daemon at default.target instead of only on demand. Applies to all user managers; lingering is a separate per-user choice.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    fonts.fontconfig.enable = lib.mkIf cfg.desktopIntegration true;
    fonts.packages = lib.mkIf cfg.desktopIntegration [
      pkgs.nerd-fonts.jetbrains-mono
      pkgs.noto-fonts-cjk-sans
      pkgs.noto-fonts-color-emoji
    ];

    systemd.user.services.splinterd = {
      description = "Splinterm persistent terminal daemon";
      documentation = [ "file://${cfg.package}/share/doc/splinterm/headless.md" ];
      wantedBy = lib.optionals cfg.startAtLogin [ "default.target" ];
      serviceConfig = {
        Type = "simple";
        EnvironmentFile = "-%h/.config/splinterm/daemon.env";
        UnsetEnvironment = "SPLINTERM_ENABLE_DEV_ATTACH";
        ExecStart = "${cfg.package}/bin/splinterd --require-workload-cgroups";
        ExecReload = "${pkgs.coreutils}/bin/kill -HUP $MAINPID";
        Restart = "on-failure";
        RestartSec = 1;
        TasksMax = 2048;
        MemoryHigh = "75%";
        KillSignal = "SIGINT";
        KillMode = "mixed";
        TimeoutStopSec = 90;
      };
    };
    systemd.user.slices.app-splinterm = {
      description = "Splinterm terminal workloads";
      unitConfig.StopWhenUnneeded = true;
      sliceConfig = {
        TasksMax = 2048;
        MemoryHigh = "75%";
      };
    };
  };
}
