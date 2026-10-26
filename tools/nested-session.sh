#!/bin/sh
# ClamShell on a desktop, to try it as on the device: its compositor nested, in a window of this desktop
# showing both screens at the Thor's size. ClamShell uses the desktop's own folders
# (~/.config/clamshell/library.json, ~/.local/share/clamshell, ~/.cache/clamshell), and what it starts
# (RetroArch, mpv) theirs. Keys stand in for the pad (arrows, Enter A, Backspace B, F Y, P X, C Start, Q
# and E the shoulders, Home and End the triggers, Tab View); a controller's Guide button is the Thor's
# Home button, alone and held with others.
#
# As on the device, ClamShell is a user unit, `clamshell-nested.service` (notifying, with its watchdog);
# this waits for it. Closing its window, or stopping the unit, ends it and what it launched (its slices,
# under `app-graphical-clamshell.slice`). Its log: journalctl --user -u clamshell-nested. Builds ClamShell
# first (the fast profile: optimised, built in seconds).
#
#   tools/nested-session.sh
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
# (The user's login shell for what ClamShell starts, foot's shell: not whatever SHELL started this.)
SHELL=$(getent passwd "$(id -un)" | cut -d: -f7)
export SHELL
BIN=$REPO/clamshell/target/fast/clamshell
(cd "$REPO/clamshell" && cargo build --profile fast --quiet) || exit 1
# (One at a time: one started before is stopped, with what it launched.)
systemctl --user stop clamshell-nested.service 2> /dev/null || true
trap 'systemctl --user stop clamshell-nested.service' INT TERM
systemd-run --user --quiet --collect --wait --unit=clamshell-nested --slice=session.slice \
    --property=Type=notify --property=NotifyAccess=main --property=WatchdogSec=30 \
    --property=ExecStopPost="systemctl --user stop app-graphical-clamshell.slice" \
    -E WAYLAND_DISPLAY -E SHELL "$BIN" --nested
