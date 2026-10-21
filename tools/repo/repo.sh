#!/bin/bash
# The Thor's package repository on this computer (~/.cache/thor-vm/repo), and its publishing as the
# GitHub release "packages" of robinei/thor, where devices' pacman reads it:
#
#   [thor]
#   Server = https://github.com/robinei/thor/releases/download/packages
#
#   repo.sh index     the packages built (built/, from tools/repo/build.sh) signed and added to the
#                     repository (thor/: thor.db, the packages, their signatures; older versions
#                     removed); thor-firmware, if one was built, into local/ instead: never published
#                     (AYN's and Qualcomm's firmware)
#   repo.sh publish   thor/ uploaded to the release (made if missing), assets no longer in thor/ deleted
#
# Signing: the key in ~/.config/thor-repo/gnupg (made for this, without a passphrase; its public half is
# thor-keyring's). Publishing: GitHub's command line (gh), logged in.
set -euo pipefail
# (THOR_REPO_DIR: another, as the test machine's, ~/.cache/thor-vm/repo-x86_64, never published.)
REPO=${THOR_REPO_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm/repo}
export GNUPGHOME=${THOR_GNUPGHOME:-$HOME/.config/thor-repo/gnupg}
KEY=6C4C5B7FB11D609B739FDAC30BB252FF8CA9B43F
GITHUB=robinei/thor
RELEASE=packages

index() {
    mkdir -p "$REPO/built" "$REPO/thor" "$REPO/local"
    shopt -s nullglob
    local new=()
    for pkg in "$REPO"/built/*.pkg.tar.*; do
        case $pkg in *.sig) continue ;; esac
        local name
        name=$(basename "$pkg")
        if [[ $name == thor-firmware-* ]]; then
            rm -f "$REPO"/local/thor-firmware-*
            mv "$pkg" "$REPO/local/"
            echo "local: $name"
            continue
        fi
        mv "$pkg" "$REPO/thor/"
        gpg --batch --yes --quiet --detach-sign --no-armor -u "$KEY" "$REPO/thor/$name"
        new+=("$REPO/thor/$name")
    done
    [ ${#new[@]} -gt 0 ] || { echo "nothing new in $REPO/built"; return 0; }
    # (-R: the versions they replace removed; -s/-k: the database signed too.)
    repo-add -q -R -s -k "$KEY" "$REPO/thor/thor.db.tar.zst" "${new[@]}"
    # (GitHub serves no links: the names pacman asks for, as files.)
    cp --remove-destination "$(readlink -f "$REPO/thor/thor.db.tar.zst")" "$REPO/thor/thor.db"
    cp --remove-destination "$(readlink -f "$REPO/thor/thor.db.tar.zst.sig")" "$REPO/thor/thor.db.sig"
    cp --remove-destination "$(readlink -f "$REPO/thor/thor.files.tar.zst")" "$REPO/thor/thor.files"
    cp --remove-destination "$(readlink -f "$REPO/thor/thor.files.tar.zst.sig")" "$REPO/thor/thor.files.sig"
    echo "indexed: ${new[*]##*/}"
}

publish() {
    [ -z "${THOR_REPO_DIR:-}" ] || { echo "only the Thor's repository is published (THOR_REPO_DIR is set)"; exit 1; }
    command -v gh > /dev/null || { echo "needs GitHub's command line: sudo pacman -S github-cli; gh auth login"; exit 1; }
    cd "$REPO/thor"
    [ -f thor.db ] || { echo "no repository yet: repo.sh index"; exit 1; }
    gh release view "$RELEASE" -R "$GITHUB" > /dev/null 2>&1 \
        || gh release create "$RELEASE" -R "$GITHUB" --title "Packages" --latest=false \
            --notes "The Thor's pacman repository (not a release): [thor] Server = https://github.com/$GITHUB/releases/download/$RELEASE"
    local files=()
    for f in *; do [ -L "$f" ] || files+=("$f"); done
    gh release upload "$RELEASE" -R "$GITHUB" --clobber "${files[@]}"
    # Assets no longer in the repository (older versions) deleted.
    gh release view "$RELEASE" -R "$GITHUB" --json assets -q '.assets[].name' | while read -r asset; do
        [ -e "$asset" ] && [ ! -L "$asset" ] || gh release delete-asset "$RELEASE" "$asset" -R "$GITHUB" -y
    done
    echo "published: https://github.com/$GITHUB/releases/tag/$RELEASE"
}

case ${1:-} in
    index) index ;;
    publish) publish ;;
    *) echo "usage: repo.sh index | publish"; exit 2 ;;
esac
