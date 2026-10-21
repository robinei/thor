#!/bin/bash
# An image built on this PC from the packages (tools/repo/build.sh first): the Thor's SD card image, or the
# test machine's (x86_64: the same system with Arch's kernel, for tools/vm/vm.py thor-image), then
# tools/image/thor-image.sh as root (sudo). The Thor's needs qemu-user's binfmt for aarch64
# (qemu-user-static-binfmt) and squashfs-tools (thor-firmware), and Arch Linux ARM's keyring (from the
# aarch64 machine's root tarball, which tools/vm/vm.py checked when it downloaded it).
#
#   image.sh [thor [SIZE_GB]]   (default 12) into ~/.cache/thor-vm/thor-sd.img
#   image.sh vm [SIZE_GB]       (default 32: room for the first boot's swap file) into ~/.cache/thor-vm/thor-vm.img
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm
TARGET=${1:-thor}
case $TARGET in
    thor)
        SIZE=${2:-12} IMAGE=$CACHE/thor-sd.img
        "$HERE/../repo/build.sh"
        firmware=$(ls -t "$CACHE"/repo/local/thor-firmware-*.pkg.tar.* | head -1)
        release=$(basename "$firmware" | sed -E 's/^thor-firmware-([0-9]+)-.*/\1/')
        rocknix=$CACHE/rocknix/ROCKNIX-SM8550.aarch64-$release.tar
        keyrings=$CACHE/alarm-keyrings
        if [ ! -f "$keyrings/archlinuxarm.gpg" ]; then
            tarball=$CACHE/aarch64/ArchLinuxARM-aarch64-latest.tar.gz
            [ -f "$tarball" ] || { echo "no Arch Linux ARM tarball: tools/vm/vm.py --arch aarch64 image"; exit 1; }
            mkdir -p "$keyrings"
            bsdtar -xf "$tarball" -C "$keyrings" --strip-components=5 \
                ./usr/share/pacman/keyrings/archlinuxarm.gpg ./usr/share/pacman/keyrings/archlinuxarm-trusted \
                ./usr/share/pacman/keyrings/archlinuxarm-revoked
        fi
        rm -f "$IMAGE"; truncate -s "${SIZE}G" "$IMAGE"
        sudo bash "$HERE/thor-image.sh" thor "$IMAGE" "$CACHE/repo/thor" "$keyrings" "$CACHE/pacman-aarch64" "$rocknix" "$firmware"
        echo "$IMAGE: write it to an SD card (dd, bs=4M)"
        ;;
    vm)
        SIZE=${2:-32} IMAGE=$CACHE/thor-vm.img
        THOR_ARCH=x86_64 "$HERE/../repo/build.sh"
        rm -f "$IMAGE"; truncate -s "${SIZE}G" "$IMAGE"
        sudo bash "$HERE/thor-image.sh" vm "$IMAGE" "$CACHE/repo-x86_64/thor" /usr/share/pacman/keyrings "$CACHE/pacman-x86_64"
        sudo chown "$(id -u):$(id -g)" "$IMAGE"
        echo "$IMAGE: tools/vm/vm.py thor-image starts it"
        ;;
    *) echo "image.sh [thor [SIZE_GB] | vm [SIZE_GB]]"; exit 2 ;;
esac
