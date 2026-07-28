#!/usr/bin/env bash
set -u
set +e
env SPLINTERM_CONFIG=/home/oldjobobo/Projects/splinterm/tools/benchmark/profiles/splinterm.ini SPLINTERM_ENABLE_DEV_ATTACH=1 SPLINTERM_SOCKET=/tmp/splinterbench-retention-splinterm-1003366/splinterd.sock XDG_STATE_HOME=/tmp/splinterbench-retention-splinterm-1003366/xdg-state /tmp/splinterm-plan0011-final-candidate-bin/splinterm launch --new --name splinterbench -- /usr/bin/python /home/oldjobobo/Projects/splinterm/tools/benchmark/workloads/bench-child.py retention --lines 5000 --columns 80 --ready-file /tmp/splinterbench-retention-splinterm-1003366/ready.json --start-file /tmp/splinterbench-retention-splinterm-1003366/start --done-file /tmp/splinterbench-retention-splinterm-1003366/done.json --hold-seconds 12.0 >/tmp/splinterbench-retention-splinterm-1003366/launch.stdout 2>/tmp/splinterbench-retention-splinterm-1003366/launch.stderr
exit_code=$?
printf '{"exit_code":%s}\n' "$exit_code" >/tmp/splinterbench-retention-splinterm-1003366/launch.status.json
exit "$exit_code"
