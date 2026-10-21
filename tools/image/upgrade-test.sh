#!/bin/bash
# Kernel updates on an installed system, tried on a copy of the Thor's image (tools/image/image.sh's): the
# built linux-thor under two further releases (as its next versions would be), installed with pacman
# inside the image (through qemu-user, as root: sudo), without a reboot between them:
#
# 1. the first with thor-boot failing (our initramfs config taken away): the card's files unchanged, and
#    the installed kernel's modules (the one the card still boots) kept;
# 2. thor-boot again, fixed: the card's /KERNEL and initramfs new;
# 3. the second update, still no reboot: the card's kernel (the first update's) keeps its modules too;
# 4. the next boot's cleanup (thor-modules clean): only the newest kernel's modules left.
#
#   upgrade-test.sh
set -euo pipefail
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm
IMAGE=$CACHE/thor-sd.img
[ -f "$IMAGE" ] || { echo "no image: tools/image/image.sh"; exit 1; }
built=$(ls -t "$CACHE"/repo/thor/linux-thor-*-aarch64.pkg.tar.* | grep -v '\.sig$' | head -1)
work=$(mktemp -d "$CACHE/upgrade-test.XXXXXX")
trap 'sudo rm -rf "$work"' EXIT
{ cat /etc/makepkg.conf; echo CARCH=aarch64; echo "OPTIONS=(!strip !debug)"; } > "$work/makepkg.conf"
base=$(bsdtar -tf "$built" | sed -n 's|^usr/lib/modules/\([^/]*\)/Image$|\1|p')
# (The built kernel's release: its own, with Armada's commit and pkgrel: linux-thor's PKGBUILD.)
pkgrel=$(bsdtar -xOf "$built" .PKGINFO | sed -n 's/^pkgver = .*-//p')
[[ $base == *-thor-*-$pkgrel ]] || { echo "FAIL  the built kernel's release, $base, isn't its own (-thor-<commit>-$pkgrel)"; exit 1; }
echo "ok    the built kernel's release is its own: $base"
oldver=$(bsdtar -xOf "$built" .PKGINFO | sed -n 's/^pkgver = //p')

# The built kernel under release $base-nextN, as version pkgrel+N.
next() {  # N
    local dir=$work/next$1
    mkdir -p "$dir/files"
    bsdtar -xf "$built" -C "$dir/files" usr
    mv "$dir/files/usr/lib/modules/$base" "$dir/files/usr/lib/modules/$base-next$1"
    cat > "$dir/PKGBUILD" <<EOF
pkgname=linux-thor
pkgver=${oldver%-*}
pkgrel=$(( ${oldver##*-} + $1 ))
pkgdesc="linux-thor, under another release (tools/image/upgrade-test.sh)"
arch=(aarch64)
license=(GPL-2.0-only)
options=(!strip !debug)
package() { cp -a "\$startdir/files/." "\$pkgdir/"; }
EOF
    (cd "$dir" && PKGDEST="$work" makepkg --nodeps --noconfirm --config "$work/makepkg.conf" > "$dir/makepkg.log" 2>&1) \
        || { tail -20 "$dir/makepkg.log"; exit 1; }
}
echo "== two next versions of $(basename "$built")"
next 1
next 2

echo "== the image, a copy"
cp --reflink=auto --sparse=always "$IMAGE" "$work/sd.img"
sudo bash -s "$work" "$base" <<'ROOT'
set -euo pipefail
work=$1 a=$2 b=$2-next1 c=$2-next2
dev=$(losetup -Pf --show "$work/sd.img")
root=$work/root boot=$work/boot
mkdir -p "$root" "$boot"
trap 'umount -R -l "$root" 2> /dev/null || true; umount -l "$boot" 2> /dev/null || true; losetup -d "$dev"' EXIT
udevadm settle
mount "${dev}p2" "$root"
mods=$root/usr/lib/modules
card() { mount -o ro "${dev}p1" "$boot"; sha256sum "$boot/KERNEL" "$boot/Image" "$boot/initramfs-thor.img" | cut -c1-16; ls "$boot"/*.new "$boot"/*/*.new 2> /dev/null; umount "$boot"; }
cp "$work"/linux-thor-*.pkg.tar.* "$root/var/cache/"
fail=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi; }
install() { arch-chroot "$root" pacman -U --noconfirm "/var/cache/$(basename "$(ls "$work"/linux-thor-*-"$1"-aarch64.pkg.tar.*)")" >> "$work/pacman.log" 2>&1 || true; }
rel() { basename "$(ls "$work"/linux-thor-*.pkg.tar.* | sort -V | sed -n "${1}p")" | sed -E 's/.*-([0-9]+)-aarch64.*/\1/'; }

echo "-- 1: an update whose thor-boot fails"
before=$(card)
mv "$root/usr/lib/thor/mkinitcpio.conf" "$root/usr/lib/thor/mkinitcpio.conf.away"
install "$(rel 1)"
check "thor-boot failed" "! grep -q 'thor-boot: done' '$work/pacman.log'"
check "the card's files unchanged, no half-written ones" "[ \"\$(card)\" = \"\$before\" ]"
check "the card's kernel's modules kept ($a)" "[ -f '$mods/$a/modules.dep' ]"
check "the new kernel's there ($b)" "[ -f '$mods/$b/Image' ]"

echo "-- 2: thor-boot again, fixed"
mv "$root/usr/lib/thor/mkinitcpio.conf.away" "$root/usr/lib/thor/mkinitcpio.conf"
arch-chroot "$root" thor-boot >> "$work/pacman.log" 2>&1 || true
check "thor-boot done, with $b" "grep -q 'thor-boot: kernel and initramfs $b' '$work/pacman.log'"
after=$(card)
check "the card's files new" "[ \"\$after\" != \"\$before\" ]"

echo "-- 3: a second update, no reboot since"
: > "$work/pacman.log"
install "$(rel 2)"
check "thor-boot done, with $c" "grep -q 'thor-boot: kernel and initramfs $c' '$work/pacman.log'"
check "the card's previous kernel's modules kept ($b)" "[ -f '$mods/$b/modules.dep' ]"
check "no copies left over" "! ls -d '$mods'/.thor-kept-* 2> /dev/null"

echo "-- 4: the next boot's cleanup"
arch-chroot "$root" /usr/lib/thor/thor-modules clean > /dev/null
check "only the newest kernel's modules left" "[ \"\$(ls '$mods')\" = '$c' ]"
[ "$fail" = 0 ] || { echo "--- pacman's and thor-boot's output"; tail -40 "$work/pacman.log"; exit 1; }
ROOT
echo "== kernel updates work"
