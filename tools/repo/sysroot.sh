#!/bin/bash
# The aarch64 sysroot ClamShell is cross-compiled against on this PC (tools/repo/build.sh): what linking it
# needs (glibc, libgcc, libxkbcommon, libdbus, their pkg-config files), from Arch Linux ARM's packages,
# installed by this PC's pacman with their signatures checked (tools/image/pacman-conf.sh's config and
# keyring), under fakeroot, without their scripts, into a scratch root; the files linking needs copied
# out of it. Again after Arch Linux ARM's glibc or those libraries move on (an older sysroot still makes
# binaries that run on a newer system).
#
#   sysroot.sh [OUT]     (default ~/.cache/thor-vm/sysroot)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm
OUT=${1:-$CACHE/sysroot}
PACKAGES=(glibc libgcc gcc libxkbcommon dbus systemd-libs)
# What linking needs from /usr/lib (links kept as links), besides the libraries' own .so names.
LIBS="crt1.o crti.o crtn.o Scrt1.o ld-linux-aarch64.so.1 libc.so libc.so.6 libc_nonshared.a libm.so libm.so.6
      libmvec.so libmvec.so.1 libdl.so.2 libpthread.so.0 librt.so.1 libutil.so.1 libgcc_s.so libgcc_s.so.1
      libdl.a libpthread.a librt.a libutil.a"
command -v fakeroot > /dev/null || { echo "needs fakeroot (base-devel)"; exit 1; }

# (A short path: gpg's agent socket goes in it.)
work=$(mktemp -d /tmp/thor-sysroot.XXXXXX)
trap 'rm -rf "$work"' EXIT
fakeroot bash "$HERE/../image/pacman-conf.sh" aarch64 "$work/pacman" "$CACHE/pacman-aarch64"
mkdir -p "$work/root/var/lib/pacman"
# (-dd: only these, not what they depend on; --noscriptlet: nothing of theirs run.)
fakeroot pacman --config "$work/pacman/pacman.conf" --root "$work/root" --dbpath "$work/root/var/lib/pacman" \
    --noconfirm --noscriptlet --noprogressbar -Sydd "${PACKAGES[@]}" > "$work/pacman.log" 2>&1 \
    || { tail -20 "$work/pacman.log"; exit 1; }
gpgconf --homedir "$work/pacman/gnupg" --kill all 2> /dev/null || true

lib=$work/root/usr/lib
rm -rf "$OUT"
mkdir -p "$OUT/usr/lib/pkgconfig" "$OUT/usr/lib/gcc"
for name in $LIBS; do
    [ -e "$lib/$name" ] || [ -L "$lib/$name" ] || { echo "missing from Arch Linux ARM's packages: $name"; exit 1; }
    cp -a "$lib/$name" "$OUT/usr/lib/"
done
cp -a "$lib"/libxkbcommon.so* "$lib"/libdbus-1.so* "$OUT/usr/lib/"
# (dbus-1.pc requires libsystemd.pc.)
cp -a "$lib/pkgconfig/dbus-1.pc" "$lib/pkgconfig/xkbcommon.pc" "$lib/pkgconfig/libsystemd.pc" "$OUT/usr/lib/pkgconfig/"
# Only the compiler's runtime from gcc's folder: not its plugins and headers.
cp -a "$lib/gcc/aarch64-unknown-linux-gnu" "$OUT/usr/lib/gcc/"
find "$OUT/usr/lib/gcc" -mindepth 3 -maxdepth 3 ! -name 'crt*.o' ! -name 'libgcc*.a' -exec rm -rf {} +
echo "sysroot: $OUT ($(du -sh "$OUT" | cut -f1)), from: $(pacman --config "$work/pacman/pacman.conf" --root "$work/root" --dbpath "$work/root/var/lib/pacman" -Q "${PACKAGES[@]}" | tr '\n' ' ')"
