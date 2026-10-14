#!/usr/bin/env python3
"""A virtual Thor: a QEMU machine with the device's session around the system program, for testing what
doesn't need the real hardware. Two displays at the Thor's sizes (sway on real DRM outputs), a touchscreen
and a mouse-driven absolute pointer on the bottom one, a fake battery and Wi-Fi, uinput for our virtual
devices, the system program as a user service started by sway, and this repository shared read-only (so:
build on the host, restart the service in the machine).

  tools/vm.py image          download Arch's cloud image and make a fresh machine disk
  tools/vm.py run            start the machine (the first start provisions it, a few minutes)
  tools/vm.py run --headless without windows (the screens drawn offscreen; ssh and grim still work)
  tools/vm.py run --pad 045e:028e   also pass a USB gamepad through (vendor:product)
  tools/vm.py ssh [command]  a shell, or a command, in the machine
  tools/vm.py restart        restart the system program in the machine (after a host build)
  tools/vm.py sync           write this script's session files (sway config, service) into the machine
  tools/vm.py reset          throw the machine disk away (the downloaded image stays)

x86-64 with KVM (fast). The machine has no Thor GPU, no cpufreq/devfreq or fan, and the Thor's own
controller and panel quirks are unknown: those wait for the device. Standard library only.
"""

import argparse
import hashlib
import http.server
import json
import os
import shutil
import socketserver
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
STATE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "thor-vm"
IMAGE_URL = "https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2"
BASE = STATE / "arch-cloudimg.qcow2"
DISK = STATE / "disk.qcow2"
PROVISIONED = STATE / "provisioned"
DISK_SIZE = "16G"
SSH_PORT = 2222
USER = "thor"
# The Thor's screens: the top (main) one, and the bottom (second) one under it.
TOP = (1920, 1080)
BOTTOM = (1240, 1080)
# Packages the session needs (the program itself comes from the shared repository).
# The screens' modes for the kernel; the second forced on ("e"): QEMU only enables a virtio-gpu output
# once a display window reports its size, which an offscreen (headless) one doesn't.
KERNEL_VIDEO = f"video=Virtual-1:{TOP[0]}x{TOP[1]}@60 video=Virtual-2:{BOTTOM[0]}x{BOTTOM[1]}@60e"
SET_KERNEL_VIDEO = (
    "sed -i -E 's/^GRUB_CMDLINE_LINUX=\"([^\"]*)\"/GRUB_CMDLINE_LINUX=\"net.ifnames=0 "
    + KERNEL_VIDEO
    + "\"/' /etc/default/grub && grub-mkconfig -o /boot/grub/grub.cfg"
)
PACKAGES = [
    "sway", "swaybg", "foot", "xorg-xwayland", "mesa", "pipewire", "pipewire-pulse", "wireplumber",
    "sqlite", "grim", "python", "libinput-tools", "evtest", "iw", "noto-fonts",
]


def run(cmd, **kw):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    return subprocess.run([str(c) for c in cmd], check=True, **kw)


def need(*programs):
    missing = [p for p in programs if shutil.which(p) is None]
    if missing:
        sys.exit(f"missing: {' '.join(missing)} (pacman -S qemu-desktop edk2-ovmf)")


# --- the image

def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def download():
    STATE.mkdir(parents=True, exist_ok=True)
    expected = urllib.request.urlopen(IMAGE_URL + ".SHA256").read().decode().split()[0]
    if BASE.exists() and sha256(BASE) == expected:
        print(f"{BASE.name}: up to date")
        return
    part = BASE.with_suffix(".part")
    print(f"downloading {IMAGE_URL}")
    with urllib.request.urlopen(IMAGE_URL) as r, open(part, "wb") as out:
        total = int(r.headers.get("content-length", 0))
        done = 0
        while block := r.read(1 << 20):
            out.write(block)
            done += len(block)
            print(f"\r  {done >> 20} of {total >> 20} MB", end="", flush=True)
    print()
    if sha256(part) != expected:
        part.unlink()
        sys.exit("checksum mismatch: download again")
    part.rename(BASE)


def image(_args):
    need("qemu-img")
    download()
    if DISK.exists():
        sys.exit(f"{DISK} exists: `tools/vm.py reset` first")
    # A disk on top of the image (the image stays as downloaded).
    run(["qemu-img", "create", "-f", "qcow2", "-F", "qcow2", "-b", BASE, DISK, DISK_SIZE])
    PROVISIONED.unlink(missing_ok=True)
    print("ready: tools/vm.py run")


# --- provisioning (cloud-init, served over HTTP to the first boot)

