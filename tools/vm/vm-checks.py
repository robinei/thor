#!/usr/bin/env python3
"""Checks of ClamShell in the virtual Thor through its real inputs: touches on the bottom screen's
touchscreen (QEMU's virtio-multitouch, driven over QMP), a fake controller (the fake_pad example), and
two virtual Bluetooth controllers (btvirt) with a test device. Each check acts as a user would and looks
at what came of it: ClamShell's journal, its saved files, its windows (`window events`), the devices it
makes, the system (NetworkManager, BlueZ, PipeWire, a fake backlight), the screens (QEMU's).

    tools/vm/vm.py run --headless &          (the machine running; ClamShell built on the host)
    tools/vm/vm-checks.py [CHECK ...]        (all of them by default; --list names them)

For the checks ClamShell runs, as the device's session (it adopts programs it didn't start), with a test
library (a game that opens an empty mpv window), a fake backlight and its own
state directory (a runtime drop-in for its service, removed at the end), PipeWire gets a null sink (the
machine has no audio device), and Bluetooth is set up (tools/vm/vm.py bluetooth). The exit status is the
number of failed checks. Standard library only.
"""

import argparse
import json
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import importlib  # noqa: E402

vm = importlib.import_module("vm")

HOME = f"/home/{vm.USER}"
CHECKS_DIR = f"{HOME}/.cache/thor-checks"
# What the checks' terminal was given to type (see Checks._terminal_with_keyboard).
TYPED = f"{CHECKS_DIR}/typed"
# ClamShell's library config, and what it keeps (play time, favourites, settings, keymaps, the dock, where
# shelf was, brightness: JSON files, see clamshell-system's store).
CONFIG = f"{CHECKS_DIR}/config/clamshell"
DATA = f"{CHECKS_DIR}/data/clamshell"
BACKLIGHT = f"{CHECKS_DIR}/backlight/panel"
DROP_IN = "$XDG_RUNTIME_DIR/systemd/user/clamshell.service.d/thor-checks.conf"
# The bar's icons, in the bottom screen's pixels (their centres; the burn-in orbit moves them a few
# pixels).
BAR_Y = 45
ICONS = {"settings": 51, "dock": 145, "keyboard": 238, "touchpad": 332, "sysinfo": 425, "volume": 518,
         "brightness": 612, "bluetooth": 706, "wifi": 800, "battery": 893, "clock": 996, "pad-mouse": 1097,
         "windows-below": 1190}
# A list's first row (Wi-Fi's networks, Bluetooth's devices), and a row's height.
FIRST_ROW_Y = 240
ROW_H = 103
# shelf's list on the main screen: where its first row's middle is, and a row's height (1920x1080).
SHELF_X, SHELF_FIRST_Y, SHELF_ROW_H = 300, 150, 94.6
# The time tab: its list's first row, and its header's back arrow.
TIME_FIRST_Y = 252
TIME_BACK = (83, 135)
# Below the bar: where a finger points (the touchpad tab: at once).
PAD_Y = 600

PRELUDE = ("export WAYLAND_DISPLAY=wayland-1; "
           "dev() { for d in /sys/class/input/event*; do [ \"$(cat $d/device/name)\" = \"$1\" ] && echo /dev/input/${d##*/}; done; }; ")


def sh(command, input=None, binary=False, check=False):
    """A command in the machine (bash, as the user); its output."""
    result = subprocess.run(vm.ssh_args([PRELUDE + command], quiet=True), input=input, capture_output=True,
                            text=not binary)
    if check and result.returncode != 0:
        raise RuntimeError(f"{command}: {result.stderr.strip()}")
    return result.stdout


def until(condition, seconds=5.0, every=0.2):
    """Waits for a condition; whether it came."""
    end = time.monotonic() + seconds
    while True:
        if condition():
            return True
        if time.monotonic() > end:
            return False
        time.sleep(every)


# --- what ClamShell does

def now():
    """Where the user's journal is now, to read it from: its cursor (not the clock, which the time check
    steps an hour and NTP may move: lines stamped in the future would be read again by every check after;
    the clock only if the journal has no cursor yet)."""
    out = sh("journalctl --user -n 1 --show-cursor -o cat --no-pager")
    cursor = next((l.split("cursor: ", 1)[1].strip() for l in out.splitlines() if l.startswith("-- cursor: ")), "")
    return cursor or sh("date +%s.%N").strip()


def after(since):
    """journalctl's arguments for the lines after `since` (see `now`)."""
    return f"--after-cursor {shlex.quote(since)}" if since.startswith("s=") else f"--since @{since}"


def journal(since):
    return sh(f"journalctl --user -u clamshell {after(since)} -o cat --no-pager")


def logged(since, text, seconds=5.0):
    return until(lambda: text in journal(since), seconds)


def control(line):
    sh(f"echo {shlex.quote(line)} > $XDG_RUNTIME_DIR/clamshell-control")
    time.sleep(0.3)


def kept(name):
    """One of ClamShell's files, read; None if it isn't there (or doesn't read)."""
    out = sh(f"cat {DATA}/{name} 2>/dev/null")
    try:
        return json.loads(out) if out.strip() else None
    except ValueError:
        return None


def resume():
    """Where shelf and the second screen were (as saved)."""
    return kept("resume.json") or {}


def collection():
    return resume().get("collection")


def selection():
    """shelf's selected entry in the collection shown (as saved)."""
    place = dict(map(tuple, resume().get("places", []))).get(collection())
    return place[-1] if place else None


def tab():
    """The second screen's tab (as saved)."""
    return resume().get("tab")


def screens():
    """The settings screens' files (all, runners', entries'), read."""
    out = sh(f"for f in {DATA}/settings/*.json; do [ -e \"$f\" ] && tr -d '\\n' < \"$f\" && echo; done")
    return [json.loads(line) for line in out.splitlines() if line.strip()]


def plays():
    """How many plays were recorded."""
    out = sh(f"grep -c . {DATA}/plays.jsonl 2>/dev/null")
    return int(out.strip() or 0)


def saved_keymaps():
    """The keymaps as saved (a file each in keymaps/), by id: each as JSON (clamshell-state's Keymap)."""
    out = sh(f"cat {DATA}/keymaps/*.json 2>/dev/null")
    decoder, keymaps, at = json.JSONDecoder(), [], 0
    while out[at:].strip():
        at = len(out) - len(out[at:].lstrip())
        keymap, at = decoder.raw_decode(out, at)
        keymaps.append(keymap)
    return sorted(keymaps, key=lambda k: k["id"])


def keymap_named(name):
    return next((k for k in saved_keymaps() if k["name"] == name), None)


def bound(keymap, input, act="press", layer=0):
    """A keymap's binding of `input` (as JSON: {"Button": "A"}, "Gyro", ...) in a layer, if any."""
    for l in (keymap or {}).get("layers", []):
        if l["id"] == layer:
            return next((b for i, b in l["bindings"][act] if i == input), None)
    return None


def output(keymap, input, act="press", layer=0):
    return (bound(keymap, input, act, layer) or {}).get("output")


def chord(char, ctrl=False):
    """A key chord's JSON: a character, Ctrl held or not."""
    return {"Key": {"key": {"Char": char}, "shift": False, "ctrl": ctrl, "alt": False, "logo": False}}


def windows():
    """ClamShell's windows as its state has them (`window events`): each with its id, app id, group
    (`group`: `{"Entry": key}`, `{"Window": id}` or none), whether it is below (`down`), and whether the
    top screen shows it (`focused`)."""
    path = f"{CHECKS_DIR}/windows.json"
    sh(f"rm -f {path}")
    control(f"window events {path}")
    for _ in range(25):
        try:
            return json.loads(sh(f"cat {path} 2>/dev/null"))["windows"]
        except (ValueError, KeyError):
            time.sleep(0.1)
    return []


def front():
    """The group in front on the main screen, as the compositor shows it: the focused top window's, else
    "shelf". An entry's group is its key, a window's own "window <id>"."""
    shown = next((w for w in windows() if w["focused"]), None)
    if not shown:
        return "shelf"
    group = shown.get("group") or {}
    return group.get("Entry") or f"window {group.get('Window')}"


def typed():
    """How many characters the checks' terminal has been given."""
    out = sh(f"stat -c %s {TYPED} 2>/dev/null").strip()
    return int(out) if out.isdigit() else 0


def units(prefix):
    return sh(f"systemctl --user list-units 'clamshell-{prefix}*' --plain --no-legend | awk '{{print $1}}'").split()


def light_pixels(shot, y, x0, x1, level=150):
    """How many pixels of a row stretch are light (the slider's track and knob, against the black bar)."""
    w, _, rgb = shot
    return sum(1 for x in range(x0, x1) if min(rgb[(y * w + x) * 3:(y * w + x) * 3 + 3]) >= level)


def device(name):
    return sh(f"dev {shlex.quote(name)}").strip()


