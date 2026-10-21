#!/bin/bash
# Builds an SD card image for the AYN Thor on this PC, as root (tools/image/image.sh runs it with sudo):
# Arch Linux ARM from its mirrors and the Thor's own packages (packages/*, built by tools/repo/build.sh),
# installed with this PC's pacman for aarch64, their install scripts and hooks run through qemu-user
# (binfmt: qemu-user-static-binfmt), in ROCKNIX's SD layout so their ABL (or OpenABL) boots it.
# Afterwards pacman keeps the device current: [thor] in its pacman.conf is the published repository
# (tools/repo/repo.sh). Or the same system for the test machine (vm): Arch Linux for x86_64 with Arch's
# kernel and the Thor's packages built for it, booted by systemd-boot under UEFI (tools/vm/vm.py
# thor-image), to try the first boot and the first session.
#
#   thor-image.sh thor IMAGE REPO KEYRINGS CACHE ROCKNIX_TAR FIRMWARE
#   thor-image.sh vm IMAGE REPO KEYRINGS CACHE
#
# IMAGE: the image file (made beforehand at its size). REPO: the repository's folder (thor.db and the
# packages). KEYRINGS: the distribution's keyring files (Arch Linux ARM's from its root tarball; Arch's
# from this PC). CACHE: pacman's package cache (kept between builds). ROCKNIX_TAR: ROCKNIX's release (its
# /KERNEL: the boot image whose header ours keeps). FIRMWARE: the thor-firmware package (never published).
#
# Nothing in it is anyone's in particular: the user thor logs in on its own (autologin) with its account
# locked, no password, until one is set in ClamShell (its first-run Setup tab: thor-account); sudo takes
# that password; ssh is off until turned on there; the time zone is UTC until set.
#
# Two ways to boot, one FAT partition: the stock bootloader (U-Boot with UEFI, as Kettle Linux's Thor notes
# have it) starts systemd-boot (EFI/BOOT/BOOTAA64.EFI), which loads the kernel (an EFI program), our
# initramfs and the Thor's device tree as files; the ABL boots /KERNEL, an Android boot image of the same
# kernel. Layout: GPT; 1: FAT "ROCKNIX" at 16 MiB, an EFI system partition with ROCKNIX's GPT name
# "system" and legacy-boot attribute (their ABL looks for the label); 2: ext4 THOR_ROOT, the system.
# thor-boot (thor-system) fills the FAT partition, here and after every update of what it is made of. The
# image is small to write; the card's first boot grows the root to the card and makes the swap file for
# hibernating (thor-firstboot, thor-system's). The test machine's: the same partitions, the FAT one its
# /boot (Arch's kernel and initramfs there, systemd-boot's EFI/BOOT/BOOTX64.EFI).
#
# Environment: THOR_TIMEZONE (default UTC), THOR_LOCALE (default en_US.UTF-8).
set -euo pipefail
TARGET=$1 IMAGE=$2 REPO=$3 KEYRINGS=$4 CACHE=$5 ROCKNIX=${6:-} FIRMWARE=${7:-}
HERE=$(cd "$(dirname "$0")" && pwd)
USER_NAME=thor
WORK=/var/tmp/thor-image
ROOT=$WORK/root
ZONE=${THOR_TIMEZONE:-UTC}
LOCALE=${THOR_LOCALE:-en_US.UTF-8}
# The device's packages: the base system, network, the GPU (Mesa), sound, screenshots (grim), the day-one
# checks' tools (evtest; alsa-utils: arecord, speaker-test, alsaucm), what the starter library plays
# (RetroArch and its cores, mpv; ffmpeg's ffprobe for videos' lengths), removable storage (exFAT's tools
# for SD cards), archives browsed (ratarmount, FUSE) and extracted (bsdtar, libarchive's, in base), the
# viewers ClamShell's programs open files with (imv, zathura with its PDF backend; xdg-open for the rest),
# and what maintenance needs: pacman-contrib (checkupdates, pacdiff); base-devel, squashfs-tools and git
# (thor-firmware built on the device after a ROCKNIX release, from a checkout). The Thor's own after them
# (their dependencies bring sway, uwsm, mako, foot, udiskie, zram-generator and the rest).
PACKAGES=(base sudo networkmanager upower bluez openssh xorg-xwayland mesa
          pipewire pipewire-pulse wireplumber alsa-ucm-conf noto-fonts less vim grim gptfdisk
          retroarch retroarch-assets-xmb retroarch-assets-glui retroarch-assets-ozone libretro-core-info libretro-nestopia libretro-snes9x libretro-mupen64plus-next libretro-gambatte
          libretro-mgba libretro-melonds libretro-genesis-plus-gx libretro-beetle-psx-hw libretro-flycast mpv ffmpeg wireless-regdb
          udisks2 exfatprogs ratarmount imv zathura zathura-pdf-mupdf xdg-utils pacman-contrib base-devel squashfs-tools git
          evtest alsa-utils
          thor-keyring thor-system thor-session clamshell)
