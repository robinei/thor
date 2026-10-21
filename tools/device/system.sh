#!/bin/bash
# The Thor's system files from this repository, into a root (as session.sh writes the session into a
# home): the fan daemon, the session user's write access to what ClamShell's settings set, the gamepad's
# stick range, zram and swapping, the SD card's scheduler, PipeWire's buffers, the checks at boot, the
# wake log, and Steam's starter. thor-image.sh runs it on the image's root; maintenance
# runs it again on the device after pulling (as root: sudo tools/device/system.sh / FAN_PROGRAM; then
# systemctl daemon-reload, systemctl restart clamshell-fan, udevadm control --reload; the udev and
# modprobe changes take effect at the next boot).
#
#   system.sh ROOT FAN_PROGRAM
#
# ROOT: the system's root (the image's, or /). FAN_PROGRAM: clamshell-fan built for aarch64 (a plain
# cargo build in clamshell/ builds it with ClamShell).
set -euo pipefail
R=$1 FAN=$2
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$R/usr/local/bin" "$R/usr/local/lib/thor" "$R/usr/local/share/applications" "$R/etc/systemd/system" \
    "$R/etc/udev/rules.d" "$R/etc/tmpfiles.d" "$R/etc/modprobe.d" "$R/usr/lib/systemd/system-sleep" \
    "$R/etc/sysctl.d" "$R/etc/pipewire/pipewire.conf.d" "$R/etc/pipewire/pipewire-pulse.conf.d"

# The fan daemon (clamshell/crates/fan): nothing in the kernel drives the Thor's fan, so it runs from early
# in boot, and again if it stops. Its file replaced, not written into, as it may be running.
install -m 755 "$FAN" "$R/usr/local/bin/clamshell-fan.new"
mv "$R/usr/local/bin/clamshell-fan.new" "$R/usr/local/bin/clamshell-fan"
cat > "$R/etc/systemd/system/clamshell-fan.service" <<'EOF'
[Unit]
Description=The fan's speed from the CPU's and GPU's temperatures
DefaultDependencies=no
After=systemd-tmpfiles-setup.service
Conflicts=shutdown.target
Before=sysinit.target shutdown.target

[Service]
ExecStart=/usr/local/bin/clamshell-fan
# (Stopped or failed: the fan left at half speed, not wherever it was, possibly off.)
ExecStopPost=/usr/local/bin/clamshell-fan --safe
Restart=always
RestartSec=2
ProtectSystem=strict
ProtectHome=yes
PrivateNetwork=yes
NoNewPrivileges=yes

[Install]
WantedBy=sysinit.target
EOF
systemctl --root "$R" --quiet enable clamshell-fan.service

# The power group (the session's user is in it): writes what ClamShell's settings set. cpufreq's caps
# and the CPUs' online (Power, CPU cap, Big cores), the GPU's cap (GPU cap), the battery's charge limit
# and charge current (Charge limit, Bypass), the power key's wakeup (off while the lid is closed), the
# fan daemon's mode (Fan). Given as each device appears (udev), the mode file at boot (tmpfiles).
groupadd -R "$R" -f -r power
cat > "$R/usr/local/lib/thor/power-access" <<'EOF'
#!/bin/sh
# Gives the power group write access to the files given that exist (udev: 73-thor-power.rules).
for f in "$@"; do
    [ -e "$f" ] && chgrp power "$f" && chmod g+w "$f"
done
exit 0
EOF
chmod 755 "$R/usr/local/lib/thor/power-access"
A=/usr/local/lib/thor/power-access
cat > "$R/etc/udev/rules.d/73-thor-power.rules" <<EOF
# Written by tools/device/system.sh: the power group's write access (ClamShell's settings).
ACTION=="add|change", SUBSYSTEM=="cpu", RUN+="$A /sys%p/online /sys%p/cpufreq/scaling_max_freq"
ACTION=="add|change", SUBSYSTEM=="devfreq", RUN+="$A /sys%p/max_freq"
ACTION=="add|change", SUBSYSTEM=="power_supply", ATTR{type}=="Battery", RUN+="$A /sys%p/charge_control_end_threshold /sys%p/constant_charge_current"
ACTION=="add|change", SUBSYSTEM=="input", KERNEL=="input*", ATTR{name}=="*pwrkey*", RUN+="$A /sys%p/device/power/wakeup"
EOF
printf 'f /run/clamshell-fan 0664 root power - auto\n' > "$R/etc/tmpfiles.d/clamshell-fan.conf"

# The sticks' full travel: the gamepad driver (rsinput) takes ±1408 by default, of which the sticks
# reach ±1024 (Thorch's axis-range, its dts-patches/0007), so games saw about 73 % of it. Its module
# parameters, read as it starts (Armada calibrates through the same ones). To check on the device: the
# stick's corners reach the axis' ends (evtest).
{
    echo "# Written by tools/device/system.sh: the sticks' range (see there)."
    printf 'options rsinput'
    for axis in leftx lefty rightx righty; do
        printf ' axis_%s_min=-1024 axis_%s_max=1024' "$axis" "$axis"
    done
    echo
} > "$R/etc/modprobe.d/thor-rsinput.conf"

