#!/bin/bash
# Builds an SD card image for the AYN Thor, best effort, inside the aarch64 machine (tools/vm/vm.py
# --arch aarch64 thor-image runs it as root there): Arch Linux ARM from its mirrors, ROCKNIX's SM8550
# kernel (with the Thor's device tree and out-of-tree drivers), its modules and firmware, our initramfs
# and session, in ROCKNIX's SD layout so their ABL boots it.
#
#   thor-image.sh DEVICE ROCKNIX_TARGET PROGRAM SSH_KEY
#
# DEVICE: the SD image (a virtual disk). ROCKNIX_TARGET: the release's target/ (KERNEL, SYSTEM).
# PROGRAM: ClamShell built for aarch64. SSH_KEY: a public key to log in with.
#
# Two ways to boot, one FAT partition: the stock bootloader (U-Boot with UEFI, as Kettle Linux's Thor notes
# have it) starts systemd-boot (EFI/BOOT/BOOTAA64.EFI), which loads the kernel (an EFI program: ROCKNIX's
# is built with the EFI stub), our initramfs and the Thor's device tree as files; ROCKNIX's ABL boots
# /KERNEL, an Android boot image of the same kernel. Layout: GPT; 1: FAT "ROCKNIX" at 16 MiB, an EFI
# system partition with ROCKNIX's GPT name "system" and legacy-boot attribute (their ABL looks for the
# label); 2: ext4 THOR_ROOT, the system. The image is small to write; the card's first boot grows the root to
# the card and makes the swap file for hibernating (thor-firstboot, below).
#
# Environment: THOR_TIMEZONE (default: this machine's /etc/localtime, else UTC), THOR_LOCALE (default
# en_US.UTF-8). Through sudo: sudo THOR_TIMEZONE=Europe/Oslo bash thor-image.sh ...
set -euo pipefail
DEVICE=$1 ROCKNIX=$2 PROGRAM=$3 SSH_KEY=$4
USER_NAME=thor
WORK=/var/tmp/thor-image
ROOT=$WORK/root
ZONE=${THOR_TIMEZONE:-$(readlink /etc/localtime 2> /dev/null | sed 's|.*/zoneinfo/||')}
ZONE=${ZONE:-UTC}
LOCALE=${THOR_LOCALE:-en_US.UTF-8}
# The swap file for hibernating, in GiB: as much as the RAM (16 GB), so a hibernate with everything in use
# fits. Made on the card's first boot, not here (it would make the image 16 GB bigger to write).
SWAP_GIB=16
# The device's packages: the base system, network, the session, the GPU (Freedreno/Turnip), sound,
# notifications (mako), screenshots (grim), growing the root on first boot (growpart, which uses sgdisk for GPT),
# what the starter library plays (RetroArch and its cores, mpv; ffmpeg's ffprobe for videos' lengths),
# and what maintenance on the device needs: git (this repository, in ~/thor), Rust and a C toolchain to
# build ClamShell, pacman-contrib (checkupdates, pacdiff).
PACKAGES=(base sudo mkinitcpio networkmanager polkit upower bluez openssh sway mako foot xorg-xwayland mesa vulkan-freedreno
          pipewire pipewire-pulse wireplumber alsa-ucm-conf noto-fonts sqlite python less vim grim cloud-guest-utils gptfdisk
          retroarch retroarch-assets-xmb python-pillow libretro-core-info libretro-nestopia libretro-snes9x libretro-mupen64plus-next libretro-gambatte
          libretro-mgba libretro-melonds libretro-genesis-plus-gx libretro-pcsx-rearmed libretro-flycast mpv ffmpeg
          git rust base-devel pacman-contrib)
# The kernel command line: ROCKNIX's tuning for SM8550, our root. The first boot adds resume= and
# resume_offset= (the swap file's place) to both copies of it: /KERNEL's and the loader entry's.
CMDLINE="root=LABEL=THOR_ROOT rw rootwait console=tty0 allow_mismatched_32bit_el0 fw_devlink.strict=1 pcie_ports=compat irqaffinity=0-2 cgroup.memory=nokmem,nosocket nosoftlockup usbcore.interrupt_interval_override=045e:028e:2"

say() { echo "== $*"; }

say "tools"
pacman -S --noconfirm --needed arch-install-scripts squashfs-tools dosfstools gptfdisk git > /dev/null