case $TARGET in
    thor)
        # The Thor's: its kernel, Turnip (Adreno's Vulkan), its sound profiles (the firmware first, below).
        PACKAGES+=(linux-thor vulkan-freedreno thor-ucm)
        ARCH=aarch64 KEYRING=archlinuxarm
        MIRRORS="Server = http://mirror.archlinuxarm.org/\$arch/\$repo"
        REPOS=(core extra alarm)
        # The kernel command line: ROCKNIX's tuning for SM8550, our root; PSI on (Armada's kernel has it
        # off by default; Android's memory killer in Waydroid needs it), not per cgroup (as Armada's
        # kargs.d/10-armada); the internal UFS given 3 s to answer across sleep (Armada's device-quirks:
        # SM8550's needs up to that, against the default 0.5 s). The loader entry holds it; thor-boot copies
        # it into /KERNEL; the first boot adds resume= and resume_offset= (the swap file's place).
        CMDLINE="root=LABEL=THOR_ROOT rw rootwait console=tty0 fbcon=rotate:1 allow_mismatched_32bit_el0 fw_devlink.strict=1 pcie_ports=compat irqaffinity=0-2 cgroup.memory=nokmem,nosocket nosoftlockup usbcore.interrupt_interval_override=045e:028e:2 psi=1 cgroup_disable=pressure ufshcd_core.uic_cmd_timeout=3000"
        [ -e /proc/sys/fs/binfmt_misc/qemu-aarch64 ] || { echo "needs qemu-user's binfmt for aarch64: qemu-user-static-binfmt"; exit 1; }
        [ -f "$FIRMWARE" ] || { echo "no thor-firmware package: $FIRMWARE"; exit 1; }
        ;;
    vm)
        PACKAGES+=(linux)
        ARCH=x86_64 KEYRING=archlinux
        MIRRORS="Include = /etc/pacman.d/mirrorlist"
        REPOS=(core extra)
        # (QEMU's two screens' modes, the second forced on: QEMU enables an output only once its window
        # reports a size, which a headless one doesn't; tools/vm/vm.py's KERNEL_VIDEO.)
        CMDLINE="root=LABEL=THOR_ROOT rw console=tty0 console=ttyS0 psi=1 cgroup_disable=pressure video=Virtual-1:1920x1080@60 video=Virtual-2:1240x1080@60e"
        ;;
    *) echo "thor-image.sh thor|vm ..."; exit 2 ;;
esac
# The published repository, as the device's pacman reads it.
SERVER=https://github.com/robinei/thor/releases/download/packages

say() { echo "== $*"; }

[ "$(id -u)" = 0 ] || { echo "run as root (tools/image/image.sh)"; exit 1; }
[ -f "$REPO/thor.db" ] || { echo "no packages in $REPO: tools/repo/build.sh"; exit 1; }
# A failed run may have left its mounts and loop device.
umount -R -l "$ROOT" 2> /dev/null || true; umount -l "$WORK/boot" 2> /dev/null || true
losetup -j "$IMAGE" -O NAME -n | xargs -r -n1 losetup -d
rm -rf "$WORK"; mkdir -p "$WORK" "$CACHE"

