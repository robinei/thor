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
#   repo.sh publish   thor/ uploaded to the release (made if missing): the packages, then the database;
#                     the files of the version before them deleted (one publish late)
#
# Signing: the key in ~/.config/thor-repo/gnupg (made for this, without a passphrase; its public half is
# thor-keyring's), when it is there; without it (a checkout elsewhere) the repository is indexed unsigned,
# which builds images (they take their own [thor] as it is) but can't be published. Publishing: GitHub's
# command line (gh), logged in.
set -euo pipefail
# (THOR_REPO_DIR: another, as the test machine's, ~/.cache/thor-vm/repo-x86_64, never published.)
REPO=${THOR_REPO_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/thor-vm/repo}
export GNUPGHOME=${THOR_GNUPGHOME:-$HOME/.config/thor-repo/gnupg}
KEY=6C4C5B7FB11D609B739FDAC30BB252FF8CA9B43F
GITHUB=robinei/thor
RELEASE=packages

# The signing key is here.
signing() { gpg --list-secret-keys "$KEY" > /dev/null 2>&1; }

index() {
    mkdir -p "$REPO/built" "$REPO/thor" "$REPO/local"
    shopt -s nullglob
    local new=()
    # The kernel's corresponding source (linux-thor-VERSION-REL.src.tar.gz: its source package), beside
    # its package: the older ones go with the packages they were the source of.
    for src in "$REPO"/built/*.src.tar.gz; do
        rm -f "$REPO"/thor/linux-thor-*.src.tar.gz "$REPO"/thor/linux-thor-*.source.tar
        mv "$src" "$REPO/thor/"
        echo "source: $(basename "$src")"
    done
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
        rm -f "$REPO/thor/$name.sig"
        ! signing || gpg --batch --yes --quiet --detach-sign --no-armor -u "$KEY" "$REPO/thor/$name"
        new+=("$REPO/thor/$name")
    done
    [ ${#new[@]} -gt 0 ] || { echo "nothing new in $REPO/built"; return 0; }
    # (-R: the versions they replace removed; -s/-k: the database signed too.)
    local sign=()
    ! signing || sign=(-s -k "$KEY")
    repo-add -q -R "${sign[@]}" "$REPO/thor/thor.db.tar.zst" "${new[@]}"
    rm -f "$REPO"/thor/*.old "$REPO"/thor/*.old.sig
    # (GitHub serves no links: the names pacman asks for, as files.)
    local f
    for f in thor.db thor.files; do
        cp --remove-destination "$(readlink -f "$REPO/thor/$f.tar.zst")" "$REPO/thor/$f"
        rm -f "$REPO/thor/$f.sig"
        ! signing || cp --remove-destination "$(readlink -f "$REPO/thor/$f.tar.zst.sig")" "$REPO/thor/$f.sig"
    done
    echo "indexed: ${new[*]##*/}"
}

publish() {
    [ -z "${THOR_REPO_DIR:-}" ] || { echo "only the Thor's repository is published (THOR_REPO_DIR is set)"; exit 1; }
    command -v gh > /dev/null || { echo "needs GitHub's command line: sudo pacman -S github-cli; gh auth login"; exit 1; }
    cd "$REPO/thor"
    [ -f thor.db ] || { echo "no repository yet: repo.sh index"; exit 1; }
    signing && [ -f thor.db.sig ] || { echo "the repository isn't signed (no signing key here): not published"; exit 1; }
    gh release view "$RELEASE" -R "$GITHUB" > /dev/null 2>&1 \
        || gh release create "$RELEASE" -R "$GITHUB" --title "Packages" --latest=false \
            --notes "The Thor's pacman repository (not a release): [thor] Server = https://github.com/$GITHUB/releases/download/$RELEASE"
    # As Arch's repositories: the packages first, the database last, so that a device syncing meanwhile
    # finds every file the database it gets names (or the old database, whose files are still there).
    local files=() db=() f
    for f in *; do
        [ -L "$f" ] && continue
        case $f in thor.db* | thor.files*) db+=("$f") ;; *) files+=("$f") ;; esac
    done
    gh release upload "$RELEASE" -R "$GITHUB" --clobber "${files[@]}"
    gh release upload "$RELEASE" -R "$GITHUB" --clobber "${db[@]}"
    # Older versions deleted one publish late: what the previous database named stays until the next
    # publish, for a device that synced just before this one (recorded in ../published).
    local kept=../published
    gh release view "$RELEASE" -R "$GITHUB" --json assets -q '.assets[].name' | while read -r asset; do
        [ -e "$asset" ] && [ ! -L "$asset" ] && continue
        grep -qxF "$asset" "$kept" 2> /dev/null && continue
        gh release delete-asset "$RELEASE" "$asset" -R "$GITHUB" -y
    done
    printf '%s\n' "${files[@]}" "${db[@]}" > "$kept"
    echo "published: https://github.com/$GITHUB/releases/tag/$RELEASE"
}

case ${1:-} in
    index) index ;;
    publish) publish ;;
    *) echo "usage: repo.sh index | publish"; exit 2 ;;
esac