def ssh_key():
    for name in ("id_ed25519.pub", "id_rsa.pub"):
        key = Path.home() / ".ssh" / name
        if key.exists():
            return key.read_text().strip()
    sys.exit("no ssh key in ~/.ssh (ssh-keygen -t ed25519)")


SWAY_CONFIG = f"""\
# The virtual Thor: the main screen on top, the second screen under it (as on the device).
output Virtual-1 mode {TOP[0]}x{TOP[1]} position 0 0
output Virtual-2 mode --custom {BOTTOM[0]}x{BOTTOM[1]} position 0 {TOP[1]}
default_border none
default_floating_border none
# The host's mouse: an absolute pointer per screen window (QEMU binds each to its display head).
input "1575:3:QEMU_Virtio_Tablet" map_to_output Virtual-1
input "1575:1:QEMU_QEMU_USB_Tablet" map_to_output Virtual-2
# The touchscreen is the second screen's (it is also a pointer, for single touches).
input "1575:3:QEMU_Virtio_MultiTouch" map_to_output Virtual-2
input type:touch map_to_output Virtual-2
# The environment first, then the program (two execs would run at once).
exec systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP && systemctl --user restart sysprog.service
"""

SERVICE = """\
[Unit]
Description=The system program (from the host's repository)
After=graphical-session.target

[Service]
ExecStart=/mnt/thor/sysprog/target/release/sysprog
Environment=SYSPROG_BOTTOM_OUTPUT=Virtual-2
Restart=on-failure
RestartSec=2
"""

BASH_PROFILE = """\
# The session: sway on the first console.
# No GPU acceleration in the machine: the pixman renderer.
[ "$(tty)" = /dev/tty1 ] && WLR_RENDERER=pixman exec sway > "$HOME/sway.log" 2>&1
"""


def user_data():
    files = [
        ("/etc/systemd/system/getty@tty1.service.d/autologin.conf",
         f"[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin {USER} --noclear %I $TERM\n", "root"),
        ("/etc/modules-load.d/thor-vm.conf", "uinput\ntest_power\nmac80211_hwsim\n", "root"),
        # Our virtual devices (keyboard, touchpad, pad) and grabbing the touchscreen and pad: the input group.
        ("/etc/udev/rules.d/70-thor-vm.rules", 'KERNEL=="uinput", GROUP="input", MODE="0660"\n', "root"),
    ] + [(path, content, USER) for path, content in session_files()]
    config = {
        "users": [{
            "name": USER,
            "groups": ["wheel", "video", "input", "audio"],
            "sudo": "ALL=(ALL) NOPASSWD:ALL",
            "shell": "/bin/bash",
            "ssh_authorized_keys": [ssh_key()],
        }],
        "package_update": True,
        "packages": PACKAGES,
        "mounts": [["thor", "/mnt/thor", "9p", "trans=virtio,version=9p2000.L,ro,nofail", "0", "0"]],
        "write_files": [
            {"path": path, "content": content, "owner": f"{owner}:{owner}", "defer": owner != "root"}
            for path, content, owner in files
        ],
        "runcmd": [
            ["mkdir", "-p", "/mnt/thor"],
            ["chown", "-R", f"{USER}:{USER}", f"/home/{USER}"],
            ["loginctl", "enable-linger", USER],
            ["sh", "-c", SET_KERNEL_VIDEO],
        ],
        # Once installed: boot again into the session.
        "power_state": {"mode": "reboot", "condition": True},
    }
    return "#cloud-config\n" + json.dumps(config, indent=1)