def capture(command, seconds, action):
    """Runs a reading command in the machine (evtest, libinput) for `seconds` while `action` is done;
    what it printed."""
    out = "/tmp/thor-checks-capture"
    sh(f"rm -f {out}; setsid -f sh -c {shlex.quote(f'timeout {seconds} {command} > {out} 2>&1')} < /dev/null > /dev/null 2>&1")
    time.sleep(0.5)
    action()
    time.sleep(max(0.0, seconds - 0.5))
    # (Anchored: the remote shell's own command line holds the pattern too.)
    until(lambda: not sh("pgrep -f '^timeout .*thor-checks-capture' || true").strip(), 3)
    return sh(f"cat {out}")


def grabbed(path):
    """Whether someone holds an input device to itself (EVIOCGRAB)."""
    probe = ("import fcntl,os,sys\nfd=os.open(sys.argv[1],os.O_RDONLY)\ntry:\n fcntl.ioctl(fd,0x40044590,1);"
             "fcntl.ioctl(fd,0x40044590,0);print('free')\nexcept OSError as e:\n print('grabbed' if e.errno==16 else e)")
    return sh(f"python3 -c {shlex.quote(probe)} {path}").strip() == "grabbed"


# --- inputs

class Inputs:
    def __init__(self):
        self.qmp = vm.Qmp()
        self.touch = vm.Touch(self.qmp)

    def pointer(self, x, y, button=None, head=0):
        """The absolute pointer on a screen (head 0: the main one) to (x, y); a button down or up there
        ("down", "up")."""
        size = vm.SCREENS[head]
        events = [{"type": "abs", "data": {"axis": "x", "value": vm.scaled(x, size[0])}},
                  {"type": "abs", "data": {"axis": "y", "value": vm.scaled(y, size[1])}}]
        if button:
            events.append({"type": "btn", "data": {"button": "left", "down": button == "down"}})
        self.qmp.command("input-send-event", device="video0", head=head, events=events)

    def screenshot(self, head=1):
        """A screen as (width, height, RGB bytes), as QEMU shows it: the main screen head 0, the second
        1 (through this connection: QMP takes one at a time)."""
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "screen.ppm"
            self.qmp.command("screendump", filename=str(path), device="video0", head=head)
            parts = path.read_bytes().split(maxsplit=4)
        if len(parts) < 5 or parts[0] != b"P6":
            raise RuntimeError("screendump: no picture")
        return int(parts[1]), int(parts[2]), parts[4]

    def tap(self, x, y, slot=0):
        self.touch.down(slot, x, y)
        time.sleep(0.05)
        self.touch.up(slot)
        time.sleep(0.3)

    def hold(self, x, y, seconds, slot=0):
        self.touch.down(slot, x, y)
        time.sleep(seconds)
        self.touch.up(slot)
        time.sleep(0.3)

    def lift_all(self):
        for slot in list(self.touch.ids):
            self.touch.up(slot)


def pad(*commands):
    """Commands for the fake pad (tools/vm/vm.py pad do)."""
    sh(f"timeout 5 sh -c 'cat > $XDG_RUNTIME_DIR/fake-pad'", input="\n".join(commands) + "\n")


def press(button, ms=80):
    pad(f"tap {button} {ms}")
    time.sleep(ms / 1000 + 0.3)


def dpad(direction):
    pad(f"dpad {direction}", "sleep 80", "dpad none")
    time.sleep(0.4)


def focus_shelf():
    """The pad's focus into shelf's list (after a touch the focus is where the finger was: on the
    second screen, which is below the main one), on its first game."""
    for _ in range(4):
        dpad("up")


def plug_pad():
    since = now()
    subprocess.run([sys.executable, str(Path(vm.__file__)), "pad", "plug"], check=True)
    return logged(since, "fake pad grabbed", 5)


# --- the test environment

# Test games: test.game, zeta.game, zulu01.game to zulu14.game, all in the Test tab, each an empty mpv window (not foot: it asks for text input, which brings up the keyboard). Started through env: ClamShell takes a file mpv starts for a film, not a game.
LIBRARY = f"""{{ "places": [
  {{ "id": "recent", "path": "Recent", "type": "recent", "icon": "clock" }},
  {{ "id": "favourites", "path": "Favourites", "type": "favourites", "icon": "star" }},
  {{ "id": "test", "path": "Test" }},
  {{ "id": "games", "path": "Games", "icon": "gamepad" }},
  {{ "id": "apps", "path": "Apps", "icon": "apps", "recent": false }},
  {{ "id": "tools", "path": "Tools", "type": "tools", "icon": "tools" }}
],
  "runners": [
  {{ "type": "desktop", "name": "Desktop", "match": ["*.desktop"] }},
  {{ "name": "Test game", "match": ["*.game"],
    "launch": ["env", "mpv", "--idle", "--force-window", "--vo=wlshm", "--no-terminal", "--wayland-app-id=testgame"] }}
],
  "sources": [
  {{ "type": "dir", "id": "test", "dir": "{CHECKS_DIR}/games", "place": "@test", "match": ["*.game"] }},
  {{ "type": "desktop", "categories": "Emulator", "place": "@apps" }},
  {{ "type": "desktop", "categories": "Game", "place": "@games" }},
  {{ "type": "desktop", "place": "@apps" }}
] }}
"""


def setup(bluetooth):
    print("setting up: test library, fake backlight, null sink, ClamShell's test environment")
    since = now()
    script = f"""set -e
mkdir -p {CONFIG} {DATA} {CHECKS_DIR}/games {BACKLIGHT}
# (ClamShell stopped first: it may be writing.)
systemctl --user stop clamshell
rm -rf {DATA} && mkdir -p {DATA}
echo '["desktop:foot"]' > {DATA}/dock.json
# (Set up: not a first start, which would open the Device view over shelf.)
echo '{{"done": true}}' > {DATA}/first-start.json
cat > {CONFIG}/library.json <<'EOF'
{LIBRARY}EOF
touch {CHECKS_DIR}/games/test.game {CHECKS_DIR}/games/zeta.game {" ".join(f"{CHECKS_DIR}/games/zulu{i:02}.game" for i in range(1, 15))}
echo 100 > {BACKLIGHT}/max_brightness; echo 50 > {BACKLIGHT}/brightness
mkdir -p $(dirname {DROP_IN})
printf '%s\\n' '[Service]' 'Environment=XDG_CONFIG_HOME={CHECKS_DIR}/config' 'Environment=XDG_DATA_HOME={CHECKS_DIR}/data' \\
    'Environment=CLAMSHELL_BACKLIGHTS={CHECKS_DIR}/backlight' > {DROP_IN}
wpctl status | grep -q thor-checks-sink || wpctl status | grep -q 'Test sink' || pw-cli create-node adapter \\
    '{{ factory.name=support.null-audio-sink node.name=thor-checks-sink node.description="Test sink" media.class=Audio/Sink object.linger=true audio.position=[FL FR] }}' > /dev/null 2>&1
sleep 0.5
wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.5 2>/dev/null
systemctl --user stop 'clamshell-run-*' 'app-graphical-clamshell-run-*' 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user reset-failed clamshell; systemctl --user restart clamshell
"""
    sh("bash -s", input=script, check=True)
    if bluetooth:
        result = subprocess.run([sys.executable, str(Path(vm.__file__)), "bluetooth"], capture_output=True, text=True)
        if result.returncode != 0:
            print(f"  (Bluetooth not set up: {result.stderr.strip() or result.stdout.strip()})")
    if not until(lambda: sh("systemctl --user is-active clamshell").strip() == "active" and front() == "shelf", 15):
        sys.exit("ClamShell didn't start")
    time.sleep(1)
    return since


def teardown():
    subprocess.run([sys.executable, str(Path(vm.__file__)), "pad", "unplug"], capture_output=True)
    sh(f"rm -f {DROP_IN}; systemctl --user stop 'clamshell-run-*' 'app-graphical-clamshell-run-*' 2>/dev/null; "
       "systemctl --user daemon-reload; systemctl --user reset-failed clamshell; systemctl --user restart clamshell")


# --- the checks

