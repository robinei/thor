#!/bin/bash
# Builds an SD card image for the AYN Thor, best effort, inside the aarch64 machine (tools/vm.py
# --arch aarch64 thor-image runs it as root there): Arch Linux ARM from its mirrors, ROCKNIX's SM8550
# kernel (with the Thor's device tree and out-of-tree drivers), its modules and firmware, our initramfs
# and session, in ROCKNIX's SD layout so their ABL boots it.
#
#   thor-image.sh DEVICE ROCKNIX_TARGET PROGRAM SSH_KEY
#
# DEVICE: the SD image (a virtual disk). ROCKNIX_TARGET: the release's target/ (KERNEL, SYSTEM).
# PROGRAM: the system program built for aarch64. SSH_KEY: a public key to log in with.
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
          pipewire pipewire-pulse wireplumber noto-fonts sqlite python less vim)
# The kernel command line: ROCKNIX's tuning for SM8550, our root.
CMDLINE="root=LABEL=THOR_ROOT rw rootwait console=tty0 allow_mismatched_32bit_el0 fw_devlink.strict=1 pcie_ports=compat irqaffinity=0-2 cgroup.memory=nokmem,nosocket nosoftlockup usbcore.interrupt_interval_override=045e:028e:2"

say() { echo "== $*"; }

say "tools"
pacman -S --noconfirm --needed arch-install-scripts squashfs-tools dosfstools gptfdisk > /dev/null

say "ROCKNIX's system (modules, firmware, its sway outputs)"
rm -rf "$WORK"; mkdir -p "$WORK"
unsquashfs -q -d "$WORK/system" "$ROCKNIX/SYSTEM" 'usr/lib/modules' 'usr/lib/firmware' 'usr/lib/kernel-overlays' 'usr/config' 'etc/sway' > /dev/null || true
MODULES=$(find "$WORK/system" -maxdepth 6 -type d -path '*/lib/modules/*' -name '[0-9]*' | head -1)
[ -n "$MODULES" ] || { echo "no kernel modules in ROCKNIX's SYSTEM"; exit 1; }
KVER=$(basename "$MODULES")
say "  kernel $KVER"

say "partitions"
sgdisk -Z "$DEVICE" > /dev/null
sgdisk -n 1:16M:+256M -t 1:ef00 -c 1:system -A 1:set:2 -n 2:0:0 -t 2:8300 -c 2:THOR_ROOT "$DEVICE" > /dev/null
partprobe "$DEVICE"; udevadm settle
mkfs.vfat -F 32 -n ROCKNIX "${DEVICE}1" > /dev/null
mkfs.ext4 -q -F -L THOR_ROOT "${DEVICE}2"
mkdir -p "$ROOT"; mount "${DEVICE}2" "$ROOT"
trap 'umount -R "$ROOT" 2> /dev/null || true' EXIT

say "Arch Linux ARM (pacstrap)"
pacstrap -K "$ROOT" "${PACKAGES[@]}" > "$WORK/pacstrap.log"

say "ROCKNIX's modules and firmware"
mkdir -p "$ROOT/usr/lib/modules" "$ROOT/usr/lib/firmware"
cp -a "$MODULES" "$ROOT/usr/lib/modules/"
FIRMWARE=$(find "$WORK/system" -maxdepth 4 -type d -path '*/lib/firmware' | head -1)
[ -n "$FIRMWARE" ] && cp -a "$FIRMWARE/." "$ROOT/usr/lib/firmware/"
arch-chroot "$ROOT" depmod "$KVER"

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

say "the session (sway, the system program)"
H=$ROOT/home/$USER_NAME
mkdir -p "$H/.config/sway" "$H/.config/systemd/user" "$H/.local/bin"
install -m 755 "$PROGRAM" "$H/.local/bin/sysprog"
echo '[ "$(tty)" = /dev/tty1 ] && exec sway > "$HOME/sway.log" 2>&1' > "$H/.bash_profile"
{
    echo "# The Thor: DSI-2 the main (top) screen, DSI-1 the second (bottom) screen, as ROCKNIX has them."
    # ROCKNIX's own output lines (rotation, scale), if its sway config has them.
    grep -rhE '^\s*output\s+DSI-[12]' "$WORK/system" 2> /dev/null | sort -u || true
    echo "default_border none"
    echo "input type:touch map_to_output DSI-1"
    echo "exec systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP && systemctl --user restart sysprog.service"
} > "$H/.config/sway/config"
cat > "$H/.config/systemd/user/sysprog.service" <<EOF
[Unit]
Description=The system program
After=graphical-session.target

[Service]
ExecStart=/home/$USER_NAME/.local/bin/sysprog
Environment=SYSPROG_BOTTOM_OUTPUT=DSI-1
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
