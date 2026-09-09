{
  self,
  nixpkgs,
  pkgs,
  system,
}:
let
  evaluate =
    settings:
    (nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        self.nixosModules.default
        {
          system.stateVersion = "25.11";
          programs.splinterm = settings;
        }
      ];
    }).config;
  desktop = evaluate { enable = true; };
  headless = evaluate {
    enable = true;
    desktopIntegration = false;
    startAtLogin = true;
  };
  disabled = evaluate { enable = false; };
  service = desktop.systemd.user.services.splinterd;
in
assert
  service.serviceConfig.ExecStart
  == "${self.packages.${system}.splinterm}/bin/splinterd --require-workload-cgroups";
assert service.wantedBy == [ ];
assert headless.systemd.user.services.splinterd.wantedBy == [ "default.target" ];
assert desktop.fonts.fontconfig.enable;
assert builtins.elem pkgs.nerd-fonts.jetbrains-mono desktop.fonts.packages;
assert !(builtins.elem pkgs.nerd-fonts.jetbrains-mono headless.fonts.packages);
assert !(disabled.systemd.user.services ? splinterd);
assert desktop.systemd.user.slices.app-splinterm.sliceConfig.TasksMax == 2048;
pkgs.runCommand "splinterm-nixos-module-check" { } ''
  # Force generated unit evaluation, not only option declarations.
  test -n '${desktop.systemd.user.units."splinterd.service".text}'
  touch "$out"
''