class Checks:
    def __init__(self, inputs, started):
        self.i = inputs
        self.started = started
        self.failed = 0
        self.passed = 0

    def check(self, what, ok, evidence=""):
        if ok:
            self.passed += 1
            print(f"  ok    {what}")
        else:
            self.failed += 1
            print(f"  FAIL  {what}" + (f"\n        {evidence}" if evidence else ""))
        return ok

    def icon(self, name, how="tap", seconds=0.0):
        if how == "tap":
            self.i.tap(ICONS[name], BAR_Y)
        else:
            self.i.hold(ICONS[name], BAR_Y, seconds)

    def open_tab(self, name):
        """A tab open, through the control pipe (not a tap on its icon, at a place that moves with the
        bar's layout: the checks of the bar's taps tap it)."""
        if tab() == name:
            return True
        control(f"tab {name}")
        return until(lambda: tab() == name, 5)

    def route(self):
        """Where the pad goes now (the last route ClamShell logged since the checks started it)."""
        routes = [line for line in journal(self.started).splitlines() if "effect: PadRoute(" in line]
        return routes[-1].split("PadRoute(")[1].rstrip(")") if routes else None

    def reset(self):
        """A known start: no fingers down, no game or app, shelf in front, the dock tab, the pad mouse
        off."""
        self.i.lift_all()
        sh("systemctl --user stop 'clamshell-run-*' 'app-graphical-clamshell-run-*' 2>/dev/null || true")
        until(lambda: front() == "shelf", 5)
        if self.route() == "Mouse":
            self.icon("pad-mouse")
        self.icon("dock")
        until(lambda: tab() in (None, "dock"), 5)

    # bar

    def bar_tap(self):
        since = now()
        self.icon("wifi")
        self.check("bar: a tap on Wi-Fi opens its tab", until(lambda: tab() == "wifi") and logged(since, "Watch(Wifi, true)", 1),
                   f"tab {tab()!r}")
        self.icon("dock")
        self.check("bar: a tap on the dock opens it", until(lambda: tab() == "dock"), f"tab {tab()!r}")

    def time_zone(self):
        """The clock opens the time tab, on its main page; the time zone's row shows the regions, a region
        then a zone in it sets the system's time zone (the session's user may: the polkit rule) and goes
        back to the main page; back from a region goes to the regions. Then Set automatically off, the
        hour stepped up and down, and on again. Put back after."""
        zone = lambda: sh("timedatectl show --property=Timezone --value").strip()
        ntp = lambda: sh("timedatectl show --property=NTP --value").strip()
        before, ntp_before = zone(), ntp()
        since = now()
        self.icon("clock")
        self.check("time: a tap on the clock opens the time tab", until(lambda: tab() == "time") and logged(since, "Watch(Time, true)", 2),
                   f"tab {tab()!r}")
        # The regions: Africa, America, Antarctica, Arctic, Asia (the fifth), ...; Asia's first zone.
        control("tap time-zone")
        time.sleep(0.5)
        self.i.tap(300, TIME_FIRST_Y + 4 * ROW_H)
        time.sleep(0.5)
        since = now()
        self.i.tap(300, TIME_FIRST_Y)
        self.check("  the time zone's row, Asia, then its first zone, sets it", until(lambda: zone().startswith("Asia/")) and logged(since, "SetTimezone Asia/", 2),
                   f"time zone {zone()!r}")
        # From a region's zones, back to the regions: Africa's first zone.
        control("tap time-zone")
        time.sleep(0.5)
        self.i.tap(300, TIME_FIRST_Y + 4 * ROW_H)
        time.sleep(0.5)
        control("tap time-back")
        time.sleep(0.5)
        self.i.tap(300, TIME_FIRST_Y)
        time.sleep(0.5)
        self.i.tap(300, TIME_FIRST_Y)
        self.check("  back to the regions, then Africa's first zone", until(lambda: zone().startswith("Africa/")), f"time zone {zone()!r}")
        # Set automatically off: the clock by hand, an hour on and back.
        if ntp() == "yes":
            control("tap time-ntp")
        self.check("  Set automatically off", until(lambda: ntp() == "no", 5), f"NTP {ntp()!r}")
        clock = lambda: int(sh("date +%s").strip() or "0")
        start = clock()
        control("tap time-step hour up")
        moved = until(lambda: clock() - start >= 3500, 5)
        self.check("  the hour stepped up sets the clock an hour on", moved, f"{clock() - start} s on")
        control("tap time-step hour down")
        self.check("  and down, back", until(lambda: abs(clock() - start) < 120, 5), f"{clock() - start} s on")
        if ntp_before == "yes":
            control("tap time-ntp")
            self.check("  Set automatically on again", until(lambda: ntp() == "yes", 5), f"NTP {ntp()!r}")
        sh(f"sudo timedatectl set-timezone {shlex.quote(before)}")
        self.icon("dock")

    def volume_hold(self):
        before = light_pixels(self.i.screenshot(), BAR_Y, 20, 600)
        since = now()
        self.i.touch.down(0, ICONS["volume"], BAR_Y)
        time.sleep(0.5)
        during = light_pixels(self.i.screenshot(), BAR_Y, 20, 600)
        self.i.touch.up(0)
        time.sleep(3)
        self.check("bar: a hold on volume shows the slider", during > before + 150, f"light pixels on the bar {before} -> {during}")
        self.check("  and doesn't open the volume tab", tab() == "dock" and "Watch(Audio, true)" not in journal(since),
                   f"tab {tab()!r}")

    def volume_slide(self):
        def volume():
            out = sh("wpctl get-volume @DEFAULT_AUDIO_SINK@ 2>/dev/null").split()
            return round(float(out[1]) * 100) if len(out) > 1 else None
        sh("wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.5")
        time.sleep(1.5)
        start = volume()
        x = ICONS["volume"]
        self.i.touch.slide(0, x, BAR_Y, x - 200, BAR_Y, 10, 0.6)
        self.i.touch.up(0)
        lowered = until(lambda: (volume() or 100) < (start or 0) - 10, 3)
        low = volume()
        self.i.touch.slide(0, x, BAR_Y, x + 150, BAR_Y, 10, 0.6)
        self.i.touch.up(0)
        raised = until(lambda: (volume() or 0) > (low or 100) + 10, 3)
        self.check("bar: a slide on volume sets the volume (PipeWire's null sink)", lowered and raised,
                   f"{start} -> {low} -> {volume()}")
        time.sleep(2.5)
        self.check("  and doesn't open the volume tab", tab() == "dock", f"tab {tab()!r}")

    def brightness_slide(self):
        def level():
            return int(sh(f"cat {BACKLIGHT}/brightness").strip() or 0)
        start = level()
        x = ICONS["brightness"]
        self.i.touch.slide(0, x, BAR_Y, x + 150, BAR_Y, 10, 0.6)
        self.i.touch.up(0)
        raised = until(lambda: level() > start + 10, 3)
        self.check("bar: a slide on brightness sets the backlight (a fake one)", raised, f"{start} -> {level()}")
        self.check("  and is saved", until(lambda: kept("brightness.json") is not None, 5), sh(f"ls {DATA}"))
        time.sleep(1)
        self.check("  and doesn't open the brightness tab", tab() == "dock", f"tab {tab()!r}")

    def two_fingers(self):
        def level():
            return int(sh(f"cat {BACKLIGHT}/brightness").strip() or 0)
        start = level()
        x = ICONS["brightness"]
        self.i.touch.slide(0, x, BAR_Y, x - 150, BAR_Y, 6, 0.5)  # finger 0 sliding down, still on the screen
        time.sleep(0.5)
        middle = level()
        self.i.tap(ICONS["wifi"], BAR_Y, slot=1)  # finger 1 taps another tab meanwhile
        for step in range(1, 11):  # finger 0 goes on, up
            time.sleep(0.05)
            self.i.touch.move(0, x - 150 + step * 20, BAR_Y)
        time.sleep(0.5)
        end = level()
        self.i.touch.up(0)
        self.check("bar, two fingers: a slide goes on while another finger taps a tab",
                   middle < start - 10 and end > middle + 10, f"backlight {start} -> {middle} (the tap) -> {end}")
        self.check("  and the tap opens its tab", until(lambda: tab() == "wifi"), f"tab {tab()!r}")

    # lists

    def wifi_forget(self):
        sh("sudo nmcli connection delete ThorTest > /dev/null 2>&1; "
           "sudo nmcli dev wifi connect ThorTest password thortest123 ifname wlan0 > /dev/null 2>&1")
        saved_before = "ThorTest" in sh("nmcli -t -f NAME connection show")
        self.open_tab("wifi")
        time.sleep(3)  # the list read (the connected network first)
        since = now()
        self.i.hold(300, FIRST_ROW_Y, 0.8)
        forgot = logged(since, "wifi: forgot ThorTest")
        gone = until(lambda: "ThorTest" not in sh("nmcli -t -f NAME connection show"))
        self.check("Wi-Fi: a long press on the saved network forgets it", saved_before and forgot and gone,
                   f"saved before {saved_before}, logged {forgot}, gone from NetworkManager {gone}")

    def wifi_scroll(self):
        """More networks than fit: the simulated radios loaded again with eight more, each an open test
        network (ThorExtra3..10, hostapd), the machine's own radio managed as before. A drag scrolls the
        list, and a tap afterwards joins the network under the finger."""
        extras = [f"ThorExtra{i}" for i in range(3, 11)]
        confs = "; ".join(f"printf 'interface=wlan{i}\\ndriver=nl80211\\nssid=ThorExtra{i}\\nhw_mode=g\\nchannel=1\\n' "
                          f"| sudo tee /run/thor-extra/wlan{i}.conf > /dev/null" for i in range(3, 11))
        sh("sudo systemctl stop thor-test-ap; sudo nmcli dev disconnect wlan0 > /dev/null 2>&1; "
           f"for c in {' '.join(extras)} ThorOpen; do sudo nmcli connection delete $c > /dev/null 2>&1; done; "
           "sudo modprobe -r mac80211_hwsim && sudo modprobe mac80211_hwsim radios=11; sleep 2; "
           "for i in $(seq 3 10); do sudo nmcli device set wlan$i managed no; done; "
           f"sudo rm -rf /run/thor-extra; sudo mkdir -p /run/thor-extra; {confs}; "
           "sudo systemd-run --quiet --unit=thor-extra-ap hostapd /etc/hostapd/thor-test.conf /etc/hostapd/thor-open.conf "
           "/run/thor-extra/wlan3.conf /run/thor-extra/wlan4.conf /run/thor-extra/wlan5.conf /run/thor-extra/wlan6.conf "
           "/run/thor-extra/wlan7.conf /run/thor-extra/wlan8.conf /run/thor-extra/wlan9.conf /run/thor-extra/wlan10.conf")
        try:
            seen = until(lambda: all(e in sh("nmcli -t -f SSID dev wifi list --rescan yes") for e in extras), 30, 2)
            if not self.check("Wi-Fi: extra test networks on the air", seen, sh("nmcli -t -f SSID dev wifi list")):
                return
            self.open_tab("wifi")
            time.sleep(4)  # ClamShell's own scan and list
            # ClamShell's order: connected, saved, stronger, then by name.
            known = set(sh("nmcli -t -f NAME connection show").split())
            rows = [line.rsplit(":", 1) for line in sh("nmcli -t -f SSID,SIGNAL dev wifi list").splitlines() if ":" in line]
            best = {}
            for ssid, signal in rows:
                if ssid:
                    best[ssid] = max(best.get(ssid, 0), int(signal or 0))
            order = sorted(best, key=lambda n: (n not in known, -best[n], n))
            since = now()
            # Up by two rows and a bit, slowly (no fling): the list follows the finger.
            self.i.touch.slide(0, 300, FIRST_ROW_Y + 3 * ROW_H, 300, FIRST_ROW_Y + 0.8 * ROW_H, 20, 1.5)
            time.sleep(0.5)
            self.i.touch.up(0)
            time.sleep(1)
            # The row now under the first row's place (the list is 2.2 rows down: row 2's lower part).
            self.i.tap(300, FIRST_ROW_Y + 0.2 * ROW_H)
            joined = until(lambda: "wifi: joining " in journal(since), 5)
            line = next((l for l in journal(since).splitlines() if "wifi: joining " in l), "")
            expected = order[2] if len(order) > 2 else None
            self.check("  a drag scrolls the list and a tap after it acts on the row under it", joined and expected and
                       line.endswith(f"joining {expected}"), f"joined {line!r}, expected {expected!r} of {order}")
            joins = [l for l in journal(since).splitlines() if "wifi: joining " in l]
            self.check("  the drag itself joined nothing", len(joins) == 1, "\n".join(joins))
        finally:
            sh("sudo systemctl stop thor-extra-ap; sudo systemctl reset-failed thor-extra-ap 2>/dev/null; "
               f"for c in {' '.join(extras)}; do sudo nmcli connection delete $c > /dev/null 2>&1; done; "
               "sudo modprobe -r mac80211_hwsim && sudo modprobe mac80211_hwsim; sleep 2; "
               "sudo systemctl start thor-test-ap thor-test-dhcp")
            # The test networks in sight again before the next check joins one.
            until(lambda: "ThorTest" in sh("nmcli -t -f SSID dev wifi list --rescan yes"), 30, 2)

    # shelf

    def shelf_scroll(self):
        """More games than fit in shelf's list: a drag with the pointer scrolls it, settling on a whole row,
        and a click then selects the game under it."""
        control("home")
        until(lambda: front() == "shelf", 5)
        if collection() not in (None, "Test"):
            return self.check("shelf: the Test collection in front", False, f"collection {collection()!r}")
        # Down by three rows and a bit, slowly; it settles on three.
        x, y0 = SHELF_X, SHELF_FIRST_Y + 6 * SHELF_ROW_H
        self.i.pointer(x, y0, "down")
        for step in range(1, 21):
            time.sleep(0.07)
            self.i.pointer(x, y0 - 3.3 * SHELF_ROW_H * step / 20)
        time.sleep(0.5)
        self.i.pointer(x, y0 - 3.3 * SHELF_ROW_H, "up")
        time.sleep(1)
        before = selection()
        self.check("shelf: a drag selects nothing", before is None or before.endswith("test.game"), f"selection {before!r}")
        self.i.pointer(x, SHELF_FIRST_Y, "down")
        time.sleep(0.05)
        self.i.pointer(x, SHELF_FIRST_Y, "up")
        # Rows: test, zeta, zulu01, zulu02 ...: three down, the first row is zulu02.
        self.check("  a drag scrolls the list (whole rows) and a click selects the game under it",
                   until(lambda: (selection() or "").endswith("zulu02.game"), 5), f"selection {selection()!r}")

    # the touchpad tab

    def touchpad(self):
        self.icon("touchpad")
        virtual = device("clamshell touchpad")
        if not self.check("touchpad tab: the virtual touchpad", until(lambda: tab() == "touchpad") and bool(virtual), f"tab {tab()!r}"):
            return

        def tap_then_slide():
            self.i.tap(600, PAD_Y)
            time.sleep(0.5)
            self.i.touch.slide(0, 300, PAD_Y, 900, PAD_Y, 20, 1.0)
            self.i.touch.up(0)
        out = capture(f"sudo libinput debug-events --enable-tap --device {virtual}", 4, tap_then_slide)
        self.check("  a tap below the bar clicks at once", "POINTER_BUTTON" in out, out.strip()[-300:])
        motion = out.count("POINTER_MOTION")
        self.check("  a finger below the bar moves the pointer", motion > 3, f"{motion} motion events; {out.strip()[-300:]!r}")
        self.check("  the touchscreen isn't grabbed", not grabbed(device("QEMU Virtio MultiTouch")))

        def two_fingers_up():
            self.i.touch.down(0, 500, PAD_Y + 200)
            self.i.touch.down(1, 700, PAD_Y + 200)
            for step in range(1, 21):
                time.sleep(0.04)
                self.i.touch.move(0, 500, PAD_Y + 200 - step * 15)
                self.i.touch.move(1, 700, PAD_Y + 200 - step * 15)
            self.i.touch.up(0)
            self.i.touch.up(1)
        out = capture(f"sudo libinput debug-events --enable-tap --device {virtual}", 4, two_fingers_up)
        self.check("  two fingers moving together scroll", "POINTER_SCROLL_FINGER" in out, out.strip()[-300:])
        self.icon("dock")
        self.check("  a finger on the bar still taps the bar (the dock opens)", until(lambda: tab() == "dock"), f"tab {tab()!r}")

    # windows remembered for the bottom screen

    def window_rules(self):
        def where():
            """foot's windows: whether each is below."""
            return [w["down"] for w in windows() if w["app_id"] == "foot"]

        def reopen():
            sh("pkill -x foot || true")
            # (Its launch ended too: a tap on a running entry shows it rather than starting it.)
            until(lambda: not where() and not units("run-"), 8)
            control("tap dock 0")  # foot, pinned by the setup
            until(lambda: bool(where()), 8)
            time.sleep(1)
            return where()

        control("tap dock 0")
        if not self.check("bottom screen: foot open", until(lambda: bool(where()), 8), where()):
            return
        control("window down")
        self.check("  moved below", until(lambda: where() == [True], 5), where())
        down = reopen()
        self.check("  remembered: opened again, straight below, and kept there", down == [True], down)
        control("window up")
        until(lambda: where() == [False], 5)
        up = reopen()
        self.check("  moved up, forgotten: opened again on top", up == [False], up)
        sh("pkill -x foot || true")

    def pad_windows(self):
        if not plug_pad():
            return self.check("window flicks: the fake pad grabbed", False)

        def foot():
            return [(w["id"], w["down"], w["focused"]) for w in windows() if w["app_id"] == "foot"]

        def focused():
            return next((id for id, down, focus in foot() if focus and not down), None)

        def below():
            return [id for id, down, _ in foot() if down]

        def flick(x, y):
            pad("press home", f"stick right {x} {y}", "sleep 120", "stick right 0 0", "release home")
            time.sleep(0.5)

        control("tap dock 0")
        until(lambda: len(foot()) == 1, 8)
        control("long dock 0")
        control("tap dock-menu open-another")
        if not self.check("window flicks: two top windows of one program", until(lambda: len(foot()) == 2, 8), foot()):
            return
        first = focused()
        flick(-1, 0)
        self.check("  Home + right stick left selects the other top tab", focused() not in (None, first), foot())
        flick(0, 1)
        down = below()
        self.check("  a downward flick moves exactly one window below", len(down) == 1 and focused() is not None, foot())
        flick(0, 1)
        self.check("  the next downward flick swaps that bottom slot", len(below()) == 1 and below() != down and focused() in down, foot())
        flick(0, -1)
        self.check("  an upward flick empties the slot and shows its window on top", not below() and len(foot()) == 2, foot())

    # the keyboard's pointing

    def keyboard_points(self):
        # (An app in front: with shelf there, what is typed finds entries instead.)
        if not self._terminal_with_keyboard():
            return
        if not self.check("  its touchpad ready", until(lambda: bool(device("clamshell touchpad")), 3)):
            return
        virtual = device("clamshell touchpad")

        def slide_then_tap():
            self.i.touch.slide(0, 300, PAD_Y, 900, PAD_Y, 20, 1.0)
            self.i.touch.up(0)
            time.sleep(0.3)
            self.i.tap(600, PAD_Y)
        # (libinput's own context taps, as the compositor's does on ClamShell's touchpad.)
        out = capture(f"sudo libinput debug-events --enable-tap --device {virtual}", 4, slide_then_tap)
        motion = out.count("POINTER_MOTION")
        self.check("  a finger sliding over the keys moves the pointer", motion > 3, f"{motion} motion events; {out.strip()[-300:]!r}")
        self.check("  a tap while it points clicks", "POINTER_BUTTON" in out, out.strip()[-300:])
        time.sleep(1.5)
        before = typed()
        self.i.tap(600, PAD_Y)
        self.check("  a moment later a tap types again", until(lambda: typed() > before, 2), f"{before} -> {typed()} characters")

    # the pad

    def pad_shelf(self):
        if not self.check("pad: the fake pad grabbed", plug_pad()):
            return
        focus_shelf()
        until(lambda: (selection() or "").endswith("test.game"), 4)
        before = selection()
        moved = False
        # (The focus may be on the collection's name above the list: then the first press enters it.)
        for _ in range(2):
            dpad("down")
            moved = until(lambda: selection() not in (None, before), 3)
            if moved:
                break
        self.check("pad, shelf in front: the D-pad moves the selection", moved, f"selection {before!r} -> {selection()!r}")
        dpad("up")
        until(lambda: selection() == before, 3)
        press("a")
        launched = until(lambda: bool(units("run-")) and front() not in ("shelf", "window None"), 10)
        game = front()
        self.check("  A launches the selected game, in front", launched, f"units {units('game')}, front {game!r}")
        if not launched:
            return
        press("home")
        self.check("  Home goes to shelf", until(lambda: front() == "shelf"), f"front {front()!r}")
        press("home")
        self.check("  Home again stays on shelf", until(lambda: front() == "shelf"), f"front {front()!r}")
        press("a")
        self.check("  activating the program shows its top window", until(lambda: front() == game), f"front {front()!r}")

    def pad_view(self):
        if not plug_pad():
            return self.check("pad: the fake pad grabbed", False)
        focus_shelf()
        start = tab()
        press("view")  # focus to the second screen
        press("rb")  # the next tab
        changed = until(lambda: tab() not in (None, start), 5)
        self.check("pad: View moves focus to the second screen (a shoulder then steps its tabs)", changed,
                   f"tab {start!r} -> {tab()!r}")
        press("lb")
        press("view")

    def pad_mouse(self):
        if not plug_pad():
            return self.check("pad: the fake pad grabbed", False)
        since = now()
        self.icon("pad-mouse")
        on = logged(since, "PadRoute(Mouse)")
        mouse = device("clamshell pad mouse")
        out = capture(f"evtest {mouse}", 2, lambda: pad("stick left 1 0", "sleep 500", "stick left 0 0"))
        moves = out.count("(REL_X)")
        self.check("pad mouse: on from the bar, the stick moves the pointer", on and moves > 10,
                   f"route logged {on}, {moves} REL_X events")
        self.icon("pad-mouse")
        self.check("  off again", until(lambda: self.route() not in ("Mouse", None)), f"route {self.route()}")

    def _terminal_with_keyboard(self):
        """A terminal in front that writes what is typed into it to TYPED (foot, started outside ClamShell
        and found as the Terminal entry), and the keyboard tab open."""
        since = now()
        sh("pkill -x foot || true")
        until(lambda: not sh("pgrep -x foot || true").strip(), 5)
        sh(f"rm -f {TYPED}; setsid -f foot sh -c 'stty -icanon -echo; exec cat > {TYPED}' >/dev/null 2>&1")
        if not until(lambda: front().startswith("foot_"), 8):
            return self.check("an app in front (Terminal)", False, f"front {front()!r}")
        self.icon("keyboard")
        return self.check("pad with an app in front and the keyboard tab: typing",
                          until(lambda: tab() == "keyboard") and logged(since, "PadRoute(Keys)"), f"tab {tab()!r}")

    def pad_typing(self):
        if not plug_pad():
            return self.check("pad: the fake pad grabbed", False)
        if not self._terminal_with_keyboard():
            return
        mouse = device("clamshell pad mouse")
        out = capture(f"evtest {mouse}", 2, lambda: dpad("up") or dpad("left"))
        self.check("  the D-pad types arrows", "(KEY_UP), value 1" in out and "(KEY_LEFT), value 1" in out,
                   out.strip()[-300:])

    def pad_held_key(self):
        if not plug_pad():
            return self.check("pad: the fake pad grabbed", False)
        if not self._terminal_with_keyboard():
            return
        since = now()
        press("view")
        if not self.check("  View moves the pad to the on-screen keyboard", logged(since, "PadRoute(Ui)")):
            return
        for _ in range(3):  # from the tab's icon (where the tap left the focus) down to a letter
            dpad("down")
        before = typed()
        pad("press a")
        time.sleep(1.2)
        pad("tap view")
        # (What was typed until View moved the pad away, with a moment for it to arrive.)
        time.sleep(0.4)
        at_view = typed()
        time.sleep(1.5)
        pad("release a")
        repeats = at_view - before
        self.check("  A held on a key repeats it", repeats > 5, f"{repeats} characters")
        self.check("  and View, moving the pad away, stops the repeat", repeats > 0 and typed() == at_view,
                   f"{typed() - at_view} characters after View")
        press("view")

    def pad_unplug(self):
        if not plug_pad():
            return self.check("pad: the fake pad grabbed", False)
        since = now()
        control("tap launch")
        launched = until(lambda: bool(units("run-")) and front() not in ("shelf", "window None"), 10)
        game_route = logged(since, "PadRoute(Game)")
        if not self.check("pad: a game in front, the pad goes to it", launched and game_route, f"front {front()!r}"):
            return
        subprocess.run([sys.executable, str(Path(vm.__file__)), "pad", "unplug"], check=True)
        time.sleep(1.5)
        self.check("  unplugging the controller mid-game leaves the virtual pad",
                   bool(device("clamshell pad (Xbox 360)")) and not device("clamshell fake pad"))
        self.check("  plugged in again: grabbed again", plug_pad() and bool(device("clamshell pad (Xbox 360)")))

    def pad_guide(self):
        if not plug_pad():
            return self.check("pad: the fake pad grabbed", False)
        control("tap launch")
        launched = until(lambda: bool(units("run-")) and front() not in ("shelf", "window None"), 10)
        if not self.check("pad: a game in front", launched, f"front {front()!r}"):
            return
        game = front()
        virtual = device("clamshell pad (Xbox 360)")
        out = capture(f"evtest {virtual}", 3, lambda: pad("press home", "sleep 100", "press back", "sleep 400", "release back",
                                                         "sleep 100", "release home"))
        mode = [line for line in out.splitlines() if "BTN_MODE" in line and "value" in line]
        self.check("  Home then Back: the game's Guide, down then up", len(mode) == 2 and "value 1" in mode[0]
                   and "value 0" in mode[1], f"{mode}")
        self.check("  and not ClamShell's Home (the game stays in front), Back not the game's",
                   front() == game and "BTN_BACK" not in out.split("Testing ...")[-1], f"front {front()!r}")
        out = capture(f"evtest {virtual}", 3, lambda: pad("tap ayn 200"))
        mode = [line for line in out.split("Testing ...")[-1].splitlines() if "BTN_MODE" in line and "value" in line]
        self.check("  AYN: the game's Guide, down then up (the game stays in front)", len(mode) == 2 and "value 1" in mode[0]
                   and "value 0" in mode[1] and front() == game, f"{mode}, front {front()!r}")
        out = capture(f"evtest {virtual}", 3, lambda: pad("tap home 900"))
        self.check("  Home held long alone: not Guide (Home on release: shelf in front)",
                   "BTN_MODE" not in out.split("Testing ...")[-1] and until(lambda: front() == "shelf", 3),
                   f"front {front()!r}")
        press("home")
        out = capture(f"evtest {virtual}", 2, lambda: press("home"))
        self.check("  Home pressed: ClamShell's (shelf in front), the game sees nothing",
                   until(lambda: front() == "shelf", 3) and "BTN_MODE" not in out.split("Testing ...")[-1],
                   f"front {front()!r}")

    # keymaps

    def keymaps(self):
        """A keymap made through the screens (the control pipe, and the pad on the edit screen): A to
        Ctrl+X, the right stick to the pointer, the gyro to the pointer while LB is held; then a game with
        it, read from the virtual devices; then Default."""
        if not self.check("keymaps: the fake pad grabbed", plug_pad()):
            return
        self.check("  the first start made the Nintendo layout", keymap_named("Nintendo layout") is not None, saved_keymaps())
        since = now()
        for line in ["tab settings", "tap scope game", "long setting keymap", "tap keymap-new", "type Check", "tap save"]:
            control(line)
        listening = logged(since, "PadRoute(Listen)", 3)
        self.check("  named; the edit screen listens to the pad", listening, journal(since)[-300:])
        press("a")  # selects A on the edit screen
        for line in ["tap map-to key", "tap key ctrl", "tap key x", "tap map-from rstick",
                     "tap map-to pointer", "tap map-from gyro", "tap map-to pointer",
                     "tap keymap-active", "tap gyro-hold lb", "tap map-from x", "tap map-to pick"]:
            control(line)
        press("y")  # picks the pad's Y for X
        control("tap keymap-back")
        # (Its id and its row in the picker, after the game's own keymap's row and Default: other checks may
        # have made keymaps before.)
        ids = [k["id"] for k in saved_keymaps()]
        k = keymap_named("Check")
        ident = k["id"] if k else "?"
        row = ids.index(ident) + 2 if ident in ids else 1
        control(f"tap keymap {row}")
        gyro = bound(k, "Gyro") or {}
        saved = (output(k, {"Button": "A"}) == chord("x", ctrl=True) and output(k, {"Stick": "Right"}) == "Pointer"
                 and gyro.get("output") == "Pointer" and gyro.get("hold") == {"Button": "LB"}
                 and output(k, {"Button": "X"}) == {"Pad": {"Button": "Y"}})
        self.check("  A to Ctrl+X, the right stick and the gyro (while LB) to the pointer, X to Y picked by pressing it, saved",
                   saved, k)
        chosen = until(lambda: any(s.get("id") and s.get("settings", {}).get("keymap") == int(ident) for s in screens()), 3)
        self.check("  chosen for the selected game", chosen, screens())
        # Choosing a map can already apply it to the program in force, before it is shown again.
        # The virtual-device assertions below check the mapping itself; no duplicate command is needed.
        applied_since = since
        since = now()
        control("tap launch")
        launched = until(lambda: bool(units("run-")) and front() not in ("shelf", "window None"), 10)
        if not self.check("  the game runs with it", launched and logged(applied_since, f"pad: keymap {ident} (Check)", 5),
                          f"front {front()!r}; {journal(since)[-300:]!r}"):
            return
        mouse, virtual = device("clamshell pad mouse"), device("clamshell pad (Xbox 360)")
        both = f"sh -c 'evtest {mouse} > /tmp/thor-checks-mouse 2>&1 & evtest {virtual}'"
        out = capture(both, 2, lambda: press("a", 150))
        keys = [line.split("(")[2].split(")")[0] + line.rsplit(" ", 1)[1] for line in sh("cat /tmp/thor-checks-mouse").splitlines()
                if "(EV_KEY)" in line and "value" in line]
        self.check("  A on the pad: Ctrl+X on the virtual keyboard, Ctrl first and last",
                   keys == ["KEY_LEFTCTRL1", "KEY_X1", "KEY_X0", "KEY_LEFTCTRL0"], f"{keys}")
        self.check("  and nothing on the game's pad", "BTN_SOUTH" not in out.split("Testing ...")[-1], out.strip()[-300:])
        out = capture(f"evtest {mouse}", 2, lambda: pad("stick right 1 0", "sleep 600", "stick right 0 0"))
        moves = len([line for line in out.splitlines() if "(REL_X)" in line and "value" in line])
        self.check("  the right stick moves the pointer", moves > 20, f"{moves} REL_X events")
        out = capture(f"evtest {mouse}", 2, lambda: pad("gyro 0 -60 0", "sleep 600", "gyro 0 0 0"))
        moves = [line for line in out.splitlines() if "(REL_X)" in line and "value" in line]
        self.check("  the gyro without LB: nothing", not moves, moves[:5])
        out = capture(f"evtest {mouse}", 2.5, lambda: pad("press lb", "gyro 0 -60 0", "sleep 600", "gyro 0 0 0", "release lb"))
        moved = sum(int(line.rsplit(" ", 1)[1]) for line in out.splitlines() if "(REL_X)" in line and "value" in line)
        self.check("  the gyro with LB held moves the pointer", moved > 100, f"pointer moved {moved} px")
        # The gyro's speed slid up on the edit screen's panel, the game still running: further.
        since = now()
        for line in ["tab settings", "long setting keymap", f"tap keymap-edit {row}", "tap map-from gyro", "slide speed 300 450"]:
            control(line)
        sent = logged(since, f"pad: keymap {ident} (Check)", 3)
        out = capture(f"evtest {mouse}", 2.5, lambda: pad("press lb", "gyro 0 -60 0", "sleep 600", "gyro 0 0 0", "release lb"))
        faster = sum(int(line.rsplit(" ", 1)[1]) for line in out.splitlines() if "(REL_X)" in line and "value" in line)
        speed = (bound(keymap_named("Check"), "Gyro") or {}).get("speed", 0)
        self.check("  the gyro's speed slid up: sent to the pad at once, saved, the pointer goes further",
                   sent and speed >= 90 and faster > moved * 1.5, f"{moved} px, then {faster} px; speed {speed}")
        for line in ["tap keymap-back", "tap keymap-back"]:
            control(line)
        # Default for the game again: everything as on the pad.
        since = now()
        for line in ["tab settings", "long setting keymap", "tap keymap 1"]:
            control(line)
        self.check("  Default chosen, sent to the pad", logged(since, "pad: keymap 0 (Default)", 3), journal(since)[-300:])
        out = capture(both, 2, lambda: press("a", 150))
        mouse_keys = [line for line in sh("cat /tmp/thor-checks-mouse").splitlines() if "(EV_KEY)" in line and "value" in line]
        self.check("  Default: A is the game's A, nothing on the keyboard", "(BTN_SOUTH), value 1" in out and not mouse_keys,
                   f"{out.strip()[-200:]!r}; mouse {mouse_keys}")

    def keymap_layers(self):
        """A keymap with A to Ctrl+X, and to Ctrl+Y in a layer Back turns on while held, made through the
        screens (New layer in the layer list, a name, Back to the layer as a target); in a game: A, Back with
        A, and Back let go while A is held, read from the virtual keyboard; then the layer made Once."""
        if not self.check("keymap layers: the fake pad grabbed", plug_pad()):
            return
        for line in ["tab settings", "tap scope game", "long setting keymap", "tap keymap-new", "type Layers", "tap save",
                     "tap map-from a", "tap map-to key", "tap key ctrl", "tap key x",
                     "tap keymap-layers", "tap new-layer", "type held", "tap save", "tap map-from a", "tap map-to key", "tap key ctrl",
                     "tap key y", "tap keymap-layers", "tap keymap-layer base", "tap map-from back", "tap map-to layers", "tap to-layer 1",
                     "tap keymap-back"]:
            control(line)
        # (The last keymap's row: after the game's own keymap's row and Default.)
        rows = len(saved_keymaps()) + 1
        k = keymap_named("Layers")
        held = next((l for l in (k or {}).get("layers", []) if l["id"] == 1), {})
        layers = (output(k, {"Button": "A"}) == chord("x", ctrl=True) and output(k, {"Button": "Back"}) == {"Layer": 1}
                  and held.get("name") == "held" and output(k, {"Button": "A"}, layer=1) == chord("y", ctrl=True))
        if not self.check("  A to Ctrl+X, and to Ctrl+Y while Back is held, saved", layers, k):
            return
        since = now()
        control(f"tap keymap {rows}")
        control("tap launch")
        launched = until(lambda: bool(units("run-")) and front() not in ("shelf", "window None"), 10)
        if not self.check("  the game runs with it", launched and logged(since, "(Layers)", 5), f"front {front()!r}"):
            return
        game = front()
        mouse = device("clamshell pad mouse")

        def keys(*commands, seconds=2.5):
            out = capture(f"evtest {mouse}", seconds, lambda: pad(*commands))
            return [line.split("(")[2].split(")")[0] + line.rsplit(" ", 1)[1] for line in out.splitlines()
                    if "(EV_KEY)" in line and "value" in line]

        def held(events):
            down = set()
            for e in events:
                (down.add if e.endswith("1") else down.discard)(e[:-1])
            return down

        got = keys("press a", "sleep 150", "release a")
        self.check("  A: Ctrl+X", got == ["KEY_LEFTCTRL1", "KEY_X1", "KEY_X0", "KEY_LEFTCTRL0"], f"{got}")
        got = keys("press back", "sleep 100", "press a", "sleep 150", "release a", "sleep 100", "release back")
        self.check("  Back held, A: Ctrl+Y (Back itself nothing)", got == ["KEY_LEFTCTRL1", "KEY_Y1", "KEY_Y0", "KEY_LEFTCTRL0"], f"{got}")
        got = keys("press back", "sleep 100", "press a", "sleep 150", "release back", "sleep 150", "release a")
        self.check("  Back let go while A is held: Ctrl+Y up, then A's Ctrl+X, nothing left down",
                   "KEY_Y0" in got and "KEY_X1" in got and not held(got), f"{got}")
        # The layer made Once in its menu, the game still running: Back tapped, then A twice: Ctrl+Y, then
        # Ctrl+X; the controller buzzes as the layer is armed and as it goes off.
        for line in ["tab settings", "long setting keymap", f"tap keymap-edit {rows}", "tap keymap-layers", "long keymap-layer 1",
                     "tap layer-once", "tap keymap-back", "tap keymap-back", "tap keymap-back", "tap keymap-back"]:
            control(line)
        k = keymap_named("Layers")
        once = any(l["id"] == 1 and l["once"] for l in (k or {}).get("layers", []))
        self.check("  the layer made Once, saved", once, k)
        since = now()
        got = keys("press back", "sleep 80", "release back", "sleep 150", "press a", "sleep 150", "release a", "sleep 150",
                   "press a", "sleep 150", "release a", seconds=3.0)
        self.check("  Once: Back tapped, then A twice: Ctrl+Y, then Ctrl+X",
                   got == ["KEY_LEFTCTRL1", "KEY_Y1", "KEY_Y0", "KEY_LEFTCTRL0", "KEY_LEFTCTRL1", "KEY_X1", "KEY_X0", "KEY_LEFTCTRL0"], f"{got}")
        said = sh(f"journalctl --user -u {vm.FAKE_PAD_UNIT} {after(since)} -o cat --no-pager | grep rumble || true")
        self.check("  a buzz as it is armed, another as it goes off", "rumble 0 40000" in said and "rumble 30000 0" in said, said)
        self.check("  the game stays in front", front() == game, f"front {front()!r}")
        press("home")
        self.check("  a Home tap: shelf", until(lambda: front() == "shelf", 3), f"front {front()!r}")

    def keymap_actuations(self):
        """A keymap with A to X pressed and to E held, B to Z pressed twice, Y to T with turbo and toggled,
        made through the screens; in a game, read from the virtual keyboard."""
        if not self.check("keymap actuations: the fake pad grabbed", plug_pad()):
            return
        for line in ["tab settings", "tap scope game", "long setting keymap", "tap keymap-new", "type Acts", "tap save",
                     "tap map-from a", "tap map-to key", "tap key x", "tap keymap-act hold", "tap map-to key", "tap key e",
                     "tap map-from b", "tap keymap-act double", "tap map-to key", "tap key z",
                     "tap map-from y", "tap map-to key", "tap key t", "tap toggle", "tap keymap-back"]:
            control(line)
        rows = len(saved_keymaps()) + 1
        k = keymap_named("Acts")
        saved = (output(k, {"Button": "A"}) == chord("x") and output(k, {"Button": "Y"}) == chord("t")
                 and (bound(k, {"Button": "Y"}) or {}).get("toggle") is True
                 and output(k, {"Button": "A"}, "hold") == chord("e") and output(k, {"Button": "B"}, "double") == chord("z"))
        if not self.check("  A: X, held E; B twice: Z; Y: T toggled; saved", saved, k):
            return
        since = now()
        control(f"tap keymap {rows}")
        control("tap launch")
        launched = until(lambda: bool(units("run-")) and front() not in ("shelf", "window None"), 10)
        if not self.check("  the game runs with it", launched and logged(since, "(Acts)", 5), f"front {front()!r}"):
            return
        mouse = device("clamshell pad mouse")
        virtual = device("clamshell pad (Xbox 360)")

        def keys(*commands, seconds=2.5, dev=mouse):
            out = capture(f"evtest {dev}", seconds, lambda: pad(*commands))
            return [line.split("(")[2].split(")")[0] + line.rsplit(" ", 1)[1] for line in out.splitlines()
                    if "(EV_KEY)" in line and "value" in line]

        got = keys("press a", "sleep 100", "release a")
        self.check("  A tapped: X, on letting go", got == ["KEY_X1", "KEY_X0"], f"{got}")
        got = keys("press a", "sleep 700", "release a")
        self.check("  A held: E only, until let go", got == ["KEY_E1", "KEY_E0"], f"{got}")
        got = keys("press b", "sleep 80", "release b", "sleep 100", "press b", "sleep 150", "release b")
        self.check("  B twice: Z, held while the second press is", got == ["KEY_Z1", "KEY_Z0"], f"{got}")
        got = keys("press b", "sleep 80", "release b", seconds=2.0, dev=virtual)
        self.check("  B once: the game's B, after the double's window", got == ["BTN_EAST1", "BTN_EAST0"], f"{got}")
        got = keys("press y", "sleep 80", "release y", "sleep 300")
        self.check("  Y: T on at the press, staying on", got == ["KEY_T1"], f"{got}")
        got = keys("press y", "sleep 80", "release y")
        self.check("  Y again: T off", got == ["KEY_T0"], f"{got}")
        press("home")

    def rumble(self):
        """A game's rumble on the virtual pad reaches the controller: an effect uploaded to the virtual pad,
        played and stopped, read back from the fake pad's own rumble (it says what it is asked to play)."""
        if not self.check("rumble: the fake pad grabbed", plug_pad()):
            return
        control("tap launch")
        if not self.check("  a game in front", until(lambda: bool(units("run-")) and front() not in ("shelf", "window None"), 10)):
            return
        virtual = device("clamshell pad (Xbox 360)")
        # struct ff_effect: type, id, direction, trigger, replay (length 500 ms), then the rumble's
        # strong and weak magnitudes in a 32-byte union at offset 16; EVIOCSFF; then EV_FF play and stop.
        game = (
            "import fcntl,os,struct,sys,time\n"
            "fd=os.open(sys.argv[1],os.O_RDWR)\n"
            "e=bytearray(struct.pack('HhHHHHH',0x50,-1,0,0,0,500,0)+b'\\0'*2+struct.pack('HH',40000,20000)+b'\\0'*28)\n"
            "fcntl.ioctl(fd,0x40304580,e)\n"
            "i=struct.unpack_from('h',e,2)[0]\n"
            "os.write(fd,struct.pack('llHHi',0,0,0x15,i,1));time.sleep(0.3)\n"
            "os.write(fd,struct.pack('llHHi',0,0,0x15,i,0));time.sleep(0.2)\n"
        )
        since = now()
        ran = sh(f"python3 -c {shlex.quote(game)} {virtual} && echo played").strip()
        time.sleep(0.5)
        said = sh(f"journalctl --user -u {vm.FAKE_PAD_UNIT} {after(since)} -o cat --no-pager | grep rumble || true")
        self.check("  the game's effect uploaded and played on the virtual pad", ran.endswith("played"), ran)
        self.check("  the controller rumbles at the game's strengths, then stops",
                   "rumble 40000 20000" in said and said.strip().endswith("rumble stop"), said)
        press("home")

    # process control

    def _launch_test_game(self):
        """The Test tab's first game (test.game, an idle mpv window) launched from shelf; its group,
        once in front (none if it didn't come)."""
        # (Its collection's place depends on which are shown: Recent and Favourites only with entries.)
        for collection in range(4):
            control(f"tap collection {collection}")
            control("list -1000")
            if until(lambda: (selection() or "").endswith("test.game"), 2):
                break
        control("tap launch")
        if not until(lambda: not front().startswith(("shelf", "window ")), 10):
            self.check("process control: the test game launched, in front", False, f"front {front()!r}, units {units('run-')}")
            return None
        return front()

    def launch_and_quit(self):
        launches = lambda: [u for u in units("run-") if u.endswith(".service")]
        before = plays()
        if not self._launch_test_game():
            return
        self.check("process control: a game launched from shelf runs as a service of its own, in front", bool(launches()), f"units {units('run-')}")
        time.sleep(2)
        control("window quit")
        self.check("  Quit ends it (its service gone)", until(lambda: not launches(), 10), f"units {units('run-')}")
        self.check("  one play recorded", until(lambda: plays() == before + 1, 5), f"plays {before} -> {plays()}")

    def pause_hidden(self):
        """A game, with no setting (Pauses games, the default): frozen once hidden for the grace (10 s,
        then a 2 s look at whether it works without drawing), thawed when shown again."""
        game = self._launch_test_game()
        if not game:
            return
        slice_ = f"app-graphical-clamshell-run-{game}.slice"
        state = lambda: sh(f"systemctl --user show {slice_} -p FreezerState --value").strip()
        time.sleep(1)
        control("home")
        self.check("pause: a game behind shelf is frozen after the grace", until(lambda: state() == "frozen", 20), f"{slice_}: {state()!r}")
        control("tap launch")
        self.check("  thawed when shown again", until(lambda: state() == "running" and front() == game, 5), f"{state()!r}, front {front()!r}")
        control("window quit")
        until(lambda: not [u for u in units("run-") if u.endswith((".service", ".scope"))], 10)

    def _foot_outside(self, how):
        """foot started outside ClamShell (`how`: a command starting it): whether it came in front in its
        entry's group, and a reader of its process's cgroup."""
        sh("pkill -x foot || true")
        # (Gone, its entry ended, shelf in front: a window opening in an entry's group is that entry's.)
        until(lambda: not sh("pgrep -x foot || true").strip() and not [u for u in units("run-foot") if u.endswith((".service", ".scope"))]
              and front() == "shelf", 10)
        sh(how)
        on = until(lambda: front().startswith("foot_"), 10)
        pid = sh("pgrep -n -x foot || true").strip()
        return on, lambda: sh(f"cat /proc/{pid}/cgroup 2>/dev/null").strip() if pid else ""

    def found_window(self):
        """A window started outside ClamShell: its entry's (foot's desktop file), in its group, in
        front, its process moved into a scope of ours: from a desktop's app scope (as a launcher starts
        one), and from a login session's (as from a terminal over ssh)."""
        adopt = lambda since: [line for line in journal(since).splitlines() if "adopt" in line][-3:]
        since = now()
        on, cgroup = self._foot_outside("setsid -f systemd-run --user --scope --quiet --collect --unit=app-thor-checks-foot.scope foot >/dev/null 2>&1")
        self.check("found: a window started outside ClamShell (an app scope) is its entry's, in its group, in front", on, f"front {front()!r}")
        self.check("  its process moved into a scope of ours", until(lambda: "clamshell-run-foot_" in cgroup(), 5), f"{cgroup()!r}; {adopt(since)}")
        since = now()
        on, cgroup = self._foot_outside("setsid -f foot >/dev/null 2>&1")
        self.check("  one started in a login session too, in its group, in front", on, f"front {front()!r}")
        self.check("  its process moved into a scope of ours", until(lambda: "clamshell-run-foot_" in cgroup(), 5), f"{cgroup()!r}; {adopt(since)}")
        sh("pkill -x foot || true")

    def confirm_key(self):
        """The first password's confirmation (`clamshell device confirm`, run as root by thor-account):
        Volume Up on a physical device ends it at once; with no press it gives up after 20 s."""
        sh(f"sudo rm -f /tmp/confirm.status; (sudo {vm.A.program} device confirm; echo $? > /tmp/confirm.status) > /dev/null 2>&1 &")
        time.sleep(1.5)
        self.i.qmp.command("send-key", keys=[{"type": "qcode", "data": "volumeup"}])
        done = until(lambda: sh("cat /tmp/confirm.status 2>/dev/null || true").strip() != "", 5)
        self.check("confirm: Volume Up on the virtio keyboard confirms", done and sh("cat /tmp/confirm.status").strip() == "0",
                   sh("cat /tmp/confirm.status 2>/dev/null || true").strip())
        status = sh(f"sudo {vm.A.program} device confirm > /dev/null 2>&1; echo $?").strip()
        self.check("  with no press it fails", status == "1", status)

    def session_units(self):
        """The session as systemd sees it: the displays given to the user manager for its services, the
        status line, and the compositor's own tapping on ClamShell's touchpad."""
        environment = sh("systemctl --user show-environment")
        self.check("session: the user manager has the displays", "WAYLAND_DISPLAY=" in environment and "\nDISPLAY=" in "\n" + environment,
                   environment.strip()[-200:])
        status = sh("systemctl --user show -P StatusText clamshell").strip()
        self.check("  systemctl status says what runs", "running" in status and "windows" in status, repr(status))
        self.check("  the compositor taps and drags on ClamShell's touchpad", "ClamShell's touchpad taps and drags" in sh("journalctl --user -b -u clamshell -o cat --no-pager"))

    def tray_program(self):
        """A program with a tray icon, started outside ClamShell (an app scope): an entry found by it, its
        process moved into a scope of ours."""
        if "ok" not in sh("python3 -c 'import gi; print(\"ok\")' 2>/dev/null"):
            return self.check("tray: python-gobject in the machine (vm.py's packages)", False, "not installed")
        since = now()
        sh("setsid -f systemd-run --user --scope --quiet --collect --unit=app-thor-checks-tray.scope python3 -c "
           f"{shlex.quote(TRAY_ITEM)} >/dev/null 2>&1")
        # (The pattern can't match this command itself.)
        pid = lambda: sh("pgrep -n -f 'thor-checks-tray-ite[m]' || true").strip()
        cgroup = lambda: sh(f"cat /proc/{pid()}/cgroup 2>/dev/null").strip() if pid() else ""
        self.check("tray: a program's icon registered", logged(since, "Tray test", 10), journal(since).strip()[-300:])
        self.check("  its process moved into a scope of ours", until(lambda: "clamshell-run-" in cgroup(), 8),
                   f"{cgroup()!r}; {[line for line in journal(since).splitlines() if 'adopt' in line][-3:]}")
        sh("pkill -f thor-checks-tray-item || true")

    def clipboard(self):
        """Text set by a program that then quits is still on the clipboard (ClamShell offers it)."""
        program = vm.A.program
        self.check("clipboard: kept by ClamShell", "clipboard: kept by ClamShell" in journal(self.started), "")
        sh(f"printf kept-by-clamshell | timeout 10 {program} --clipboard set")
        got = sh(f"timeout 5 {program} --clipboard get").strip()
        self.check("  a text set by a program that quit can still be pasted", got == "kept-by-clamshell", f"got {got!r}")

    # Bluetooth

    def gyro_sleep(self):
        """Sleep closes the gyro and waking finds it again (ClamShell's delay lock, so the Thor's sensor hub
        doesn't sample while asleep): a suspend and a wake, read from ClamShell's log. Last of all: QEMU
        sometimes resets the machine a few minutes after a resume, which would cut off the checks after it."""
        if not self.check("gyro over sleep: the fake pad grabbed", plug_pad()):
            return
        since = now()
        sh("sudo -n systemctl suspend")
        time.sleep(4)
        vm.wake(None)
        closed = logged(since, "pad: the gyro closed for sleep", 20)
        found = logged(since, "pad: gyro from clamshell fake pad Motion Sensors", 20)
        self.check("  the gyro closed before sleep, found again after it", closed and found, journal(since)[-400:])

    def bluetooth(self):
        if "ready" not in sh("cat /run/thor-bt-device 2>/dev/null"):
            return self.check("Bluetooth: the test device (tools/vm/vm.py bluetooth)", False, "not set up")
        address = vm.BT_DEVICE
        self.open_tab("bluetooth")
        shown = until(lambda: "ThorTestDevice" in sh("bluetoothctl devices"), 20)
        time.sleep(1.5)
        self.check("Bluetooth: the test device is found", shown)
        if not shown:
            return
        since = now()
        self.i.tap(300, FIRST_ROW_Y)
        connected = logged(since, f"bluetooth: connected {address}", 15)
        info = sh(f"bluetoothctl info {address}")
        self.check("  a tap pairs and connects it", connected and "Paired: yes" in info and "Connected: yes" in info,
                   " ".join(line.strip() for line in info.splitlines() if "Paired" in line or "Connected" in line)
                   + f"; {journal(since).strip()[-300:]!r}")
        since = now()
        self.i.hold(300, FIRST_ROW_Y, 0.8)
        forgot = logged(since, f"bluetooth: forgot {address}", 5)
        self.check("  a long press forgets it", forgot and "Paired: yes" not in sh(f"bluetoothctl info {address}"))


