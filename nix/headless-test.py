# Executed by the NixOS test driver, with constants supplied by headless-test.nix.
import shlex

from test_driver.machine import Machine


def as_user(command: str) -> str:
    # No PAM session: probes must not create logins or accidentally enable linger.
    return (
        "setpriv --reuid=1000 --regid=1000 --init-groups env -i "
        "HOME=/home/operator USER=operator LOGNAME=operator "
        "PATH=/run/current-system/sw/bin XDG_RUNTIME_DIR=/run/user/1000 "
        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
        f"SHELL={SHELL} {SHELL} -c {shlex.quote(command)}"
    )


def user(machine: Machine, command: str) -> str:
    return machine.succeed(as_user(command), timeout=30).strip()


def assert_unauthorized(machine: Machine, command: str) -> None:
    status, output = machine.execute(as_user(command) + " 2>&1", timeout=30)
    assert status != 0, output
    assert "[unauthorized]" in output, output


def property_value(machine: Machine, unit: str, name: str) -> str:
    return user(machine, f"systemctl --user show {shlex.quote(unit)} -p {name} --value")


def wait_for_daemon(machine: Machine, package: str = PACKAGE) -> str:
    machine.wait_for_unit("user@1000.service", timeout=60)
    machine.wait_until_succeeds(
        as_user("systemctl --user is-active splinterd.service"), timeout=60
    )
    machine.wait_until_succeeds(as_user("splinterm list"), timeout=30)
    pid = property_value(machine, "splinterd.service", "MainPID")
    assert (
        machine.succeed(f"readlink /proc/{pid}/exe").strip()
        == f"{package}/bin/splinterd"
    )
    environment = machine.succeed(f"tr '\\0' '\\n' < /proc/{pid}/environ")
    assert "DISPLAY=" not in environment
    assert "WAYLAND_DISPLAY=" not in environment
    assert "SPLINTERM_ENABLE_DEV_ATTACH=" not in environment
    return pid


def create_workload(machine: Machine, label: str) -> tuple[str, str]:
    directory = f"/home/operator/{label}"
    user(machine, f"mkdir -p {directory}")
    user(machine, f"splinterm new {label} --cwd {directory} -- {WORKLOAD} {directory}")
    machine.wait_for_file(f"{directory}/ready", timeout=30)
    pid = machine.succeed(f"cat {directory}/pid").strip()
    assert machine.succeed(f"cat {directory}/uid").strip() == "1000"
    assert machine.succeed(f"cat {directory}/home").strip() == "/home/operator"
    assert machine.succeed(f"cat {directory}/shell").strip() == SHELL
    assert (
        machine.succeed(f"cat {directory}/environment").strip() == "owner-environment"
    )
    assert machine.succeed(f"cat {directory}/cwd").strip() == directory
    assert machine.succeed(f"cat {directory}/tty").strip().startswith("/dev/pts/")
    cgroup = machine.succeed(f"cat /proc/{pid}/cgroup").strip().removeprefix("0::")
    assert "/app-splinterm.slice/app-splinterm-dojo" in cgroup
    assert cgroup.endswith(".scope")
    assert "splinterd.service" not in cgroup
    scope = cgroup.rsplit("/", 1)[-1]
    dojo = cgroup.rsplit("/", 2)[-2]
    assert property_value(machine, scope, "TasksMax") == "512"
    assert property_value(machine, dojo, "TasksMax") == "1024"
    assert property_value(machine, "app-splinterm.slice", "TasksMax") == "2048"
    assert "splinterd.service" in property_value(machine, dojo, "PartOf").split()
    limits = [
        int(property_value(machine, unit, "MemoryHigh"))
        for unit in (scope, dojo, "app-splinterm.slice")
    ]
    assert 0 < limits[0] < limits[1] < limits[2]
    return pid, cgroup


def wait_for_cleanup(machine: Machine, pid: str, cgroup: str) -> None:
    machine.wait_until_fails(f"test -e /proc/{pid}", timeout=60)
    machine.wait_until_fails(f"test -d /sys/fs/cgroup{cgroup}", timeout=60)
    machine.wait_until_fails(
        f"test -d /sys/fs/cgroup{cgroup.rsplit('/', 1)[0]}", timeout=60
    )


ondemand.start()
automatic.start(allow_reboot=True)
ondemand.wait_for_unit("multi-user.target", timeout=60)
automatic.wait_for_unit("multi-user.target", timeout=60)

