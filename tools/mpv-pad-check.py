#!/usr/bin/env python3
"""mpv's pad bindings (thor-session's input.conf), with a virtual Xbox 360 pad (as ClamShell's virtual
pad): a test video (three chapters, two audio and two subtitle tracks, silent) in a headless sway, mpv
muted with its own watch-later folder; each button pressed, mpv's state read over IPC before and after.
Exit status: the number of failed checks. Needs python-evdev and write access to /dev/uinput.

    tools/mpv-pad-check.py
"""
import json, os, shutil, socket, subprocess, sys, tempfile, time
from evdev import UInput, ecodes as e, AbsInfo

D = tempfile.mkdtemp(prefix="mpv-pad-check.")
CONFIG = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "packages", "thor-session", "root", "usr", "share", "thor", "home", ".config", "mpv")
VIDEO = f"{D}/test.mkv"
SOCK = f"{D}/mpv.sock"
WL = f"{D}/watch_later"


def make_video():
    if os.path.exists(VIDEO):
        return
    with open(f"{D}/chapters.txt", "w") as f:
        f.write(";FFMETADATA1\n")
        for i, (a, b) in enumerate([(0, 60), (60, 120), (120, 180)]):
            f.write(f"[CHAPTER]\nTIMEBASE=1/1\nSTART={a}\nEND={b}\ntitle=Part {i + 1}\n")
    for n in (1, 2):
        with open(f"{D}/sub{n}.srt", "w") as f:
            f.write(f"1\n00:00:00,000 --> 00:03:00,000\nSubtitle {n}\n")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=180",
                    "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo",
                    "-i", f"{D}/sub1.srt", "-i", f"{D}/sub2.srt", "-i", f"{D}/chapters.txt",
                    "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3", "-map", "4", "-map_metadata", "5",
                    "-t", "180", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-c:s", "srt",
                    "-metadata:s:a:0", "language=eng", "-metadata:s:a:1", "language=jpn", VIDEO], check=True)


def pad():
    stick = AbsInfo(0, -32768, 32767, 16, 128, 0)
    trigger = AbsInfo(0, 0, 255, 0, 0, 0)
    hat = AbsInfo(0, -1, 1, 0, 0, 0)
    caps = {
        e.EV_KEY: [e.BTN_SOUTH, e.BTN_EAST, e.BTN_NORTH, e.BTN_WEST, e.BTN_TL, e.BTN_TR, e.BTN_SELECT, e.BTN_START,
                   e.BTN_MODE, e.BTN_THUMBL, e.BTN_THUMBR],
        e.EV_ABS: [(e.ABS_X, stick), (e.ABS_Y, stick), (e.ABS_RX, stick), (e.ABS_RY, stick), (e.ABS_Z, trigger),
                   (e.ABS_RZ, trigger), (e.ABS_HAT0X, hat), (e.ABS_HAT0Y, hat)],
    }
    return UInput(caps, name="Microsoft X-Box 360 pad", vendor=0x045E, product=0x028E, version=0x110, bustype=e.BUS_USB)


def ipc(*command):
    s = socket.socket(socket.AF_UNIX)
    s.connect(SOCK)
    s.sendall((json.dumps({"command": list(command)}) + "\n").encode())
    data = b""
    while b"\n" not in data:
        data += s.recv(4096)
    s.close()
    for line in data.decode().splitlines():
        reply = json.loads(line)
        if "error" in reply:
            return reply.get("data")


def get(prop):
    return ipc("get_property", prop)


def press(ui, kind, code, value=1):
    """A button (EV_KEY), a trigger (to 255 and back) or the d-pad (a hat axis, to value and back)."""
    if kind == "key":
        ui.write(e.EV_KEY, code, 1); ui.syn(); time.sleep(0.08)
        ui.write(e.EV_KEY, code, 0); ui.syn()
    elif kind in ("trigger", "trigger-analog"):
        # (Released at once, as a digital trigger; or through the middle, as an analog one.)
        ui.write(e.EV_ABS, code, 255); ui.syn(); time.sleep(0.08)
        if kind == "trigger-analog":
            ui.write(e.EV_ABS, code, 100); ui.syn(); time.sleep(0.03)
        ui.write(e.EV_ABS, code, 0); ui.syn()
    else:
        ui.write(e.EV_ABS, code, value); ui.syn(); time.sleep(0.08)
        ui.write(e.EV_ABS, code, 0); ui.syn()
    time.sleep(0.6)


