# NixOS installation

The repository flake builds the **complete Wayland desktop terminal**, not a
headless-only edition. The default package includes `splinterm`, `splinterd`,
`splinterm-relay`, `splinterm-pty-child`, the desktop entry/icon, and the Dojo
launcher aliases. The same package works as a headless SSH session host without
a compositor. Omarchy is not required; its optional integration commands are
not part of NixOS setup.

Initial packaging targets `x86_64-linux`. Other architectures are not advertised
until built and tested. A native Wayland desktop is required for GUI windows;
headless operation requires no graphical session. This packaging work does not
establish graphical acceptance on every NixOS compositor.

## Recommended: NixOS module

Add Splinterm to your configuration flake and commit the resulting `flake.lock`:

```nix
inputs.splinterm.url = "github:OldJobobo/splinterm/<reviewed-revision>";
```

Replace `<reviewed-revision>` with the commit containing Nix support. Until that
branch is merged/published, use a local checkout containing the implementation.
The public release installer and Arch packages are **not** NixOS installers.
The package's own pinned nixpkgs supplies its Rust/native toolchain; following a
host's older nixpkgs input may not provide a sufficiently recent Rust compiler.

Import the module into each desired `nixosConfigurations.<host>.modules` list:

```nix
inputs.splinterm.nixosModules.default
```

Then enable it in a host module:

```nix
{
  programs.splinterm.enable = true;
}
```

Rebuild NixOS normally from an external terminal or SSH connection, **not a shell
owned by the daemon being replaced**. The module installs the full package and
user service, enables Fontconfig, and adds JetBrains Mono Nerd Font plus CJK and
emoji fonts. It does not select a compositor, change your default terminal,
configure SSH authorization, enable lingering, or modify home-directory files.

Launch **Splinterm** from the application menu. Its launcher starts the user
daemon on demand; Recent Dojos and Reopen Last Dojo use the same package. To
start it from an existing shell:

```sh
splinterm-xdg-terminal-exec
```

`nix run .# -- --help` runs the raw CLI, not the daemon-starting desktop wrapper.
Installing only the package with `nix profile install` does not register its
systemd user unit with NixOS. Use the module for the supported desktop setup.

## Headless hosts

Use the same module and package, but omit the desktop font integration and start
the daemon automatically:

```nix
{
  programs.splinterm = {
    enable = true;
    desktopIntegration = false;
    startAtLogin = true;
  };

  # Optional: start this user's manager at boot and keep it after logout.
  users.users.operator.linger = true;
}
```

Replace `operator` with an existing account. `startAtLogin` enables the service
for all user managers; lingering is explicitly per user. The GUI remains in the
package, so a desktop profile can simply restore `desktopIntegration = true`.
No local daemon TCP listener or firewall opening is needed; remote access uses
OpenSSH. Configure the policy separately using `docs/remote.md`.

The unit retains the upstream `--require-workload-cgroups` requirement,
`app-splinterm.slice`, resource limits, SIGHUP reload and SIGINT cleanup. It
reads the optional owner-controlled `~/.config/splinterm/daemon.env`, matching
the Arch unit. Do not blindly add service sandboxing that blocks user PTYs,
Unix sockets, user-manager D-Bus, or workload scope creation.

## Remote profiles targeting NixOS

On each connecting client, set the remote executable explicitly:

```toml
version = 1

[remotes.server]
host = "server"
executable = "/run/current-system/sw/bin/splinterm"
```

`executable` defaults to `/usr/bin/splinterm` for existing Arch profiles. It is
an absolute path on the **remote** host, not a local file or arbitrary command.
Only ASCII letters, digits, `/`, `.`, `_`, and `-` are accepted; empty, `.` and
`..` components are rejected. No whitespace, tilde expansion, shell operators,
or user-supplied arguments are accepted. The relay arguments remain fixed.
Both ends must run protocol-compatible versions. The connecting client also
needs a version supporting the new profile field.

Use the resolved store executable and its SHA-256 digest when authorizing the
relay or MCP adapter in policy, not an assumed `/usr/bin` identity:

```sh
readlink -f /run/current-system/sw/bin/splinterm-relay
sha256sum /run/current-system/sw/bin/splinterm-relay
```

Review/update policy after package changes. A NixOS activation does not silently
grant a new executable automation authority.

## Optional MCP adapter

The default package does not include MCP. Select the complete matching variant
rather than mixing binaries from different derivations:

```nix
{ pkgs, ... }: {
  programs.splinterm.package =
    inputs.splinterm.packages.${pkgs.stdenv.hostPlatform.system}.splinterm-with-mcp;
}
```

Here `inputs` must be in your module's lexical scope or passed through your
configuration's `specialArgs`. This variant includes the main package and the
adapter; it does not configure an MCP host or grant policy permissions. See
`docs/mcp.md` for explicit setup.

## Executable identity and upgrades

The client, daemon, relay and PTY helper are unwrapped sibling ELF binaries in
one immutable store output. NixOS profile symlinks lead to those binaries; the
package does not rename them to `.wrapped` files. Desktop scripts and service
units use matching store paths, and native helpers such as `fc-match` are bound
to their Nix dependencies without an `LD_LIBRARY_PATH` wrapper.

A store generation change does not migrate a running daemon's processes.
Schedule upgrades, stop the old user daemon, switch configuration, and restart
it; close and reopen old GUI windows. Otherwise old daemon/new client identity
checks may reject connections even when the protocol version is unchanged.
A rollback must restore both the package/service generation and any associated
policy identities. Persistent shell processes are not restored by rollback.

## Build and non-graphical checks

From a Git checkout containing the flake:

```sh
nix build .#splinterm
nix flake check
nix build .#splinterm-with-mcp
```

Commit provenance comes from the Git flake revision; a dirty checkout is for
development only and is not a release artifact. `nix/package.nix` also accepts
an explicit 40-character `sourceRevision` for non-flake source packaging; do not
invent a revision for a source snapshot.

Checks cover remote-profile validation, installed desktop-file validity,
unwrapped sibling layout, a private non-graphical daemon with trusted human CLI
access, PTY helper execution and clean shutdown, and desktop/headless/disabled
NixOS module evaluation.
The private daemon test does not require a live user manager and does not test
workload cgroups. The broader upstream Rust suite remains a separate boundary.
Live NixOS user-service/cgroup/SSH testing and approved Wayland graphical
acceptance are additional deployment checks, not implied by a successful build.
