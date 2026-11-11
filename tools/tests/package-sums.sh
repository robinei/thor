#!/bin/bash
# Each package's own files (the sources that aren't downloaded) against its PKGBUILD's sha256sums, so
# that a changed file without its new checksum fails here rather than in makepkg on the next build.
#
#   tools/tests/package-sums.sh
set -euo pipefail
cd "$(dirname "$0")/../../packages"
failed=0
for pkgbuild in */PKGBUILD; do
    dir=${pkgbuild%/PKGBUILD}
    # (In a subshell: each PKGBUILD's variables stay its own.)
    (
        cd "$dir"
        source ./PKGBUILD
        bad=0
        for i in "${!source[@]}"; do
            entry=${source[$i]}
            case $entry in *://*) continue ;; esac
            file=${entry%%::*}
            want=${sha256sums[$i]:-}
            [ "$want" = SKIP ] && continue
            have=$(sha256sum < "$file" | cut -d' ' -f1)
            if [ "$have" != "$want" ]; then
                echo "packages/$dir/$file: sha256 $have, the PKGBUILD says $want" >&2
                bad=1
            fi
        done
        exit $bad
    ) || failed=1
done
exit $failed
