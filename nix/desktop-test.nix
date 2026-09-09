{
  self,
  pkgs,
  smokeOnly ? false,
}:
let
  package = self.packages.${pkgs.stdenv.hostPlatform.system}.splinterm;
  urlSink = pkgs.writeShellScript "splinterm-vm-url" ''
    printf '%s\n' "$1" >> /home/operator/acceptance/url-opened
  '';
  swayConfig = pkgs.writeText "splinterm-vm-sway.conf" ''
    output * bg #20242c solid_color
    output * mode 1280x960
    seat seat0 fallback true
    seat seat0 hide_cursor 0
    focus_follows_mouse no
    default_border none
    default_floating_border none
    workspace 8
    seat seat0 cursor set 0 0
    exec touch /home/operator/acceptance/compositor-ready
  '';
in
pkgs.testers.runNixOSTest {
  name = "splinterm-desktop";
  globalTimeout = 900;
  enableOCR = true;
  nodes.desktop = {
    imports = [ self.nixosModules.default ];
    programs.splinterm.enable = true;
    programs.sway.enable = true;
    users.groups.operator.gid = 1000;
    users.users.operator = {
      isNormalUser = true;
      uid = 1000;
      group = "operator";
      shell = pkgs.bashInteractive;
    };
    services.getty.autologinUser = "operator";
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ]; then
        export SWAYSOCK=/run/user/1000/sway-test.sock
        export WLR_RENDERER=pixman
        exec sway --config ${swayConfig} > /home/operator/acceptance/sway.log 2>&1
      fi
    '';
    environment.systemPackages = with pkgs; [
      glib
      grim
      wtype
      wlrctl
      wl-clipboard
      procps
      util-linux
      xdg-utils
    ];
    system.activationScripts.splinterm-desktop-fixtures = {
      deps = [ "users" ];
      text = ''
        install -d -m700 -o operator -g operator /home/operator/acceptance
        install -d -m700 -o operator -g operator /home/operator/.config/splinterm
        install -d -m700 -o operator -g operator /home/operator/.local/share/applications
        printf '%s\n' 'SHELL=${pkgs.bashInteractive}/bin/bash' > /home/operator/.config/splinterm/daemon.env
        printf '%s\n' 'PS1="VM> "' > /home/operator/.bashrc
        printf '%s\n' '[main]' 'font-pixelsize=18' > /home/operator/.config/splinterm/config.ini
        cat > /home/operator/.local/share/applications/splinterm-vm-url.desktop <<'EOF'
        [Desktop Entry]
        Type=Application
        Name=Guest-only URL recorder
        Exec=${urlSink} %u
        MimeType=x-scheme-handler/http;x-scheme-handler/https;
        NoDisplay=true
        EOF
        chown -R operator:operator /home/operator/.config /home/operator/.local /home/operator/.bashrc
      '';
    };
    virtualisation = {
      memorySize = 2048;
      cores = 2;
      # A guest-only virtual GPU. The NixOS driver owns QEMU; no host viewer.
      qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    };
  };
  testScript = ''
    SMOKE_ONLY = ${if smokeOnly then "True" else "False"}
    PACKAGE = "${package}"
    SHELL = "${pkgs.bashInteractive}/bin/bash"
  ''
  + builtins.readFile ./desktop-test.py;
}
