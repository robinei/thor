#!/bin/bash
# Points linux-thor at another Armada commit (default: their main's latest): packages/linux-thor/PKGBUILD's
# _armada, _armada_time, _kver (their packages/kernel/BASE.env's Linux) and checksums, pkgrel back to 1.
# Prints where to read what changed in their packages/kernel. Then: commit, tools/repo/build.sh linux-thor,
# try it on the device, publish.
#
#   follow-armada.sh [COMMIT]
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
PKGBUILD=$HERE/../../packages/linux-thor/PKGBUILD
REF=${1:-main}
old=$(sed -n 's/^_armada=//p' "$PKGBUILD")
commit=$(curl -fsSL "https://api.github.com/repos/armada-os/armada/commits/$REF" | python3 -c 'import json, sys; print(json.load(sys.stdin)["sha"])')
[ "$commit" != "$old" ] || { echo "already at $commit"; exit 0; }
time=$(curl -fsSL "https://api.github.com/repos/armada-os/armada/commits/$commit" \
    | python3 -c 'import json, sys, datetime; d = json.load(sys.stdin)["commit"]["committer"]["date"]; print(datetime.datetime.fromisoformat(d.replace("Z", "+00:00")).strftime("%Y%m%d%H%M"))')
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL -o "$work/armada.tar.gz" "https://github.com/armada-os/armada/archive/$commit.tar.gz"
armada_sum=$(sha256sum "$work/armada.tar.gz" | cut -d' ' -f1)
kver=$(tar -xOzf "$work/armada.tar.gz" "armada-$commit/packages/kernel/BASE.env" | sed -n 's/^VERSION=//p')
kernel_sum=$(curl -fsSL "https://cdn.kernel.org/pub/linux/kernel/v${kver%%.*}.x/sha256sums.asc" \
    | awk -v f="linux-$kver.tar.xz" '$2 == f { print $1 }')
[ -n "$kernel_sum" ] || { echo "no checksum for linux-$kver.tar.xz on kernel.org"; exit 1; }
python3 - "$PKGBUILD" "$commit" "$time" "$kver" "$armada_sum" "$kernel_sum" <<'EOF'
import re, sys
path, commit, time, kver, armada_sum, kernel_sum = sys.argv[1:]
s = open(path).read()
s = re.sub(r"^_armada=.*$", f"_armada={commit}", s, flags=re.M)
s = re.sub(r"^_armada_time=.*$", f"_armada_time={time}", s, flags=re.M)
s = re.sub(r"^_kver=.*$", f"_kver={kver}", s, flags=re.M)
s = re.sub(r"^pkgrel=.*$", "pkgrel=1", s, flags=re.M)
# The checksums: kernel.org's tarball's first, Armada's archive's third (source= order).
m = re.search(r"sha256sums=\(([^)]*)\)", s)
sums = m.group(1).split()
sums[0], sums[2] = kernel_sum, armada_sum
s = s[:m.start(1)] + ("\n" + " " * 12).join(sums) + s[m.end(1):]
open(path, "w").write(s)
EOF
echo "linux-thor: Armada $commit ($time), Linux $kver"
echo "What changed in their kernel: https://github.com/armada-os/armada/compare/$old...$commit (packages/kernel)"
