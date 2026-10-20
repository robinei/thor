#!/bin/bash
# Builds Armada's kernel (github.com/armada-os/armada, packages/kernel: their Linux version, patch series,
# device trees and config) with our few changes, cross-compiled on an x86_64 host with LLVM. Armada carries
# the furthest community work for the Thor (ROCKNIX's stack, newer, with Thorch's, Pocknix's, Kettle's and
# their own: sleep, the gamepad across it; each patch's source in their PATCHES.md). thor-image.sh uses it
# instead of ROCKNIX's prebuilt kernel when THOR_KERNEL names the output (tools/vm/vm.py passes it on);
# firmware, audio profiles and the rest stay ROCKNIX's.
#
#   tools/image/thor-kernel.sh [OUT]
#
# OUT (default ~/.cache/thor-vm/kernel, where vm.py's machine sees it): Image, dtbs/ (Armada's SM8550 device
# trees, the Thor's qcs8550-ayn-thor.dtb among them), lib/modules/<release>/, config, System.map, release
# (the kernel release), source (what it was built from: Armada's commit). Sources and the build tree are
# kept in ~/.cache/thor-kernel; a second run rebuilds only what changed.
#
# Environment: ARMADA_REF (default main: their latest), JOBS (default: all CPUs).
#
# As Armada's scripts/build-kernel.sh does: kernel.org's tarball of BASE.env's version; patches/series in
# order, each applied without fuzz; their dts/*.dts(i) copied into arch/arm64/boot/dts/qcom, then their
# dts/*.patch; defconfig with config/armada-kernel.config.overrides merged (merge_config.sh), each =y
# checked. Then ours: kernel/series (patches of our own, after theirs; none yet), and a release suffix
# -thor, DMA-BUF heaps on (Android's allocators use /dev/dma_heap/system, which Waydroid passes into the
# container), binder's devices left to binderfs (Waydroid makes them). Binder itself (the C driver), its
# filesystem and the rest Waydroid needs are in Armada's config. Ours, differing: LLVM instead of their
# GCC; no built-in initramfs (ours comes from the boot loader); modules stripped of debug information (as
# theirs).
set -euo pipefail
export LC_ALL=C
REF=${ARMADA_REF:-main}
OUT=$(realpath -m "${1:-${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm/kernel}")
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-kernel
JOBS=${JOBS:-$(nproc)}

say() { echo "== $*"; }

say "tools"
missing=""
# (bc: the build computes the timer constants and the OIDs' encodings with it; without it they come out
# wrong, silently.)
for tool in clang ld.lld llvm-ar llvm-nm llvm-objcopy llvm-objdump llvm-readelf llvm-strip make flex bison perl python3 \
            bc git curl xz depmod patch; do
    command -v "$tool" > /dev/null || missing+=" $tool"
done
[ -z "$missing" ] || { echo "missing:$missing"; exit 1; }

say "Armada's kernel, $REF"
AR=$CACHE/armada
if [ ! -d "$AR/.git" ]; then
    rm -rf "$AR"
    git clone -q --filter=blob:none --sparse https://github.com/armada-os/armada "$AR"
    git -C "$AR" sparse-checkout set packages/kernel
fi
git -C "$AR" fetch -q origin "$REF"
git -C "$AR" -c advice.detachedHead=false checkout -q FETCH_HEAD
COMMIT=$(git -C "$AR" rev-parse --short=12 HEAD)
K=$AR/packages/kernel
VERSION=$(. "$K/BASE.env"; echo "$VERSION")
mapfile -t SERIES < <(sed 's/#.*//' "$K/patches/series" | awk 'NF { print $1 }')
say "  commit $COMMIT ($(git -C "$AR" log -1 --format=%cs)): Linux $VERSION, ${#SERIES[@]} patches"

# Ours (tools/image/kernel), after Armada's, in kernel/series' order.
OURS=$(dirname "$(realpath "$0")")/kernel
mapfile -t MINE < <(sed 's/#.*//' "$OURS/series" | awk 'NF { print $1 }')

# kernel.org's tarball, checked against its signed checksum list's entry.
TARBALL=$CACHE/linux-$VERSION.tar.xz
if [ ! -f "$TARBALL" ]; then
    say "  downloading linux-$VERSION.tar.xz"
    curl -fsSL -o "$TARBALL.part" "https://cdn.kernel.org/pub/linux/kernel/v${VERSION%%.*}.x/linux-$VERSION.tar.xz"
    SUM=$(curl -fsSL "https://cdn.kernel.org/pub/linux/kernel/v${VERSION%%.*}.x/sha256sums.asc" | awk -v f="linux-$VERSION.tar.xz" '$2 == f { print $1 }')
    [ -n "$SUM" ] && echo "$SUM  $TARBALL.part" | sha256sum -c --status || { echo "checksum mismatch: linux-$VERSION.tar.xz"; exit 1; }
    mv "$TARBALL.part" "$TARBALL"
fi

# (Each patch applied with no fuzz and every hunk required, as Armada's build does.)
apply() {  # PATCH
    local log expected
    log=$(patch -d "$SRC" -p1 --batch --forward -F0 --dry-run --verbose < "$1" 2>&1) || { echo "$log"; echo "doesn't apply: $1"; exit 1; }
    expected=$(grep -c '^@@ ' "$1" || true)
    [ "$(grep -c '^Hunk #[0-9].*succeeded' <<< "$log" || true)" -eq "$expected" ] || { echo "$log"; echo "not every hunk applies: $1"; exit 1; }
    patch -d "$SRC" -p1 --batch --forward -F0 --no-backup-if-mismatch -s < "$1"
}

