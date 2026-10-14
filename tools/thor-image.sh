#!/bin/bash
# Builds an SD card image for the AYN Thor, best effort, inside the aarch64 machine (tools/vm.py
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
# label); 2: ext4 THOR_ROOT, the system.
set -euo pipefail
DEVICE=$1 ROCKNIX=$2 PROGRAM=$3 SSH_KEY=$4
USER_NAME=thor
WORK=/var/tmp/thor-image
ROOT=$WORK/root
# The device's packages: the base system, network, the session, the GPU (Freedreno/Turnip), sound.
PACKAGES=(base sudo mkinitcpio networkmanager openssh sway swaybg foot xorg-xwayland mesa vulkan-freedreno
          pipewire pipewire-pulse wireplumber alsa-ucm-conf noto-fonts sqlite python less vim)
# The kernel command line: ROCKNIX's tuning for SM8550, our root.
CMDLINE="root=LABEL=THOR_ROOT rw rootwait console=tty0 allow_mismatched_32bit_el0 fw_devlink.strict=1 pcie_ports=compat irqaffinity=0-2 cgroup.memory=nokmem,nosocket nosoftlockup usbcore.interrupt_interval_override=045e:028e:2"

say() { echo "== $*"; }

say "tools"
pacman -S --noconfirm --needed arch-install-scripts squashfs-tools dosfstools gptfdisk > /dev/null

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

say "ROCKNIX's audio profiles (over alsa-ucm-conf's: an update of that package undoes them)"
cp -a "$WORK/system/usr/share/alsa/ucm2/." "$ROOT/usr/share/alsa/ucm2/"

say "system setup"
echo thor > "$ROOT/etc/hostname"
arch-chroot "$ROOT" useradd -m -G wheel,video,input,audio -s /bin/bash "$USER_NAME"
echo "$USER_NAME ALL=(ALL) NOPASSWD: ALL" > "$ROOT/etc/sudoers.d/$USER_NAME"
install -d -m 700 "$ROOT/home/$USER_NAME/.ssh"
echo "$SSH_KEY" > "$ROOT/home/$USER_NAME/.ssh/authorized_keys"
mkdir -p "$ROOT/etc/systemd/system/getty@tty1.service.d"
printf '[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin %s --noclear %%I $TERM\n' "$USER_NAME" \
    > "$ROOT/etc/systemd/system/getty@tty1.service.d/autologin.conf"
echo uinput > "$ROOT/etc/modules-load.d/thor.conf"
echo 'KERNEL=="uinput", GROUP="input", MODE="0660"' > "$ROOT/etc/udev/rules.d/70-thor.rules"
arch-chroot "$ROOT" systemctl enable NetworkManager sshd > /dev/null

say "the session (sway, ClamShell)"
H=$ROOT/home/$USER_NAME
mkdir -p "$H/.config/sway" "$H/.config/systemd/user" "$H/.local/bin"
install -m 755 "$PROGRAM" "$H/.local/bin/clamshell"
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
cat > "$H/.config/sway/config" <<'CONFIG'
# The Thor: DSI-2 the main (top) screen, DSI-1 the second (bottom) screen (as ROCKNIX and Kettle have them).
include ~/.config/sway/outputs
default_border none
exec systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP && systemctl --user restart clamshell.service
CONFIG
cat > "$H/.config/systemd/user/clamshell.service" <<EOF
[Unit]
Description=ClamShell
After=graphical-session.target

[Service]
ExecStart=/home/$USER_NAME/.local/bin/clamshell
Environment=CLAMSHELL_BOTTOM_OUTPUT=DSI-1
# Touchpad mode takes the bottom screen's touchscreen (the Thor has two).
Environment=CLAMSHELL_TOUCHSCREEN=bottom_touchscreen
Restart=on-failure
RestartSec=2
EOF
arch-chroot "$ROOT" chown -R "$USER_NAME:$USER_NAME" "/home/$USER_NAME"
chmod 600 "$H/.ssh/authorized_keys"

say "initramfs (ours: the root by label; all storage drivers, no autodetect)"
cat > "$ROOT/etc/mkinitcpio-thor.conf" <<'EOF'
MODULES=()
BINARIES=()
FILES=()
HOOKS=(base udev modconf block filesystems fsck)
COMPRESSION=zstd
EOF
arch-chroot "$ROOT" mkinitcpio -k "$KVER" -c /etc/mkinitcpio-thor.conf -g /boot/initramfs-thor.img > "$WORK/mkinitcpio.log" 2>&1 \
    || { tail -20 "$WORK/mkinitcpio.log"; exit 1; }

say "boot image: ROCKNIX's kernel and device trees, our initramfs and command line"
mkdir -p "$WORK/boot"; mount "${DEVICE}1" "$WORK/boot"
python3 "$(dirname "$0")/bootimg.py" repack "$ROCKNIX/KERNEL" --ramdisk "$ROOT/boot/initramfs-thor.img" \
    --cmdline "$CMDLINE" -o "$WORK/boot/KERNEL"
python3 "$(dirname "$0")/bootimg.py" info "$WORK/boot/KERNEL"

say "UEFI boot (stock U-Boot): systemd-boot, the kernel, our initramfs, the Thor's device tree"
mkdir -p "$WORK/boot/EFI/BOOT" "$WORK/boot/loader/entries" "$WORK/boot/dtbs"
cp "$ROOT/usr/lib/systemd/boot/efi/systemd-bootaa64.efi" "$WORK/boot/EFI/BOOT/BOOTAA64.EFI"
python3 "$(dirname "$0")/bootimg.py" split "$ROCKNIX/KERNEL" --model "AYN Thor" \
    --kernel "$WORK/boot/Image" --dtb "$WORK/boot/dtbs/qcs8550-ayn-thor.dtb"
cp "$ROOT/boot/initramfs-thor.img" "$WORK/boot/"
printf 'default thor.conf\ntimeout 3\n' > "$WORK/boot/loader/loader.conf"
cat > "$WORK/boot/loader/entries/thor.conf" <<ENTRY
title   Arch Linux ARM (Thor)
linux   /Image
initrd  /initramfs-thor.img
devicetree /dtbs/qcs8550-ayn-thor.dtb
options $CMDLINE
ENTRY
umount "$WORK/boot"

say "done: $(du -sh "$ROOT" --exclude=proc 2> /dev/null | cut -f1) on the root partition"
