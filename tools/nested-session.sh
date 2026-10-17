#!/bin/sh
# ClamShell on a desktop, to try it as on the device: a nested sway in two windows (the Thor's screens at
# their size: the top 1920x1080, the bottom 1240x1080) with ClamShell in it, as the session would run
# it. ClamShell uses the desktop's own folders (~/.config/clamshell/library.json, ~/.local/share/clamshell,
# ~/.cache/clamshell), and what it starts (RetroArch, mpv) theirs. Keys stand in for the pad (arrows,
# Enter A, Backspace B, F Y, P X, C Start, Q and E the shoulders, Home and End the triggers, Tab View).
# Closing either window ends it. Needs a release build (cargo build --release in clamshell/).
#
#   tools/nested-session.sh
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=$REPO/clamshell/target/release/clamshell
[ -x "$BIN" ] || { echo "no $BIN: cargo build --release in clamshell/"; exit 1; }
CONF=$(mktemp "${XDG_RUNTIME_DIR:-/tmp}/clamshell-nested.XXXXXX.conf")
trap 'rm -f "$CONF"' EXIT
cat > "$CONF" <<EOF
include $REPO/clamshell/sway/clamshell.conf
output WL-1 resolution 1920x1080 position 0 0
output WL-2 resolution 1240x1080 position 0 1080
exec env CLAMSHELL_BOTTOM_OUTPUT=WL-2 $BIN
EOF
# (A sway of its own, in windows on this desktop; the GPU renderer, so games' Vulkan and GL work.)
env -u SWAYSOCK -u I3SOCK WLR_BACKENDS=wayland WLR_WL_OUTPUTS=2 sway -c "$CONF"