# Memory: compressed swap in memory (zram, zstd) before the swap file on the card, which is slow (about
# 13 MB/s) and stays for hibernating (systemd hibernates to the swap file, never to zram); the kernel's
# swapping tuned for zram, as Armada's 60-armada-gaming.conf (swappiness above 100 prefers swapping to
# dropping file pages; no readahead from swap). Sizes as Armada's zram-generator.conf.
cat > "$R/etc/systemd/zram-generator.conf" <<'EOF'
# Written by tools/device/system.sh.
[zram0]
zram-size = min(ram, 16384)
zram-resident-limit = min(ram / 2, 8192)
compression-algorithm = zstd
swap-priority = 100
EOF
cat > "$R/etc/sysctl.d/70-thor-zram.conf" <<'EOF'
# Written by tools/device/system.sh: swapping to zram.
vm.swappiness = 180
vm.page-cluster = 0
vm.watermark_boost_factor = 0
vm.watermark_scale_factor = 125
EOF

# The SD card (the root's): BFQ and a long readahead, as Armada's 60-armada-block-scheduler.rules and
# ROCKNIX's 10-bfq-sched.rules.
echo 'ACTION=="add|change", KERNEL=="mmcblk[0-9]", ATTR{queue/scheduler}="bfq", ATTR{queue/read_ahead_kb}="4096"' \
    > "$R/etc/udev/rules.d/74-thor-sd.rules"

# Sound: PipeWire's buffers no shorter than Armada's (its 99-armada-buffers.conf, against crackling on
# waking from sleep).
cat > "$R/etc/pipewire/pipewire.conf.d/70-thor-buffers.conf" <<'EOF'
# Written by tools/device/system.sh.
context.properties = {
    default.clock.min-quantum = 1024
}
EOF
cat > "$R/etc/pipewire/pipewire-pulse.conf.d/70-thor-buffers.conf" <<'EOF'
# Written by tools/device/system.sh.
pulse.properties = {
    pulse.min.req = 512/48000
    pulse.min.quantum = 512/48000
    pulse.min.frag = 512/48000
}
EOF

# The account helper for ClamShell's Password and Remote login settings (tools/device/thor-account, run
# through pkexec; polkit allows it in 50-thor.rules).
install -m 755 "$HERE/thor-account" "$R/usr/local/lib/thor/thor-account"

# The checks at each boot (tools/device/thor-selftest): into the journal, kept on the card (its
# directory makes journald's storage persistent), and /var/log/thor-selftest.log, so that a boot that
# goes wrong can be read on another computer. Started early (once the root is writable), not waited for.
install -m 755 "$HERE/thor-selftest" "$R/usr/local/bin/thor-selftest"
mkdir -p "$R/var/log/journal"
cat > "$R/etc/systemd/system/thor-selftest.service" <<'EOF'
[Unit]
Description=The Thor's checks at boot (journalctl -t thor-selftest, /var/log/thor-selftest.log)
DefaultDependencies=no
After=local-fs.target systemd-journald.socket
Conflicts=shutdown.target
Before=shutdown.target

[Service]
ExecStart=/usr/local/bin/thor-selftest
SyslogIdentifier=thor-selftest

[Install]
WantedBy=sysinit.target
EOF
systemctl --root "$R" --quiet enable thor-selftest.service

# What woke the device, each time (the kernel keeps only the last): for the battery's drain while
# asleep (as Armada's 40-armada-wake-ledger). The last 500 wakes.
cat > "$R/usr/lib/systemd/system-sleep/thor-wake-log" <<'EOF'
#!/bin/sh
# Written by tools/device/system.sh: each wake's interrupt into /var/log/thor-wake.log.
[ "$1" = post ] || exit 0
log=/var/log/thor-wake.log
irq=$(cat /sys/power/pm_wakeup_irq 2> /dev/null)
name=
[ -n "$irq" ] && name=$(awk -v irq="$irq:" '$1 == irq { print $NF }' /proc/interrupts)
echo "$(date --iso-8601=seconds) $2: irq ${irq:-none} (${name:-unknown})" >> "$log"
[ "$(wc -l < "$log")" -le 500 ] || { tail -n 250 "$log" > "$log.new" && mv "$log.new" "$log"; }
exit 0
EOF
chmod 755 "$R/usr/lib/systemd/system-sleep/thor-wake-log"

# Steam: its starter and setup (tools/device/steam, steam-setup); the client itself is the user's,
# downloaded at its first start. Its desktop file for the dock's pin and the apps.
install -m 755 "$HERE/steam" "$HERE/steam-setup" "$R/usr/local/bin/"
cat > "$R/usr/local/share/applications/steam.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Steam
Exec=steam %U
Icon=steam
Terminal=false
Categories=Network;
MimeType=x-scheme-handler/steam;
EOF
