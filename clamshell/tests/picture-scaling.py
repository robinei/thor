#!/usr/bin/env python3
"""Launch test games as ClamShell games in a nested session and check that their pictures fill the top screen.

The games draw a pattern with 16-pixel corner blocks (red top left, green top right, blue bottom left,
yellow bottom right), a 1-pixel white border and a 1-pixel magenta cross through the middle; only those
are checked. They are:

- pattern-game (crates/compositor/examples/pattern-game.rs): as Wine shows a game, a fixed-size
  fullscreen toplevel with the picture in a subsurface scaled by a viewport;
- sdl-pattern (tests/sdl-pattern.c) through SDL 3 on Wayland and on X11 (Xwayland), at the desktop's size
  and after switching to a lower fullscreen mode, as a game picking a resolution.

Each is a library entry that is a game, launched from shelf, with Resolution, Scaling, AA and Sharpen set
through the settings tiles. A screenshot per case is kept in the printed directory.

Build first (in clamshell/): cargo build --features compositor
    cargo build -p clamshell-compositor --example pattern-game
Needs sway, Xwayland, grim, a C compiler and SDL 3. --gpu runs the host on GLES (as the device does);
--keep leaves the session running at the end.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
CLAMSHELL = ROOT / "clamshell"
BIN = CLAMSHELL / "target/debug/clamshell"
WINDOWS = ["wine-wayland", "wine-x11", "wine-hdr"]
# The Windows game's process, as Wine names it, anchored so that no other command line mentioning it
# matches: by the Unix path it was started with, or a Windows path to it.
def windows_process(exe):
    return "^(" + re.escape(str(exe)) + r"|[A-Z]:\\.*\\wine-pattern\.exe)( |$)"
PATTERN = CLAMSHELL / "target/debug/examples/pattern-game"

RESOLUTIONS = ["native", "5/6", "2/3", "1/2", "1/3"]
FRACTIONS = {"native": None, "5/6": (5, 6), "2/3": (2, 3), "1/2": (1, 2), "1/3": (1, 3)}
SCALINGS = ["bilinear", "nearest", "fsr"]
AAS = ["off", "fxaa", "smaa"]
SHARPENS = [0, 25, 50, 75, 100]
CYCLES = {"resolution": RESOLUTIONS, "scaling": SCALINGS, "aa": AAS, "sharpen": SHARPENS}


def size_on(resolution, screen):
    """Resolution's size on a screen of `screen`, as the state computes it."""
    fraction = FRACTIONS[resolution]
    if fraction is None:
        return screen
    num, den = fraction
    return tuple((v * num // den) & ~1 for v in screen)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--gpu", action="store_true", help="the host on GLES")
    parser.add_argument("--keep", action="store_true", help="leave the session running at the end")
    parser.add_argument("--only", choices=["pattern", "pattern-pq", "pattern-linear", "sdl-wayland", "sdl-x11", *WINDOWS],
                        help="run only this game's cases")
    args = parser.parse_args()
    for path in [BIN, PATTERN]:
        if not path.exists():
            sys.exit(f"{path} isn't built (see --help)")
    directory = Path(tempfile.mkdtemp(prefix="clamshell-picture-"))
    print(f"picture scaling artifacts: {directory}", flush=True)

    sdl = directory / "sdl-pattern"
    flags = subprocess.run(["pkg-config", "--cflags", "--libs", "sdl3"], capture_output=True, text=True, check=True).stdout.split()
    subprocess.run(["cc", "-O2", "-o", str(sdl), str(CLAMSHELL / "tests/sdl-pattern.c"), *flags, "-lm"], check=True)
    # The Windows game, under Wine with DXVK, in a prefix kept between runs: made, and DXVK put in it by
    # winetricks, the first time. Without Mono and Gecko, which the game doesn't use and whose installers
    # would ask. Without zig, Wine or winetricks those cases are skipped.
    exe = directory / "wine-pattern.exe"
    prefix = Path.home() / ".cache/clamshell-tests/wine"
    wine_env = ["env", f"WINEPREFIX={prefix}", "WINEDEBUG=-all", "WINEDLLOVERRIDES=mscoree,mshtml="]
    windows = all(shutil.which(tool) for tool in ["zig", "wine", "winetricks"])
    if not windows:
        print("note: no zig, wine or winetricks: the Wine cases are skipped", flush=True)
    else:
        built = subprocess.run(["zig", "cc", "-target", "x86_64-windows-gnu", "-O2", str(CLAMSHELL / "tests/wine-pattern.c"),
                                "-o", str(exe), "-ld3d11", "-ldxgi"])
        if built.returncode != 0:
            sys.exit("the Windows game didn't build")
        # Bounded, so a prefix that asks for something (a dialog) fails instead of waiting.
        def winetricks(*arguments, limit):
            try:
                return subprocess.run([*wine_env, "winetricks", *arguments], capture_output=True, text=True, timeout=limit)
            except subprocess.TimeoutExpired:
                subprocess.run(["wineserver", "-k"], env={**os.environ, "WINEPREFIX": str(prefix)}, capture_output=True)
                sys.exit(f"winetricks {' '.join(arguments)} took over {limit} s in {prefix}")
        if "dxvk" not in winetricks("list-installed", limit=180).stdout.split():
            print("making the Wine prefix with DXVK", flush=True)
            if winetricks("-q", "dxvk", limit=600).returncode != 0:
                sys.exit(f"winetricks didn't put DXVK in {prefix}")

    runtime = directory / "runtime"
    runtime.mkdir(mode=0o700)
    # The user's systemd manager and bus, through which ClamShell starts games in scopes of their own.
    for name in ["systemd", "bus"]:
        (runtime / name).symlink_to(f"/run/user/{os.getuid()}/{name}")
    data = directory / "data/clamshell"
    data.mkdir(parents=True)
    (data / "first-start.json").write_text('{"done":true}\n')
    config = directory / "config/clamshell"
    config.mkdir(parents=True)
    games = directory / "games"
    games.mkdir()
    # Each game: a file the runner starts through the wrapper, its commands from a FIFO, its output to a file.
    # SDL logs its video subsystem to the game's .err, for what it did with a mode.
    kinds = {"pattern": [str(PATTERN)], "pattern-pq": [str(PATTERN), "--colour", "pq"],
             "pattern-linear": [str(PATTERN), "--colour", "linear"], "sdl-wayland": ["env", "SDL_VIDEO_DRIVER=wayland", "SDL_LOGGING=video=debug", str(sdl)],
             "sdl-x11": ["env", "SDL_VIDEO_DRIVER=x11", "SDL_LOGGING=video=debug", str(sdl)]}
    if windows:
        # Wine's Wayland driver is used when there is no X display. With it, about one run in three, Wine 11.20's
        # process breaks while its swapchain is made anew for a mode (libwayland's "Proxy requested for unref
        # has no references", or malloc's heap corruption, in the game's .err): its window goes, or it
        # crashes, and that mode's cases fail. The compositor sees no protocol error.
        kinds.update({
            "wine-wayland": ["env", "-u", "DISPLAY", *wine_env[1:], "wine", str(exe)],
            "wine-x11": [*wine_env, "wine", str(exe)],
            "wine-hdr": ["env", "-u", "DISPLAY", *wine_env[1:], "DXVK_HDR=1", "wine", str(exe), "--hdr"],
        })
    wrapper = directory / "run-game.sh"
    wrapper.write_text("#!/bin/sh\n"
                       "# The game named by its file, its commands from <name>.in, its output to <name>.out.\n"
                       "name=$(basename \"$1\" .pgame)\n"
                       f"dir={directory}\n"
                       "case $name in\n"
                       + "".join(f"  {name}) set -- {' '.join(argv)} ;;\n" for name, argv in kinds.items())
                       + "esac\n"
                       "exec \"$@\" < \"$dir/$name.in\" > \"$dir/$name.out\" 2> \"$dir/$name.err\"\n")
    wrapper.chmod(0o755)
    fifos = {}
    for name in kinds:
        (games / f"{name}.pgame").touch()
        fifo = directory / f"{name}.in"
        os.mkfifo(fifo)
        # Held open for reading and writing, so the game's end opens at once and sees no end of input.
        fifos[name] = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
    library = {
        "places": [{"id": "test", "path": "Test"}],
        "runners": [{"name": "Pattern game", "match": ["*.pgame"], "launch": [str(wrapper), "{path}"]}],
        "sources": [{"type": "dir", "id": "test", "dir": str(games), "place": "@test", "match": ["*.pgame"]}],
    }
    (config / "library.json").write_text(json.dumps(library))
    sway_config = directory / "sway.conf"
    sway_config.write_text('output HEADLESS-1 resolution 1580x540\ndefault_border none\n'
                           'for_window [title="ClamShell nested"] fullscreen enable\n')
    private_home = directory / "home"
    private_home.mkdir()
    env = {**os.environ, "HOME": str(private_home), "XDG_RUNTIME_DIR": str(runtime), "XDG_CONFIG_HOME": str(directory / "config"),
           "XDG_DATA_HOME": str(directory / "data"), "XDG_CACHE_HOME": str(directory / "cache"),
           "WLR_BACKENDS": "headless", "WLR_HEADLESS_OUTPUTS": "1", "WLR_RENDERER": "gles2" if args.gpu else "pixman",
           "WLR_LIBINPUT_NO_DEVICES": "1", "LIBGL_ALWAYS_SOFTWARE": "0" if args.gpu else "1", "CLAMSHELL_STEAM": "off",
           "CLAMSHELL_CONTROL": str(directory / "control"), "CLAMSHELL_BACKLIGHTS": str(directory / "backlights"),
           "CLAMSHELL_GAMEPAD": str(directory / "no-controller"), "RUST_LOG": os.environ.get("RUST_LOG", "info")}
    env["DBUS_SESSION_BUS_ADDRESS"] = os.environ.get("DBUS_SESSION_BUS_ADDRESS", f"unix:path=/run/user/{os.getuid()}/bus")
    for key in ["SWAYSOCK", "WAYLAND_DISPLAY", "DISPLAY", "WAYLAND_SOCKET", "CLAMSHELL_SESSION"]:
        env.pop(key, None)
    processes = []
    failures = []

    def spawn(argv, name, environment):
        log = (directory / f"{name}.log").open("w")
        try:
            process = subprocess.Popen(argv, env=environment, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        finally:
            log.close()
        processes.append(process)
        return process

    def wait(predicate, description, seconds=10):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            failed = [process.args[0] for process in processes if process.poll() is not None]
            if failed:
                raise RuntimeError(f"{description}: exited: {failed}; logs in {directory}")
            value = predicate()
            if value:
                return value
            time.sleep(0.1)
        raise RuntimeError(f"timed out: {description}; logs in {directory}")

    try:
        spawn(["sway", "-c", str(sway_config)], "sway", env)
        parent_socket = wait(lambda: next((p.name for p in runtime.glob("wayland-*") if p.is_socket()), None), "parent Wayland socket")
        host = {**env, "WAYLAND_DISPLAY": parent_socket}
        spawn([str(BIN), "--nested"], "compositor", host)
        wait(lambda: (directory / "control").exists(), "control pipe")

        def control(*words):
            subprocess.run([str(BIN), "ctl", *words], env=host, check=True)
            time.sleep(0.15)

        def image():
            result = subprocess.run(["grim", "-o", "HEADLESS-1", "-t", "ppm", "-"], env=host, capture_output=True, check=True)
            magic, dimensions, maximum, pixels = result.stdout.split(b"\n", 3)
            assert magic == b"P6" and maximum == b"255"
            width, height = map(int, dimensions.split())
            return width, height, pixels

        def resume():
            try:
                return json.loads((data / "resume.json").read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                return {}

        def selection():
            r = resume()
            place = dict(map(tuple, r.get("places", []))).get(r.get("collection"))
            return place[-1] if place else None

        def output(name):
            try:
                return (directory / f"{name}.out").read_text().splitlines()
            except FileNotFoundError:
                return []

        def send(name, line):
            os.write(fifos[name], (line + "\n").encode())

        def running(name):
            # Wine's games are known by the program they run; the others by their own.
            program = windows_process(exe) if name in WINDOWS else "^" + next(a for a in kinds[name] if a.startswith("/")) + "( |$)"
            return subprocess.run(["pgrep", "-f", program], capture_output=True).returncode == 0

        settings = {"resolution": "native", "scaling": "bilinear", "aa": "off", "sharpen": 0}

        def set_setting(key, value):
            """The game's own setting `key` tapped round to `value` (it must be in front, Settings open)."""
            cycle = CYCLES[key]
            taps = (cycle.index(value) - cycle.index(settings[key])) % len(cycle)
            for _ in range(taps):
                control("tap", "setting", key)
            settings[key] = value

        def open_settings():
            control("tab", "settings")
            control("tap", "scope", "game")

        def until(predicate, seconds):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if predicate():
                    return True
                time.sleep(0.2)
            return False

        def launch(name):
            """`name` selected on shelf and launched. The selection is read where ClamShell saves it, about
            two seconds after it stops changing."""
            (directory / f"{name}.out").write_text("")
            control("home")
            wanted = lambda: (selection() or "").endswith(f"{name}.pgame")
            found = False
            for collection in range(2):
                control("tap", "collection", str(collection))
                control("list", "-1000")
                for _ in range(len(kinds)):
                    if until(wanted, 4):
                        found = True
                        break
                    control("list", "1")
                if found:
                    break
            if not found:
                raise RuntimeError(f"{name}: not found on shelf (selection {selection()!r})")
            control("tap", "launch")
            wait(lambda: running(name), f"{name} started", 300 if name in WINDOWS else 10)

        def quit(name):
            send(name, "quit")
            wait(lambda: not running(name), f"{name} ended")
            time.sleep(0.5)

        # The top screen's size: the screen as a game at Native is told it.
        screen = None

        def check(case, expected_mode=None, integer=False):
            """The picture fills the top screen, of `screen`, or for a mode of another shape (more than 2 %
            off) is fitted in it and centred between black bars: each corner block touches the picture's
            corner, the border shows at Native, and the magenta cross is there once each way, near the
            middle. With `integer` (Nearest+Integer) it is the largest whole multiple of the mode that fits,
            centred."""
            time.sleep(0.8)
            subprocess.run(["grim", "-o", "HEADLESS-1", str(directory / f"{case}.png")], env=host, check=True)
            width, height, pixels = image()
            sw, sh = screen
            x0, y0, w, h = 0, 0, sw, sh
            times = min(sw // expected_mode[0], sh // expected_mode[1]) if expected_mode and integer else 0
            if times >= 1:
                w, h = expected_mode[0] * times, expected_mode[1] * times
                x0, y0 = (sw - w) // 2, (sh - h) // 2
            elif expected_mode and abs((expected_mode[0] / expected_mode[1]) / (sw / sh) - 1) > 0.02:
                fit = min(sw / expected_mode[0], sh / expected_mode[1])
                w, h = round(expected_mode[0] * fit), round(expected_mode[1] * fit)
                x0, y0 = (sw - w) // 2, (sh - h) // 2

            def at(x, y):
                x, y = x + x0, y + y0
                o = (y * width + x) * 3
                return pixels[o], pixels[o + 1], pixels[o + 2]

            colours = {
                "red": lambda r, g, b: r > 170 and g < 90 and b < 90,
                "green": lambda r, g, b: g > 170 and r < 90 and b < 90,
                "blue": lambda r, g, b: b > 170 and r < 90 and g < 90,
                "yellow": lambda r, g, b: r > 170 and g > 170 and b < 90,
            }
            corners = {"red": (0, 0), "green": (w - 1, 0), "blue": (0, h - 1), "yellow": (w - 1, h - 1)}
            problems = []
            measured = []
            for colour, (cx, cy) in corners.items():
                # The block's extent within 64 pixels of its corner, inside the top screen.
                xs = range(0, min(64, w)) if cx == 0 else range(max(0, w - 64), w)
                ys = range(0, min(64, h)) if cy == 0 else range(max(0, h - 64), h)
                hits = [(x, y) for y in ys for x in xs if colours[colour](*at(x, y))]
                if not hits:
                    problems.append(f"no {colour} block")
                    measured.append(f"{colour} -")
                    continue
                near_x = min(abs(x - cx) for x, _ in hits)
                near_y = min(abs(y - cy) for _, y in hits)
                measured.append(f"{colour} {near_x},{near_y}")
                if near_x > 2 or near_y > 2:
                    problems.append(f"{colour} block {near_x},{near_y} px from its corner")
            # Past the right edge (the second screen) no green or yellow block continues.
            if sw + 2 < width and any(colours["green"](*at(sw - x0 + 1, y)) for y in range(0, 16)):
                problems.append("green past the top screen's right edge")
            # The bars around a fitted picture are black.
            if x0 > 2 and any(max(at(-x0 + 1, y)) > 16 or max(at(w + 1, y)) > 16 for y in range(16, h - 16, 37)):
                problems.append("the bars beside the picture aren't black")
            if x0 or y0:
                measured.append(f"fitted {w}x{h} at {x0},{y0}")
            # The cross: magenta runs across rows around a quarter down and columns around a quarter across,
            # where most of five of them see it. Anti-aliasing may blend the 1-pixel line where it crosses an
            # edge of the pattern, which at a small mode comes close.
            # Strongly: anti-aliasing tints a dark edge next to the line a little.
            magenta = lambda r, g, b: r > 150 and b > 150 and g < 100
            near = lambda quarter: [quarter * k // 4 for k in (2, 3, 4, 5, 6)]

            def runs(values):
                found, inside = [], False
                for i, hit in enumerate(values):
                    if hit and not inside:
                        found.append(i)
                    inside = hit
                return found

            columns = runs([sum(magenta(*at(x, y)) for y in near(h // 4)) >= 3 for x in range(w)])
            rows = runs([sum(magenta(*at(x, y)) for x in near(w // 4)) >= 3 for y in range(h)])
            measured.append(f"cross x{columns} y{rows}")
            if len(columns) != 1 or abs(columns[0] - w // 2) > w // 20:
                problems.append(f"vertical line of the cross at {columns}, expected one near {w // 2}")
            if len(rows) != 1 or abs(rows[0] - h // 2) > h // 20:
                problems.append(f"horizontal line of the cross at {rows}, expected one near {h // 2}")
            if settings["resolution"] == "native" and expected_mode == screen:
                white = lambda r, g, b: min(r, g, b) > 150
                edges = [at(w // 3, 0), at(w // 3, h - 1), at(0, h // 3), at(w - 1, h // 3)]
                if not all(white(*p) for p in edges):
                    problems.append(f"border not white at the edges: {edges}")
            line = f"{'ok' if not problems else 'FAIL'}: {case}: {'; '.join(measured)}"
            if problems:
                line += f" -- {'; '.join(problems)}"
                failures.append(case)
            print(line, flush=True)

        def combos(full):
            if full:
                return [(s, a, p) for s in SCALINGS for a in AAS for p in (0, 50)]
            return [("bilinear", "off", 0), ("bilinear", "smaa", 0), ("nearest", "fxaa", 0), ("fsr", "off", 50),
                    ("fsr", "smaa", 50)]

        def go_through(name, prefix, full, mode):
            open_settings()
            for scaling, aa, sharpen in combos(full):
                set_setting("scaling", scaling)
                set_setting("aa", aa)
                set_setting("sharpen", sharpen)
                check(f"{prefix}-{scaling}-{aa}-sharpen{sharpen}", mode, integer=scaling == "nearest")
            for key, value in [("scaling", "bilinear"), ("aa", "off"), ("sharpen", 0)]:
                set_setting(key, value)

        def check_colour(name, expected):
            """The game's window has the colour it declares, as the compositor last logged a window's: its
            primaries and transfer function, such as "Bt2020 Pq"."""
            lines = [l.replace(", colour ", "'s colour is ") for l in (directory / "compositor.log").read_text().splitlines()
                     if "compositor: window" in l and "colour" in l]
            last = lines[-1] if lines else "none logged"
            ok = last.endswith(expected)
            print(f"{'ok' if ok else 'FAIL'}: {name}-colour: {last.split('compositor: ')[-1]}", flush=True)
            if not ok:
                failures.append(f"{name}-colour")

        def check_histogram(name):
            """The tone curve's edit screen plots the picture's histogram: its bars, pure greys, are drawn
            there (everything else on the screen is tinted or black)."""
            open_settings()
            control("long", "setting", "curve")
            control("tap", "curve-edit", "2")
            time.sleep(1.5)
            subprocess.run(["grim", "-o", "HEADLESS-1", str(directory / f"{name}-curve-histogram.png")], env=host, check=True)
            width, height, pixels = image()
            plot = [(x, y) for y in range(0, height // 2) for x in range(screen[0], width)]

            def neutral(x, y):
                r, g, b = pixels[(y * width + x) * 3:(y * width + x) * 3 + 3]
                return r == g == b and r > 8
            drawn = sum(1 for x, y in plot if neutral(x, y))
            print(f"{'ok' if drawn > 200 else 'FAIL'}: {name}-curve-histogram: {drawn} pixels of the histogram drawn", flush=True)
            if drawn <= 200:
                failures.append(f"{name}-curve-histogram")
            control("tap", "curve-back")
            control("tap", "curve-back")

        # pattern-game: at Native, then relaunched at 1/2.
        for resolution in ["native", "1/2"] if args.only in (None, "pattern") else []:
            launch("pattern")
            wait(lambda: any(l.startswith("mode") for l in output("pattern")), "pattern-game's modes")
            lines = output("pattern")
            outputs = [l.split()[1] for l in lines if l.startswith("output ")]
            modes = [tuple(map(int, l.split()[1:])) for l in lines if l.startswith("mode ")]
            if screen is None:
                screen = modes[0]
            expected = size_on(resolution, screen)
            label = resolution.replace("/", "of")
            problems = []
            if outputs != ["ClamShell-Top"]:
                problems.append(f"sees outputs {outputs}, expected only ClamShell-Top")
            if modes[-1] != expected:
                problems.append(f"told mode {modes[-1]}, expected {expected}")
            print(f"{'ok' if not problems else 'FAIL'}: pattern-{label}-launch: outputs {outputs}, modes {modes}"
                  + (f" -- {'; '.join(problems)}" if problems else ""), flush=True)
            if problems:
                failures.append(f"pattern-{label}-launch")
            go_through("pattern", f"pattern-{label}", True, modes[-1])
            if resolution == "native":
                check_histogram("pattern")
            if resolution == "native":
                open_settings()
                set_setting("resolution", "1/2")
            else:
                open_settings()
                set_setting("resolution", "native")
            quit("pattern")

        # HDR content: the pattern as PQ, and as linear light (scRGB's), at Native.
        for name in [n for n in ["pattern-pq", "pattern-linear"] if args.only in (None, n)]:
            launch(name)
            wait(lambda: any(l.startswith("mode") for l in output(name)), f"{name}'s modes")
            modes = [tuple(map(int, l.split()[1:])) for l in output(name) if l.startswith("mode ")]
            screen = screen or modes[0]
            go_through(name, name, False, modes[-1])
            check_colour(name, {"pattern-pq": "Bt2020 Pq", "pattern-linear": "Srgb Linear"}[name])
            check_histogram(name)
            quit(name)

        def mode_game(name, hdr=False):
            """A game choosing its own mode: at the desktop's size, then a lower mode it lists, each through
            the sampled settings; for HDR also the curve's histogram."""
            nonlocal screen
            launch(name)
            wait(lambda: any(l.startswith("size") for l in output(name)), f"{name} drawing", 300 if name in WINDOWS else 10)
            lines = output(name)
            desktop = next(tuple(map(int, l.split()[1:])) for l in lines if l.startswith("desktop "))
            listed = [tuple(map(int, l.split()[1:])) for l in lines if l.startswith("mode ")]
            print(f"{name}: desktop {desktop}, modes {listed}", flush=True)
            # Without pattern-game's run (--only), the screen is taken as the game's desktop.
            screen = screen or desktop
            if desktop != screen:
                print(f"FAIL: {name}-desktop-size: the game's desktop {desktop}, the screen {screen}", flush=True)
                failures.append(f"{name}-desktop-size")
            go_through(name, f"{name}-desktop", False, desktop)
            if hdr:
                check_colour(name, "Bt2020 Pq")
                check_histogram(name)
            lower = [m for m in listed if m[0] < desktop[0]]
            if not lower:
                print(f"FAIL: {name}-lower: no mode lower than the desktop's listed", flush=True)
                failures.append(f"{name}-lower")
            else:
                target = min(lower, key=lambda m: abs(m[0] - desktop[0] // 2))
                sizes_before = len([l for l in output(name) if l.startswith("size")])
                send(name, f"mode {target[0]} {target[1]}")
                try:
                    wait(lambda: len([l for l in output(name) if l.startswith("size")]) > sizes_before, f"{name} at {target}")
                except RuntimeError as error:
                    print(f"note: {error}", flush=True)
                drawn = tuple(map(int, [l for l in output(name) if l.startswith("size")][-1].split()[1:]))
                print(f"{'ok' if drawn == target else 'FAIL'}: {name}-mode{target[0]}x{target[1]}-drawn: drawing at {drawn}", flush=True)
                if drawn != target:
                    failures.append(f"{name}-mode{target[0]}x{target[1]}-drawn")
                go_through(name, f"{name}-mode{target[0]}x{target[1]}", False, target)
            quit(name)

        # SDL, on Wayland and on X11; Windows' game under Wine.
        for name in [n for n in ["sdl-wayland", "sdl-x11", *WINDOWS] if args.only in (None, n) and n in kinds]:
            mode_game(name, hdr=name == "wine-hdr")

        print(f"{'FAIL' if failures else 'ok'}: {len(failures)} failed{': ' + ', '.join(failures) if failures else ''}", flush=True)
        if args.keep:
            print(f"kept running; WAYLAND_DISPLAY={parent_socket} XDG_RUNTIME_DIR={runtime}; Ctrl+C ends it", flush=True)
            signal.pause()
    finally:
        for fd in fifos.values():
            os.close(fd)
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
        for process in processes:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        for name in kinds:
            program = windows_process(exe) if name in WINDOWS else "^" + next(a for a in kinds[name] if a.startswith("/")) + "( |$)"
            subprocess.run(["pkill", "-f", program], capture_output=True)
        # Wine's server.
        if windows:
            subprocess.run(["wineserver", "-k"], env={**os.environ, "WINEPREFIX": str(prefix)}, capture_output=True)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