def main():
    make_video()
    os.makedirs(WL, exist_ok=True)
    for f in os.listdir(WL):
        os.remove(f"{WL}/{f}")
    ui = pad()
    time.sleep(1)
    with open(f"{D}/sway.conf", "w") as f:
        f.write("output HEADLESS-1 resolution 1920x1080\n")
    env = {k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "DISPLAY")}
    env.update(WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1", WLR_RENDERER="vulkan")
    sway = subprocess.Popen(["sway", "-c", f"{D}/sway.conf"], env=env, stdout=open(f"{D}/sway.log", "w"), stderr=subprocess.STDOUT)
    time.sleep(2)
    wl = next(n for n in os.listdir(f"/run/user/{os.getuid()}") if n.startswith("wayland-") and not n.endswith(".lock")
              and os.path.getmtime(f"/run/user/{os.getuid()}/{n}") > time.time() - 10)
    menv = dict(env, WAYLAND_DISPLAY=wl)
    menv.pop("WLR_BACKENDS")
    if os.path.exists(SOCK):
        os.remove(SOCK)
    mpv = subprocess.Popen(["mpv", f"--config-dir={CONFIG}", "--ao=null", "--mute=yes", f"--watch-later-dir={WL}",
                            f"--input-ipc-server={SOCK}", "--msg-level=all=warn,input=v", VIDEO],
                           env=menv, stdout=open(f"{D}/mpv.log", "w"), stderr=subprocess.STDOUT)
    for _ in range(50):
        if os.path.exists(SOCK) and get("time-pos") is not None:
            break
        time.sleep(0.2)
    time.sleep(1.5)
    failed = 0

    def check(what, ok, got):
        nonlocal failed
        failed += not ok
        print(f"  {'ok  ' if ok else 'FAIL'}  {what}  ({got})")

    print("vo:", get("current-vo"), " hwdec:", get("hwdec-current"), " fullscreen:", get("fullscreen"),
          " paused:", get("pause"))
    ipc("set_property", "pause", True)
    ipc("seek", 90, "absolute", "exact")
    time.sleep(0.5)

    def seek_check(what, kind, code, delta, value=1):
        before = get("time-pos")
        press(ui, kind, code, value)
        after = get("time-pos")
        # (Relative seeks land on keyframes: up to one keyframe interval, 8 s here, past the mark.)
        moved = (after or 0) - before
        check(what, after is not None and moved * delta > 0 and abs(delta) - 1 <= abs(moved) <= abs(delta) + 9, f"{before:.0f} -> {after:.0f}")

    seek_check("d-pad right: +10 s", "hat", e.ABS_HAT0X, 10, 1)
    seek_check("d-pad left: -10 s", "hat", e.ABS_HAT0X, -10, -1)
    seek_check("d-pad up: +60 s", "hat", e.ABS_HAT0Y, 60, -1)
    seek_check("d-pad down: -60 s", "hat", e.ABS_HAT0Y, -60, 1)
    seek_check("R2: +5 s", "trigger", e.ABS_RZ, 5)
    seek_check("L2: -5 s", "trigger", e.ABS_Z, -5)
    seek_check("R2 released gradually: +5 s", "trigger-analog", e.ABS_RZ, 5)
    seek_check("L2 released gradually: -5 s", "trigger-analog", e.ABS_Z, -5)
    seek_check("R2 again: +5 s, once", "trigger", e.ABS_RZ, 5)
    c = get("chapter")
    press(ui, "key", e.BTN_TR)
    check("R1: next chapter", get("chapter") == c + 1, f"{c} -> {get('chapter')}")
    c = get("chapter")
    press(ui, "key", e.BTN_TL)
    check("L1: chapter back", get("chapter") == c - 1, f"{c} -> {get('chapter')}")
    s = get("sid")
    press(ui, "key", e.BTN_NORTH)  # xpad's X
    check("X: next subtitles", get("sid") != s, f"{s} -> {get('sid')}")
    a = get("aid")
    press(ui, "key", e.BTN_WEST)  # xpad's Y
    check("Y: next audio", get("aid") != a, f"{a} -> {get('aid')}")
    p = get("pause")
    press(ui, "key", e.BTN_SOUTH)
    check("A: play/pause", get("pause") != p, f"{p} -> {get('pause')}")
    press(ui, "key", e.BTN_SOUTH)
    check("A again", get("pause") == p, f"-> {get('pause')}")
    at = get("time-pos")
    press(ui, "key", e.BTN_EAST)
    time.sleep(1)
    check("B: quits", mpv.poll() is not None, f"exit {mpv.poll()}")
    kept = os.listdir(WL)
    check("  the place kept", len(kept) == 1, f"{kept}, at {at:.0f} s")
    if mpv.poll() is None:
        mpv.kill()
    sway.terminate()
    sway.wait()
    ui.close()
    shutil.rmtree(D, ignore_errors=True)
    print(f"{'all ok' if not failed else f'{failed} failed'}")
    return failed


if __name__ == "__main__":
    sys.exit(main())