say "ROCKNIX's system (modules, firmware, audio profiles)"
# A failed run may have left its mounts.
umount -R -l "$ROOT" 2> /dev/null || true; umount -l "$WORK/boot" 2> /dev/null || true
rm -rf "$WORK"; mkdir -p "$WORK"
# ROCKNIX keeps the kernel's modules and part of the firmware in a kernel overlay
# (usr/lib/kernel-overlays/base/lib/{modules,firmware}), the rest of the firmware in usr/lib/firmware, and
# its ALSA UCM profiles (AYN/Thor, conf.d/sm8550) in usr/share/alsa/ucm2.
unsquashfs -q -d "$WORK/system" "$ROCKNIX/SYSTEM" usr/lib/kernel-overlays usr/lib/firmware usr/share/alsa/ucm2 > /dev/null
MODULES=$(find "$WORK/system/usr/lib/kernel-overlays" -maxdepth 4 -type d -path '*/lib/modules/*' -name '[0-9]*' | head -1)
[ -n "$MODULES" ] || { echo "no kernel modules in ROCKNIX's SYSTEM"; exit 1; }
KVER=$(basename "$MODULES")
say "  kernel $KVER"

say "partitions"
sgdisk -Z "$DEVICE" > /dev/null
sgdisk -n 1:16M:+256M -t 1:ef00 -c 1:system -A 1:set:2 -n 2:0:0 -t 2:8300 -c 2:THOR_ROOT "$DEVICE" > /dev/null
blockdev --rereadpt "$DEVICE"; udevadm settle
mkfs.vfat -F 32 -n ROCKNIX "${DEVICE}1" > /dev/null
mkfs.ext4 -q -F -L THOR_ROOT "${DEVICE}2"
mkdir -p "$ROOT"; mount "${DEVICE}2" "$ROOT"
trap 'umount -l "$WORK/boot" 2> /dev/null || true; umount -R -l "$ROOT" 2> /dev/null || true' EXIT

say "Arch Linux ARM (pacstrap)"
pacstrap -K "$ROOT" "${PACKAGES[@]}" > "$WORK/pacstrap.log"
# pacstrap's keyring setup leaves a gpg-agent running in the new root, which keeps it mounted.
gpgconf --homedir "$ROOT/etc/pacman.d/gnupg" --kill all 2> /dev/null || true

say "ROCKNIX's modules and firmware"
mkdir -p "$ROOT/usr/lib/modules" "$ROOT/usr/lib/firmware"
cp -a "$MODULES" "$ROOT/usr/lib/modules/"
# (usr/lib/firmware may be a link into the overlay: then the overlay's is all of it.)
[ -d "$WORK/system/usr/lib/firmware" ] && [ ! -L "$WORK/system/usr/lib/firmware" ] && cp -a "$WORK/system/usr/lib/firmware/." "$ROOT/usr/lib/firmware/"
cp -a "$WORK/system/usr/lib/kernel-overlays/base/lib/firmware/." "$ROOT/usr/lib/firmware/"
arch-chroot "$ROOT" depmod "$KVER"

say "ROCKNIX's audio profiles (over alsa-ucm-conf's; a copy kept, which a pacman hook puts back after an update of it)"
cp -a "$WORK/system/usr/share/alsa/ucm2/." "$ROOT/usr/share/alsa/ucm2/"
mkdir -p "$ROOT/usr/local/share/thor"
cp -a "$WORK/system/usr/share/alsa/ucm2" "$ROOT/usr/local/share/thor/ucm2"
# Which ROCKNIX release the kernel, modules, firmware and profiles are from (maintenance compares it with
# the latest).
basename "$(dirname "$(realpath "$ROCKNIX")")" > "$ROOT/usr/local/share/thor/rocknix-release"

say "system setup"
echo thor > "$ROOT/etc/hostname"
[ -e "$ROOT/usr/share/zoneinfo/$ZONE" ] || { echo "no time zone $ZONE"; exit 1; }
ln -sf "/usr/share/zoneinfo/$ZONE" "$ROOT/etc/localtime"
grep -qx "$LOCALE ${LOCALE#*.}" "$ROOT/etc/locale.gen" || echo "$LOCALE ${LOCALE#*.}" >> "$ROOT/etc/locale.gen"
arch-chroot "$ROOT" locale-gen > /dev/null
echo "LANG=$LOCALE" > "$ROOT/etc/locale.conf"
say "  time zone $ZONE, locale $LOCALE"
# input: /dev/input/event* (udev gives them to the group), which ClamShell reads for keys and idle.
arch-chroot "$ROOT" useradd -m -G wheel,video,input,audio -s /bin/bash "$USER_NAME"
echo "$USER_NAME ALL=(ALL) NOPASSWD: ALL" > "$ROOT/etc/sudoers.d/$USER_NAME"
install -d -m 700 "$ROOT/home/$USER_NAME/.ssh"
echo "$SSH_KEY" > "$ROOT/home/$USER_NAME/.ssh/authorized_keys"
mkdir -p "$ROOT/etc/systemd/system/getty@tty1.service.d"
printf '[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin %s --noclear %%I $TERM\n' "$USER_NAME" \
    > "$ROOT/etc/systemd/system/getty@tty1.service.d/autologin.conf"
