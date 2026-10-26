"""ClamShell nested in headless sway, in an environment of its own, for the tests that run real clients
(nested-compositor.py, real-apps.py): a temporary directory for its home, runtime and XDG folders, a
session bus of its own (so that ClamShell serves the notifications there rather than meet the desktop's
server), and helpers to start clients, wait, look at the screens and drive ClamShell.

    with Nested(gpu=False, library='{"places":[...]}') as n:
        n.spawn(["foot"], "client", n.nested)
        n.wait(lambda: ..., "what is waited for")

Screenshots and logs stay in the printed directory.
"""
import os
from pathlib import Path
import json
import re
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "clamshell/target/debug/clamshell"


class Nested:
    def __init__(self, gpu=False, library='{"places":[{"id":"apps","path":"Apps"}]}', prefix="clamshell-compositor-"):
        self.directory = directory = Path(tempfile.mkdtemp(prefix=prefix))
        print(f"nested compositor artifacts: {directory}", flush=True)
        self.runtime = directory / "runtime"
        self.runtime.mkdir(mode=0o700)
        data = directory / "data/clamshell"
        data.mkdir(parents=True)
        (data / "first-start.json").write_text('{"done":true}\n')
        config = directory / "config/clamshell"
        config.mkdir(parents=True)
        (config / "library.json").write_text(library + "\n")
        sway_config = directory / "sway.conf"
        sway_config.write_text('output HEADLESS-1 resolution 1580x540\ndefault_border none\n'
                               'for_window [title="ClamShell nested"] fullscreen enable\n')
        self.home = directory / "home"
        self.home.mkdir()
        env = {**os.environ, "HOME": str(self.home), "XDG_RUNTIME_DIR": str(self.runtime), "XDG_CONFIG_HOME": str(directory / "config"),
               "XDG_DATA_HOME": str(directory / "data"), "XDG_CACHE_HOME": str(directory / "cache"),
               "WLR_BACKENDS": "headless", "WLR_HEADLESS_OUTPUTS": "1", "WLR_RENDERER": "gles2" if gpu else "pixman",
               "WLR_LIBINPUT_NO_DEVICES": "1", "LIBGL_ALWAYS_SOFTWARE": "0" if gpu else "1", "CLAMSHELL_STEAM": "off",
               "CLAMSHELL_CONTROL": str(directory / "control"), "CLAMSHELL_BACKLIGHTS": str(directory / "backlights"),
               "CLAMSHELL_GAMEPAD": str(directory / "no-controller"), "RUST_LOG": "info,clamshell_compositor::x11=debug"}
        self.bus = directory / "bus"
        env["DBUS_SESSION_BUS_ADDRESS"] = f"unix:path={self.bus}"
        for key in ["SWAYSOCK", "WAYLAND_DISPLAY", "DISPLAY", "WAYLAND_SOCKET", "CLAMSHELL_SESSION"]:
            env.pop(key, None)
        self.env = env
        self.sway_config = sway_config
        self.processes = []
        self.expected_exits = set()

    def __enter__(self):
        try:
            self.start()
        except BaseException:
            self.stop()
            raise
        return self

    def __exit__(self, *_):
        self.stop()

    def start(self):
        """The bus, sway, and ClamShell nested in it, up to its control pipe."""
        self.spawn(["dbus-daemon", "--session", "--nofork", "--nopidfile", f"--address=unix:path={self.bus}"], "dbus", self.env)
        self.wait(lambda: self.bus.is_socket(), "the test's session bus")
        self.spawn(["sway", "-c", str(self.sway_config)], "sway", self.env)
        parent_socket = self.wait(lambda: next((p.name for p in self.runtime.glob("wayland-*") if p.is_socket()), None), "parent Wayland socket")
        self.host = {**self.env, "WAYLAND_DISPLAY": parent_socket}
        self.compositor = self.spawn([str(BIN), "--nested"], "compositor", self.host)
        self.nested_socket = self.wait(lambda: self.logged(r"compositor: WAYLAND_DISPLAY=(\S+)"), "nested Wayland socket")
        self.wait(lambda: (self.directory / "control").exists(), "control pipe")
        self.nested = {**self.host, "WAYLAND_DISPLAY": self.nested_socket}
        display = self.wait(lambda: self.logged(r"X11 socket ready display=:(\d+)"), "X11 socket")
        self.x11 = {**self.nested, "DISPLAY": f":{display}"}

    def stop(self):
        # The whole group of each, even when its leader already exited, so that a stopped Xwayland can't
        # survive a failing test's teardown.
        for process in reversed(self.processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
        for process in self.processes:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    def spawn(self, argv, name, environment, stdin=None):
        log = (self.directory / f"{name}.log").open("w")
        try:
            process = subprocess.Popen(argv, env=environment, stdin=stdin, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        finally:
            log.close()
        self.processes.append(process)
        return process

    def wait(self, predicate, description, seconds=10):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            failed = [process.returncode for process in self.processes if process.poll() is not None and process not in self.expected_exits]
            if failed:
                raise RuntimeError(f"{description}: a process exited: {failed}; logs in {self.directory}")
            value = predicate()
            if value:
                return value
            time.sleep(0.1)
        raise RuntimeError(f"timed out: {description}; logs in {self.directory}")

    def log(self):
        return (self.directory / "compositor.log").read_text()

    def logged(self, pattern):
        """The first group of `pattern` in the compositor's log, once there."""
        match = re.search(pattern, self.log())
        return match.group(1) if match else None

    def image(self):
        """The headless output (both screens side by side): width, height, RGB bytes."""
        result = subprocess.run(["grim", "-o", "HEADLESS-1", "-t", "ppm", "-"], env=self.host, capture_output=True, check=True)
        magic, dimensions, maximum, image = result.stdout.split(b"\n", 3)
        assert magic == b"P6" and maximum == b"255"
        width, height = map(int, dimensions.split())
        assert len(image) == width * height * 3
        return width, height, image

    def shot(self, name):
        subprocess.run(["grim", "-o", "HEADLESS-1", str(self.directory / f"{name}.png")], env=self.host, check=True)

    def control(self, *words):
        subprocess.run([str(BIN), "ctl", *words], env=self.host, check=True)

    def windows(self):
        """ClamShell's windows (`window events`), as its state has them."""
        path = self.directory / "windows.json"
        path.unlink(missing_ok=True)
        self.control("window", "events", str(path))
        def read():
            try:
                return json.loads(path.read_text())
            except (FileNotFoundError, ValueError):
                return None
        return self.wait(read, "the windows dumped")["windows"]
