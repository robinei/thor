#!/bin/bash
# What this PC needs for each step from a checkout of this repository, and the one command that installs
# what is missing (Arch Linux; the rest of the steps download what they need into ~/.cache/thor-vm).
#
#   host-check.sh STEP...    kernel | packages | image | vm-image | vm-run
#
# kernel: tools/image/thor-kernel.sh (Armada's kernel, with LLVM). packages: tools/repo/build.sh (makepkg,
# ClamShell cross-compiled with rustup's aarch64 target, clang and lld). image: tools/image/image.sh (the
# Thor's: pacstrap through qemu-user's binfmt, as root). vm-image: tools/image/image.sh vm. vm-run:
# tools/vm/vm.py thor-image (QEMU with UEFI).
set -u
missing=() notes=()
# need COMMAND PACKAGE: the command, else its package.
need() { command -v "$1" > /dev/null || missing+=("$2"); }
for step in "$@"; do
    case $step in
        kernel)
            for t in clang:clang ld.lld:lld llvm-ar:llvm llvm-objcopy:llvm make:make flex:flex bison:bison perl:perl \
                     python3:python bc:bc git:git curl:curl xz:xz depmod:kmod patch:patch; do need "${t%%:*}" "${t#*:}"; done
            ;;
        packages)
            for t in makepkg:pacman fakeroot:fakeroot git:git rustup:rustup clang:clang ld.lld:lld pkg-config:pkgconf \
                     repo-add:pacman gpg:gnupg unsquashfs:squashfs-tools bsdtar:libarchive curl:curl; do need "${t%%:*}" "${t#*:}"; done
            command -v rustup > /dev/null && ! rustup target list --installed | grep -qx aarch64-unknown-linux-gnu \
                && notes+=("rustup target add aarch64-unknown-linux-gnu")
            ;;
        image | vm-image)
            for t in pacstrap:arch-install-scripts arch-chroot:arch-install-scripts mkfs.vfat:dosfstools mkfs.ext4:e2fsprogs \
                     sfdisk:util-linux losetup:util-linux bsdtar:libarchive sudo:sudo; do need "${t%%:*}" "${t#*:}"; done
            [ "$step" = vm-image ] || [ -e /proc/sys/fs/binfmt_misc/qemu-aarch64 ] \
                || missing+=(qemu-user-static qemu-user-static-binfmt)
            ;;
        vm-run)
            need qemu-system-x86_64 qemu-system-x86
            [ -f /usr/share/edk2/x64/OVMF_CODE.4m.fd ] || missing+=(edk2-ovmf)
            ;;
        *) echo "host-check.sh kernel|packages|image|vm-image|vm-run ..."; exit 2 ;;
    esac
done
[ ${#missing[@]} -eq 0 ] && [ ${#notes[@]} -eq 0 ] && exit 0
echo "This PC lacks what $* needs:"
[ ${#missing[@]} -eq 0 ] || echo "  sudo pacman -S --needed $(printf '%s\n' "${missing[@]}" | sort -u | tr '\n' ' ')"
for n in "${notes[@]}"; do echo "  $n"; done
exit 1
