#!/usr/bin/env python3
"""Real apps in nested ClamShell (see nested_env.py): each one installed here is started as a program
outside ClamShell is, and must open its window in front on the top screen, draw there, cause no error
in the compositor's log, and end when ClamShell closes its window. Apps not installed are skipped; each
app bug fixed adds a case here or in nested-compositor.py.

    tests/real-apps.py [APP...]     (all by default; --list names them)

Build first (in clamshell/): cargo build. Steam isn't among them: it allows one instance per user, so it
would hand over to the desktop's.
"""
import argparse
import shutil
import subprocess
import sys
import time

from nested_env import Nested

# Each app: the program it needs, its command (DIR: a folder of its own for its profile), whether it is an
# X11 client, what is run to the end before it (if anything), and variables of its own.
APPS = {
    "retroarch": (["retroarch"], False, None, {}),
    "firefox": (["firefox", "--new-instance", "--no-remote", "--profile", "DIR", "about:blank"], False, None, {}),
    # Wine's prefix made first, without asking to install Mono and Gecko: in the empty home, wine would
    # show its dialogs before Notepad's window.
    "wine-notepad": (["wine", "notepad"], True, ["wineboot", "--init"], {"WINEDLLOVERRIDES": "mscoree,mshtml="}),
    "lutris": (["lutris"], False, None, {}),
    "heroic": (["heroic"], False, None, {}),
}


def top_screen(n):
    """The top screen's pixels (the left 960 of the scene)."""
    width, height, data = n.image()
    return b"".join(data[(row * width) * 3:(row * width + 960) * 3] for row in range(height))


def run(n, name, argv, x11, before, variables):
    """One app's case: whether it passed, and what was seen."""
    folder = n.directory / name
    folder.mkdir()
    argv = [str(folder) if word == "DIR" else word for word in argv]
    env = {**(n.x11 if x11 else n.nested), **variables}
    if before:
        subprocess.run(before, env=env, capture_output=True, timeout=180)
    before_ids = {w["id"] for w in n.windows()}
    before_pixels = top_screen(n)
    log_before = len(n.log())
    process = n.spawn(argv, name, env)
    n.expected_exits.add(process)
    seen = []
    def window():
        return next((w for w in n.windows() if w["id"] not in before_ids and w.get("parent") is None), None)
    try:
        opened = n.wait(window, f"{name}: its window", 60)
    except RuntimeError as e:
        return False, [str(e)]
    seen.append(f"window {opened.get('app_id')!r} {opened.get('title')!r}")
    in_front = n.wait(lambda: any(w["id"] == opened["id"] and w.get("focused") for w in n.windows()) or None, f"{name}: in front", 15)
    drew = n.wait(lambda: top_screen(n) != before_pixels or None, f"{name}: drawn on the top screen", 15)
    n.shot(name)
    errors = [line for line in n.log()[log_before:].splitlines() if "[ERROR" in line or "protocol error" in line]
    n.control("window", "close")
    try:
        n.wait(lambda: process.poll() is not None, f"{name}: ended when its window closed", 20)
        ended = True
    except RuntimeError:
        ended = False
    ok = bool(in_front and drew) and not errors and ended
    seen += [f"in front {bool(in_front)}, drew {bool(drew)}, ended when closed {ended}"] + errors[:5]
    return ok, seen


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("apps", nargs="*", metavar="APP")
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--gpu", action="store_true", help="hardware rendering in sway (as nested-compositor.py --gpu)")
    args = parser.parse_args()
    if args.list:
        print("\n".join(APPS))
        return 0
    chosen = args.apps or list(APPS)
    failed = 0
    with Nested(gpu=args.gpu, prefix="clamshell-apps-") as n:
        for name in chosen:
            argv, x11, before, variables = APPS[name]
            if not shutil.which(argv[0]):
                print(f"skip  {name}: {argv[0]} isn't installed", flush=True)
                continue
            try:
                ok, seen = run(n, name, argv, x11, before, variables)
            except RuntimeError as e:
                ok, seen = False, [str(e)]
            failed += not ok
            print(f"{'ok   ' if ok else 'FAIL '} {name}", flush=True)
            for line in seen:
                print(f"        {line}", flush=True)
            # Whatever it left, gone before the next.
            time.sleep(1)
    print(f"{len(chosen) - failed} of {len(chosen)} not failed; logs in {n.directory}")
    return failed


if __name__ == "__main__":
    sys.exit(main())
