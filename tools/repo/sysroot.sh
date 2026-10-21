#!/bin/bash
# The aarch64 sysroot ClamShell is cross-compiled against on this PC (tools/repo/build.sh): what linking it
# needs (glibc, libgcc, libxkbcommon, libdbus, their pkg-config files), taken from the aarch64 machine's
# disk (tools/vm/vm.py --arch aarch64: Arch Linux ARM, installed by pacman with its signatures checked),
# read with debugfs while the machine is off. Again after the machine's system is updated.
#
#   sysroot.sh [OUT]     (default ~/.cache/thor-vm/sysroot)
set -euo pipefail
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm
DISK=$CACHE/aarch64/disk.img
OUT=${1:-$CACHE/sysroot}
[ -f "$DISK" ] || { echo "no aarch64 machine: tools/vm/vm.py --arch aarch64 image"; exit 1; }
! pgrep -f '^qemu-system-aarch64' > /dev/null || { echo "the aarch64 machine is running: stop it first"; exit 1; }

# The names wanted in /usr/lib (links kept as links, rdump makes them).
LIBS="crt1.o crti.o crtn.o Scrt1.o ld-linux-aarch64.so.1 libc.so libc.so.6 libc_nonshared.a libm.so libm.so.6
      libmvec.so libmvec.so.1 libdl.so.2 libpthread.so.0 librt.so.1 libutil.so.1 libgcc_s.so libgcc_s.so.1
      libdl.a libpthread.a librt.a libutil.a"
rm -rf "$OUT"
mkdir -p "$OUT/usr/lib/pkgconfig" "$OUT/usr/lib/gcc"
ls_lib() { debugfs -R "ls /usr/lib" "$DISK" 2> /dev/null | tr -s ' \t' '\n'; }
names=$(ls_lib)
wanted=$(for l in $LIBS; do echo "$l"; done; echo "$names" | grep -E '^lib(xkbcommon|dbus-1)\.so')
{
    for name in $wanted; do echo "rdump /usr/lib/$name $OUT/usr/lib"; done
    echo "rdump /usr/lib/gcc/aarch64-unknown-linux-gnu $OUT/usr/lib/gcc"
    echo "rdump /usr/lib/pkgconfig/dbus-1.pc $OUT/usr/lib/pkgconfig"
    echo "rdump /usr/lib/pkgconfig/xkbcommon.pc $OUT/usr/lib/pkgconfig"
    # (dbus-1.pc requires it.)
    echo "rdump /usr/lib/pkgconfig/libsystemd.pc $OUT/usr/lib/pkgconfig"
} > "$OUT/.debugfs"
debugfs -f "$OUT/.debugfs" "$DISK" > /dev/null 2>&1
rm "$OUT/.debugfs"
# (Only the compiler's runtime from gcc's folder: not its plugins and headers.)
find "$OUT/usr/lib/gcc" -mindepth 3 -maxdepth 3 ! -name 'crt*.o' ! -name 'libgcc*.a' -exec rm -rf {} +
for l in $LIBS; do [ -e "$OUT/usr/lib/$l" ] || [ -L "$OUT/usr/lib/$l" ] || { echo "missing from the machine: $l"; exit 1; }; done
echo "sysroot: $OUT ($(du -sh "$OUT" | cut -f1))"