echo uinput > "$ROOT/etc/modules-load.d/thor.conf"
echo 'KERNEL=="uinput", GROUP="input", MODE="0660"' > "$ROOT/etc/udev/rules.d/70-thor.rules"
arch-chroot "$ROOT" systemctl enable NetworkManager bluetooth sshd systemd-timesyncd > /dev/null
# The power button and the lid put the machine to sleep with logind's "sleep" action: the first that works
# of suspend-then-hibernate and suspend (so suspend still works while hibernate doesn't, e.g. before the
# swap file exists or on a card too small for it). ClamShell notices waking and applies its settings again.
# A long press of the power button powers off.
mkdir -p "$ROOT/etc/systemd/logind.conf.d" "$ROOT/etc/systemd/sleep.conf.d"
printf '[Login]\nSleepOperation=suspend-then-hibernate suspend\nHandlePowerKey=sleep\nHandlePowerKeyLongPress=poweroff\nHandleLidSwitch=sleep\nHandleLidSwitchExternalPower=sleep\n' \
    > "$ROOT/etc/systemd/logind.conf.d/thor.conf"
# Suspend-then-hibernate: hibernate after 30 minutes asleep (systemd wakes the machine with the RTC alarm
# to do it), or earlier when the battery runs low.
printf '[Sleep]\nHibernateDelaySec=30min\n' > "$ROOT/etc/systemd/sleep.conf.d/thor.conf"
# UPower: the battery's history (ClamShell's battery mode shows the last day) and, nearly empty, hibernate
# rather than dying mid-write (UPower powers off instead when hibernate isn't possible).
sed -i -e 's/^CriticalPowerAction=.*/CriticalPowerAction=Hibernate/' -e 's/^PercentageAction=.*/PercentageAction=3/' \
    "$ROOT/etc/UPower/UPower.conf"