say "pacman for $ARCH: the distribution's mirrors and keyring, the local [thor] first"
{
    printf '[options]\nArchitecture = %s\nCacheDir = %s\nGPGDir = %s\n' "$ARCH" "$CACHE" "$WORK/gnupg"
    printf 'SigLevel = Required DatabaseOptional\nLocalFileSigLevel = Optional\nParallelDownloads = 5\n\n'
    printf '[thor]\nServer = file://%s\n\n' "$REPO"
    for r in "${REPOS[@]}"; do printf '[%s]\n%s\n\n' "$r" "$MIRRORS"; done
} > "$WORK/pacman.conf"
key() { pacman-key --config "$WORK/pacman.conf" --gpgdir "$WORK/gnupg" "$@" > /dev/null 2>&1; }
key --init
key --populate-from "$KEYRINGS" --populate "$KEYRING"
key --add "$HERE/../../packages/thor-keyring/thor.gpg"
key --lsign-key 6C4C5B7FB11D609B739FDAC30BB252FF8CA9B43F

say "partitions"
# (sfdisk: GPT; 1 an EFI system partition named "system" with the legacy-boot attribute; 2 Linux.)
sfdisk -q "$IMAGE" <<EOF
label: gpt
start=16MiB, size=256MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name="system", attrs="LegacyBIOSBootable"
type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name="THOR_ROOT"
EOF
DEVICE=$(losetup -Pf --show "$IMAGE")
trap 'umount -l "$WORK/boot" 2> /dev/null || true; umount -R -l "$ROOT" 2> /dev/null || true; losetup -d "$DEVICE" 2> /dev/null || true' EXIT
udevadm settle
mkfs.vfat -F 32 -n ROCKNIX "${DEVICE}p1" > /dev/null
mkfs.ext4 -q -F -L THOR_ROOT "${DEVICE}p2"
mkdir -p "$ROOT"; mount "${DEVICE}p2" "$ROOT"

say "the FAT partition: the loader and our command line"
mkdir -p "$WORK/boot"; mount "${DEVICE}p1" "$WORK/boot"
mkdir -p "$WORK/boot/EFI/BOOT" "$WORK/boot/loader/entries"
printf 'default thor.conf\ntimeout 3\n' > "$WORK/boot/loader/loader.conf"
if [ "$TARGET" = thor ]; then
    # /KERNEL starts as ROCKNIX's (its addresses and page size are what the ABL wants); thor-boot puts our
    # kernel, initramfs and command line into it, and the files systemd-boot loads beside it.
    mkdir -p "$WORK/boot/dtbs"
    bsdtar -xOf "$ROCKNIX" '*/target/KERNEL' > "$WORK/boot/KERNEL"
    printf 'title   Arch Linux ARM (Thor)\nlinux   /Image\ninitrd  /initramfs-thor.img\ndevicetree /dtbs/qcs8550-ayn-thor.dtb\noptions %s\n' \
        "$CMDLINE" > "$WORK/boot/loader/entries/thor.conf"
    umount "$WORK/boot"
else
    # The test machine's: Arch's kernel and initramfs, put into /boot (this partition) by its packages.
    printf 'title   Arch Linux (Thor test machine)\nlinux   /vmlinuz-linux\ninitrd  /initramfs-linux.img\noptions %s\n' \
        "$CMDLINE" > "$WORK/boot/loader/entries/thor.conf"
    umount "$WORK/boot"
    mkdir -p "$ROOT/boot"; mount "${DEVICE}p1" "$ROOT/boot"
fi

say "the system and the Thor's packages (pacstrap)"
# The Thor's firmware first (never published), then the rest. -M: the device's own mirror list (its
# pacman-mirrorlist's), not this PC's.
if [ "$TARGET" = thor ]; then
    pacstrap -C "$WORK/pacman.conf" -K -M -U "$ROOT" "$FIRMWARE" > "$WORK/pacstrap.log" 2>&1 \
        || { tail -30 "$WORK/pacstrap.log"; exit 1; }
    pacstrap -C "$WORK/pacman.conf" -M "$ROOT" "${PACKAGES[@]}" >> "$WORK/pacstrap.log" 2>&1 \
        || { tail -30 "$WORK/pacstrap.log"; exit 1; }
else
    pacstrap -C "$WORK/pacman.conf" -K -M "$ROOT" "${PACKAGES[@]}" > "$WORK/pacstrap.log" 2>&1 \
        || { tail -30 "$WORK/pacstrap.log"; exit 1; }
