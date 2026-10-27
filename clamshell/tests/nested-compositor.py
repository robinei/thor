#!/usr/bin/env python3
"""Run the in-process compositor in headless sway and check a real client's pixels on both screens.

Build first (in clamshell/): cargo build
    cargo build -p clamshell-compositor --examples
Run with --gpu to require native dmabuf on a machine with an accessible render node.
Needs sway, Xwayland, foot, grim, dbus-daemon and gdbus; --gpu also needs vkcube with Wayland WSI support and a Vulkan driver. Runs on a session bus of its own, so that ClamShell serves the notifications there rather than meet the desktop's server. Leaves screenshots and logs in the printed temporary directory.
"""
import os
import argparse
import json
from pathlib import Path
import re
import signal
import sys
import subprocess
import time

from nested_env import BIN, ROOT, Nested


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gpu", action="store_true", help="require hardware EGL and native dmabuf traffic")
    args = parser.parse_args()
    with Nested(gpu=args.gpu) as n:
        directory, runtime, private_home, env, host, nested = n.directory, n.runtime, n.home, n.env, n.host, n.nested
        nested_socket, processes, expected_exits = n.nested_socket, n.processes, n.expected_exits
        spawn, wait, image, shot, control = n.spawn, n.wait, n.image, n.shot, n.control
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
            width, height, data = image()
            def blue(x, y):
                offset = (y * width + x) * 3
                return all(abs(a - b) < 3 for a, b in zip(data[offset:offset + 3], colour))
            return blue(300, height // 2), blue(width - 300, height // 2)

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
        def read_settled(expected, before, description, *args):
            # Just after saving a text the keeper replaces both selections: it copies the text to the
            # primary selection, and offers the clipboard itself once `--clipboard set` has quit. A read
            # during either is refused, or gets nothing or the text before, and is made again.
            def attempt():
                result = read_x11_selection(*args)
                if b"selection refused" in result.stderr or result.stdout in (b"", before):
                    return None
                assert result.returncode == 0 and result.stdout == expected, (result.returncode, len(result.stdout), result.stdout[:40], result.stderr)
                return result
            return wait(attempt, description)
        for selection in ("CLIPBOARD", "PRIMARY"):
            result = read_settled(large_clipboard, b"nested clipboard", f"the large text read from {selection}", selection, "UTF8_STRING", "--slow")
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
        edge = read_settled(b"a" * (16 * 1024 - 1) + b"?", large_clipboard.decode().encode("latin-1"), "the straddling text read as STRING", "CLIPBOARD", "STRING")
        assert b"INCR" in edge.stderr
        assert read_x11_selection("CLIPBOARD", "UTF8_STRING", "--take").stdout == b"owned\n", "a new X11 owner is never reclaimed without a new Wayland copy"
        subprocess.run([str(BIN), "--clipboard", "set"], input="café €", text=True, env=nested, check=True, timeout=10)
        wait(lambda: clipboard(nested) == "café €", "replacement selection reaches the Wayland seat")
        wait(lambda: read_x11_selection("CLIPBOARD", "STRING").stdout == b"caf\xe9 ?", "X11 STRING uses Latin-1 with replacement")
        assert clipboard(host) == "", "X11 clipboard reads remain private to the nested session"
        copies = 0
        def copy_x11(selection, content, *options):
            nonlocal copies
            copies += 1
            name = f"x11-copy-{copies}"
            source = directory / f"{name}.bin"
            source.write_bytes(content)
            process = spawn([str(BIN.parent / "examples/x11-copy"), selection, str(source), *options], name, x11_environment)
            wait(lambda: "owned" in (directory / f"{name}.log").read_text(), "X11 selection source owns its selection")
            return process
        def read_wayland_selection(mime, *options):
            return subprocess.run([str(BIN.parent / "examples/selection"), "CLIPBOARD", mime, *options],
                                  env=nested, capture_output=True, timeout=10)
        copied = copy_x11("CLIPBOARD", b"copied from X11")
        wait(lambda: clipboard(nested) == "copied from X11", "Wayland reads an X11-owned clipboard")
        assert read_x11_selection().stdout == b"copied from X11", "the imported offer is not echoed back as a new X11 owner"
        copied.terminate()
        copied.wait(timeout=5)
        processes.remove(copied)
        wait(lambda: clipboard(nested) == "copied from X11", "the keeper retains text after the X11 owner exits")
        imported_large = ("X11 streaming café\n" * 8192).encode()
        for selection in ("CLIPBOARD", "PRIMARY"):
            copied = copy_x11(selection, imported_large, "--incr", "--incr-targets")
            wait(lambda: clipboard(nested).encode() == imported_large, f"Wayland imports large X11 {selection} through INCR")
            copied.terminate()
            copied.wait(timeout=5)
            processes.remove(copied)
            wait(lambda: clipboard(nested).encode() == imported_large, "keeper takes over after an incremental source exits")
        normal_large = b"n" * (48 * 1024)
        copied = copy_x11("CLIPBOARD", normal_large)
        wait(lambda: clipboard(nested).encode() == normal_large, "large ordinary X11 properties are sliced without early deletion")
        copied.terminate()
        copied.wait(timeout=5)
        processes.remove(copied)
        wait(lambda: clipboard(nested).encode() == normal_large, "keeper takes over after a normal-property source exits")
        binary = bytes(range(256)) * 1024
        copied = copy_x11("CLIPBOARD", binary, "--mime", "application/x-thor-bytes", "--incr")
        wait(lambda: read_wayland_selection("application/x-thor-bytes", "--slow").stdout == binary, "Wayland imports arbitrary MIME bytes with pipe backpressure")
        assert read_wayland_selection("application/x-thor-bytes", "--abandon").returncode == 0
        stalled = read_wayland_selection("application/x-thor-bytes", "--stall")
        assert stalled.returncode == 0 and len(stalled.stdout) < len(binary), "a stalled Wayland reader is cancelled"
        assert read_wayland_selection("application/x-thor-bytes").stdout == binary, "a failed reader does not disrupt another transfer"
        copied.terminate()
        copied.wait(timeout=5)
        processes.remove(copied)
        for option, mime in [("--refuse", "application/x-thor-refused"), ("--bad-format", "application/x-thor-invalid")]:
            copied = copy_x11("CLIPBOARD", b"invalid data", "--mime", mime, option)
            wait(lambda: read_wayland_selection(mime).returncode == 0, "a refused or malformed transfer closes only its reader")
            assert read_wayland_selection(mime).stdout == b""
            copied.terminate()
            copied.wait(timeout=5)
            processes.remove(copied)
        old = copy_x11("CLIPBOARD", b"stale owner", "--stall-targets")
        copied = copy_x11("CLIPBOARD", b"replacement owner")
        wait(lambda: clipboard(nested) == "replacement owner", "a replacement X11 owner supersedes pending TARGETS")
        old.terminate()
        old.wait(timeout=5)
        processes.remove(old)
        assert clipboard(nested) == "replacement owner", "an old owner dying cannot clear its replacement"
        copied.terminate()
        copied.wait(timeout=5)
        processes.remove(copied)
        # Finish the keeper's asynchronous restore before the next independent copy case.
        wait(lambda: read_x11_selection().stdout == b"replacement owner", "keeper exports the retained selection before another X11 source starts")
        copied = copy_x11("CLIPBOARD", b"caf\xe9", "--latin1")
        wait(lambda: clipboard(nested) == "café", "X11 STRING is translated from Latin-1 to UTF-8")
        copied.terminate()
        copied.wait(timeout=5)
        processes.remove(copied)
        assert clipboard(host) == "", "X11 copies remain isolated from the host clipboard"
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
        def wait_focus(expected, description):
            def replies():
                return re.findall(r"^focus (true|false)$", (directory / "x11-client.log").read_text(), re.MULTILINE)
            seen = len(replies())
            x11_command("focus")
            def checked():
                nonlocal seen
                current = replies()
                if len(current) <= seen:
                    return False
                seen = len(current)
                if current[-1] == str(expected).lower():
                    return True
                x11_command("focus")
                return False
            wait(checked, description)
        expected_keys = 0
        for mode, gets_keys, takes_focus in [("passive", True, False), ("local-focus", True, True), ("global-focus", True, True), ("no-focus", False, False), ("passive", True, False)]:
            before_take = (directory / "x11-client.log").read_text().count("take focus ")
            x11_command(mode)
            if takes_focus:
                wait(lambda: (directory / "x11-client.log").read_text().count("take focus ") > before_take, "active X11 client receives a timestamped focus offer")
            wait_focus(gets_keys, f"X11 {mode} focus policy")
            control("tap", "key", "a")
            if gets_keys:
                expected_keys += 1
                wait(lambda: (directory / "x11-client.log").read_text().count("key up 38") == expected_keys, f"X11 {mode} receives a complete key stroke")
        assert (directory / "x11-client.log").read_text().count("key down 38") == expected_keys, "no-input clients receive no keys"
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
        wait_focus(False, "Home removes X11 keyboard focus")
        wait(lambda: not colour_at(lower[0] + 20, lower[1] + 60, (64, 128, 32)), "Home dismisses a menu even while its bottom parent remains visible")

        control("window", "up")
        wait(lambda: pixels((128, 80, 32)) == (True, False), "X11 client returned to top")
        x11.stdin.write(b"unmap\n")
        x11.stdin.flush()
        # Home showed shelf before the X11 window came back up, so shelf is what was in front before it.
        wait(lambda: pixels() == (False, False) and pixels((128, 80, 32)) == (False, False), "unmapping X11 goes back to shelf")
        x11.stdin.write(b"map\n")
        x11.stdin.flush()
        wait(lambda: pixels((128, 80, 32)) == (True, False), "the same X11 window maps again")
        wait_focus(True, "remapping restores X11 keyboard focus")
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
        wait(lambda: pixels((128, 80, 32)) == (False, False), "shelf back after X11 closes")
        # The Wayland client, the one entry running, brought back by its dock entry.
        control("tab", "dock")
        control("tap", "dock", "0")
        wait(lambda: pixels() == (True, False), "the Wayland client back from the dock")
        crash_copy = copy_x11("CLIPBOARD", b"kept across Xwayland death")
        wait(lambda: clipboard(nested) == "kept across Xwayland death", "X11 clipboard is cached before Xwayland crashes")
        count = len(xwayland_pids())
        killed = xwayland_pids()[-1]
        if Path(f"/proc/{killed}").exists():
            os.kill(killed, signal.SIGSTOP)
            control("window", "down")
            wait(lambda: pixels() == (False, True), "Wayland still moves while idle Xwayland is stopped")
            control("window", "up")
            wait(lambda: pixels() == (True, False), "Wayland returns while idle Xwayland is stopped")
            expected_exits.add(crash_copy)
            os.kill(killed, signal.SIGKILL)
        wait(lambda: not Path(f"/proc/{killed}").exists(), "crashed Xwayland is reaped through pidfd readiness")
        assert len(xwayland_pids()) == count, "crash recovery waits for another X11 connection"
        crash_copy.wait(timeout=5)
        processes.remove(crash_copy)
        expected_exits.discard(crash_copy)
        wait(lambda: clipboard(nested) == "kept across Xwayland death", "keeper survives destruction of the imported X11 offer")
        x11 = spawn([str(BIN.parent / "examples/x11-client"), "--fixed"], "x11-restarted", {**nested, "DISPLAY": f":{display_match[1]}"}, stdin=subprocess.PIPE)
        wait(lambda: pixels((128, 80, 32)) == (True, False), "another client starts a replacement Xwayland")
        wait(lambda: len(xwayland_pids()) == count + 1, "Xwayland starts again on demand")
        wait(lambda: read_x11_selection().stdout == b"kept across Xwayland death", "replacement Xwayland exports the retained Wayland selection")
        copied = copy_x11("CLIPBOARD", b"copied after Xwayland restart")
        wait(lambda: clipboard(nested) == "copied after Xwayland restart", "a replacement adapter imports a fresh X11 generation")
        copied.terminate()
        copied.wait(timeout=5)
        processes.remove(copied)
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
        # Notifications: ClamShell is the bus's notification server and draws them over the bottom screen,
        # just below the bar, on a fill barely above black.
        wait(lambda: "notifications: org.freedesktop.Notifications is ours" in (directory / "compositor.log").read_text(), "ClamShell serves the notifications")
        monitor = spawn(["gdbus", "monitor", "--session", "--dest", "org.freedesktop.Notifications"], "notifications", env)
        def notifications(method, *args):
            return subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.Notifications", "--object-path", "/org/freedesktop/Notifications",
                                   "--method", f"org.freedesktop.Notifications.{method}", *args], env=env, capture_output=True, text=True, check=True, timeout=10).stdout
        def notify(summary, actions):
            return int(re.search(r"uint32 (\d+)", notifications("Notify", "nested", "0", "", summary, "from the test", actions, "{}", "0")).group(1))
        def signalled(text):
            return text in (directory / "notifications.log").read_text()
        # A strip across the card, clear of its text and its close button. One pixel isn't enough: the
        # anti-aliased edge of a glyph below can be as dark as the card's fill.
        surface = (0x13, 0x13, 0x16)
        strip = range(960 + 140, 960 + 540, 4)
        def on_card():
            return sum(colour_at(x, 100, surface) for x in strip)
        def card_shown():
            return on_card() >= len(strip) * 0.95
        def card_gone():
            return on_card() < len(strip) / 2
        first = notify("Nested", "[]")
        wait(card_shown, "a notification is drawn over the bottom screen")
        shot("notification")
        notifications("CloseNotification", str(first))
        wait(card_gone, "a notification its program closed is gone")
        wait(lambda: signalled(f"NotificationClosed (uint32 {first}, uint32 3)"), "closed by the call, as the signal says")
        second = notify("Tap me", "['default', 'Open']")
        wait(card_shown, "a second notification is drawn")
        move_pointer(960 + 200, 80, True)
        wait(lambda: signalled(f"ActionInvoked (uint32 {second}, 'default')") and signalled(f"NotificationClosed (uint32 {second}, uint32 2)"),
             "a tap invokes the default action and dismisses the notification")
        wait(card_gone, "a tapped notification is gone")
        monitor.terminate()
        monitor.wait(timeout=5)
        processes.remove(monitor)
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
        for mode, expected in [("pq", 186), ("linear", 118), ("srgb", 128)]:
            colour = spawn([str(BIN.parent / "examples/colour-client"), mode], f"colour-{mode}", egl_env)
            wait(lambda: pixels((expected, expected, expected)) == (True, False), f"{mode} EGL pixels converted through the linear scene")
            control("window", "events", str(directory / f"colour-{mode}.json"))
            def colour_recording():
                try:
                    return json.loads((directory / f"colour-{mode}.json").read_text())
                except (FileNotFoundError, json.JSONDecodeError):
                    return None
            facts = wait(colour_recording, "colour facts exported")
            assert any(any(record["input"].get("Window", {}).get(kind, {}).get("colour") for kind in ["Opened", "Changed"]) for record in facts["records"] if isinstance(record["input"], dict)), "surface colour reaches the common window facts"
            control("window", "close")
            assert colour.wait(timeout=5) == 0, "colour client receives close"
            processes.remove(colour)
            wait(lambda: pixels() == (True, False), "SDR client remains unchanged after colour fixture exits")
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
            # SMAA shown one to one, unsharpened: its blending in the final pass, its weights bound to the
            # second texture unit around Smithay's draw (docs/picture.md, Smithay).
            def in_between():
                width, data = 750, window_region(False)
                luma = [data[i] * 3 + data[i + 1] * 6 + data[i + 2] for i in range(0, len(data), 3)]
                edges = mids = 0
                for y in range(1, len(luma) // width - 1):
                    for x in range(width):
                        up, c, down = luma[(y - 1) * width + x], luma[y * width + x], luma[(y + 1) * width + x]
                        if abs(up - down) > 250:
                            edges += 1
                            mids += min(up, down) + 40 < c < max(up, down) - 40
                return mids / max(edges, 1)
            def averaged():
                values = []
                for _ in range(8):
                    values.append(in_between())
                    time.sleep(0.1)
                return sum(values) / len(values)
            plain = averaged()
            control("tab", "settings")
            control("tap", "setting", "aa")
            control("tap", "setting", "aa")
            animating(False, "Vulkan swapchain keeps presenting with SMAA")
            time.sleep(0.5)
            smaa = averaged()
            assert smaa > plain + 0.15, f"SMAA must blend the Vulkan client's edges: {plain:.2f} of edge pixels in between without, {smaa:.2f} with"
            assert "picture passes failed" not in (directory / "compositor.log").read_text(), "SMAA's passes must not fail"
            shot("vulkan-smaa")
            control("tap", "setting", "aa")
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
            print("ok: Vulkan WSI animation, resize, close, dmabuf feedback, native submission and SMAA blended in the final pass; EGL native submission", flush=True)
        print("ok: PQ, linear and SDR EGL colour conversion; compositor cursor and damage, in-process screenshot, window-event export, clipboard isolation and persistence, presentation feedback, lazy Xwayland, X11 presentation and crash recovery, Wayland EGL and shm rendering, bottom-slot move, return and Home, notifications shown, closed and tapped", flush=True)


if __name__ == "__main__":
    main()
