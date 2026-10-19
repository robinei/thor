#!/bin/bash
# Builds ROCKNIX's SM8550 kernel (their Linux version, patches, device trees and config, from their git tag)
# with Android binder added, for Waydroid; cross-compiled on an x86_64 host with LLVM. thor-image.sh uses it
# instead of ROCKNIX's prebuilt kernel when THOR_KERNEL names the output (tools/vm/vm.py passes it on).
#
#   tools/image/thor-kernel.sh [OUT]
#
# OUT (default ~/.cache/thor-vm/kernel, where vm.py's machine sees it): Image, dtbs/ (the 14 SM8550 device
# trees ROCKNIX builds, the Thor's qcs8550-ayn-thor.dtb among them), lib/modules/<release>/, config,
# System.map, release (the kernel release), source (what it was built from). Sources, patches and the
# build tree are kept in ~/.cache/thor-kernel; a second run rebuilds only what changed.
#
# Environment: ROCKNIX_TAG (default 20261001: the release thor-image.sh takes firmware and the rest from),
# JOBS (default: all CPUs).
#
# What ROCKNIX's build does for this kernel (packages/linux/package.mk of projects/ROCKNIX, DEVICE=SM8550)
# and is done here the same way: kernel.org's tarball of the version in its SM8550 case; the patches, in
# their order (scripts/unpack, LC_ALL=C): projects/ROCKNIX/packages/linux/patches/{mainline,<version>}, then
# projects/ROCKNIX/devices/SM8550/patches/linux; their device tree sources copied over the kernel's; the
# device's config (projects/ROCKNIX/devices/SM8550/linux/linux.aarch64.conf) with the build's changes
# (CIFS and iSCSI off, wireless-regdb's regulatory.db built in); Image, modules and the device trees listed
# for SM8550 in projects/ROCKNIX/config.xml, with DTC_FLAGS=-@. No out-of-tree modules: SM8550's options
# have none (ADDITIONAL_DRIVERS="", ROCKNIX_JOYPAD="no"; the Thor's gamepad driver is one of the patches).
#
# Ours, differing: LLVM instead of their GCC 15; no built-in initramfs (theirs builds in ROCKNIX's own: ours
# comes from the boot loader, so it is the only one); binder and binderfs on, no devices made at boot
# (Waydroid makes them in binderfs); DMA-BUF heaps on (Android's allocators use /dev/dma_heap/system, which
# Waydroid passes into the container); release suffix -thor; modules stripped of debug information
# (ROCKNIX's are too). Waydroid needs nothing else from the kernel: ashmem is not used (without /dev/ashmem
# it sets sys.use_memfd=true and Android uses memfd), PSI, namespaces, cgroups, veth, bridge and nftables'
# NAT and masquerade (waydroid-net.sh's nft rules) are already in ROCKNIX's config.
set -euo pipefail
export LC_ALL=C
TAG=${ROCKNIX_TAG:-20261001}
OUT=$(realpath -m "${1:-${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm/kernel}")
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/thor-kernel
JOBS=${JOBS:-$(nproc)}
DEVICE=SM8550

say() { echo "== $*"; }

say "tools"
missing=""
# (bc: the build computes the timer constants and the OIDs' encodings with it; without it they come out
# wrong, silently.)
for tool in clang ld.lld llvm-ar llvm-nm llvm-objcopy llvm-objdump llvm-readelf llvm-strip make flex bison perl python3 \
            bc git curl xz depmod; do
    command -v "$tool" > /dev/null || missing+=" $tool"
done
[ -z "$missing" ] || { echo "missing:$missing"; exit 1; }
say "ROCKNIX's build recipe, tag $TAG"
RX=$CACHE/rocknix-$TAG
if [ ! -d "$RX" ]; then
    git -c advice.detachedHead=false clone -q --depth 1 --branch "$TAG" --filter=blob:none --sparse https://github.com/ROCKNIX/distribution "$RX.tmp"
    git -C "$RX.tmp" sparse-checkout set --skip-checks projects/ROCKNIX/packages/linux "projects/ROCKNIX/devices/$DEVICE" \
        projects/ROCKNIX/patches projects/ROCKNIX/config.xml packages/network/wireless-regdb
    mv "$RX.tmp" "$RX"
