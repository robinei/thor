#!/bin/bash
# A pacman config and keyring for installing another distribution's packages from this PC: Arch Linux
# ARM's for aarch64 (the Thor's image, the cross-compile sysroot), or Arch's for x86_64 (the test
# machine's image), the local [thor] repository first when given (taken signed or not: it is this
# PC's own build). The keyring: the distribution's keys (Arch Linux ARM's public ones kept in this
# repository, tools/image/keyrings, from its root filesystem tarball; Arch's from this PC) and the Thor's
# packages' key, trusted. Run by thor-image.sh (as root) and sysroot.sh (under fakeroot). DIR's path must
# be short: gpg's agent socket goes there.
#
#   pacman-conf.sh ARCH DIR CACHE [REPO]
#
# DIR: where pacman.conf and gnupg/ go. CACHE: the package cache. REPO: the [thor] repository's folder.
set -euo pipefail
ARCH=$1 DIR=$2 CACHE=$3 REPO=${4:-}
HERE=$(cd "$(dirname "$0")" && pwd)
case $ARCH in
    aarch64)
        KEYRING=archlinuxarm REPOS=(core extra alarm) KEYRINGS=$HERE/keyrings
        MIRRORS="Server = http://mirror.archlinuxarm.org/\$arch/\$repo"
        ;;
    x86_64)
        KEYRING=archlinux REPOS=(core extra) KEYRINGS=/usr/share/pacman/keyrings
        MIRRORS="Include = /etc/pacman.d/mirrorlist"
        ;;
    *) echo "pacman-conf.sh aarch64|x86_64 DIR CACHE [REPO]"; exit 2 ;;
esac
mkdir -p "$DIR" "$CACHE"
{
    printf '[options]\nArchitecture = %s\nCacheDir = %s\nGPGDir = %s\n' "$ARCH" "$CACHE" "$DIR/gnupg"
    printf 'SigLevel = Required DatabaseOptional\nLocalFileSigLevel = Optional\nParallelDownloads = 5\n\n'
    [ -z "$REPO" ] || printf '[thor]\nSigLevel = Optional TrustAll\nServer = file://%s\n\n' "$REPO"
    for r in "${REPOS[@]}"; do printf '[%s]\n%s\n\n' "$r" "$MIRRORS"; done
} > "$DIR/pacman.conf"
key() {
    pacman-key --config "$DIR/pacman.conf" --gpgdir "$DIR/gnupg" "$@" > "$DIR/pacman-key.log" 2>&1 \
        || { cat "$DIR/pacman-key.log"; exit 1; }
}
key --init
key --populate-from "$KEYRINGS" --populate "$KEYRING"
key --add "$HERE/../../packages/thor-keyring/thor.gpg"
key --lsign-key 6C4C5B7FB11D609B739FDAC30BB252FF8CA9B43F
gpgconf --homedir "$DIR/gnupg" --kill all 2> /dev/null || true
