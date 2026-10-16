#!/bin/sh
# The VM suite end to end: builds what the VM runs, boots it headless, waits for ClamShell, brings up
# the Bluetooth test device, runs tools/vm-checks.py, and stops the VM. About twenty minutes; run before
# pushing, after changes to the pad, the session or the devices. Exit status: the checks'.
#
#   tools/vm-test.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT/clamshell" && cargo build --release --quiet && cargo build --release --quiet -p clamshell-policy --examples || exit 1
cd "$ROOT"
LOG=$(mktemp "${TMPDIR:-/tmp}/vm-run.XXXXXX")
python3 tools/vm.py run --headless > "$LOG" 2>&1 &
VM=$!
# (Checks that restart the machine leave a QEMU that isn't ours by parentage: stop any on the VM's disk.)
stop() { kill "$VM" 2>/dev/null; wait "$VM" 2>/dev/null; pkill -f "qemu-system-.*thor-vm/.*/disk.qcow2" 2>/dev/null; }
trap stop EXIT INT TERM
up=""
for _ in $(seq 1 60); do
    if timeout 8 python3 tools/vm.py ssh 'systemctl --user is-active clamshell' 2>/dev/null | grep -q '^active'; then up=1; break; fi
    sleep 4
done
[ -n "$up" ] || { echo "the VM didn't start ClamShell (log: $LOG)"; exit 1; }
python3 tools/vm.py bluetooth
sleep 5
python3 tools/vm-checks.py
