#!/usr/bin/env python3
"""The licenses of the Rust crates built into ClamShell's programs (clamshell, clamshell-fan), for the
clamshell package to carry (/usr/share/licenses/clamshell/THIRD-PARTY): each crate the programs link for
the target (normal dependencies, not build or dev ones), its declared license, and the license and notice
files it ships; then the font built in (Noto Sans, OFL). From cargo's metadata of the locked dependencies.

    third-party-licenses.py CLAMSHELL_DIR TARGET > THIRD-PARTY
"""

import json
import re
import subprocess
import sys
from pathlib import Path

LICENSE_FILE = re.compile(r"^(LICEN[SC]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)", re.IGNORECASE)


def main():
    root, target = Path(sys.argv[1]), sys.argv[2]
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
        cwd=root, check=True, capture_output=True, text=True).stdout)
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    workspace = set(meta["workspace_members"])
    # Everything the workspace's crates reach through normal dependencies.
    seen, todo = set(), list(workspace)
    while todo:
        node = nodes[todo.pop()]
        for dep in node["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                todo.append(dep["pkg"])
    out = sys.stdout
    out.write("Third-party software in ClamShell's programs (clamshell, clamshell-fan)\n")
    out.write("=" * 72 + "\n\n")
    missing = []
    for pid in sorted(seen - workspace, key=lambda i: (packages[i]["name"], packages[i]["version"])):
        p = packages[pid]
        folder = Path(p["manifest_path"]).parent
        files = sorted(f for f in folder.iterdir() if f.is_file() and LICENSE_FILE.match(f.name))
        out.write(f"{p['name']} {p['version']}\n")
        out.write(f"License: {p.get('license') or p.get('license_file') or 'not declared'}\n")
        if p.get("repository"):
            out.write(f"Source: {p['repository']}\n")
        if p.get("authors"):
            out.write(f"Authors: {', '.join(p['authors'])}\n")
        if not files:
            missing.append(p["name"])
            out.write("(The crate ships no license file: its declared license's standard text applies, as in\n"
                      "/usr/share/licenses/spdx or https://spdx.org/licenses/.)\n")
        for f in files:
            out.write(f"\n--- {f.name}\n\n{f.read_text(errors='replace').rstrip()}\n")
        out.write("\n" + "=" * 72 + "\n\n")
    font = root / "crates/ui/fonts/OFL.txt"
    out.write("Noto Sans (crates/ui/fonts/NotoSans-Regular.ttf, built into clamshell)\nLicense: OFL-1.1\n\n")
    out.write(font.read_text().rstrip() + "\n")
    if missing:
        print(f"third-party-licenses: no license file in: {', '.join(missing)}", file=sys.stderr)


if __name__ == "__main__":
    main()