with subtest("on-demand defaults do not enable service or lingering"):
    ondemand.fail("test -e /var/lib/systemd/linger/operator")
    ondemand.fail("systemctl is-active user@1000.service")
    ondemand.succeed("systemctl start user@1000.service")
    ondemand.wait_for_unit("user@1000.service", timeout=60)
    # Nix-generated units may be static/linked: is-enabled's exit code is not
    # an autostart assertion. Check the actual target dependency instead.
    assert (
        "default.target"
        not in property_value(ondemand, "splinterd.service", "WantedBy").split()
    )
    ondemand.fail(as_user("systemctl --user is-active splinterd.service"))
    user(ondemand, "systemctl --user start splinterd.service")
    wait_for_daemon(ondemand)
    assert property_value(ondemand, "splinterd.service", "TasksMax") == "2048"
    assert "--require-workload-cgroups" in property_value(
        ondemand, "splinterd.service", "ExecStart"
    )
    user(ondemand, "systemctl --user stop splinterd.service")
    ondemand.wait_until_fails(
        "test -S /run/user/1000/splinterm/splinterd.sock", timeout=30
    )
    ondemand.succeed("systemctl stop user@1000.service")

with subtest("automatic service starts headlessly under explicit lingering"):
    automatic.succeed("test -e /var/lib/systemd/linger/operator")
    daemon_pid = wait_for_daemon(automatic)
    assert (
        "default.target"
        in property_value(automatic, "splinterd.service", "WantedBy").split()
    )
    assert_unauthorized(automatic, "splinterm --output json list")
    # A valid client from the other complete derivation must not gain local UI authority.
    assert_unauthorized(automatic, f"{WITH_MCP}/bin/splinterm list")

with subtest("workload scopes use real delegated cgroups and are collected on exit"):
    pid, cgroup = create_workload(automatic, "exiting-workload")
    automatic.succeed(f"kill -TERM {pid}")
    wait_for_cleanup(automatic, pid, cgroup)
    assert property_value(automatic, "splinterd.service", "MainPID") == daemon_pid
    user(automatic, "splinterm list")

with subtest("configured default shell gets a PTY and the requested working directory"):
    user(automatic, "mkdir -p /home/operator/default-shell")
    user(automatic, "splinterm new default-shell --cwd /home/operator/default-shell")
    automatic.wait_until_succeeds(f"pgrep -P {daemon_pid}", timeout=30)
    children = automatic.succeed(f"pgrep -P {daemon_pid}").split()
    assert len(children) == 1, children
    shell_pid = children[0]
    # pgrep can observe the short-lived PTY helper before it execs the shell.
    automatic.wait_until_succeeds(
        f'test "$(readlink /proc/{shell_pid}/exe)" = {SHELL}', timeout=30
    )
    assert (
        automatic.succeed(f"readlink /proc/{shell_pid}/cwd").strip()
        == "/home/operator/default-shell"
    )
    assert (
        automatic.succeed(f"readlink /proc/{shell_pid}/fd/0")
        .strip()
        .startswith("/dev/pts/")
    )
    user(automatic, "systemctl --user stop splinterd.service")
    automatic.wait_until_fails(f"test -e /proc/{shell_pid}", timeout=60)
    user(automatic, "systemctl --user start splinterd.service")
    daemon_pid = wait_for_daemon(automatic)

