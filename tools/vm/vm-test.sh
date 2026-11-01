#!/bin/bash
# The VM suite end to end: builds what the VM runs, boots it headless, waits for ClamShell, brings up
# the Bluetooth test device, runs tools/vm/vm-checks.py, and stops the VM. About twenty minutes; run before
# pushing, after changes to the pad, the session or the devices. Exit status: the checks'. Each run's whole
# output, and the machine's, are kept in ~/.local/state/thor/vm-runs/, and the run ends by listing what
# failed.
#
#   tools/vm/vm-test.sh [CHECK...]   (all checks, or these: vm-checks.py --list)
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# One machine at a time. A second run's QEMU can't open the disk, but the run would go on with the first
# run's machine, which answers on the same ssh port: set it up again and restart its session under its
# checks, and stop it at the end.
DISK="qemu-system-.*thor-vm/.*/disk.qcow2"
if pgrep -f "^$DISK" > /dev/null; then echo "a VM runs already (another run?): not starting another"; exit 1; fi
cd "$ROOT/clamshell" && cargo build --release --quiet && cargo build --release --quiet -p clamshell-system --examples || exit 1
cd "$ROOT"
RUNS="${XDG_STATE_HOME:-$HOME/.local/state}/thor/vm-runs"
STAMP=$(date +%Y%m%d-%H%M%S)
mkdir -p "$RUNS"
LOG="$RUNS/$STAMP-machine.log"
python3 tools/vm/vm.py run --headless > "$LOG" 2>&1 &
VM=$!
# (Checks that restart the machine leave a QEMU that isn't ours by parentage: stop any on the VM's disk.)
stop() { kill "$VM" 2>/dev/null; wait "$VM" 2>/dev/null; pkill -f "^$DISK" 2>/dev/null; }
trap stop EXIT INT TERM
# The session and system files as this repository has them, as soon as the machine answers (the login
# script among them: one from an older checkout may not start the session at all); then the console's
# login again if the session isn't up.
for _ in $(seq 1 60); do
    kill -0 "$VM" 2> /dev/null || { echo "the VM stopped (log: $LOG)"; exit 1; }
    timeout 8 python3 tools/vm/vm.py ssh true 2> /dev/null && break
    sleep 4
done
python3 tools/vm/vm.py sync > /dev/null || { echo "syncing the VM's files failed (log: $LOG)"; exit 1; }
timeout 8 python3 tools/vm/vm.py ssh 'systemctl --user is-active clamshell' 2> /dev/null | grep -q '^active' \
    || python3 tools/vm/vm.py ssh 'sudo systemctl daemon-reload; sudo systemctl restart getty@tty1' > /dev/null
up=""
for _ in $(seq 1 60); do
    if timeout 8 python3 tools/vm/vm.py ssh 'systemctl --user is-active clamshell' 2>/dev/null | grep -q '^active'; then up=1; break; fi
    sleep 4
done
[ -n "$up" ] || { echo "the VM didn't start ClamShell (log: $LOG)"; exit 1; }
# (The session again, with the unit as synced.)
python3 tools/vm/vm.py restart > /dev/null || { echo "restarting the session failed (log: $LOG)"; exit 1; }
sleep 3
python3 tools/vm/vm.py bluetooth || { echo "the Bluetooth test device didn't come up (log: $LOG)"; exit 1; }
sleep 5
OUT="$RUNS/$STAMP-checks.log"
python3 -u tools/vm/vm-checks.py "$@" 2>&1 | tee "$OUT"
status=${PIPESTATUS[0]}
echo "the whole run: $OUT (the machine's log: $LOG)"
exit "$status"