SRC=$CACHE/linux-$VERSION-armada
STAMP="Armada $COMMIT; ours $(cat "$OURS/series" "${MINE[@]/#/$OURS/}" | sha256sum | cut -c1-12)"
if [ "$(cat "$SRC/.thor-source" 2> /dev/null)" != "$STAMP" ]; then
    say "sources: Linux $VERSION, ${#SERIES[@]} patches, their device trees${MINE:+, ${#MINE[@]} of ours}"
    rm -rf "$SRC" "$CACHE/linux-$VERSION"
    tar -xf "$TARBALL" -C "$CACHE"
    mv "$CACHE/linux-$VERSION" "$SRC"
    for patch in "${SERIES[@]}"; do
        apply "$K/patches/$patch"
    done
    cp "$K"/dts/*.dts "$K"/dts/*.dtsi "$SRC/arch/arm64/boot/dts/qcom/"
    # (Applied straight, as Armada's build does: a file may hold several patches to the same source, so a
    # dry run of the whole would fail on the second.)
    for patch in "$K"/dts/*.patch; do
        patch -d "$SRC" -p1 --batch --forward -F0 --no-backup-if-mismatch -s < "$patch" || { echo "doesn't apply: $patch"; exit 1; }
    done
    for dts in "$K"/dts/*.dts; do
        echo "dtb-\$(CONFIG_ARCH_QCOM) += $(basename "$dts" .dts).dtb"
    done >> "$SRC/arch/arm64/boot/dts/qcom/Makefile"
    for patch in "${MINE[@]}"; do
        echo "   ours: $patch"
        apply "$OURS/$patch"
    done
    echo "$STAMP" > "$SRC/.thor-source"
fi
# The SM8550 boards' device trees (ROCKNIX's ABL picks one by its Device model; the EFI path loads the Thor's).
mapfile -t DTBS < <(for dts in "$K"/dts/qcs8550-*.dts; do echo "qcom/$(basename "$dts" .dts).dtb"; done)

KMAKE=(make -C "$SRC" -j"$JOBS" ARCH=arm64 LLVM=1)

say "config: defconfig, Armada's overrides, ours"
FRAG=$CACHE/config.fragment
{
    # (kconfig can't read comments after an assignment.)
    sed -E '/^[[:space:]]*CONFIG_[A-Z0-9_]+=/ s/[[:space:]]*#.*$//' "$K/config/armada-kernel.config.overrides"
    printf '%s\n' 'CONFIG_LOCALVERSION="-thor"' 'CONFIG_INITRAMFS_SOURCE=""' 'CONFIG_DEFAULT_HOSTNAME="thor"' \
        'CONFIG_ANDROID_BINDER_DEVICES=""' CONFIG_DMABUF_HEAPS=y CONFIG_DMABUF_HEAPS_SYSTEM=y
} > "$FRAG"
# Only replace .config when it changes, so a rebuild with the same config rebuilds nothing.
if ! cmp -s "$FRAG" "$SRC/.config.fragment" 2> /dev/null || [ ! -f "$SRC/.config" ]; then
    "${KMAKE[@]}" -s defconfig
    (cd "$SRC" && ARCH=arm64 LLVM=1 bash scripts/kconfig/merge_config.sh -m .config "$FRAG" > /dev/null)
    "${KMAKE[@]}" -s olddefconfig
    cp "$FRAG" "$SRC/.config.fragment"
fi
# (Those asked for as built in, as Armada's build checks, and ours.)
lost=$(grep -E '^CONFIG_[A-Z0-9_]+=y$|^CONFIG_(LOCALVERSION|INITRAMFS_SOURCE|ANDROID_BINDER_DEVICES)=' "$FRAG" | while read -r want; do grep -qxF "$want" "$SRC/.config" || echo "$want"; done)
[ -z "$lost" ] || { echo "didn't survive Kconfig's dependencies:"; echo "$lost"; exit 1; }
say "build: Image, modules, ${#DTBS[@]} device trees (LLVM, $JOBS jobs)"
DTC_FLAGS=-@ "${KMAKE[@]}" Image modules "${DTBS[@]}" > "$CACHE/build.log" 2>&1 || { tail -40 "$CACHE/build.log"; exit 1; }
# (Known once built: before, a fresh tree's kernelrelease lacks the config's suffix.)
RELEASE=$(cat "$SRC/include/config/kernel.release")
say "  release $RELEASE"

say "install into $OUT"
rm -rf "$OUT"; mkdir -p "$OUT/dtbs"
cp "$SRC/arch/arm64/boot/Image" "$SRC/System.map" "$OUT/"
cp "$SRC/.config" "$OUT/config"
for dtb in "${DTBS[@]}"; do cp "$SRC/arch/arm64/boot/dts/$dtb" "$OUT/dtbs/"; done
"${KMAKE[@]}" -s INSTALL_MOD_PATH="$OUT" INSTALL_MOD_STRIP=1 modules_install > /dev/null
rm -f "$OUT/lib/modules/$RELEASE/build" "$OUT/lib/modules/$RELEASE/source"
echo "$RELEASE" > "$OUT/release"
printf "Armada's kernel, commit %s: Linux %s, %s patches%s; built %s with %s\n" "$COMMIT" "$VERSION" "${#SERIES[@]}" \
    "${MINE:+ and ${#MINE[@]} of ours}" "$(date -u +%F)" "$(clang --version | head -1)" > "$OUT/source"
say "done: $RELEASE, Image $(du -h "$OUT/Image" | cut -f1), $(find "$OUT/lib/modules" -name '*.ko' | wc -l) modules"