def serve_seed():
    """Serves cloud-init's NoCloud data on a free port; returns the port."""
    data = {"/user-data": user_data(), "/meta-data": "instance-id: thor-vm\nlocal-hostname: thor\n", "/vendor-data": ""}

    class Seed(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = data.get(self.path)
            self.send_response(200 if body is not None else 404)
            self.end_headers()
            if body is not None:
                self.wfile.write(body.encode())
                if self.path == "/user-data":
                    print("  (the machine has its provisioning)", flush=True)

        def log_message(self, *_):
            pass

    server = socketserver.TCPServer(("127.0.0.1", 0), Seed)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


# --- running

def qemu_command(args, seed_port):
    cmd = [
        # No default VGA card (one GPU: the virtio one with both screens) and no VMware mouse.
        "qemu-system-x86_64", "-enable-kvm", "-machine", "q35,vmport=off", "-vga", "none",
        "-cpu", "host", "-smp", "4", "-m", "4G",
        "-drive", f"file={DISK},if=virtio",
        "-nic", f"user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:{SSH_PORT}-:22",
        "-virtfs", f"local,path={REPO},mount_tag=thor,security_model=none,readonly=on",
        # Two screens: Virtual-1 (top) and Virtual-2 (bottom).
        "-device", f"virtio-gpu-pci,id=video0,max_outputs=2,xres={TOP[0]},yres={TOP[1]}",
        "-device", "virtio-keyboard-pci",
        # The host mouse as an absolute pointer on each screen's window, and a touchscreen on the bottom.
        "-device", "virtio-tablet-pci,display=video0,head=0",
        "-device", "qemu-xhci",
        "-device", "usb-tablet,display=video0,head=1",
        "-device", "virtio-multitouch-pci,display=video0,head=1",
        "-serial", f"file:{STATE / 'serial.log'}",
    ]
    if seed_port is not None:
        cmd += ["-smbios", f"type=1,serial=ds=nocloud;s=http://10.0.2.2:{seed_port}/"]
    # Each screen is an SDL window; QEMU enables a virtio-gpu output only once its window exists, so
    # headless means SDL drawing offscreen, not no display.
    cmd += ["-display", "sdl"]
    if args.pad:
        vendor, product = args.pad.split(":")
        cmd += ["-device", f"usb-host,vendorid=0x{vendor},productid=0x{product}"]
    return cmd


def start(args):
    need("qemu-system-x86_64")
    if not DISK.exists():
        sys.exit("no machine yet: tools/vm.py image")
    binary = REPO / "sysprog/target/release/sysprog"
    if not binary.exists():
        print("note: no release build yet (cd sysprog && cargo build --release); the service starts once there is")
    first = not PROVISIONED.exists()
    seed = serve_seed() if first else None
    if first:
        print("first start: provisioning (packages, the session); the machine reboots once when done")
    env = dict(os.environ, SDL_VIDEODRIVER="offscreen") if args.headless else None
    qemu = subprocess.Popen(qemu_command(args, seed), env=env)
    if first:
        threading.Thread(target=mark_provisioned, args=(qemu,), daemon=True).start()
    try:
        return qemu.wait()
    except KeyboardInterrupt:
        qemu.terminate()
        return qemu.wait()


def mark_provisioned(qemu):
    """Once the machine answers ssh with the session running, the first start is done."""
    while qemu.poll() is None:
        result = subprocess.run(ssh_args(["systemctl", "--user", "is-active", "sysprog.service"], quiet=True),
                                capture_output=True, text=True)
        if result.stdout.strip() in ("active", "activating"):
            PROVISIONED.touch()
            print("provisioned: the session and the system program run", flush=True)
            return
        time.sleep(5)


def ssh_args(command, quiet=False):
    args = ["ssh", "-p", str(SSH_PORT), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=3"]
    if quiet:
        args += ["-o", "BatchMode=yes"]
    return args + [f"{USER}@127.0.0.1"] + list(command)


def ssh(args):
    os.execvp("ssh", ssh_args(args.command))


def session_files():
    return [
        (f"/home/{USER}/.bash_profile", BASH_PROFILE),
        (f"/home/{USER}/.config/sway/config", SWAY_CONFIG),
        (f"/home/{USER}/.config/systemd/user/sysprog.service", SERVICE),
    ]


def sync(_args):
    for path, content in session_files():
        subprocess.run(ssh_args(["mkdir -p $(dirname " + path + ") && cat > " + path]), input=content, text=True, check=True)
        print(f"wrote {path}")
    reload = "systemctl --user daemon-reload; SWAYSOCK=$(ls /run/user/$(id -u)/sway-ipc.*.sock) swaymsg reload"
    subprocess.run(ssh_args([reload]), check=False)


def restart(_args):
    run(ssh_args(["systemctl", "--user", "restart", "sysprog.service"]))


def reset(_args):
    for path in (DISK, PROVISIONED):
        path.unlink(missing_ok=True)
    print("the machine disk is gone; the downloaded image stays (tools/vm.py image for a new one)")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("image").set_defaults(func=image)
    p = sub.add_parser("run")
    p.add_argument("--headless", action="store_true")
    p.add_argument("--pad", metavar="VENDOR:PRODUCT")
    p.set_defaults(func=start)
    p = sub.add_parser("ssh")
    p.add_argument("command", nargs=argparse.REMAINDER)
    p.set_defaults(func=ssh)
    sub.add_parser("restart").set_defaults(func=restart)
    sub.add_parser("sync").set_defaults(func=sync)
    sub.add_parser("reset").set_defaults(func=reset)
    args = parser.parse_args()
    sys.exit(args.func(args) or 0)


if __name__ == "__main__":
    main()