with subtest("SSH logout and native remote reconnect do not kill lingering workloads"):
    automatic.wait_for_unit("sshd.service", timeout=60)
    host_key = automatic.succeed("cat /etc/ssh/ssh_host_ed25519_key.pub").strip()
    ondemand.succeed("install -d -m700 -o operator -g operator /home/operator/.ssh")
    ondemand.succeed(
        "install -m600 -o operator -g operator /etc/splinterm-test-key /home/operator/.ssh/test-key"
    )
    known_hosts = shlex.quote(f"automatic {host_key}\n")
    ondemand.succeed(f"printf %s {known_hosts} > /home/operator/.ssh/known_hosts")
    ondemand.succeed(
        "chown operator:operator /home/operator/.ssh/known_hosts; chmod 600 /home/operator/.ssh/known_hosts"
    )
    ssh = (
        "ssh -T -i /home/operator/.ssh/test-key -o IdentitiesOnly=yes "
        "-o BatchMode=yes -o StrictHostKeyChecking=yes -o ConnectTimeout=5 "
        "-o UserKnownHostsFile=/home/operator/.ssh/known_hosts operator@automatic "
    )
    user(ondemand, ssh + shlex.quote("splinterm list"))
    remote_config = (
        'version = 1\n[remotes.vm]\nhost = "automatic"\nuser = "operator"\n'
        'executable = "/run/current-system/sw/bin/splinterm"\n'
        'identity_files = ["/home/operator/.ssh/test-key"]\n'
        'known_hosts_file = "/home/operator/.ssh/known_hosts"\n'
    )
    user(
        ondemand,
        f"printf %s {shlex.quote(remote_config)} > /home/operator/.config/splinterm/remotes.toml",
    )
    user(ondemand, "splinterm remote check vm")
    directory = "/home/operator/ssh-workload"
    user(
        ondemand,
        ssh
        + shlex.quote(
            f"mkdir -p {directory}; splinterm new ssh-workload --cwd {directory} -- {WORKLOAD} {directory}"
        ),
    )
    automatic.wait_for_file(f"{directory}/ready", timeout=30)
    pid = automatic.succeed(f"cat {directory}/pid").strip()
    cgroup = automatic.succeed(f"cat /proc/{pid}/cgroup").strip().removeprefix("0::")
    # systemd 258 also lists the lingering user manager as a session. Require
    # every human SSH session to be gone, without rejecting that manager.
    automatic.wait_until_succeeds(
        "for session in $(loginctl show-user operator -p Sessions --value); do "
        'test "$(loginctl show-session "$session" -p Class --value)" = manager || exit 1; done',
        timeout=60,
    )
    automatic.succeed(f"kill -0 {pid}")
    assert property_value(automatic, "splinterd.service", "MainPID") == daemon_pid
    assert "ssh-workload" in user(ondemand, "splinterm --remote vm list")
    user(ondemand, "splinterm remote check vm")
    automatic.succeed(f"kill -0 {pid}")

with subtest("stopping the service reaps workloads and removes socket and scopes"):
    user(automatic, "systemctl --user stop splinterd.service")
    wait_for_cleanup(automatic, pid, cgroup)
    automatic.wait_until_fails(
        "test -S /run/user/1000/splinterm/splinterd.sock", timeout=30
    )
    automatic.wait_until_fails("pgrep -u operator -x splinterm-pty-c", timeout=30)

with subtest(
    "switch complete package generations and roll back without relaxing identity"
):
    base_system = automatic.succeed("readlink -f /run/current-system").strip()
    automatic.succeed(
        f"{base_system}/specialisation/with-mcp/bin/switch-to-configuration test",
        timeout=120,
    )
    user(
        automatic,
        "systemctl --user daemon-reload; systemctl --user start splinterd.service",
    )
    wait_for_daemon(automatic, WITH_MCP)
    automatic.succeed("test -x /run/current-system/sw/bin/splinterm-mcp")
    assert_unauthorized(automatic, f"{PACKAGE}/bin/splinterm list")
    pid, cgroup = create_workload(automatic, "alternate-generation")
    user(automatic, "systemctl --user stop splinterd.service")
    wait_for_cleanup(automatic, pid, cgroup)
    automatic.succeed(f"{base_system}/bin/switch-to-configuration test", timeout=120)
    user(
        automatic,
        "systemctl --user daemon-reload; systemctl --user start splinterd.service",
    )
    wait_for_daemon(automatic)
    automatic.fail("test -e /run/current-system/sw/bin/splinterm-mcp")
    assert_unauthorized(automatic, f"{WITH_MCP}/bin/splinterm list")

with subtest("reboot restarts the service but never automatically restores commands"):
    pid, cgroup = create_workload(automatic, "reboot-workload")
    before = automatic.succeed("cat /home/operator/reboot-workload/ready").strip()
    assert "reboot-workload" in user(automatic, "splinterm list --all")
    automatic.reboot()
    daemon_pid = wait_for_daemon(automatic)
    assert automatic.succeed("cat /proc/sys/kernel/random/boot_id").strip() != before
    assert (
        automatic.succeed("cat /home/operator/reboot-workload/ready").strip() == before
    )
    automatic.fail(f"pgrep -P {daemon_pid}")
    assert "reboot-workload" in user(automatic, "splinterm list --all")
    automatic.fail(f"test -d /sys/fs/cgroup{cgroup}")
    user(automatic, "systemctl --user stop splinterd.service")
    automatic.wait_until_fails(
        "test -S /run/user/1000/splinterm/splinterd.sock", timeout=30
    )
