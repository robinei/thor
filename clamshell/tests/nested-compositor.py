#!/usr/bin/env python3
"""Run the in-process compositor in headless sway and check a real client's pixels on both screens.

Build first (in clamshell/): cargo build --features compositor
    cargo build -p clamshell-compositor --example egl-client
Run with --gpu to require native dmabuf on a machine with an accessible render node.
Needs sway, foot and grim. Leaves screenshots and logs in the printed temporary directory.
"""
import os
import argparse
import json
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "clamshell/target/debug/clamshell"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gpu", action="store_true", help="require hardware EGL and native dmabuf traffic")
    args = parser.parse_args()
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
    private_home = directory / "home"
    private_home.mkdir()
    env = {**os.environ, "HOME": str(private_home), "XDG_RUNTIME_DIR": str(runtime), "XDG_CONFIG_HOME": str(directory / "config"),
           "XDG_DATA_HOME": str(directory / "data"), "XDG_CACHE_HOME": str(directory / "cache"),
           "WLR_BACKENDS": "headless", "WLR_HEADLESS_OUTPUTS": "1", "WLR_RENDERER": "gles2" if args.gpu else "pixman",
           "WLR_LIBINPUT_NO_DEVICES": "1", "LIBGL_ALWAYS_SOFTWARE": "0" if args.gpu else "1", "CLAMSHELL_STEAM": "off",
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

        def pixels(colour=(32, 64, 128)):
            result = subprocess.run(["grim", "-o", "HEADLESS-1", "-t", "ppm", "-"], env=host, capture_output=True, check=True)
            magic, dimensions, maximum, image = result.stdout.split(b"\n", 3)
            assert magic == b"P6" and maximum == b"255"
            width, height = map(int, dimensions.split())
            assert len(image) == width * height * 3
            def blue(x, y):
                offset = (y * width + x) * 3
                return all(abs(a - b) < 3 for a, b in zip(image[offset:offset + 3], colour))
            return blue(300, height // 2), blue(width - 300, height // 2)

        def shot(name):
            subprocess.run(["grim", "-o", "HEADLESS-1", str(directory / f"{name}.png")], env=host, check=True)

        def control(*words):
            subprocess.run([str(BIN), "ctl", *words], env=host, check=True)

        wait(lambda: pixels() == (True, False), "client rendered on top")
        shot("top")
        control("screenshot")
        screenshots = private_home / "Pictures/Screenshots"
        captured = wait(lambda: next(screenshots.glob("clamshell-*.png"), None), "in-process screenshot saved")
        def png_size():
            data = captured.read_bytes()
            if len(data) < 24 or not data.endswith(b"\x00\x00\x00\x00IEND\xaeB`\x82"): return None
            assert data[:8] == b"\x89PNG\r\n\x1a\n"
            return int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")
        assert wait(png_size, "screenshot PNG header") == (1580,540)
        wait(lambda: pixels() == (True,False), "rendering continues after framebuffer readback")
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
        egl_env = {**nested, "WAYLAND_DEBUG": "1"} if args.gpu else nested
        egl = spawn([str(BIN.parent / "examples/egl-client")], "egl-client", egl_env)
        wait(lambda: pixels((128, 32, 64)) == (True, False), "Wayland EGL client rendered on top")
        if args.gpu:
            protocol = (directory / "egl-client.log").read_text()
            assert re.search(r"zwp_linux_buffer_params_v1#\d+\.create(?:_immed)?\(", protocol), "GPU mode must actually submit dmabufs"
        shot("egl-top")
        control("window", "down")
        wait(lambda: pixels((128, 32, 64)) == (False, True), "Wayland EGL client rendered below")
        shot("egl-bottom")
        control("window", "up")
        wait(lambda: pixels((128, 32, 64)) == (True, False), "Wayland EGL client returned to top")
        control("window", "close")
        assert egl.wait(timeout=5) == 0, "EGL client must receive the close request"
        processes.remove(egl)
        wait(lambda: pixels() == (True, False), "previous client visible after EGL client exits")
        control("home")
        wait(lambda: pixels() == (False, False), "Home rendered shelf")
        shot("home")
        assert client.poll() is None, "Home must leave the client alive"
        os.killpg(client.pid, signal.SIGTERM)
        client.wait(timeout=5)
        processes.remove(client)
        wait(lambda: clipboard(nested) == "native clipboard", "native copy survived the source client exiting")
        assert clipboard(host) == "", "native copy must remain isolated from the parent"
        trace = directory / "window-events.json"
        control("window", "events", str(trace))
        def recording():
            try:
                value = json.loads(trace.read_text())
                return value if value.get("records") else None
            except (FileNotFoundError, json.JSONDecodeError):
                return None
        recorded = wait(recording, "window-event ring exported by control command")
        assert len(recorded["records"]) <= 256
        assert any("Window" in event["input"] for event in recorded["records"])
        assert recorded["after"]["windows"] == 0
        if args.gpu:
            print("ok: native dmabuf submission and rendering on the GPU path", flush=True)
        print("ok: in-process screenshot, window-event export, clipboard isolation and persistence, Wayland EGL and shm rendering, bottom-slot move, return and Home", flush=True)
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
