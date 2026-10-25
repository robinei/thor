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
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
CLAMSHELL = ROOT / "clamshell"
BIN = CLAMSHELL / "target/debug/clamshell"
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
    args = parser.parse_args()
    for path in [BIN, PATTERN]:
        if not path.exists():
            sys.exit(f"{path} isn't built (see --help)")
    directory = Path(tempfile.mkdtemp(prefix="clamshell-picture-"))
    print(f"picture scaling artifacts: {directory}", flush=True)

    sdl = directory / "sdl-pattern"
    flags = subprocess.run(["pkg-config", "--cflags", "--libs", "sdl3"], capture_output=True, text=True, check=True).stdout.split()
    subprocess.run(["cc", "-O2", "-o", str(sdl), str(CLAMSHELL / "tests/sdl-pattern.c"), *flags, "-lm"], check=True)

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
    kinds = {"pattern": [str(PATTERN)], "sdl-wayland": ["env", "SDL_VIDEO_DRIVER=wayland", str(sdl)],
             "sdl-x11": ["env", "SDL_VIDEO_DRIVER=x11", str(sdl)]}
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
           "CLAMSHELL_GAMEPAD": str(directory / "no-controller"), "RUST_LOG": "info"}
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
            return subprocess.run(["pgrep", "-f", f"^{kinds[name][-1]}( |$)"], capture_output=True).returncode == 0

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
            wait(lambda: running(name), f"{name} started")

        def quit(name):
            send(name, "quit")
            wait(lambda: not running(name), f"{name} ended")
            time.sleep(0.5)

        # The top screen's size: the screen as a game at Native is told it.
        screen = None

        def check(case, expected_mode=None):
            """The picture fills the top screen, of `screen`: each corner block touches its corner, the
            border shows at Native, and the magenta cross is there once each way, near the middle."""
            time.sleep(0.8)
            subprocess.run(["grim", "-o", "HEADLESS-1", str(directory / f"{case}.png")], env=host, check=True)
            width, height, pixels = image()
            w, h = screen

            def at(x, y):
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
            if w + 2 < width and any(colours["green"](*at(w + 1, y)) for y in range(0, 16)):
                problems.append("green past the top screen's right edge")
            # The cross: magenta runs across a row a quarter down and a column a quarter across.
            magenta = lambda r, g, b: r - g > 60 and b - g > 60

            def runs(values):
                found, inside = [], False
                for i, hit in enumerate(values):
                    if hit and not inside:
                        found.append(i)
                    inside = hit
                return found

            columns = runs([magenta(*at(x, h // 4)) for x in range(w)])
            rows = runs([magenta(*at(w // 4, y)) for y in range(h)])
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
                check(f"{prefix}-{scaling}-{aa}-sharpen{sharpen}", mode)
            for key, value in [("scaling", "bilinear"), ("aa", "off"), ("sharpen", 0)]:
                set_setting(key, value)

        # pattern-game: at Native, then relaunched at 1/2.
        for resolution in ["native", "1/2"]:
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
                open_settings()
                set_setting("resolution", "1/2")
            else:
                open_settings()
                set_setting("resolution", "native")
            quit("pattern")

        # SDL, on Wayland and on X11: at the desktop's size, then a lower mode it lists.
        for name in ["sdl-wayland", "sdl-x11"]:
            launch(name)
            wait(lambda: any(l.startswith("size") for l in output(name)), f"{name} drawing")
            lines = output(name)
            desktop = next(tuple(map(int, l.split()[1:])) for l in lines if l.startswith("desktop "))
            listed = [tuple(map(int, l.split()[1:])) for l in lines if l.startswith("mode ")]
            print(f"{name}: desktop {desktop}, modes {listed}", flush=True)
            if desktop != screen:
                print(f"FAIL: {name}-desktop-size: SDL's desktop {desktop}, the screen {screen}", flush=True)
                failures.append(f"{name}-desktop-size")
            go_through(name, f"{name}-desktop", False, desktop)
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
                drawn = [l for l in output(name) if l.startswith("size")][-1]
                print(f"{name}: mode {target}, drawing at {drawn}", flush=True)
                go_through(name, f"{name}-mode{target[0]}x{target[1]}", False, target)
            quit(name)

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
            subprocess.run(["pkill", "-f", f"^{kinds[name][-1]}( |$)"], capture_output=True)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
