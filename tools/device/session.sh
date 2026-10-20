#!/bin/bash
# The Thor's session, from this repository, into a home directory: ClamShell (the program given), sway's
# config and the outputs script, ClamShell's service, mako's and foot's configs, the Maintenance app's
# desktop file, the dock's first pins and the starter library.json (each only where there is none yet:
# the user's are kept) with the folders it names, and the login that starts the session on tty1: sway
# through uwsm (a systemd unit, graphical-session.target reached once its environment is in; ClamShell,
# mako and udiskie are user services of that target, launches in its app slice; all stopped, games
# first, when the session ends). thor-image.sh writes it into the image; maintenance writes it again on the device after pulling
# (then: systemctl --user daemon-reload, and ClamShell restarted: reset-failed first, so that a deliberate
# restart isn't counted as a failure; see thor-session).
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
install -m 755 "$REPO/clamshell/android/clamshell-android" "$H/.local/bin/clamshell-android"
install -D -m 644 "$REPO/clamshell/android/pyclip.py" "$H/.local/share/clamshell/python/pyclip.py"

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
install -m 755 "$REPO/tools/device/thor-session" "$H/.local/bin/thor-session"
cat > "$H/.bash_profile" <<'PROFILE'
# The session on the first console: sway through uwsm (thor-session); once it has run and ended, logged
# out, so that the console's autologin starts a new one. When it can't go on (it keeps failing, or
# ClamShell does), the console stays, saying why.
if [ "$(tty)" = /dev/tty1 ] && uwsm check may-start; then
    "$HOME/.local/bin/thor-session" && exit
fi
PROFILE

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
# ClamShell, mako and udiskie are user services of graphical-session.target, which uwsm reaches once
# sway is up and has put its environment (WAYLAND_DISPLAY, SWAYSOCK) in the user manager's: this.
exec exec uwsm finalize
CONFIG

cat > "$H/.config/systemd/user/clamshell.service" <<EOF
[Unit]
Description=ClamShell
# Started once the session is up, stopped with it.
PartOf=graphical-session.target
After=graphical-session.target
StartLimitIntervalSec=60
StartLimitBurst=4
OnFailure=clamshell-failed.service

[Service]
# Up once its screens are drawn; its main loop says it turns, and one that stops for 30 s (hung) is
# restarted: what it launched runs on in its scopes.
Type=notify
NotifyAccess=main
WatchdogSec=30
# The control pipe: sway's key bindings (clamshell ctl) write to it.
ExecStartPre=-/usr/bin/mkfifo %t/clamshell-control
ExecStart=$C
Environment=CLAMSHELL_CONTROL=%t/clamshell-control
# ClamShell is the device's session: it puts the device to sleep when idle (dim, screens off, then sleep
# through logind) and takes the hardware keys (volume, brightness). Desktop runs leave this unset.
Environment=CLAMSHELL_SESSION=device
Environment=CLAMSHELL_BOTTOM_OUTPUT=DSI-1
# Restarted when it fails; failing 4 times within a minute, given up: then the session ends and the
# console says why (clamshell-failed.service, thor-session).
Restart=on-failure
RestartSec=2
# (With the session's own: uwsm's slice for what must stay responsive.)
Slice=session-graphical.slice

[Install]
WantedBy=graphical-session.target
EOF

# ClamShell given up on (see clamshell.service): the session stopped, why said on the console.
cat > "$H/.config/systemd/user/clamshell-failed.service" <<EOF
[Unit]
Description=ClamShell failed: the session ends, the console says why

[Service]
Type=oneshot
ExecStart=$FINAL/.local/bin/thor-session --clamshell-failed
EOF

# Notifications (ClamShell's low battery warning, apps'), on the main screen: ~/.config/mako/config.
cat > "$H/.config/systemd/user/mako.service" <<EOF
[Unit]
Description=mako, the notifications
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=dbus
BusName=org.freedesktop.Notifications
ExecStart=/usr/bin/mako
Restart=on-failure
Slice=session-graphical.slice

