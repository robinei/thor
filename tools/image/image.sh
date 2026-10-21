#!/bin/bash
# An image built on this PC from the packages (tools/repo/build.sh first): the Thor's SD card image, or the
# test machine's (x86_64: the same system with Arch's kernel, for tools/vm/vm.py thor-image), then
# tools/image/thor-image.sh as root (sudo). The Thor's needs qemu-user's binfmt for aarch64
# (qemu-user-static-binfmt) and squashfs-tools (thor-firmware); pacman's config and keyring:
# tools/image/pacman-conf.sh's.
#
#   image.sh [thor [SIZE_GB]]   (default 12) into ~/.cache/thor-vm/thor-sd.img
#   image.sh vm [SIZE_GB]       (default 32: room for the first boot's swap file) into ~/.cache/thor-vm/thor-vm.img
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm
TARGET=${1:-thor}
case $TARGET in
    thor)
        "$HERE/../host-check.sh" packages image || exit 1
        SIZE=${2:-12} IMAGE=$CACHE/thor-sd.img
        "$HERE/../repo/build.sh"
        firmware=$(ls -t "$CACHE"/repo/local/thor-firmware-*.pkg.tar.* | head -1)
        release=$(basename "$firmware" | sed -E 's/^thor-firmware-([0-9]+)-.*/\1/')
        rocknix=$CACHE/rocknix/ROCKNIX-SM8550.aarch64-$release.tar
        rm -f "$IMAGE"; truncate -s "${SIZE}G" "$IMAGE"
        sudo XDG_CACHE_HOME="${CACHE%/thor-vm}" bash "$HERE/thor-image.sh" thor "$IMAGE" "$CACHE/repo/thor" "$CACHE/pacman-aarch64" "$rocknix" "$firmware"
        echo "$IMAGE: write it to an SD card (dd, bs=4M)"
        ;;
    vm)
        "$HERE/../host-check.sh" packages vm-image vm-run || exit 1
        SIZE=${2:-32} IMAGE=$CACHE/thor-vm.img
        THOR_ARCH=x86_64 "$HERE/../repo/build.sh"
        rm -f "$IMAGE"; truncate -s "${SIZE}G" "$IMAGE"
        sudo XDG_CACHE_HOME="${CACHE%/thor-vm}" bash "$HERE/thor-image.sh" vm "$IMAGE" "$CACHE/repo-x86_64/thor" "$CACHE/pacman-x86_64"
        sudo chown "$(id -u):$(id -g)" "$IMAGE"
        echo "$IMAGE: tools/vm/vm.py thor-image starts it"
        ;;
    *) echo "image.sh [thor [SIZE_GB] | vm [SIZE_GB]]"; exit 2 ;;
esac
