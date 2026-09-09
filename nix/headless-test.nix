{ self, pkgs }:
let
  package = self.packages.${pkgs.stdenv.hostPlatform.system}.splinterm;
  withMcp = self.packages.${pkgs.stdenv.hostPlatform.system}.splinterm-with-mcp;
  # Public, deliberately insecure fixture keys, confined to the disposable VLAN.
  keys = import (pkgs.path + "/nixos/tests/ssh-keys.nix") pkgs;
  workload = pkgs.writeShellScript "splinterm-headless-workload" ''
    set -eu
    output="$1"
    printf '%s\n' "$$" > "$output/pid"
    id -u > "$output/uid"
    printf '%s\n' "$HOME" > "$output/home"
    printf '%s\n' "$SHELL" > "$output/shell"
    printf '%s\n' "$SPLINTERM_TEST_MARKER" > "$output/environment"
    pwd > "$output/cwd"
    readlink /proc/$$/fd/0 > "$output/tty"
    cat /proc/$$/cgroup > "$output/cgroup"
    cat /proc/sys/kernel/random/boot_id > "$output/ready"
    exec sleep 600
  '';
in
pkgs.testers.runNixOSTest {
  name = "splinterm-headless";
  globalTimeout = 900;

  defaults = {
    imports = [ self.nixosModules.default ];
    programs.splinterm = {
      enable = true;
      desktopIntegration = false;
    };
    virtualisation = {
      graphics = false;
      memorySize = 1536;
      cores = 2;
    };
    users.groups.operator.gid = 1000;
    users.users.operator = {
      isNormalUser = true;
      uid = 1000;
      group = "operator";
      shell = pkgs.bashInteractive;
      openssh.authorizedKeys.keys = [ keys.snakeOilEd25519PublicKey ];
    };
    services.openssh = {
      enable = true;
      settings = {
        PasswordAuthentication = false;
        KbdInteractiveAuthentication = false;
        PermitRootLogin = "no";
      };
    };
    environment.systemPackages = [
      pkgs.procps
      pkgs.util-linux
    ];
    environment.etc."splinterm-test-key" = {
      source = keys.snakeOilEd25519PrivateKey;
      mode = "0600";
    };
    system.activationScripts.splinterm-test-config = {
      deps = [ "users" ];
      text = ''
        install -d -m700 -o operator -g operator /home/operator/.config/splinterm
        printf '%s\n' \
          'SHELL=${pkgs.bashInteractive}/bin/bash' \
          'SPLINTERM_TEST_MARKER=owner-environment' \
          'SPLINTERM_ENABLE_DEV_ATTACH=1' \
          > /home/operator/.config/splinterm/daemon.env
        chown operator:operator /home/operator/.config/splinterm/daemon.env
        chmod 600 /home/operator/.config/splinterm/daemon.env
      '';
    };
  };

  nodes = {
    ondemand = { };
    automatic = {
      programs.splinterm.startAtLogin = true;
      users.users.operator.linger = true;
      specialisation.with-mcp.configuration.programs.splinterm.package = pkgs.lib.mkForce withMcp;
    };
  };

  testScript = ''
    PACKAGE = "${package}"
    WITH_MCP = "${withMcp}"
    WORKLOAD = "${workload}"
    SHELL = "${pkgs.bashInteractive}/bin/bash"
  ''
  + builtins.readFile ./headless-test.py;
}