# The tray check's program: a StatusNotifierItem (named in its first line, to be found by).
TRAY_ITEM = """# thor-checks-tray-item
from gi.repository import Gio, GLib
XML = ('<node><interface name="org.kde.StatusNotifierItem">'
       '<property name="Id" type="s" access="read"/><property name="Title" type="s" access="read"/>'
       '<property name="Status" type="s" access="read"/><property name="IconName" type="s" access="read"/>'
       '<property name="Category" type="s" access="read"/></interface></node>')
PROPS = {"Id": "thor-checks-tray", "Title": "Tray test", "Status": "Active", "IconName": "folder", "Category": "ApplicationStatus"}
bus = Gio.bus_get_sync(Gio.BusType.SESSION)
info = Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0]
bus.register_object("/StatusNotifierItem", info, None, lambda c, s, p, i, prop: GLib.Variant("s", PROPS[prop]), None)
bus.call_sync("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
              "RegisterStatusNotifierItem", GLib.Variant("(s)", (bus.get_unique_name(),)), None, 0, -1, None)
GLib.MainLoop().run()
"""



CHECKS = ["confirm_key", "session_units", "shelf_scroll", "bar_tap", "time_zone", "volume_hold", "volume_slide", "brightness_slide", "two_fingers", "wifi_forget", "wifi_scroll", "touchpad", "keyboard_points", "window_rules", "pad_windows",
          "pad_shelf", "pad_view", "pad_mouse", "pad_typing", "pad_held_key", "pad_unplug", "pad_guide", "keymaps", "keymap_layers",
          "keymap_actuations", "rumble", "launch_and_quit", "pause_hidden", "found_window", "tray_program", "clipboard",
          "bluetooth", "gyro_sleep"]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("checks", nargs="*", metavar="CHECK")
    parser.add_argument("--list", action="store_true", help="the checks' names")
    parser.add_argument("--keep", action="store_true", help="leave ClamShell in its test environment afterwards")
    args = parser.parse_args()
    if args.list:
        print("\n".join(CHECKS))
        return 0
    chosen = args.checks or CHECKS
    unknown = [c for c in chosen if c not in CHECKS]
    if unknown:
        parser.error(f"no check {', '.join(unknown)} (--list)")
    inputs = Inputs()
    started = setup(bluetooth="bluetooth" in chosen)
    checks = Checks(inputs, started)
    try:
        for name in chosen:
            print(name)
            checks.reset()
            try:
                getattr(checks, name)()
            except Exception as e:  # a check that broke counts as failed; the others still run
                checks.check(f"{name}: ran", False, repr(e))
    finally:
        inputs.lift_all()
        if not args.keep:
            teardown()
    print(f"{checks.passed} ok, {checks.failed} failed")
    return checks.failed


if __name__ == "__main__":
    sys.exit(main())
