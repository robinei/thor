#!/bin/bash
# Builds the Thor's packages (packages/*) on this PC for aarch64, then signs and indexes them (repo.sh
# index): makepkg with a config of its own (CARCH=aarch64), each from a writable copy of packages/
# (makepkg writes pkgver back) out of this checkout (THOR_REPO). ClamShell is cross-compiled
# (aarch64-unknown-linux-gnu: rustup's target, clang and lld, against tools/repo/sysroot.sh's sysroot);
# linux-thor packages tools/image/thor-kernel.sh's build; thor-firmware unpacks ROCKNIX's release (kept
# in ~/.cache/thor-vm/rocknix: downloaded there if missing) and goes to the repository's local/, never
# published; the rest are files.
#
#   build.sh [PACKAGE...]     (default all)
#
# THOR_ARCH=x86_64: for the test machine's image instead (tools/image/image.sh vm): ClamShell built
# natively, the Thor's own kernel, firmware and sound left out, into ~/.cache/thor-vm/repo-x86_64 (never
# published).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
CHECKOUT=$(cd "$HERE/../.." && pwd)
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm
ARCH=${THOR_ARCH:-aarch64}
SYSROOT=$CACHE/sysroot
PACKAGES=("$@")
"$HERE/../host-check.sh" packages || exit 1
if [ "$ARCH" = aarch64 ]; then
    export THOR_REPO_DIR=$CACHE/repo
    [ ${#PACKAGES[@]} -gt 0 ] || PACKAGES=(thor-keyring thor-ucm linux-thor clamshell thor-system thor-session thor-firmware)
    [ -d "$SYSROOT/usr/lib" ] || "$HERE/sysroot.sh"
    # linux-thor's input: Armada's kernel, built here once (again by hand for a newer one).
    if printf '%s\n' "${PACKAGES[@]}" | grep -qx linux-thor && [ ! -f "$CACHE/kernel/Image" ]; then
        "$HERE/../image/thor-kernel.sh"
    fi
else
    export THOR_REPO_DIR=$CACHE/repo-$ARCH
    [ ${#PACKAGES[@]} -gt 0 ] || PACKAGES=(thor-keyring clamshell thor-system thor-session)
fi
OUT=$THOR_REPO_DIR/built

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp -r "$CHECKOUT/packages/." "$work/"
# makepkg's config, its own (not this PC's: no tuning for this CPU in packages for others): the
# architecture, generic flags, nothing stripped or split off by this PC's tools (which don't know
# aarch64): ClamShell is stripped by its linker.
cat > "$work/makepkg.conf" <<EOF
DLAGENTS=('file::/usr/bin/curl -qgC - -o %o %u'
          'http::/usr/bin/curl -qgb "" -fLC - --retry 3 --retry-delay 3 -o %o %u'
          'https::/usr/bin/curl -qgb "" -fLC - --retry 3 --retry-delay 3 -o %o %u')
VCSCLIENTS=('git::git')
CARCH=$ARCH
CHOST=$ARCH-unknown-linux-gnu
CFLAGS="-O2 -pipe"
CXXFLAGS="\$CFLAGS"
LDFLAGS="-Wl,-O1"
RUSTFLAGS=""
MAKEFLAGS="-j$(nproc)"
BUILDENV=(!distcc color !ccache check !sign)
OPTIONS=(!strip docs !libtool !staticlibs emptydirs zipman purge !debug !lto)
INTEGRITY_CHECK=(sha256)
PURGE_TARGETS=(usr/{,share}/info/dir .packlist *.pod)
COMPRESSZST=(zstd -c -T0 -)
PKGEXT='.pkg.tar.zst'
SRCEXT='.src.tar.gz'
EOF
# The cross linker: clang for aarch64 against the sysroot, with lld.
cat > "$work/aarch64-cc" <<EOF
#!/bin/sh
exec clang --target=aarch64-unknown-linux-gnu --sysroot="$SYSROOT" -fuse-ld=lld "\$@"
EOF
chmod 755 "$work/aarch64-cc"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=$work/aarch64-cc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C strip=symbols"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C strip=symbols"
[ "$ARCH" != aarch64 ] || export PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_SYSROOT_DIR=$SYSROOT PKG_CONFIG_LIBDIR=$SYSROOT/usr/lib/pkgconfig
# (ClamShell's target dir kept between builds, beside the desktop's.)
export CARGO_TARGET_DIR=$CHECKOUT/clamshell/target
mkdir -p "$OUT" "$work/.sources"
for p in "${PACKAGES[@]}"; do
    echo "== $p"
    rm -f "$OUT"/"$p"-[0-9r]*.pkg.tar.*
    src=$work/.sources
    [ "$p" != thor-firmware ] || { src=$CACHE/rocknix; mkdir -p "$src"; }
    (cd "$work/$p" && THOR_REPO=$CHECKOUT PKGDEST=$OUT SRCDEST=$src BUILDDIR=$work/.build \
        makepkg --config "$work/makepkg.conf" -f --nodeps --noconfirm --noprogressbar > "$work/$p.log" 2>&1) \
        || { tail -30 "$work/$p.log"; exit 1; }
    ls "$OUT"/"$p"-[0-9r]*.pkg.tar.*
    # The kernel's corresponding source beside it, named as the package (GPL-2.0: thor-kernel.sh's bundle).
    if [ "$p" = linux-thor ]; then
        pkg=$(basename "$(ls "$OUT"/linux-thor-*.pkg.tar.*)")
        cp "$CACHE/kernel/linux-thor.source.tar" "$OUT/${pkg%-aarch64.pkg.tar.*}.source.tar"
    fi
done
"$HERE/repo.sh" index
