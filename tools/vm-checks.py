#!/usr/bin/env python3
"""Checks of ClamShell in the virtual Thor through its real inputs: touches on the bottom screen's
touchscreen (QEMU's virtio-multitouch, driven over QMP), a fake controller (the fake_pad example), and
two virtual Bluetooth controllers (btvirt) with a test device. Each check acts as a user would and looks
at what came of it: ClamShell's journal, its saved files, sway's workspaces, the devices it makes, the
system (NetworkManager, BlueZ, PipeWire, a fake backlight), screenshots.

    tools/vm.py run --headless &          (the machine running; ClamShell built on the host)
    tools/vm-checks.py [CHECK ...]        (all of them by default; --list names them)

For the checks ClamShell runs with a test library (a game that opens foot), a fake backlight and its own
state directory (a runtime drop-in for its service, removed at the end), PipeWire gets a null sink (the
machine has no audio device), and Bluetooth is set up (tools/vm.py bluetooth). The exit status is the
number of failed checks. Standard library only.
"""

import argparse
import json
import shlex
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import importlib  # noqa: E402

vm = importlib.import_module("vm")

HOME = f"/home/{vm.USER}"
CHECKS_DIR = f"{HOME}/.cache/thor-checks"
STATE = f"{CHECKS_DIR}/state/clamshell"
BACKLIGHT = f"{CHECKS_DIR}/backlight/panel"
DROP_IN = "$XDG_RUNTIME_DIR/systemd/user/clamshell.service.d/thor-checks.conf"
# The bar's icons, in the bottom screen's pixels (their centres; the burn-in orbit moves them a few
# pixels).
BAR_Y = 45
ICONS = {"settings": 55, "dock": 157, "keyboard": 260, "touchpad": 363, "sysinfo": 466, "volume": 568,
         "brightness": 671, "bluetooth": 774, "wifi": 877, "battery": 975, "pad-mouse": 1188}
# A list's first row (Wi-Fi's networks, Bluetooth's devices), and a row's height.
FIRST_ROW_Y = 240
ROW_H = 103
# shelf's list on the main screen: where its first row's middle is, and a row's height (1920x1080).
SHELF_X, SHELF_FIRST_Y, SHELF_ROW_H = 300, 150, 94.6
# Below the bar: the touchpad's area in touchpad mode.
PAD_Y = 600