# The card's first boot (thor-firstboot.service, once): grow the root partition (the last one) and its
# filesystem to the card; make the swap file /swapfile ($SWAP_GIB GiB, if the card leaves 8 GiB free beside
# it; else no hibernate, suspend only); then add resume=LABEL=THOR_ROOT and resume_offset= (the file's first
# block, known only once it exists) to the loader entry's options line, and thor-boot puts that command
# line into /KERNEL too (thor-boot: tools/device/thor-boot, with bootimg.py copied into the image). The resume
# settings take effect from the next boot; hibernating works in the first session already (systemd-sleep
# tells the kernel where the swap file is before hibernating).
install -D -m 644 "$(dirname "$0")/bootimg.py" "$ROOT/usr/local/lib/thor/bootimg.py"
# thor-boot, and the pacman hooks that run it (and put ROCKNIX's audio profiles back) after updates.
install -D -m 755 "$(dirname "$0")/../device/thor-boot" "$ROOT/usr/local/bin/thor-boot"
install -D -m 644 -t "$ROOT/etc/pacman.d/hooks" "$(dirname "$0")"/../device/hooks/*.hook
# The session's user may set the time zone (ClamShell's time tab).
install -D -m 644 "$(dirname "$0")/../device/polkit/50-thor.rules" "$ROOT/etc/polkit-1/rules.d/50-thor.rules"
{ printf '#!/bin/bash\n# Written by thor-image.sh.\nBASE_CMDLINE=%q\nSWAP_GIB=%s\n' "$CMDLINE" "$SWAP_GIB"; cat <<'SCRIPT'
# The card's first boot (thor-firstboot.service): grow the root to the card, make the swap file for
# hibernating, put its place on both kernel command lines.
set -euo pipefail
part=$(findmnt -nvo SOURCE /)
disk=/dev/$(lsblk -ndo PKNAME "$part")
num=$(cat "/sys/class/block/${part#/dev/}/partition")
echo "growing $part (partition $num of $disk) to the card"
growpart "$disk" "$num" || echo "growpart: no change"
resize2fs "$part"

if [ ! -e /swapfile ]; then
    free=$(df --output=avail -B1G / | tail -1)
    if (( free >= SWAP_GIB + 8 )); then
        echo "making /swapfile, $SWAP_GIB GiB"
        fallocate -l "${SWAP_GIB}G" /swapfile
        chmod 600 /swapfile
        mkswap /swapfile
        echo '/swapfile none swap defaults 0 0' >> /etc/fstab
        swapon /swapfile
    else
        echo "$free GiB free: no swap file, so no hibernate (suspend only)"
    fi
fi

if [ -e /swapfile ]; then
    # The file's first block (ext4 blocks and pages are both 4 KiB here, so it is the offset in pages).
    offset=$(filefrag -v /swapfile | awk '$1 == "0:" { sub(/\.\.$/, "", $4); print $4 }')
    case $disk in *[0-9]) esp=${disk}p1 ;; *) esp=${disk}1 ;; esac
    mkdir -p /run/thor-esp
    mount "$esp" /run/thor-esp
    sed -i "s|^options .*|options $BASE_CMDLINE resume=LABEL=THOR_ROOT resume_offset=$offset|" /run/thor-esp/loader/entries/thor.conf
    umount /run/thor-esp
    thor-boot
fi
touch /var/lib/thor-firstboot
SCRIPT
} > "$ROOT/usr/local/bin/thor-firstboot"
chmod 755 "$ROOT/usr/local/bin/thor-firstboot"
cat > "$ROOT/etc/systemd/system/thor-firstboot.service" <<'EOF'
[Unit]
Description=The card's first boot: grow the root, make the swap file, resume on the command lines
ConditionPathExists=!/var/lib/thor-firstboot
After=local-fs.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/thor-firstboot
StandardOutput=journal+console

[Install]
WantedBy=multi-user.target
EOF
arch-chroot "$ROOT" systemctl enable thor-firstboot > /dev/null

say "the session (sway, ClamShell; tools/device/session.sh) and this repository in ~/thor"
H=$ROOT/home/$USER_NAME
bash "$(dirname "$0")/../device/session.sh" "$H" "/home/$USER_NAME" "$PROGRAM"
# This repository as committed, for maintenance on the device (tools/device/maintain), its origin the
# GitHub one (pushing from the device needs its key added there).
git -c safe.directory="*" clone -q "$(dirname "$0")/../.." "$H/thor"
git -C "$H/thor" remote set-url origin git@github.com:robinei/thor.git
mkdir -p "$H/.local/share/thor"
git -C "$H/thor" rev-parse HEAD > "$H/.local/share/thor/session-commit"
arch-chroot "$ROOT" chown -R "$USER_NAME:$USER_NAME" "/home/$USER_NAME"
chmod 600 "$H/.ssh/authorized_keys"

say "initramfs (ours: the root by label; all storage drivers, no autodetect; resume from hibernate)"
# resume: after block, before filesystems; it resumes from resume= / resume_offset= on the command line
# (with systemd 255 or later through systemd-hibernate-resume, which doesn't wait for the device). The SD
# card appears asynchronously, so thor-root-wait first waits up to 10 s for THOR_ROOT (the root's own
# rootwait comes only later, in filesystems).
mkdir -p "$ROOT/etc/initcpio/hooks" "$ROOT/etc/initcpio/install"
cat > "$ROOT/etc/initcpio/hooks/thor-root-wait" <<'EOF'
#!/usr/bin/ash
run_hook() {
    local i=0
    while [ ! -b /dev/disk/by-label/THOR_ROOT ] && [ "$i" -lt 10 ]; do
        sleep 1
        i=$((i + 1))
    done
}
EOF
cat > "$ROOT/etc/initcpio/install/thor-root-wait" <<'EOF'
#!/bin/bash
build() {
    add_runscript
}
help() {
    echo "Waits up to 10 s for the THOR_ROOT filesystem, for resume."
}
EOF
cat > "$ROOT/etc/mkinitcpio-thor.conf" <<'EOF'
MODULES=()
BINARIES=()
FILES=()
HOOKS=(base udev modconf block thor-root-wait resume filesystems fsck)
COMPRESSION=zstd
EOF
say "boot: ROCKNIX's kernel (/KERNEL for their ABL; split into files for systemd-boot), the loader entry"
mkdir -p "$WORK/boot"; mount "${DEVICE}1" "$WORK/boot"
cp "$ROCKNIX/KERNEL" "$WORK/boot/KERNEL"
mkdir -p "$WORK/boot/EFI/BOOT" "$WORK/boot/loader/entries" "$WORK/boot/dtbs"
python3 "$(dirname "$0")/bootimg.py" split "$ROCKNIX/KERNEL" --model "AYN Thor" \
    --kernel "$WORK/boot/Image" --dtb "$WORK/boot/dtbs/qcs8550-ayn-thor.dtb"
printf 'default thor.conf\ntimeout 3\n' > "$WORK/boot/loader/loader.conf"
cat > "$WORK/boot/loader/entries/thor.conf" <<ENTRY
title   Arch Linux ARM (Thor)
linux   /Image
initrd  /initramfs-thor.img
devicetree /dtbs/qcs8550-ayn-thor.dtb
options $CMDLINE
ENTRY
umount "$WORK/boot"

say "boot: our initramfs and command line into both, systemd-boot (thor-boot, as the device does it after updates)"
arch-chroot "$ROOT" thor-boot
mkdir -p "$WORK/boot"; mount -o ro "${DEVICE}1" "$WORK/boot"
python3 "$(dirname "$0")/bootimg.py" info "$WORK/boot/KERNEL"
umount "$WORK/boot"

say "done: $(du -sh "$ROOT" --exclude=proc 2> /dev/null | cut -f1) on the root partition"