fi
PKG=$RX/projects/ROCKNIX/packages/linux
DEV=$RX/projects/ROCKNIX/devices/$DEVICE
# The kernel version and tarball checksum from the SM8550 case of package.mk; wireless-regdb's likewise.
read -r VERSION SHA < <(awk -v dev="$DEVICE" '
    /^ *[A-Z0-9|]+\)$/ { inside = index("|" $1, "|" dev "|") || index("|" $1, "|" dev ")") }
    inside && /PKG_VERSION=/ { gsub(/.*="|"/, ""); v = $0 }
    inside && /PKG_SHA256=/ { gsub(/.*="|"/, ""); print v, $0; exit }' "$PKG/package.mk")
[ -n "${SHA:-}" ] || { echo "no $DEVICE kernel version in $PKG/package.mk"; exit 1; }
REGDB=$(sed -n 's/^PKG_VERSION="\(.*\)"/\1/p' "$RX/packages/network/wireless-regdb/package.mk")
REGDB_SHA=$(sed -n 's/^PKG_SHA256="\(.*\)"/\1/p' "$RX/packages/network/wireless-regdb/package.mk")
mapfile -t DTBS < <(python3 -I -c '
import sys, xml.etree.ElementTree as ET
dev = ET.parse(sys.argv[1]).getroot().find(sys.argv[2])
print("\n".join(dev.get("dtb_prefix") + "/" + f.text + ".dtb" for f in dev.iter("file")))' "$RX/projects/ROCKNIX/config.xml" "$DEVICE")
say "  Linux $VERSION, wireless-regdb $REGDB, ${#DTBS[@]} device trees"

fetch() {  # URL SHA256: into $CACHE, checked
    local file=$CACHE/${1##*/}
    if [ ! -f "$file" ] || ! echo "$2  $file" | sha256sum -c --status; then
        say "  downloading ${1##*/}"
        curl -fL --progress-bar -o "$file" "$1"
        echo "$2  $file" | sha256sum -c --status || { echo "checksum mismatch: $file"; exit 1; }
    fi
}
fetch "https://www.kernel.org/pub/linux/kernel/v${VERSION%%.*}.x/linux-$VERSION.tar.xz" "$SHA"
fetch "https://www.kernel.org/pub/software/network/wireless-regdb/wireless-regdb-$REGDB.tar.xz" "$REGDB_SHA"

# The patches, in scripts/unpack's order for PKG_PATCH_DIRS="mainline SM8550 default" (LINUX is unset for
# SM8550) and the <version> directory: the package's, then the project's, then the device's.
PRJ=$RX/projects/ROCKNIX/patches/linux
DPATCH=$DEV/patches/linux
shopt -s nullglob
PATCHES=("$PKG"/patches/*.patch "$PKG"/patches/aarch64/*.patch "$PKG"/patches/{mainline,$DEVICE,default}/*.patch
         "$PKG/patches/$VERSION"/*.patch "$PKG/patches/$VERSION"/aarch64/*.patch
         "$PRJ"/*.patch "$PRJ"/aarch64/*.patch
         "$PRJ"/mainline/*.patch "$DPATCH"/mainline/*.patch "$PRJ/$DEVICE"/*.patch "$DPATCH/$DEVICE"/*.patch
         "$PRJ"/default/*.patch "$DPATCH"/default/*.patch
         "$PRJ/$VERSION"/*.patch "$DPATCH"/*.patch)
shopt -u nullglob

SRC=$CACHE/linux-$VERSION
STAMP="ROCKNIX $TAG, Linux $VERSION, ${#PATCHES[@]} patches"
if [ "$(cat "$SRC/.thor-source" 2> /dev/null)" != "$STAMP" ]; then
    say "sources: Linux $VERSION and ${#PATCHES[@]} patches"
    rm -rf "$SRC"
    tar -xf "$CACHE/linux-$VERSION.tar.xz" -C "$CACHE"
    for patch in "${PATCHES[@]}"; do
        echo "   ${patch#$RX/}"
        # As scripts/unpack applies them: git apply for binary patches and renames, else patch with ROCKNIX's
        # substitutions.
        if grep -qE '^GIT binary patch$|^rename from|^rename to' "$patch"; then
            git -C "$SRC" apply -p1 --whitespace=nowarn < "$patch"
        else
            sed -e "s#@TARGET_CPU@#cortex-x3#g" -e "s#@DEVICE@#$DEVICE#g" "$patch" | patch -d "$SRC" -p1 -s
        fi
    done
    cp -r "$DEV/linux/dts/." "$SRC/arch/arm64/boot/dts/"
    mkdir -p "$SRC/external-firmware"
    tar -xf "$CACHE/wireless-regdb-$REGDB.tar.xz" -C "$SRC/external-firmware" --strip-components=1 \
        "wireless-regdb-$REGDB/regulatory.db" "wireless-regdb-$REGDB/regulatory.db.p7s"
    echo "$STAMP" > "$SRC/.thor-source"
fi

KMAKE=(make -C "$SRC" -j"$JOBS" ARCH=arm64 LLVM=1)

say "config: ROCKNIX's $DEVICE config, their build's changes, binder"
cp "$DEV/linux/linux.aarch64.conf" "$SRC/.config.new"
cfg() { "$SRC/scripts/config" --file "$SRC/.config.new" "$@"; }
# Their build's (pre_make_target, with ROCKNIX's options: SAMBA_SUPPORT=no, ISCSI_SUPPORT unset; swap, NFS
# and WireGuard stay as configured), but no built-in initramfs.
cfg --set-str INITRAMFS_SOURCE "" --set-str DEFAULT_HOSTNAME thor --disable CIFS \
    --disable SCSI_ISCSI_ATTRS --disable ISCSI_TCP --disable ISCSI_BOOT_SYSFS --disable ISCSI_IBFT_FIND --disable ISCSI_IBFT \
    --set-str EXTRA_FIRMWARE "regulatory.db regulatory.db.p7s" --set-str EXTRA_FIRMWARE_DIR external-firmware
# Ours.
cfg --set-str LOCALVERSION -thor \
    --enable ANDROID_BINDER_IPC --enable ANDROID_BINDERFS --set-str ANDROID_BINDER_DEVICES "" \
    --enable DMABUF_HEAPS --enable DMABUF_HEAPS_SYSTEM
# Only replace .config when it changes, so a rebuild with the same config rebuilds nothing.
cmp -s "$SRC/.config.new" "$SRC/.config.in" 2> /dev/null || { cp "$SRC/.config.new" "$SRC/.config.in"; cp "$SRC/.config.new" "$SRC/.config"; }
"${KMAKE[@]}" -s olddefconfig
for option in ANDROID_BINDER_IPC=y ANDROID_BINDERFS=y 'ANDROID_BINDER_DEVICES=""' DMABUF_HEAPS=y DMABUF_HEAPS_SYSTEM=y; do
    grep -qx "CONFIG_$option" "$SRC/.config" || { echo "CONFIG_$option didn't stick in .config"; exit 1; }
done
RELEASE=$("${KMAKE[@]}" -s kernelrelease)
say "  release $RELEASE"

say "build: Image, modules, ${#DTBS[@]} device trees (LLVM, $JOBS jobs)"
DTC_FLAGS=-@ "${KMAKE[@]}" Image modules "${DTBS[@]}" > "$CACHE/build.log" 2>&1 || { tail -40 "$CACHE/build.log"; exit 1; }

say "install into $OUT"
rm -rf "$OUT"; mkdir -p "$OUT/dtbs"
cp "$SRC/arch/arm64/boot/Image" "$SRC/System.map" "$OUT/"
cp "$SRC/.config" "$OUT/config"
for dtb in "${DTBS[@]}"; do cp "$SRC/arch/arm64/boot/dts/$dtb" "$OUT/dtbs/"; done
"${KMAKE[@]}" -s INSTALL_MOD_PATH="$OUT" INSTALL_MOD_STRIP=1 modules_install > /dev/null
rm -f "$OUT/lib/modules/$RELEASE/build" "$OUT/lib/modules/$RELEASE/source"
echo "$RELEASE" > "$OUT/release"
printf 'ROCKNIX %s %s kernel: Linux %s, %s patches; with binder; built %s with %s\n' "$TAG" "$DEVICE" "$VERSION" \
    "${#PATCHES[@]}" "$(date -u +%F)" "$(clang --version | head -1)" > "$OUT/source"
say "done: $RELEASE, Image $(du -h "$OUT/Image" | cut -f1), $(find "$OUT/lib/modules" -name '*.ko' | wc -l) modules"