fi
# (Install scripts' and hooks' complaints, if any.)
grep -iE "^error|error:|failed" "$WORK/pacstrap.log" | head -20 || true
# pacstrap's keyring setup leaves a gpg-agent running in the new root, which keeps it mounted.
gpgconf --homedir "$ROOT/etc/pacman.d/gnupg" --kill all 2> /dev/null || true
gpgconf --homedir "$WORK/gnupg" --kill all 2> /dev/null || true
if [ "$TARGET" = thor ]; then
    # The device's pacman: the published repository first.
    sed -i "0,/^\[core\]/s||[thor]\nServer = $SERVER\n\n[core]|" "$ROOT/etc/pacman.conf"
    grep -q '^\[thor\]' "$ROOT/etc/pacman.conf"
else
    # (The test machine's packages are this PC's own, never published: no [thor] there.)
    echo "LABEL=ROCKNIX /boot vfat defaults 0 2" >> "$ROOT/etc/fstab"
fi

say "system setup"
echo thor > "$ROOT/etc/hostname"
[ -e "$ROOT/usr/share/zoneinfo/$ZONE" ] || { echo "no time zone $ZONE"; exit 1; }
ln -sf "/usr/share/zoneinfo/$ZONE" "$ROOT/etc/localtime"
grep -qx "$LOCALE ${LOCALE#*.}" "$ROOT/etc/locale.gen" || echo "$LOCALE ${LOCALE#*.}" >> "$ROOT/etc/locale.gen"
arch-chroot "$ROOT" locale-gen > /dev/null
echo "LANG=$LOCALE" > "$ROOT/etc/locale.conf"
say "  time zone $ZONE, locale $LOCALE"
# The session's user (thor-session's autologin, polkit rules and the rest name it): input reads the
# controller and keys, power writes what ClamShell's settings set; wheel: sudo, with the user's password.
# (useradd leaves the account locked: no password.)
arch-chroot "$ROOT" useradd -m -G wheel,video,input,audio,power -s /bin/bash "$USER_NAME"
# (The password asked once counts for all the user's processes, not per terminal: maintenance asks it in
# its terminal, and Claude's commands, which have none, use it: thor-maintain.)
printf '%%wheel ALL=(ALL:ALL) ALL\nDefaults:%%wheel timestamp_type=global\n' > "$ROOT/etc/sudoers.d/wheel"
chmod 440 "$ROOT/etc/sudoers.d/wheel"
# (sshd isn't: ClamShell's Remote login turns it on, once there is a password.)
arch-chroot "$ROOT" systemctl enable NetworkManager bluetooth systemd-timesyncd > /dev/null
# UPower: the battery's history (ClamShell's battery mode shows the last day) and, nearly empty, hibernate
# rather than dying mid-write (UPower powers off instead when hibernate isn't possible).
sed -i -e 's/^CriticalPowerAction=.*/CriticalPowerAction=Hibernate/' -e 's/^PercentageAction=.*/PercentageAction=3/' \
    "$ROOT/etc/UPower/UPower.conf"

if [ "$TARGET" = thor ]; then
    say "boot: the kernel, our initramfs and command line into both ways (thor-boot, as after updates)"
    arch-chroot "$ROOT" thor-boot
    mkdir -p "$WORK/boot"; mount -o ro "${DEVICE}p1" "$WORK/boot"
    python3 "$ROOT/usr/lib/thor/bootimg.py" info "$WORK/boot/KERNEL"
    ls -l "$WORK/boot" "$WORK/boot/dtbs"
    umount "$WORK/boot"
else
    say "boot: systemd-boot (the removable path, which UEFI starts without its variables)"
    arch-chroot "$ROOT" bootctl install --esp-path=/boot --no-variables --graceful > /dev/null
    # No GPU acceleration in QEMU here: sway draws with the pixman renderer (uwsm's environment).
    mkdir -p "$ROOT/home/$USER_NAME/.config/uwsm"
    echo "export WLR_RENDERER=pixman" > "$ROOT/home/$USER_NAME/.config/uwsm/env"
    arch-chroot "$ROOT" chown -R "$USER_NAME:$USER_NAME" "/home/$USER_NAME/.config"
    ls -l "$ROOT/boot" "$ROOT/boot/EFI/BOOT"
fi

say "done: $(du -sh "$ROOT" --exclude=proc --exclude=boot 2> /dev/null | cut -f1) on the root partition"
