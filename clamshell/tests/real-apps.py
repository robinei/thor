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
import os
import re
import shutil
import signal
import subprocess
import sys

from nested_env import Nested

# A line the compositor logged at error level: `[time ERROR target] message`.
ERROR = re.compile(r"^\[\S+ ERROR ")

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
    """One app's case: whether it passed, and what was seen. The app's processes are gone after it, passed
    or not, so that a slow one doesn't open its window in the next case."""
    folder = n.directory / name
    folder.mkdir()
    argv = [str(folder) if word == "DIR" else word for word in argv]
    env = {**(n.x11 if x11 else n.nested), **variables}
    if before:
        try:
            done = subprocess.run(before, env=env, capture_output=True, text=True, timeout=180)
        except subprocess.TimeoutExpired:
            return False, [f"{' '.join(before)} didn't end in 180 s"]
        if done.returncode != 0:
            return False, [f"{' '.join(before)} failed ({done.returncode}): {done.stderr.strip()[-300:]}"]
    before_ids = {w["id"] for w in n.windows()}
    before_pixels = top_screen(n)
    log_before = len(n.log())
    process = n.spawn(argv, name, env)
    n.expected_exits.add(process)
    try:
        return case(n, name, process, before_ids, before_pixels, log_before)
    finally:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def case(n, name, process, before_ids, before_pixels, log_before):
    def window():
        return next((w for w in n.windows() if w["id"] not in before_ids and w.get("parent") is None), None)
    try:
        opened = n.wait(window, f"{name}: its window", 60)
        seen = [f"window {opened.get('app_id')!r} {opened.get('title')!r}"]
        n.wait(lambda: any(w["id"] == opened["id"] and w.get("focused") for w in n.windows()) or None, f"{name}: in front", 15)
        # Something drawn: the top screen changed from the Library, and not to one colour (a window that
        # draws nothing is black).
        def drew():
            pixels = top_screen(n)
            return pixels != before_pixels and len(set(zip(*[iter(pixels)] * 3))) > 1
        n.wait(lambda: drew() or None, f"{name}: drawn on the top screen", 15)
    except RuntimeError as e:
        return False, [str(e)]
    n.shot(name)
    errors = [line for line in n.log()[log_before:].splitlines() if ERROR.search(line) or "protocol error" in line]
    n.control("window", "close")
    def gone():
        process.poll()  # (Its leader reaped: a zombie still counts in its group.)
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return all(w["id"] != opened["id"] for w in n.windows())
        return False
    try:
        n.wait(gone, f"{name}: ended when its window closed", 20)
    except RuntimeError as e:
        return False, seen + [str(e)]
    return not errors, seen + errors[:5]


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
    print(f"{len(chosen) - failed} of {len(chosen)} not failed; logs in {n.directory}")
    return failed


if __name__ == "__main__":
    sys.exit(main())
