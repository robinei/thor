#!/usr/bin/env python3
"""Run the in-process compositor in headless sway and check a real client's pixels on both screens.

Build first (in clamshell/): cargo build --features compositor
    cargo build -p clamshell-compositor --examples
Run with --gpu to require native dmabuf on a machine with an accessible render node.
Needs sway, Xwayland, foot and grim; --gpu also needs vkcube with Wayland WSI support and a Vulkan driver. Leaves screenshots and logs in the printed temporary directory.
"""
import os
import argparse
import json
from pathlib import Path
import re
import signal
import sys
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
           "CLAMSHELL_GAMEPAD": str(directory / "no-controller"), "RUST_LOG": "info,clamshell_compositor::x11=debug"}
    env["DBUS_SESSION_BUS_ADDRESS"] = os.environ.get("DBUS_SESSION_BUS_ADDRESS", f"unix:path=/run/user/{os.getuid()}/bus")
    for key in ["SWAYSOCK", "WAYLAND_DISPLAY", "DISPLAY", "WAYLAND_SOCKET", "CLAMSHELL_SESSION"]:
        env.pop(key, None)
    processes = []

    def spawn(argv, name, environment, stdin=None):
        log = (directory / f"{name}.log").open("w")
        try:
            process = subprocess.Popen(argv, env=environment, stdin=stdin, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        finally:
            log.close()
        processes.append(process)
        return process

    def wait(predicate, description):
        until = time.monotonic() + 10
        while time.monotonic() < until:
            failed = [process.returncode for process in processes if process.poll() is not None]
            if failed:
                raise RuntimeError(f"{description}: a process exited: {failed}; logs in {directory}")
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

        def image():
            result = subprocess.run(["grim", "-o", "HEADLESS-1", "-t", "ppm", "-"], env=host, capture_output=True, check=True)
            magic, dimensions, maximum, image = result.stdout.split(b"\n", 3)
            assert magic == b"P6" and maximum == b"255"
            width, height = map(int, dimensions.split())
            assert len(image) == width * height * 3
            return width, height, image

        def pixels(colour=(32, 64, 128)):
            width, height, data = image()
            def blue(x, y):
                offset = (y * width + x) * 3
                return all(abs(a - b) < 3 for a, b in zip(data[offset:offset + 3], colour))
            return blue(300, height // 2), blue(width - 300, height // 2)

        def shot(name):
            subprocess.run(["grim", "-o", "HEADLESS-1", str(directory / f"{name}.png")], env=host, check=True)

        def control(*words):
            subprocess.run([str(BIN), "ctl", *words], env=host, check=True)

        wait(lambda: pixels() == (True, False), "client rendered on top")
        shot("top")
        def xwayland_pids():
            log = (directory / "compositor.log").read_text()
            return [int(pid) for pid in re.findall(r"Xwayland X11 transport ready display=:\d+ pid=(\d+)", log)]
        assert not xwayland_pids(), "Wayland-only use leaves Xwayland dormant"
        display_match = re.search(r"X11 socket ready display=:(\d+)", (directory / "compositor.log").read_text())
        assert display_match
        x11 = spawn([str(BIN.parent / "examples/x11-client")], "x11-client", {**nested, "DISPLAY": f":{display_match[1]}"}, stdin=subprocess.PIPE)
        wait(lambda: pixels((128, 80, 32)) == (True, False), "X11 client rendered on top")
        x11_environment = {**nested, "DISPLAY": f":{display_match[1]}"}
        def read_x11_selection(selection="CLIPBOARD", target="UTF8_STRING", *options):
            return subprocess.run([str(BIN.parent / "examples/x11-selection"), selection, target, *options],
                                  env=x11_environment, capture_output=True, timeout=10)
        wait(lambda: read_x11_selection().stdout == b"nested clipboard", "X11 reads the Wayland clipboard")
        wait(lambda: read_x11_selection("PRIMARY").stdout == b"nested clipboard", "X11 reads the Wayland primary selection")
        assert read_x11_selection("CLIPBOARD", "UTF8_STRING", "--no-property").stdout == b"nested clipboard", "None property falls back to the requested target"
        timestamp = read_x11_selection("CLIPBOARD", "TIMESTAMP")
        assert timestamp.returncode == 0 and len(timestamp.stdout) == 4 and int.from_bytes(timestamp.stdout, sys.byteorder) != 0
        targets = read_x11_selection("CLIPBOARD", "TARGETS")
        assert targets.returncode == 0 and len(targets.stdout) >= 20
        assert read_x11_selection("CLIPBOARD", "THOR_UNSUPPORTED").returncode != 0, "unsupported targets are refused"
        large_clipboard = ("café clipboard transfer\n" * 4096).encode()
        subprocess.run([str(BIN), "--clipboard", "set"], input=large_clipboard, env=nested, check=True, timeout=10)
        wait(lambda: saved.read_bytes() == large_clipboard, "large Wayland selection reaches the keeper")
        for selection in ("CLIPBOARD", "PRIMARY"):
            result = read_x11_selection(selection, "UTF8_STRING", "--slow")
            assert result.returncode == 0 and result.stdout == large_clipboard, result.stderr
            assert b"INCR" in result.stderr, "large transfers use incremental X11 properties"
        abandoned = read_x11_selection("CLIPBOARD", "UTF8_STRING", "--abandon")
        assert abandoned.returncode == 0 and b"INCR" in abandoned.stderr
        assert read_x11_selection().stdout == large_clipboard, "a dead requestor does not disrupt another reader"
        stalled = read_x11_selection("CLIPBOARD", "UTF8_STRING", "--stall")
        assert stalled.returncode != 0 and b"INCR" in stalled.stderr, "a reader that stops acknowledging times out"
        assert read_x11_selection().stdout == large_clipboard, "a timed-out transfer does not stop the clipboard"
        edge_clipboard = "a" * (16 * 1024 - 1) + "€"
        subprocess.run([str(BIN), "--clipboard", "set"], input=edge_clipboard, text=True, env=nested, check=True, timeout=10)
        wait(lambda: saved.read_text() == edge_clipboard, "a UTF-8 scalar straddles the INCR chunk boundary")
        edge = read_x11_selection("CLIPBOARD", "STRING")
        assert edge.returncode == 0 and edge.stdout == b"a" * (16 * 1024 - 1) + b"?", edge.stderr
        assert b"INCR" in edge.stderr
        assert read_x11_selection("CLIPBOARD", "UTF8_STRING", "--take").stdout == b"owned\n", "a new X11 owner is never reclaimed without a new Wayland copy"
        subprocess.run([str(BIN), "--clipboard", "set"], input="café €", text=True, env=nested, check=True, timeout=10)
        wait(lambda: clipboard(nested) == "café €", "replacement selection reaches the Wayland seat")
        wait(lambda: read_x11_selection("CLIPBOARD", "STRING").stdout == b"caf\xe9 ?", "X11 STRING uses Latin-1 with replacement")
        assert clipboard(host) == "", "X11 clipboard reads remain private to the nested session"
        pointer = spawn([str(BIN.parent / "examples/pointer")], "pointer", host, stdin=subprocess.PIPE)
        def move_pointer(x, y, click=False):
            pointer.stdin.write(f"{x} {y}{' click' if click else ''}\n".encode())
            pointer.stdin.flush()
        def colour_at(x, y, colour):
            width, _, data = image()
            start = (y * width + x) * 3
            return all(abs(a - b) < 3 for a, b in zip(data[start:start + 3], colour))
        def x11_command(command):
            x11.stdin.write(f"{command}\n".encode())
            x11.stdin.flush()
        def popup_position(kind):
            matches = re.findall(rf"popup {kind} \d+ (\d+) (\d+)", (directory / "x11-client.log").read_text())
            return tuple(map(int, matches[-1])) if matches else None
        def shell_cursor_patch():
            width, _, data = image()
            return b"".join(data[(y * width + 1390) * 3:(y * width + 1430) * 3] for y in range(440, 480))
        initial_shell_cursor = shell_cursor_patch()
        ready_moves = 0
        def pointer_ready():
            nonlocal ready_moves
            ready_moves += 1
            move_pointer(1400 + ready_moves % 2, 450)
            return shell_cursor_patch() != initial_shell_cursor
        wait(pointer_ready, "virtual pointer capability and focus reach the X11 client")
        x11_command("menu")
        wait(lambda: colour_at(120, 140, (64, 128, 32)), "X11 menu is drawn over its parent")
        move_pointer(120, 100, True)
        wait(lambda: "popup button 20 20" in (directory / "x11-client.log").read_text(), "X11 menu receives local pointer coordinates")
        x11_command("submenu")
        wait(lambda: colour_at(220, 220, (32, 64, 128)), "X11 submenu is drawn above the parent menu")
        before_submenu_click = (directory / "x11-client.log").read_text().count("popup button 20 20")
        move_pointer(220, 180, True)
        wait(lambda: (directory / "x11-client.log").read_text().count("popup button 20 20") > before_submenu_click, "X11 submenu receives its own pointer coordinates")
        move_pointer(800, 400, True)
        wait(lambda: colour_at(120, 140, (128, 80, 32)) and colour_at(220, 220, (128, 80, 32)), "an outside press dismisses the X11 menu chain")
        x11_command("hide-popups")
        x11_command("menu-no-parent")
        wait(lambda: colour_at(120, 140, (64, 128, 32)), "a parentless menu belongs to the focused client with the same XRes PID")
        x11_command("hide-popups")
        x11_command("menu")
        x11_command("submenu")
        wait(lambda: colour_at(220, 220, (32, 64, 128)), "submenu reopened before ancestor unmap")
        x11_command("unmap-menu")
        wait(lambda: colour_at(120, 140, (128, 80, 32)) and colour_at(220, 220, (128, 80, 32)), "unmapping a menu dismisses its submenu")
        x11_command("hide-popups")
        x11_command("tooltip")
        wait(lambda: colour_at(120, 140, (96, 96, 128)), "X11 tooltip is drawn")
        move_pointer(120, 100, True)
        wait(lambda: "parent button 120 100" in (directory / "x11-client.log").read_text(), "X11 tooltip does not intercept pointer input")
        x11_command("hide-popups")
        wait(lambda: colour_at(120, 140, (128, 80, 32)), "destroying X11 popups removes their pixels")
        x11_command("edge-menu")
        wait(lambda: colour_at(950, 140, (64, 128, 32)), "a menu can extend to its screen edge")
        assert not colour_at(980, 140, (64, 128, 32)), "menu pixels cannot cross into the other screen"
        x11_command("hide-popups")
        x11_command("menu")
        wait(lambda: colour_at(120, 140, (64, 128, 32)), "X11 menu reopened before moving its parent")

        control("window", "down")
        wait(lambda: pixels((128, 80, 32)) == (False, True), "X11 client rendered below")
        assert not colour_at(120, 140, (64, 128, 32)), "moving a parent dismisses its old menu"
        before_bottom_click = (directory / "x11-client.log").read_text().count("parent button")
        move_pointer(1280, 300, True)
        wait(lambda: (directory / "x11-client.log").read_text().count("parent button") > before_bottom_click, "touching the bottom parent restores its keyboard focus")
        before = len(re.findall(r"popup menu ", (directory / "x11-client.log").read_text()))
        x11_command("menu")
        wait(lambda: len(re.findall(r"popup menu ", (directory / "x11-client.log").read_text())) > before, "menu created on the bottom screen")
        lower = popup_position("menu")
        assert lower
        wait(lambda: colour_at(lower[0] + 20, lower[1] + 60, (64, 128, 32)), "X11 menu stays local to a bottom-screen parent")
        control("home")
        wait(lambda: not colour_at(lower[0] + 20, lower[1] + 60, (64, 128, 32)), "Home dismisses a menu even while its bottom parent remains visible")

        control("window", "up")
        wait(lambda: pixels((128, 80, 32)) == (True, False), "X11 client returned to top")
        x11.stdin.write(b"unmap\n")
        x11.stdin.flush()
        wait(lambda: pixels() == (True, False), "unmapping X11 restores the Wayland client")
        x11.stdin.write(b"map\n")
        x11.stdin.flush()
        wait(lambda: pixels((128, 80, 32)) == (True, False), "the same X11 window maps again")
        x11.stdin.write(b"title\n")
        x11.stdin.flush()
        x11_trace = directory / "x11-events.json"
        def renamed_x11():
            control("window", "events", str(x11_trace))
            try:
                records = json.loads(x11_trace.read_text())["records"]
            except (FileNotFoundError, json.JSONDecodeError):
                return False
            facts = [r["input"].get("Window", {}) for r in records if isinstance(r["input"], dict)]
            assert not any(r.get("Opened", {}).get("app_id") == "X11PopupFixture" for r in facts), "menus never enter the window model"
            opened = [r["Opened"] for r in facts if r.get("Opened", {}).get("app_id") == "NestedX11"]
            if not opened:
                return False
            assert opened[0]["title"] == "X11 fixture"
            assert opened[0]["pids"][0] == x11.pid, "XRes reports the app, not Xwayland"
            assert opened[0]["role"] == "Normal" and opened[0]["colour"] is None
            return any(r.get("Title", {}).get("title") == "X11 renamed" for r in facts)
        wait(renamed_x11, "X11 property changes enter the ordinary window-event ring")
        control("window", "close")
        assert x11.wait(timeout=5) == 0, "X11 client receives WM_DELETE_WINDOW"
        processes.remove(x11)
        wait(lambda: pixels() == (True, False), "Wayland client restored after X11 closes")
        count = len(xwayland_pids())
        killed = xwayland_pids()[-1]
        if Path(f"/proc/{killed}").exists():
            os.kill(killed, signal.SIGSTOP)
            control("window", "down")
            wait(lambda: pixels() == (False, True), "Wayland still moves while idle Xwayland is stopped")
            control("window", "up")
            wait(lambda: pixels() == (True, False), "Wayland returns while idle Xwayland is stopped")
            os.kill(killed, signal.SIGKILL)
        wait(lambda: not Path(f"/proc/{killed}").exists(), "crashed Xwayland is reaped through pidfd readiness")
        assert len(xwayland_pids()) == count, "crash recovery waits for another X11 connection"
        x11 = spawn([str(BIN.parent / "examples/x11-client"), "--fixed"], "x11-restarted", {**nested, "DISPLAY": f":{display_match[1]}"}, stdin=subprocess.PIPE)
        wait(lambda: pixels((128, 80, 32)) == (True, False), "another client starts a replacement Xwayland")
        wait(lambda: len(xwayland_pids()) == count + 1, "Xwayland starts again on demand")
        width, height, data = image()
        offset = (height // 2 * width + 10) * 3
        assert data[offset:offset + 3] == b"\0\0\0", "fixed-size X11 content is centred on black"
        control("window", "close")
        assert x11.wait(timeout=5) == 0
        processes.remove(x11)
        wait(lambda: pixels() == (True, False), "Wayland survives Xwayland restart")
        x11 = spawn([str(BIN.parent / "examples/x11-client"), "--transient"], "x11-transient", {**nested, "DISPLAY": f":{display_match[1]}"}, stdin=subprocess.PIPE)
        wait(lambda: pixels((128, 80, 32)) == (True, False), "X11 transient maps before its parent")
        def transient_centred():
            width, height, data = image()
            offset = (height // 2 * width + 10) * 3
            return data[offset:offset + 3] == b"\0\0\0"
        wait(transient_centred, "a transient stays centred even before its parent maps")
        control("window", "close")
        assert x11.wait(timeout=5) == 0
        processes.remove(x11)
        wait(lambda: pixels() == (True, False), "Wayland returns after the transient closes")
        def patch(x, y=250):
            width, _, data = image()
            return b"".join(data[(row * width + x - 10) * 3:(row * width + x + 40) * 3] for row in range(y - 10, y + 40))
        before_first, before_second, before_shell = patch(400), patch(500), patch(1400, 450)
        move_pointer(400, 250)
        def cursor_drawn():
            # Keep moving while the nested client's wl_pointer focus catches up.
            move_pointer(400 + int(time.monotonic() * 10) % 2, 250)
            return patch(400) != before_first
        wait(cursor_drawn, "cursor is drawn into the compositor scene")
        shot("cursor")
        move_pointer(500, 250)
        wait(lambda: patch(400) == before_first and patch(500) != before_second, "moving the cursor damages its old and new position")
        move_pointer(1400, 450)
        wait(lambda: patch(500) == before_second and patch(1400, 450) != before_shell, "the shell draws its themed cursor after leaving the client")
        # The terminal background stays fixed while the shell's readings can change.
        move_pointer(500, 250)
        wait(lambda: patch(500) != before_second, "client cursor shown before its idle timeout")
        wait(lambda: patch(500) == before_second, "cursor hides after three seconds without pointer movement")
        move_pointer(501, 250)
        wait(lambda: patch(500) != before_second, "pointer motion shows the idle cursor again")
        move_pointer(1400, 450)
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
        feedback = spawn([str(BIN.parent / "examples/feedback-client")], "feedback-client", nested)
        assert feedback.wait(timeout=10) == 0, "presentation feedback client must complete successfully"
        processes.remove(feedback)
        assert "ok: submitted and unchanged" in (directory / "feedback-client.log").read_text()
        egl_env = {**nested, "WAYLAND_DEBUG": "1"}
        egl = spawn([str(BIN.parent / "examples/egl-client")], "egl-client", egl_env)
        wait(lambda: pixels((128, 32, 64)) == (True, False), "Wayland EGL client rendered on top")
        protocol = (directory / "egl-client.log").read_text()
        for interface in ["zwp_relative_pointer_manager_v1", "zwp_pointer_constraints_v1"]:
            assert f'"{interface}"' not in protocol, "the absolute-only nested backend must not offer pointer capture"
        if not args.gpu:
            assert '"zwp_linux_dmabuf_v1"' not in protocol, "software EGL must not offer native dmabuf imports"
        if args.gpu:
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
        if args.gpu:
            def window_region(down):
                width, _, data = image()
                left, right = (1020, 1500) if down else (100, 850)
                return b"".join(data[(row * width + left) * 3:(row * width + right) * 3] for row in range(100, 440))
            def animating(down, description):
                before = window_region(down)
                wait(lambda: window_region(down) != before, description)
            vulkan_env = {**nested, "WAYLAND_DEBUG": "1"}
            vulkan = spawn(["vkcube", "--wsi", "wayland"], "vulkan-client", vulkan_env)
            wait(lambda: pixels()[0] is False, "Wayland Vulkan client rendered on top")
            animating(False, "Vulkan swapchain presents changing frames on top")
            protocol = (directory / "vulkan-client.log").read_text()
            assert re.search(r"wl_surface#\d+\.attach\(", protocol), "Vulkan WSI must attach Wayland buffers"
            assert re.search(r"zwp_linux_buffer_params_v1#\d+\.create(?:_immed)?\(", protocol), "Vulkan GPU mode must actually submit dmabufs"
            assert re.search(r"zwp_linux_dmabuf_v1#\d+\.get_(?:default|surface)_feedback\(", protocol), "Vulkan WSI must request dmabuf feedback"
            assert re.search(r"zwp_linux_dmabuf_feedback_v1#\d+\.format_table\(", protocol), "Vulkan WSI must receive the feedback format table"
            assert re.search(r"zwp_linux_dmabuf_feedback_v1#\d+\.done\(", protocol), "Vulkan WSI must receive complete feedback"
            shot("vulkan-top")
            control("window", "down")
            wait(lambda: pixels()[0] is True, "previous client visible above the Vulkan client")
            animating(True, "Vulkan swapchain keeps presenting below after resize")
            shot("vulkan-bottom")
            control("window", "up")
            wait(lambda: pixels()[0] is False, "Vulkan client returned to top")
            animating(False, "Vulkan swapchain keeps presenting after returning above")
            control("window", "close")
            assert vulkan.wait(timeout=5) == 0, "Vulkan client must receive the close request"
            processes.remove(vulkan)
            wait(lambda: pixels() == (True, False), "previous client visible after Vulkan client exits")
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
        assert not any(r["input"].get("Window", {}).get("Opened", {}).get("app_id") == "HiddenX11Helper" for r in recorded["records"] if isinstance(r["input"], dict)), "input-only X11 helpers never enter the window model"
        if args.gpu:
            print("ok: Vulkan WSI animation, resize, close, dmabuf feedback and native submission; EGL native submission", flush=True)
        print("ok: compositor cursor and damage, in-process screenshot, window-event export, clipboard isolation and persistence, presentation feedback, lazy Xwayland, X11 presentation and crash recovery, Wayland EGL and shm rendering, bottom-slot move, return and Home", flush=True)
    finally:
        # Kill the entire isolated group even when its leader already exited, so a stopped
        # Xwayland cannot survive a failing test's teardown.
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
        for process in processes:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


if __name__ == "__main__":
    main()
