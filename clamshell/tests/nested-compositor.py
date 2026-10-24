#!/usr/bin/env python3
"""Run the in-process compositor in headless sway and check a real client's pixels on both screens.

Build first: cargo build --features compositor (in clamshell/).
Needs sway, foot and grim. Leaves screenshots and logs in the printed temporary directory.
"""
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "clamshell/target/debug/clamshell"


def main():
    directory = Path(tempfile.mkdtemp(prefix="clamshell-compositor-"))
    print(f"nested compositor artifacts: {directory}", flush=True)
    runtime = directory / "runtime"
    runtime.mkdir(mode=0o700)
    data = directory / "data/clamshell"
    data.mkdir(parents=True)
    (data / "first-start.json").write_text('{"done":true}\n')
    config = directory / "config/clamshell"
    config.mkdir(parents=True)
    (config / "library.json").write_text('{"places":[{"id":"apps","path":"Apps"}]}\n')
    sway_config = directory / "sway.conf"
    sway_config.write_text('output HEADLESS-1 resolution 1580x540\ndefault_border none\n'
                           'for_window [title="ClamShell nested"] fullscreen enable\n')
    env = {**os.environ, "XDG_RUNTIME_DIR": str(runtime), "XDG_CONFIG_HOME": str(directory / "config"),
           "XDG_DATA_HOME": str(directory / "data"), "XDG_CACHE_HOME": str(directory / "cache"),
           "WLR_BACKENDS": "headless", "WLR_HEADLESS_OUTPUTS": "1", "WLR_RENDERER": "pixman",
           "WLR_LIBINPUT_NO_DEVICES": "1", "LIBGL_ALWAYS_SOFTWARE": "1", "CLAMSHELL_STEAM": "off",
           "CLAMSHELL_CONTROL": str(directory / "control"), "CLAMSHELL_BACKLIGHTS": str(directory / "backlights"),
           "CLAMSHELL_GAMEPAD": str(directory / "no-controller")}
    env["DBUS_SESSION_BUS_ADDRESS"] = os.environ.get("DBUS_SESSION_BUS_ADDRESS", f"unix:path=/run/user/{os.getuid()}/bus")
    for key in ["SWAYSOCK", "WAYLAND_DISPLAY", "DISPLAY", "WAYLAND_SOCKET", "CLAMSHELL_SESSION"]:
        env.pop(key, None)
    processes = []

    def spawn(argv, name, environment):
        log = (directory / f"{name}.log").open("w")
        try:
            process = subprocess.Popen(argv, env=environment, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        finally:
            log.close()
        processes.append(process)
        return process

    def wait(predicate, description):
        until = time.monotonic() + 10
        while time.monotonic() < until:
            value = predicate()
            if value:
                return value
            failed = [process.returncode for process in processes if process.poll() is not None]
            if failed:
                raise RuntimeError(f"{description}: a process exited: {failed}; logs in {directory}")
            time.sleep(0.1)
        raise RuntimeError(f"timed out: {description}; logs in {directory}")

    try:
        spawn(["sway", "-c", str(sway_config)], "sway", env)
        parent_socket = wait(lambda: next((p.name for p in runtime.glob("wayland-*") if p.is_socket()), None), "parent Wayland socket")
        host = {**env, "WAYLAND_DISPLAY": parent_socket}
        spawn([str(BIN), "--nested"], "compositor", host)

        def socket_name():
            match = re.search(r"nested compositor: WAYLAND_DISPLAY=(\S+)", (directory / "compositor.log").read_text())
            return match.group(1) if match else None

        nested_socket = wait(socket_name, "nested Wayland socket")
        wait(lambda: (directory / "control").exists(), "control pipe")
        nested = {**host, "WAYLAND_DISPLAY": nested_socket}
        subprocess.run([str(BIN), "--clipboard", "set"], input="nested clipboard", text=True,
                       env=nested, check=True, timeout=10)
        def clipboard(environment):
            return subprocess.run([str(BIN), "--clipboard", "get"], env=environment,
                                  capture_output=True, text=True, check=True, timeout=10).stdout
        wait(lambda: clipboard(nested) == "nested clipboard", "clipboard kept after its source exits")
        saved = runtime / "clamshell/compositor" / nested_socket / "clipboard"
        wait(lambda: saved.exists() and saved.read_text() == "nested clipboard", "nested clipboard persisted separately")
        assert clipboard(host) == "", "nested clipboard must not change the parent compositor's clipboard"
        client_program = directory / "client.py"
        client_program.write_text(r'''import base64, os, pathlib, select, sys, time, tty
root = pathlib.Path(sys.argv[1])
tty.setraw(0)
os.write(1, b"ClamShell nested client\r\n")
while not (root / "copy").exists(): time.sleep(0.02)
os.write(1, b"\x1b]52;c;" + base64.b64encode(b"native clipboard") + b"\x07")
while not (root / "primary").exists(): time.sleep(0.02)
os.write(1, b"\x1b]52;p;?\x07")
response = b""
until = time.monotonic() + 5
while time.monotonic() < until:
    if select.select([0], [], [], 0.1)[0]: response += os.read(0, 4096)
    if response.endswith((b"\x07", b"\x1b\\")): break
payload = response.split(b";", 2)[2].removesuffix(b"\x07").removesuffix(b"\x1b\\")
(root / "primary.txt").write_bytes(base64.b64decode(payload))
time.sleep(300)
''')
        client = spawn(["foot", "-o", "colors-dark.background=204080", "--app-id=nested-test",
                        "python3", str(client_program), str(directory)], "client", nested)

        def pixels():
            result = subprocess.run(["grim", "-o", "HEADLESS-1", "-t", "ppm", "-"], env=host, capture_output=True, check=True)
            magic, dimensions, maximum, image = result.stdout.split(b"\n", 3)
            assert magic == b"P6" and maximum == b"255"
            width, height = map(int, dimensions.split())
            assert len(image) == width * height * 3
            def blue(x, y):
                offset = (y * width + x) * 3
                return all(abs(a - b) < 3 for a, b in zip(image[offset:offset + 3], (32, 64, 128)))
            return blue(300, height // 2), blue(width - 300, height // 2)

        def shot(name):
            subprocess.run(["grim", "-o", "HEADLESS-1", str(directory / f"{name}.png")], env=host, check=True)

        def control(*words):
            subprocess.run([str(BIN), "ctl", *words], env=host, check=True)

        wait(lambda: pixels() == (True, False), "client rendered on top")
        shot("top")
        (directory / "copy").touch()
        wait(lambda: clipboard(nested) == "native clipboard", "native client copied through wl_data_device")
        (directory / "primary").touch()
        wait(lambda: (directory / "primary.txt").exists(), "native client read the mirrored primary selection")
        assert (directory / "primary.txt").read_text() == "native clipboard"
        control("window", "down")
        wait(lambda: pixels() == (False, True), "client rendered below")
        shot("bottom")
        control("window", "up")
        wait(lambda: pixels() == (True, False), "client returned to top")
        shot("up")
        control("home")
        wait(lambda: pixels() == (False, False), "Home rendered shelf")
        shot("home")
        assert client.poll() is None, "Home must leave the client alive"
        os.killpg(client.pid, signal.SIGTERM)
        client.wait(timeout=5)
        wait(lambda: clipboard(nested) == "native clipboard", "native copy survived the source client exiting")
        assert clipboard(host) == "", "native copy must remain isolated from the parent"
        print("ok: clipboard isolation and persistence, real client rendering, bottom-slot move, return and Home", flush=True)
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
        for process in processes:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()


if __name__ == "__main__":
    main()
