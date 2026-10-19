#!/bin/sh
# ClamShell on a desktop, to try it as on the device: a nested sway in two windows (the Thor's screens at
# their size: the top 1920x1080, the bottom 1240x1080) with ClamShell in it, as the session would run
# it. ClamShell uses the desktop's own folders (~/.config/clamshell/library.json, ~/.local/share/clamshell,
# ~/.cache/clamshell), and what it starts (RetroArch, mpv) theirs. Keys stand in for the pad (arrows,
# Enter A, Backspace B, F Y, P X, C Start, Q and E the shoulders, Home and End the triggers, Tab View);
# a controller's Guide button is the Thor's Home button, alone and held with others.
#
# As on the device, both are user units: sway `clamshell-nested-sway.service` (this waits for it), and
# ClamShell `clamshell-nested.service`, started by it once it is up and bound to it (notifying, with its
# watchdog). Closing either window, or stopping sway's unit, ends it all: ClamShell, and what it launched
# (its slices, under `app-graphical-clamshell.slice`, stopped after sway, as uwsm stops the device's).
# Their logs: journalctl --user -u clamshell-nested-sway -u clamshell-nested. Builds ClamShell first (the
# fast profile: optimised, built in seconds).
#
#   tools/nested-session.sh
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
# (The user's login shell for what ClamShell starts, foot's shell: not whatever SHELL started this.)
SHELL=$(getent passwd "$(id -un)" | cut -d: -f7)
export SHELL
BIN=$REPO/clamshell/target/fast/clamshell
(cd "$REPO/clamshell" && cargo build --profile fast --quiet) || exit 1
CONF=${XDG_RUNTIME_DIR:?}/clamshell-nested.conf
cat > "$CONF" <<EOF
include $REPO/clamshell/sway/clamshell.conf
output WL-1 resolution 1920x1080 position 0 0
output WL-2 resolution 1240x1080 position 0 1080
# ClamShell as a unit of its own, with sway's display, bound to sway's unit.
exec systemd-run --user --quiet --collect --unit=clamshell-nested \\
    --property=BindsTo=clamshell-nested-sway.service --property=After=clamshell-nested-sway.service \\
    --property=Type=notify --property=NotifyAccess=main --property=WatchdogSec=30 \\
    --property=Restart=on-failure --property=RestartSec=2 --slice=session.slice \\
    -E WAYLAND_DISPLAY -E SWAYSOCK -E SHELL -E CLAMSHELL_BOTTOM_OUTPUT=WL-2 $BIN
EOF
# (One at a time: one started before is stopped, with what it launched.)
systemctl --user stop clamshell-nested-sway.service 2> /dev/null || true
trap 'systemctl --user stop clamshell-nested-sway.service' INT TERM
# A sway of its own, in windows on this desktop (its display: this one's; the GPU renderer, so games'
# Vulkan and GL work), as a unit: stopped, it stops what ClamShell launched.
systemd-run --user --quiet --collect --wait --unit=clamshell-nested-sway --slice=session.slice \
    -E WAYLAND_DISPLAY -E XDG_CURRENT_DESKTOP=sway -E SHELL -E WLR_BACKENDS=wayland -E WLR_WL_OUTPUTS=2 \
    --property=ExecStopPost="systemctl --user stop app-graphical-clamshell.slice" \
    sway -c "$CONF"
