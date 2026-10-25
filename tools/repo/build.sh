#!/bin/bash
# Builds the Thor's packages (packages/*) on this PC for aarch64, then signs and indexes them (repo.sh
# index): makepkg with a config of its own (CARCH=aarch64), each from a writable copy of packages/
# (makepkg writes pkgver back), out of a clone of this checkout's last commit (THOR_REPO): uncommitted
# changes aren't built. ClamShell is cross-compiled
# (aarch64-unknown-linux-gnu: rustup's target, clang and lld, against tools/repo/sysroot.sh's sysroot);
# linux-thor builds Armada's kernel, thor-sensors hexagonrpc (cross-compiled too; their source packages
# also: the GPL's corresponding source, published beside them); thor-firmware unpacks ROCKNIX's release
# (kept in ~/.cache/thor-vm/rocknix: downloaded there if missing) and goes to the repository's local/,
# never published; the rest are files.
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
    [ ${#PACKAGES[@]} -gt 0 ] || PACKAGES=(thor-keyring thor-ucm linux-thor thor-sensors clamshell thor-system thor-session thor thor-firmware)
    # (Made again when it predates what it holds now: C's headers.)
    [ -e "$SYSROOT/usr/lib/libseat.so" ] || "$HERE/sysroot.sh"
else
    export THOR_REPO_DIR=$CACHE/repo-$ARCH
    [ ${#PACKAGES[@]} -gt 0 ] || PACKAGES=(thor-keyring clamshell thor-system thor-session)
fi
OUT=$THOR_REPO_DIR/built

# (On disk, not in /tmp, which may be memory: the kernel's build is several GB.)
mkdir -p "$CACHE/sources"
work=$(mktemp -d "$CACHE/build.XXXXXX")
trap 'rm -rf "$work"' EXIT
# What is built: the checkout's last commit, not its working tree, so that a package's version (the
# commit's hash) names exactly what is in it. (A clone of it, sharing its objects: its history makes the
# versions.)
git clone -q --shared "$CHECKOUT" "$work/src"
echo "building $(git -C "$work/src" log -1 --format='%h %s')"
# (The commit's time as the build's, for reproducible builds; the commit named in what is built.)
SOURCE_DATE_EPOCH=$(git -C "$work/src" log -1 --format=%ct)
THOR_COMMIT=$(git -C "$work/src" rev-parse HEAD)
export SOURCE_DATE_EPOCH THOR_COMMIT
[ -z "$(git -C "$CHECKOUT" status --porcelain)" ] || echo "(uncommitted changes in $CHECKOUT are left out: commit them to build them)"
cp -r "$work/src/packages/." "$work/"
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
# (C packages built with meson, cross: thor-sensors writes its cross file from these.)
[ "$ARCH" != aarch64 ] || export THOR_CROSS_CC=$work/aarch64-cc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C strip=symbols"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C strip=symbols"
[ "$ARCH" != aarch64 ] || export PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_SYSROOT_DIR=$SYSROOT PKG_CONFIG_LIBDIR=$SYSROOT/usr/lib/pkgconfig
# (ClamShell's target dir kept between builds, in the checkout's, its own: the clone's path differs.)
export CARGO_TARGET_DIR=$CHECKOUT/clamshell/target/packages
mkdir -p "$OUT" "$work/.sources"
for p in "${PACKAGES[@]}"; do
    echo "== $p"
    rm -f "$OUT"/"$p"-[0-9r]*.pkg.tar.*
    # Downloads kept between builds (ROCKNIX's release where it already is).
    src=$CACHE/sources
    [ "$p" != thor-firmware ] || { src=$CACHE/rocknix; mkdir -p "$src"; }
    # (A package's PGP keys, keys/pgp as Arch has them, in a keyring of the build's own: kernel.org's
    # signature checked against Linus Torvalds' and Greg Kroah-Hartman's keys.)
    export GNUPGHOME=$work/gnupg-$p
    mkdir -p -m 700 "$GNUPGHOME"
    for key in "$work/$p"/keys/pgp/*.asc; do [ -f "$key" ] && gpg --quiet --import "$key" 2> /dev/null; done
    mk() { (cd "$work/$p" && THOR_REPO=$work/src PKGDEST=$OUT SRCDEST=$src SRCPKGDEST=$OUT BUILDDIR=$work/.build \
        makepkg --config "$work/makepkg.conf" --nodeps --noconfirm --noprogressbar "$@" >> "$work/$p.log" 2>&1) \
        || { tail -30 "$work/$p.log"; exit 1; }; }
    mk -f
    ls "$OUT"/"$p"-[0-9r]*.pkg.tar.*
    # The GPL's corresponding source of what we build from others' GPL code (the kernel, hexagonrpc):
    # its source package with every source in it.
    if [ "$p" = linux-thor ] || [ "$p" = thor-sensors ]; then
        rm -f "$OUT"/"$p"-[0-9]*.src.tar.gz
        mk --allsource -f
        ls "$OUT"/"$p"-[0-9]*.src.tar.gz
    fi
    rm -rf "$work/.build/$p"
    unset GNUPGHOME
done
"$HERE/repo.sh" index