PRELUDE = ("export SWAYSOCK=$(ls /run/user/$(id -u)/sway-ipc.*.sock 2>/dev/null | head -1) WAYLAND_DISPLAY=wayland-1; "
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
    """The machine's clock, to read the journal from."""
    return sh("date +%s.%N").strip()


def journal(since):
    return sh(f"journalctl --user -u clamshell --since @{since} -o cat --no-pager")


def logged(since, text, seconds=5.0):
    return until(lambda: text in journal(since), seconds)


def control(line):
    sh(f"echo {shlex.quote(line)} > $XDG_RUNTIME_DIR/clamshell-control")
    time.sleep(0.3)


def saved():
    """ClamShell's resume file: the tab ("@mode") and shelf's selections, by name."""
    lines = sh(f"cat {STATE}/last 2>/dev/null").splitlines()
    return dict(line.split("\t", 1) for line in lines[1:] if "\t" in line)


def selection():
    """shelf's selected game in the collection shown."""
    lines = sh(f"cat {STATE}/last 2>/dev/null").splitlines()
    return saved().get(lines[0]) if lines else None


def tab():
    return saved().get("@mode")


def front():
    return sh("swaymsg -t get_workspaces | python3 -c "
              "'import json,sys; print(next((w[\"name\"] for w in json.load(sys.stdin) if w[\"focused\"]), \"\"))'").strip()


def units(prefix):
    return sh(f"systemctl --user list-units 'clamshell-{prefix}*' --plain --no-legend | awk '{{print $1}}'").split()


def screenshot(output="Virtual-2"):
    """A screen as (width, height, RGB bytes)."""
    data = sh(f"grim -t ppm -o {output} -", binary=True)
    parts = data.split(maxsplit=4)
    if len(parts) < 5 or parts[0] != b"P6":
        raise RuntimeError("grim: no picture")
    return int(parts[1]), int(parts[2]), parts[4]


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
    until(lambda: not sh("pgrep -f 'timeout .*thor-checks-capture' || true").strip(), 3)
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
    """Commands for the fake pad (tools/vm.py pad do)."""
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

LIBRARY = """collection: Test
extension: game
launch: sh -c "foot --app-id=testgame sleep 300 & sleep 300"

game: Test Game
file: test.game

game: Zeta Game
file: zeta.game
""" + "".join(f"\ngame: Zulu {i:02}\nfile: zulu{i:02}.game\n" for i in range(1, 15))


def setup(bluetooth):
    print("setting up: test library, fake backlight, null sink, ClamShell's test environment")
    since = now()
    script = f"""set -e
mkdir -p {CHECKS_DIR}/pegasus {CHECKS_DIR}/games {BACKLIGHT} {STATE}
echo {CHECKS_DIR}/games > {CHECKS_DIR}/pegasus/game_dirs.txt
cat > {CHECKS_DIR}/games/metadata.pegasus.txt <<'EOF'
{LIBRARY}EOF
touch {CHECKS_DIR}/games/test.game {CHECKS_DIR}/games/zeta.game {" ".join(f"{CHECKS_DIR}/games/zulu{i:02}.game" for i in range(1, 15))}
echo 100 > {BACKLIGHT}/max_brightness; echo 50 > {BACKLIGHT}/brightness
rm -f {STATE}/last {STATE}/brightness {CHECKS_DIR}/pegasus/stats.db* {CHECKS_DIR}/pegasus/favorites.txt \\
    {CHECKS_DIR}/pegasus/keymaps.txt {CHECKS_DIR}/pegasus/game-settings.txt
mkdir -p $(dirname {DROP_IN})
printf '%s\\n' '[Service]' 'Environment=CLAMSHELL_PEGASUS_CONFIG={CHECKS_DIR}/pegasus' \\
    'Environment=CLAMSHELL_BACKLIGHTS={CHECKS_DIR}/backlight' 'Environment=XDG_STATE_HOME={CHECKS_DIR}/state' > {DROP_IN}
wpctl status | grep -q thor-checks-sink || wpctl status | grep -q 'Test sink' || pw-cli create-node adapter \\
    '{{ factory.name=support.null-audio-sink node.name=thor-checks-sink node.description="Test sink" media.class=Audio/Sink object.linger=true audio.position=[FL FR] }}' > /dev/null 2>&1
sleep 0.5
wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.5 2>/dev/null
systemctl --user stop 'clamshell-game*' 'clamshell-app*' 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user restart clamshell
"""
    sh("bash -s", input=script, check=True)
    if bluetooth:
        result = subprocess.run([sys.executable, str(Path(vm.__file__)), "bluetooth"], capture_output=True, text=True)
        if result.returncode != 0:
            print(f"  (Bluetooth not set up: {result.stderr.strip() or result.stdout.strip()})")
    if not until(lambda: sh("systemctl --user is-active clamshell").strip() == "active" and front() == "clamshell-shelf", 15):
        sys.exit("ClamShell didn't start")
    time.sleep(1)
    return since


def teardown():
    subprocess.run([sys.executable, str(Path(vm.__file__)), "pad", "unplug"], capture_output=True)
    sh(f"rm -f {DROP_IN}; systemctl --user stop 'clamshell-game*' 'clamshell-app*' 2>/dev/null; "
       "systemctl --user daemon-reload; systemctl --user restart clamshell")


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
        """A tab open, by a tap on its icon (and the dock first if it is open already: a tap on the open
        tab's icon may do something else)."""
        if tab() == name:
            return True
        self.icon(name)
        return until(lambda: tab() == name, 5)

    def route(self):
        """Where the pad goes now (the last route ClamShell logged since the checks started it)."""
        routes = [line for line in journal(self.started).splitlines() if "effect: PadRoute(" in line]
        return routes[-1].split("PadRoute(")[1].rstrip(")") if routes else None

    def reset(self):
        """A known start: no fingers down, no game or app, shelf in front, the dock tab, the pad mouse
        off."""
        self.i.lift_all()
        sh("systemctl --user stop 'clamshell-game*' 'clamshell-app*' 2>/dev/null || true")
        until(lambda: front() == "clamshell-shelf", 5)
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

    def volume_hold(self):
        before = light_pixels(screenshot(), BAR_Y, 20, 600)
        since = now()
        self.i.touch.down(0, ICONS["volume"], BAR_Y)
        time.sleep(0.5)
        during = light_pixels(screenshot(), BAR_Y, 20, 600)
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
        self.check("  and is saved", until(lambda: sh(f"cat {STATE}/brightness 2>/dev/null").strip() != "", 5),
                   sh(f"ls {STATE}"))
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
            joined = until(lambda: "wifi: join" in journal(since), 5)
            line = next((l for l in journal(since).splitlines() if "wifi: join" in l), "")
            expected = order[2] if len(order) > 2 else None
            self.check("  a drag scrolls the list and a tap after it acts on the row under it", joined and expected and
                       line.endswith(f"join {expected}"), f"joined {line!r}, expected {expected!r} of {order}")
            self.check("  the drag itself joined nothing", journal(since).count("wifi: join ") == 1, journal(since)[-300:])
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
        sh("swaymsg workspace clamshell-shelf > /dev/null")
        lines = sh(f"cat {STATE}/last 2>/dev/null").splitlines()
        if lines and lines[0] != "Test":
            return self.check("shelf: the Test collection in front", False, f"collection {lines[0]!r}")
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
        # Rows: Test Game, Zeta Game, Zulu 01, Zulu 02 ...: three down, the first row is Zulu 02.
        self.check("  a drag scrolls the list (whole rows) and a click selects the game under it",
                   until(lambda: (selection() or "").endswith("zulu02.game"), 5), f"selection {selection()!r}")

    # the touchpad tab

    def touchpad(self):
        since = now()
        self.icon("touchpad")
        on = logged(since, "touchpad mode on")
        touchscreen = device("QEMU Virtio MultiTouch")
        self.check("touchpad tab: touchpad mode on, the touchscreen grabbed", on and grabbed(touchscreen),
                   f"logged {on}, {touchscreen} grabbed {grabbed(touchscreen)}")
        virtual = device("clamshell touchpad")
        if not self.check("  a virtual touchpad", bool(virtual)):
            return
        out = capture(f"sudo libinput debug-events --device {virtual}", 3,
                      lambda: self.i.touch.slide(0, 300, PAD_Y, 900, PAD_Y, 20, 1.0) or self.i.touch.up(0))
        motion = out.count("POINTER_MOTION")
        self.check("  a finger below the bar moves the pointer (libinput: pointer motion)", motion > 3,
                   f"{motion} motion events; {out.strip()[-300:]!r}")
        since = now()
        self.icon("dock")
        self.check("  a finger on the bar still taps the bar (the dock opens, touchpad mode off)",
                   until(lambda: tab() == "dock") and logged(since, "touchpad mode off"), f"tab {tab()!r}")
        self.check("  the touchscreen let go", not grabbed(touchscreen))

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
        launched = until(lambda: bool(units("game")) and front() not in ("", "clamshell-shelf"), 10)
        game = front()
        self.check("  A launches the selected game, in front", launched, f"units {units('game')}, front {game!r}")
        if not launched:
            return
        press("home")
        self.check("  Home goes to shelf", until(lambda: front() == "clamshell-shelf"), f"front {front()!r}")
        press("home")
        self.check("  and Home again back to the game", until(lambda: front() == game), f"front {front()!r}")

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
        since = now()
        control("tap app 1")  # Terminal (foot), the default dock's second app
        if not until(lambda: front().startswith("clamshell-app-terminal"), 8):
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
        keyboard = device("clamshell keyboard")

        def held_then_view():
            pad("press a")
            time.sleep(1.2)
            pad("tap view")
        out = capture(f"evtest {keyboard}", 4, held_then_view)
        pad("release a")
        events = [line for line in out.splitlines() if "type 1 (EV_KEY)" in line and "value 1" in line]
        times = [float(line.split()[2].rstrip(",")) for line in events]
        repeats = len(times)
        # After View (1.2 s after A went down, give or take): no more presses.
        late = [t for t in times if times and t > times[0] + 1.6]
        self.check("  A held on a key repeats it", repeats > 5, f"{repeats} presses")
        self.check("  and View, moving the pad away, stops the repeat", repeats > 0 and not late,
                   f"{len(late)} presses after View")
        press("view")

    def pad_unplug(self):
        if not plug_pad():
            return self.check("pad: the fake pad grabbed", False)
        since = now()
        control("tap launch")
        launched = until(lambda: bool(units("game")) and front() not in ("", "clamshell-shelf"), 10)
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
        launched = until(lambda: bool(units("game")) and front() not in ("", "clamshell-shelf"), 10)
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
                   front() == game and "KEY_BACK" not in out.split("Testing ...")[-1], f"front {front()!r}")
        out = capture(f"evtest {virtual}", 3, lambda: pad("tap home 900"))
        self.check("  Home held long alone: not Guide (Home on release: shelf in front)",
                   "BTN_MODE" not in out.split("Testing ...")[-1] and until(lambda: front() == "clamshell-shelf", 3),
                   f"front {front()!r}")
        press("home")
        out = capture(f"evtest {virtual}", 2, lambda: press("home"))
        self.check("  Home pressed: ClamShell's (shelf in front), the game sees nothing",
                   until(lambda: front() == "clamshell-shelf", 3) and "BTN_MODE" not in out.split("Testing ...")[-1],
                   f"front {front()!r}")

    # keymaps

    def keymaps(self):
        """A keymap made through the screens (the control pipe, and the pad on the edit screen): A to
        Ctrl+X, the right stick to the pointer, the gyro to the pointer while LB is held; then a game with
        it, read from the virtual devices; then Default."""
        if not self.check("keymaps: the fake pad grabbed", plug_pad()):
            return
        keymaps_file = f"{CHECKS_DIR}/pegasus/keymaps.txt"
        self.check("  the first start made the Nintendo layout", "keymap 1 Nintendo layout" in sh(f"cat {keymaps_file}"),
                   sh(f"cat {keymaps_file}"))
        since = now()
        for line in ["tab settings", "scope game", "tap setting keymap", "tap keymap-new", "type Check", "tap save"]:
            control(line)
        listening = logged(since, "PadRoute(Listen)", 3)
        self.check("  named; the edit screen listens to the pad", listening, journal(since)[-300:])
        press("a")  # selects A on the edit screen
        for line in ["tap map-to key", "tap key ctrl", "tap key x", "tap map-from rstick",
                     "tap map-to pointer", "tap map-from gyro", "tap map-to pointer",
                     "tap keymap-active", "tap gyro-hold lb", "tap map-from x", "tap map-to pick"]:
            control(line)
        press("y")  # picks the pad's Y for X
        for line in ["tap keymap-back", "tap keymap 2"]:
            control(line)
        text = sh(f"cat {keymaps_file}")
        saved = ("keymap 2 Check" in text and "a = key ctrl+x" in text and "rstick = pointer" in text
                 and "gyro = pointer hold=lb" in text and "x = pad y" in text)
        self.check("  A to Ctrl+X, the right stick and the gyro (while LB) to the pointer, X to Y picked by pressing it, saved",
                   saved, text)
        chosen = until(lambda: "keymap=2" in sh(f"cat {CHECKS_DIR}/pegasus/game-settings.txt 2>/dev/null"), 3)
        self.check("  chosen for the selected game", chosen, sh(f"cat {CHECKS_DIR}/pegasus/game-settings.txt 2>/dev/null"))
        since = now()
        control("tap launch")
        launched = until(lambda: bool(units("game")) and front() not in ("", "clamshell-shelf"), 10)
        if not self.check("  the game runs with it", launched and logged(since, "pad: keymap 2 (Check)", 5),
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
        for line in ["tab settings", "tap setting keymap", "tap keymap-edit 2", "tap map-from gyro", "slide speed 300 450"]:
            control(line)
        sent = logged(since, "pad: keymap 2 (Check)", 3)
        out = capture(f"evtest {mouse}", 2.5, lambda: pad("press lb", "gyro 0 -60 0", "sleep 600", "gyro 0 0 0", "release lb"))
        faster = sum(int(line.rsplit(" ", 1)[1]) for line in out.splitlines() if "(REL_X)" in line and "value" in line)
        text = sh(f"cat {keymaps_file}")
        self.check("  the gyro's speed slid up: sent to the pad at once, saved, the pointer goes further",
                   sent and "gyro = pointer speed=9" in text and faster > moved * 1.5, f"{moved} px, then {faster} px; {text}")
        for line in ["tap keymap-back", "tap keymap-back"]:
            control(line)
        # Default for the game again: everything as on the pad.
        since = now()
        for line in ["tab settings", "tap setting keymap", "tap keymap 0"]:
            control(line)
        self.check("  Default chosen, sent to the pad", logged(since, "pad: keymap 0 (Default)", 3), journal(since)[-300:])
        out = capture(both, 2, lambda: press("a", 150))
        mouse_keys = [line for line in sh("cat /tmp/thor-checks-mouse").splitlines() if "(EV_KEY)" in line and "value" in line]
        self.check("  Default: A is the game's A, nothing on the keyboard", "(BTN_SOUTH), value 1" in out and not mouse_keys,
                   f"{out.strip()[-200:]!r}; mouse {mouse_keys}")

    def keymap_layers(self):
        """A keymap with A to Ctrl+X, and to Ctrl+Y in the Back layer, made through the screens; in a game:
        A, Back with A, and Back let go while A is held, read from the virtual keyboard."""
        if not self.check("keymap layers: the fake pad grabbed", plug_pad()):
            return
        keymaps_file = f"{CHECKS_DIR}/pegasus/keymaps.txt"
        for line in ["tab settings", "scope game", "tap setting keymap", "tap keymap-new", "type Layers", "tap save",
                     "tap map-from a", "tap map-to key", "tap key ctrl", "tap key x",
                     "tap keymap-layer back", "tap map-to key", "tap key ctrl", "tap key y",
                     "tap keymap-back"]:
            control(line)
        text = sh(f"cat {keymaps_file}")
        rows = text.count("keymap ")
        layers = "keymap " in text and "a = key ctrl+x\nlayer back\na = key ctrl+y\n" in text
        if not self.check("  A to Ctrl+X, and to Ctrl+Y while Back is held, saved", layers, text):
            return
        since = now()
        control(f"tap keymap {rows}")
        control("tap launch")
        launched = until(lambda: bool(units("game")) and front() not in ("", "clamshell-shelf"), 10)
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
        self.check("  the game stays in front", front() == game, f"front {front()!r}")
        press("home")
        self.check("  a Home tap: shelf", until(lambda: front() == "clamshell-shelf", 3), f"front {front()!r}")

    # Bluetooth

    def bluetooth(self):
        if "ready" not in sh("cat /run/thor-bt-device 2>/dev/null"):
            return self.check("Bluetooth: the test device (tools/vm.py bluetooth)", False, "not set up")
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


CHECKS = ["shelf_scroll", "bar_tap", "volume_hold", "volume_slide", "brightness_slide", "two_fingers", "wifi_forget", "wifi_scroll", "touchpad",
          "pad_shelf", "pad_view", "pad_mouse", "pad_typing", "pad_held_key", "pad_unplug", "pad_guide", "keymaps", "keymap_layers", "bluetooth"]


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
