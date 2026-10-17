#!/bin/bash
# The Thor's session, from this repository, into a home directory: ClamShell (the program given), sway's
# config and the outputs script, ClamShell's service, mako's and foot's configs, the Maintenance app's
# desktop file, the dock's first pins (only where there are none yet), and the login that starts sway on
# tty1. thor-image.sh writes it into the image; maintenance writes it again on the device after pulling
# (then: systemctl --user daemon-reload, and ClamShell restarted).
#
#   session.sh HOME_DIR FINAL_HOME PROGRAM
#
# HOME_DIR: where to write (in the image: its root's /home/thor). FINAL_HOME: that home's path on the
# device (/home/thor), for paths in the files. PROGRAM: ClamShell built for aarch64.
set -euo pipefail
H=$1 FINAL=$2 PROGRAM=$3
REPO=$(cd "$(dirname "$0")/../.." && pwd)
C=$FINAL/.local/bin/clamshell
mkdir -p "$H/.config/sway" "$H/.config/mako" "$H/.config/foot" "$H/.config/systemd/user" "$H/.local/bin" \
    "$H/.local/share/applications" "$H/.local/share/clamshell"
# (A running ClamShell keeps its file: replace it rather than write into it.)
install -m 755 "$PROGRAM" "$H/.local/bin/clamshell.new"
mv "$H/.local/bin/clamshell.new" "$H/.local/bin/clamshell"
install -m 644 "$REPO/clamshell/sway/clamshell.conf" "$H/.config/sway/clamshell.conf"
install -m 644 "$REPO/clamshell/mako/config" "$H/.config/mako/config"
install -m 644 "$REPO/clamshell/foot/foot.ini" "$H/.config/foot/foot.ini"

# The outputs are written at each session start, as ROCKNIX does (its autostart/common/111-sway-init): the
# panels are natively portrait, and the kernel says how to turn them (fbcon's rotate, from the panel's
# orientation in the device tree). Unverified: that the bottom panel wants the same angle as the top.
cat > "$H/.local/bin/thor-outputs" <<'SCRIPT'
#!/bin/sh
case "$(cat /sys/class/graphics/fbcon/rotate 2> /dev/null)" in
    1) angle=90 ;; 2) angle=180 ;; 3) angle=270 ;; *) angle=0 ;;
esac
cat > "$HOME/.config/sway/outputs" <<EOF
# Written by thor-outputs at session start (fbcon rotate: $angle).
output DSI-2 transform $angle position 0 0
output DSI-1 transform $angle
input "0:0:top_touchscreen" map_to_output DSI-2
input "0:0:bottom_touchscreen" map_to_output DSI-1
EOF
SCRIPT
chmod 755 "$H/.local/bin/thor-outputs"
echo '[ "$(tty)" = /dev/tty1 ] && "$HOME/.local/bin/thor-outputs" && exec sway > "$HOME/sway.log" 2>&1' > "$H/.bash_profile"

cat > "$H/.config/sway/config" <<CONFIG
# The Thor: DSI-2 the main (top) screen, DSI-1 the second (bottom) screen (as ROCKNIX and Kettle have them).
# The machine is here (outputs, inputs, keys, starting ClamShell); how a ClamShell session behaves
# (borders, focus, tabs) is clamshell.conf. sway's default config, with its key bindings, isn't read:
# this one replaces it. Written by tools/device/session.sh.
include ~/.config/sway/outputs
include ~/.config/sway/clamshell.conf
# A keyboard's Print key: a screenshot through ClamShell (clamshell ctl writes to its control pipe). The
# Thor's volume and brightness keys ClamShell reads itself from evdev (CLAMSHELL_SESSION=device).
bindsym --locked Print exec $C ctl screenshot
# Notifications (ClamShell's low battery warning, apps'), on the main screen: ~/.config/mako/config.
exec mako
exec systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP && systemctl --user restart clamshell.service
CONFIG

cat > "$H/.config/systemd/user/clamshell.service" <<EOF
[Unit]
Description=ClamShell
After=graphical-session.target

[Service]
# The control pipe: sway's key bindings (clamshell ctl) write to it.
ExecStartPre=-/usr/bin/mkfifo %t/clamshell-control
ExecStart=$C
Environment=CLAMSHELL_CONTROL=%t/clamshell-control
# ClamShell is the device's session: it puts the device to sleep when idle (dim, screens off, then sleep
# through logind) and takes the hardware keys (volume, brightness). Desktop runs leave this unset.
Environment=CLAMSHELL_SESSION=device
Environment=CLAMSHELL_BOTTOM_OUTPUT=DSI-1
# Touchpad mode takes the bottom screen's touchscreen (the Thor has two).
Environment=CLAMSHELL_TOUCHSCREEN=bottom_touchscreen
Restart=on-failure
RestartSec=2
EOF

# Maintenance (Claude Code's pass over the system, tools/device/maintain, in a terminal): an installed app,
# in shelf's Apps like any other.
cat > "$H/.local/share/applications/thor-maintenance.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Maintenance
Exec=foot --app-id=maintenance $FINAL/thor/tools/device/maintain
Icon=utilities-terminal
EOF

# The dock's first pins: Steam, the terminal, Maintenance (the user's own, once there are any, are kept).
if [ ! -e "$H/.local/share/clamshell/dock.txt" ]; then
    printf '# ClamShell dock, format 1\ndesktop:steam\ndesktop:foot\ndesktop:thor-maintenance\n' > "$H/.local/share/clamshell/dock.txt"
fi
