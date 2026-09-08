{
  lib,
  rustPlatform,
  pkg-config,
  freetype,
  pixman,
  fontconfig,
  libxkbcommon,
  wayland,
  systemd,
  coreutils,
  openssh,
  python3,
  desktop-file-utils,
  runtimeShell,
  xdg-utils,
  src,
  sourceRevision ? null,
  withMcp ? false,
}:

assert lib.assertMsg
  (sourceRevision != null && builtins.match "[0-9a-f]{40}" sourceRevision != null)
  "Splinterm requires an exact sourceRevision; build from a Git flake, or pass the source commit when calling package.nix.";

rustPlatform.buildRustPackage {
  pname = if withMcp then "splinterm-with-mcp" else "splinterm";
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package.version;
  src = lib.fileset.toSource {
    root = src;
    fileset = lib.fileset.unions (
      map (name: src + "/${name}") [
        "Cargo.toml"
        "Cargo.lock"
        "crates"
        "config"
        "dist"
        "docs"
        "fixtures"
        "tests"
        "tools"
        "nix"
        "README.md"
        "LICENSE"
        "THIRD_PARTY.md"
      ]
    );
  };
  cargoLock.lockFile = ../Cargo.lock;
  SPLINTERM_BUILD_COMMIT = sourceRevision;

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [
    freetype
    pixman
    fontconfig
    libxkbcommon
    wayland
  ];
  nativeInstallCheckInputs = [
    desktop-file-utils
    python3
  ];

  cargoBuildFlags = [
    "-p"
    "splinterm"
    "-p"
    "splinterd"
    "-p"
    "splinterm-relay"
    "-p"
    "splinterm-pty"
    "--bin"
    "splinterm"
    "--bin"
    "splinterd"
    "--bin"
    "splinterm-relay"
    "--bin"
    "splinterm-pty-child"
  ]
  ++ lib.optionals withMcp [
    "-p"
    "splinterm-mcp"
    "--bin"
    "splinterm-mcp"
  ];

  # The complete upstream suite also requires fonts, /bin/sh, and a live user
  # manager. Keep sandbox checks bounded to the portable remote-profile contract.
  cargoTestFlags = [
    "-p"
    "splinterm"
    "--lib"
    "remote::tests"
  ];
  checkFlags = [ "--test-threads=1" ];

  postPatch = ''
    # Do not wrap/rename the ELF binaries: /proc/self/exe sibling lookup is part
    # of trusted UI authentication and PTY/relay discovery.
    substituteInPlace crates/splinterm/src/renderer/fonts.rs \
      --replace-fail 'Command::new("fc-match")' 'Command::new("${fontconfig}/bin/fc-match")'
    substituteInPlace crates/splinterm/src/wayland.rs \
      --replace-fail 'Command::new("xdg-open")' 'Command::new("${xdg-utils}/bin/xdg-open")'
    substituteInPlace crates/splinterm/src/app/local_service.rs \
      --replace-fail 'ProcessCommand::new("systemctl")' 'ProcessCommand::new("${systemd}/bin/systemctl")'
    substituteInPlace crates/splinterm/src/remote.rs \
      --replace-fail 'program: OsString::from("ssh")' 'program: OsString::from("${openssh}/bin/ssh")' \
      --replace-fail 'OsStr::new("ssh")' 'OsStr::new("${openssh}/bin/ssh")'
  '';

  postInstall = ''
    install -Dm755 dist/bin/splinterm-xdg-terminal-exec "$out/bin/splinterm-xdg-terminal-exec"
    substituteInPlace "$out/bin/splinterm-xdg-terminal-exec" \
      --replace-fail '#!/bin/sh' '#!${runtimeShell}' \
      --replace-fail 'splinterm ping' "$out/bin/splinterm ping" \
      --replace-fail 'exec splinterm ' "exec $out/bin/splinterm " \
      --replace-fail 'command -v systemctl' 'command -v ${systemd}/bin/systemctl' \
      --replace-fail 'systemctl --user' '${systemd}/bin/systemctl --user' \
      --replace-fail 'sleep 0.05' '${coreutils}/bin/sleep 0.05'
    for alias in splinterm-dojos splinterm-sessions splinterm-reopen; do
      ln -s splinterm-xdg-terminal-exec "$out/bin/$alias"
    done
    install -Dm755 tools/automation/splinterm-dojo-picker.py "$out/bin/splinterm-dojo-picker"
    substituteInPlace "$out/bin/splinterm-dojo-picker" \
      --replace-fail '#!/usr/bin/env python3' '#!${python3}/bin/python3' \
      --replace-fail 'os.environ.get("SPLINTERM_CLI", "splinterm")' "os.environ.get(\"SPLINTERM_CLI\", \"$out/bin/splinterm\")"
    ln -s splinterm-dojo-picker "$out/bin/splinterm-session-picker"

    install -Dm644 dist/applications/com.oldjobobo.splinterm.desktop \
      "$out/share/applications/com.oldjobobo.splinterm.desktop"
    substituteInPlace "$out/share/applications/com.oldjobobo.splinterm.desktop" \
      --replace-fail 'Exec=splinterm' "Exec=$out/bin/splinterm"
    install -Dm644 dist/icons/com.oldjobobo.splinterm.svg \
      "$out/share/icons/hicolor/scalable/apps/com.oldjobobo.splinterm.svg"
    install -Dm644 dist/metainfo/com.oldjobobo.splinterm.metainfo.xml \
      "$out/share/metainfo/com.oldjobobo.splinterm.metainfo.xml"
    mkdir -p "$out/share/doc/splinterm" "$out/share/licenses/splinterm"
    cp README.md docs/*.md config/splinterm/config.ini "$out/share/doc/splinterm/"
    cp config/splinterm/presets.toml "$out/share/doc/splinterm/presets.example.toml"
    cp LICENSE THIRD_PARTY.md "$out/share/licenses/splinterm/"

    install -Dm644 dist/systemd/user/splinterd.service "$out/lib/systemd/user/splinterd.service"
    install -Dm644 dist/systemd/user/app-splinterm.slice "$out/lib/systemd/user/app-splinterm.slice"
    substituteInPlace "$out/lib/systemd/user/splinterd.service" \
      --replace-fail '/usr/bin/splinterd' "$out/bin/splinterd" \
      --replace-fail '/usr/bin/kill' '${coreutils}/bin/kill' \
      --replace-fail '/usr/share/doc/splinterm' "$out/share/doc/splinterm"
    substituteInPlace "$out/lib/systemd/user/app-splinterm.slice" \
      --replace-fail '/usr/share/doc/splinterm' "$out/share/doc/splinterm"
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    for binary in splinterm splinterd splinterm-relay splinterm-pty-child; do
      test -x "$out/bin/$binary"
      test ! -L "$out/bin/$binary"
      test ! -e "$out/bin/.$binary-wrapped"
    done
    test "$(head -n 1 "$out/bin/splinterm-xdg-terminal-exec")" = '#!${runtimeShell}'
    ${if withMcp then "test -x \"$out/bin/splinterm-mcp\"" else "test ! -e \"$out/bin/splinterm-mcp\""}
    "$out/bin/splinterm" --help >/dev/null
    python3 nix/package-smoke.py "$out" '${runtimeShell}'
    desktop-file-validate "$out/share/applications/com.oldjobobo.splinterm.desktop"
    runHook postInstallCheck
  '';

  meta = {
    description = "Persistent Wayland terminal with a headless session daemon and SSH relay";
    homepage = "https://github.com/OldJobobo/splinterm";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" ];
    mainProgram = "splinterm";
  };
}