[Install]
WantedBy=graphical-session.target
EOF

# Removable storage (an SD card, a USB stick) mounted as it is put in, under /run/media/\$USER: ClamShell's
# Files tab shows it (and sources on it come back).
cat > "$H/.config/systemd/user/udiskie.service" <<EOF
[Unit]
Description=udiskie, removable storage mounted as it is put in
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/bin/udiskie --automount --no-notify --no-tray --no-file-manager
Restart=on-failure
# (The session's own, as ClamShell and mako are: not offered to be ended.)
Slice=session-graphical.slice

[Install]
WantedBy=graphical-session.target
EOF

# (Enabled: wanted by the session's target.)
mkdir -p "$H/.config/systemd/user/graphical-session.target.wants"
for unit in clamshell mako udiskie; do
    ln -sf "../$unit.service" "$H/.config/systemd/user/graphical-session.target.wants/$unit.service"
done

# Maintenance (Claude Code's pass over the system, tools/device/maintain, in a terminal): an installed app,
# in shelf's Apps like any other.
cat > "$H/.local/share/applications/thor-maintenance.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Maintenance
Exec=foot --app-id=maintenance $FINAL/thor/tools/device/maintain
Icon=utilities-terminal
EOF

# The starter library (tools/device/library.json: a Games tab with a folder per system, each with its
# RetroArch core; Films and TV played by mpv), unless there is one; and the folders it reads.
mkdir -p "$H/.config/clamshell"
if [ ! -e "$H/.config/clamshell/library.json" ]; then
    install -m 644 "$REPO/tools/device/library.json" "$H/.config/clamshell/library.json"
fi
for d in nes snes n64 gb gba nds md psx dc; do mkdir -p "$H/Games/$d"; done
mkdir -p "$H/Videos/Films" "$H/Videos/TV"

# mpv's config for the top screen and the pad (clamshell/mpv), unless there is one.
mkdir -p "$H/.config/mpv"
if [ ! -e "$H/.config/mpv/mpv.conf" ]; then
    install -m 644 "$REPO/clamshell/mpv/mpv.conf" "$REPO/clamshell/mpv/input.conf" "$H/.config/mpv/"
fi

# RetroArch's config (clamshell/retroarch: the GPD's, its pads through SDL, its hotkeys on AYN) and the
# profile of ClamShell's virtual pad (an Xbox 360 controller, through SDL), unless there is one.
mkdir -p "$H/.config/retroarch/autoconfig"
if [ ! -e "$H/.config/retroarch/retroarch.cfg" ]; then
    install -m 644 "$REPO/clamshell/retroarch/retroarch.cfg" "$H/.config/retroarch/retroarch.cfg"
    install -m 644 "$REPO/clamshell/retroarch/autoconfig/Xbox 360 Controller (SDL).cfg" "$H/.config/retroarch/autoconfig/"
fi
# Its menu's look (XMB in ClamShell's colours, GLUI too; clamshell/retroarch/menu-theme.py), unless it is there: from
# RetroArch's assets in the root this home is in (the image's, or /).
ROOT=${H%"$FINAL"}
if [ ! -e "$H/.config/retroarch/clamshell-assets" ] && [ -d "$ROOT/usr/share/retroarch/assets/xmb" ]; then
    python3 "$REPO/clamshell/retroarch/menu-theme.py" "$H/.config/retroarch/clamshell-assets" "$ROOT/usr/share/retroarch/assets"
fi

# The dock's first pins, before ClamShell has saved any: Steam, the terminal, Maintenance (once it has, the
# user's pins are kept).
DOCK=$H/.local/share/clamshell/dock.json
if [ ! -e "$DOCK" ]; then
    mkdir -p "$(dirname "$DOCK")"
    printf '["desktop:steam", "desktop:foot", "desktop:thor-maintenance"]\n' > "$DOCK"
fi
