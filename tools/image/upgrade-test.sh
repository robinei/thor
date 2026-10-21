#!/bin/bash
# A kernel update on an installed system, tried on a copy of the Thor's image (tools/image/image.sh's):
# a second linux-thor, the built one under another release (as Armada's next commit would be), installed
# with pacman inside the image (through qemu-user, as root: sudo). Checked: thor-boot finished, the card's
# /KERNEL and initramfs are new, the old release's modules are still there after the update (for the
# kernel still on the card), and gone after thor-old-modules' cleanup. In a chroot the running kernel is
# this PC's, so the copy thor-modules keep would make of the running kernel's modules is made here.
#
#   upgrade-test.sh
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm
IMAGE=$CACHE/thor-sd.img
[ -f "$IMAGE" ] || { echo "no image: tools/image/image.sh"; exit 1; }
built=$(ls -t "$CACHE"/repo/thor/linux-thor-*-aarch64.pkg.tar.* | grep -v '\.sig$' | head -1)
work=$(mktemp -d "$CACHE/upgrade-test.XXXXXX")
trap 'sudo rm -rf "$work"' EXIT

echo "== the next linux-thor: $(basename "$built") under another release"
mkdir -p "$work/pkg/files"
bsdtar -xf "$built" -C "$work/pkg/files" usr
old=$(ls "$work/pkg/files/usr/lib/modules")
new=$old-next
mv "$work/pkg/files/usr/lib/modules/$old" "$work/pkg/files/usr/lib/modules/$new"
oldver=$(bsdtar -xOf "$built" .PKGINFO | sed -n 's/^pkgver = //p')
cat > "$work/pkg/PKGBUILD" <<EOF
pkgname=linux-thor
pkgver=${oldver%-*}
pkgrel=$(( ${oldver##*-} + 1 ))
pkgdesc="linux-thor, under another release (tools/image/upgrade-test.sh)"
arch=(aarch64)
license=(GPL-2.0-only)
options=(!strip !debug)
package() { cp -a "\$startdir/files/." "\$pkgdir/"; }
EOF
{ cat /etc/makepkg.conf; echo CARCH=aarch64; echo "OPTIONS=(!strip !debug)"; } > "$work/makepkg.conf"
(cd "$work/pkg" && PKGDEST="$work" makepkg --nodeps --noconfirm --config "$work/makepkg.conf" > "$work/makepkg.log" 2>&1) \
    || { tail -20 "$work/makepkg.log"; exit 1; }
next=$(ls "$work"/linux-thor-*.pkg.tar.*)

echo "== the image, a copy"
cp --reflink=auto --sparse=always "$IMAGE" "$work/sd.img"
sudo bash -s "$work" "$next" "$old" "$new" <<'ROOT'
set -euo pipefail
work=$1 next=$2 old=$3 new=$4
dev=$(losetup -Pf --show "$work/sd.img")
root=$work/root boot=$work/boot
mkdir -p "$root" "$boot"
trap 'umount -R -l "$root" 2> /dev/null || true; umount -l "$boot" 2> /dev/null || true; losetup -d "$dev"' EXIT
udevadm settle
mount "${dev}p2" "$root"
sum() { mount -o ro "${dev}p1" "$boot"; sha256sum "$boot/KERNEL" "$boot/initramfs-thor.img" | cut -c1-16; umount "$boot"; }
before=$(sum)
cp "$next" "$root/var/cache/next.pkg.tar.zst"
# (thor-modules keep's copy: on the device of the running kernel's, here of the installed one's.)
cp -a "$root/usr/lib/modules/$old" "$root/usr/lib/modules/.thor-kept-$old"
fail=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi; }
arch-chroot "$root" pacman -U --noconfirm /var/cache/next.pkg.tar.zst > "$work/pacman.log" 2>&1 || true
check "pacman installed the next kernel" "grep -q 'thor-boot: done' '$work/pacman.log'"
check "thor-boot took the new release" "grep -q 'thor-boot: kernel and initramfs $new' '$work/pacman.log'"
after=$(sum)
check "the card's /KERNEL and initramfs are new" "[ \"\$before\" != \"\$after\" ]"
check "the old release's modules kept (the card's kernel until it is replaced)" "[ -d '$root/usr/lib/modules/$old' ]"
check "the new release's modules there" "[ -f '$root/usr/lib/modules/$new/Image' ]"
check "no copy left over" "! ls -d '$root'/usr/lib/modules/.thor-kept-* 2> /dev/null"
# (thor-old-modules at the next boot: here the running kernel is this PC's, so every folder no package
# owns goes.)
arch-chroot "$root" /usr/lib/thor/thor-modules clean > /dev/null
check "after a boot, the old release's modules gone" "[ ! -e '$root/usr/lib/modules/$old' ]"
check "and the new one's still there" "[ -f '$root/usr/lib/modules/$new/Image' ]"
[ "$fail" = 0 ] || { echo "--- pacman's output"; tail -30 "$work/pacman.log"; exit 1; }
ROOT
echo "== the kernel update works"
