#!/usr/bin/env python3
"""A virtual Thor: a QEMU machine with the device's session around ClamShell, for testing what
doesn't need the real hardware. Two displays at the Thor's sizes (sway on real DRM outputs), a touchscreen
and a mouse-driven absolute pointer on the bottom one, a fake battery and Wi-Fi, uinput for our virtual
devices, ClamShell as a user service started by sway, and this repository shared read-only.

  tools/vm.py image          make a fresh machine (downloads its system first)
  tools/vm.py run            start the machine (the first start provisions it)
  tools/vm.py run --headless without windows (the screens drawn offscreen; ssh and grim still work)
  tools/vm.py run --pad 045e:028e   also pass a USB gamepad through (vendor:product)
  tools/vm.py ssh [command]  a shell, or a command, in the machine
  tools/vm.py build          build ClamShell in the machine (aarch64: there is no host build)
  tools/vm.py restart        restart ClamShell in the machine (after a build)
  tools/vm.py wake           wake the machine from sleep
  tools/vm.py sync           write this script's session files (sway config, service) into the machine
  tools/vm.py reset          throw the machine away (the download stays)
  tools/vm.py --arch aarch64 thor-image   build an SD card image for the real Thor (best effort): Arch
                             Linux ARM with ROCKNIX's SM8550 kernel, modules and firmware, our initramfs and
                             session, in ROCKNIX's SD layout for its ABL (tools/thor-image.sh, in the machine)

--arch x86_64 (the default): Arch's cloud image, provisioned by cloud-init, with KVM: fast; it runs the
host's build from the shared repository. --arch aarch64: Arch Linux ARM's root filesystem made into a disk
image without root (fakeroot, mkfs.ext4 -d), the kernel booted directly, provisioned by a first-boot
service, emulated (no KVM: slow); it builds the program itself, natively for ARM. Neither has the Thor's
GPU, cpufreq/devfreq or fan, and its own controller and panel quirks wait for the device. Standard
library only.
"""

import argparse
import hashlib
import http.server
import json
import os
import shutil
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "thor-vm"
USER = "thor"
DISK_SIZE = "16G"
# The Thor's screens: the top (main) one, and the bottom (second) one under it.
TOP = (1920, 1080)
BOTTOM = (1240, 1080)
# The screens' modes for the kernel; the second forced on ("e"): QEMU only enables a virtio-gpu output
# once a display window reports its size, which an offscreen (headless) one doesn't.
KERNEL_VIDEO = f"video=Virtual-1:{TOP[0]}x{TOP[1]}@60 video=Virtual-2:{BOTTOM[0]}x{BOTTOM[1]}@60e"
# Packages the session needs.
PACKAGES = [
    "sway", "swaybg", "foot", "xorg-xwayland", "mesa", "pipewire", "pipewire-pulse", "wireplumber",
    "sqlite", "grim", "python", "libinput-tools", "evtest", "iw", "noto-fonts",
    # Wi-Fi as on the device (NetworkManager), and test networks to join (hostapd on simulated radios).
    "networkmanager", "polkit", "hostapd", "dnsmasq",
    # The battery's history, and Bluetooth's service (the machine has no adapter), as on the device.
    "upower", "bluez",
]
# The simulated radios: wlan0 the device's, wlan1 and wlan2 access points (see TEST_AP).
TEST_NETWORKS = "ThorTest (WPA2, password thortest123) and ThorOpen (open)"


class Arch:
    def __init__(self, name):
        self.name = name
        self.state = CACHE / name
        self.provisioned = self.state / "provisioned"
        self.arm = name == "aarch64"
        if self.arm:
            self.url = "http://os.archlinuxarm.org/os/ArchLinuxARM-aarch64-latest.tar.gz"
            self.base = self.state / "ArchLinuxARM-aarch64-latest.tar.gz"
            self.disk = self.state / "disk.img"
            self.ssh_port = 2223
            # Built in the machine, natively (the repository is read-only there).
            self.program = f"/home/{USER}/target/release/clamshell"
        else:
            self.url = "https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2"
            self.base = self.state / "arch-cloudimg.qcow2"
            self.disk = self.state / "disk.qcow2"
            self.ssh_port = 2222
            self.program = "/mnt/thor/clamshell/target/release/clamshell"


A = Arch("x86_64")


def run(cmd, **kw):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    return subprocess.run([str(c) for c in cmd], check=True, **kw)


def need(*programs):
    missing = [p for p in programs if shutil.which(p) is None]
    if missing:
        sys.exit(f"missing: {' '.join(missing)}")


# --- downloading

def digest(path, algorithm):
    h = hashlib.new(algorithm)
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def download():
    """The machine's system as published, checked against its published checksum."""
    A.state.mkdir(parents=True, exist_ok=True)
    algorithm, suffix = ("md5", ".md5") if A.arm else ("sha256", ".SHA256")
    expected = urllib.request.urlopen(A.url + suffix).read().decode().split()[0]
    if A.base.exists() and digest(A.base, algorithm) == expected:
        print(f"{A.base.name}: up to date")
        return
    part = A.base.with_name(A.base.name + ".part")
    print(f"downloading {A.url}")
    with urllib.request.urlopen(A.url) as r, open(part, "wb") as out:
        total = int(r.headers.get("content-length", 0))
        done = 0
        while block := r.read(1 << 20):
            out.write(block)
            done += len(block)
            print(f"\r  {done >> 20} of {total >> 20} MB", end="", flush=True)
    print()
    if digest(part, algorithm) != expected:
        part.unlink()
        sys.exit("checksum mismatch: download again")
    part.rename(A.base)


# --- the session (the same on both)

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
exec systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP && systemctl --user restart clamshell.service
"""

BASH_PROFILE = """\
# The session: sway on the first console. No GPU acceleration in the machine: the pixman renderer.
[ "$(tty)" = /dev/tty1 ] && WLR_RENDERER=pixman exec sway > "$HOME/sway.log" 2>&1
"""


def service():
    return f"""\
[Unit]
Description=ClamShell
After=graphical-session.target

[Service]
# Tests drive it through this pipe (tools/vm.py ssh -- 'echo mode wifi > $XDG_RUNTIME_DIR/clamshell-control').
ExecStartPre=-/usr/bin/mkfifo %t/clamshell-control
ExecStart={A.program}
Environment=CLAMSHELL_BOTTOM_OUTPUT=Virtual-2
Environment=CLAMSHELL_CONTROL=%t/clamshell-control
Restart=on-failure
RestartSec=2
"""


def session_files():
    return [
        (f"/home/{USER}/.bash_profile", BASH_PROFILE),
        (f"/home/{USER}/.config/sway/config", SWAY_CONFIG),
        (f"/home/{USER}/.config/systemd/user/clamshell.service", service()),
    ]


def system_files():
    """The same system setup on both: autologin, modules, uinput for the input group."""
    return [
        ("/etc/systemd/system/getty@tty1.service.d/autologin.conf",
         f"[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin {USER} --noclear %I $TERM\n"),
        ("/etc/modules-load.d/thor-vm.conf", "uinput\ntest_power\nmac80211_hwsim\n"),
        ("/etc/modprobe.d/thor-vm.conf", "options mac80211_hwsim radios=3\n"),
        ("/etc/udev/rules.d/70-thor-vm.rules", 'KERNEL=="uinput", GROUP="input", MODE="0660"\n'),
        # NetworkManager has the device's radio only: the wired link (ssh) stays as it is, the access
        # points' radios are hostapd's.
        ("/etc/NetworkManager/conf.d/thor-vm.conf",
         "[keyfile]\nunmanaged-devices=interface-name:eth0;interface-name:wlan1;interface-name:wlan2\n"),
        ("/etc/hostapd/thor-test.conf",
         "interface=wlan1\ndriver=nl80211\nssid=ThorTest\nhw_mode=g\nchannel=6\n"
         "wpa=2\nwpa_key_mgmt=WPA-PSK\nrsn_pairwise=CCMP\nwpa_passphrase=thortest123\n"),
        ("/etc/hostapd/thor-open.conf", "interface=wlan2\ndriver=nl80211\nssid=ThorOpen\nhw_mode=g\nchannel=11\n"),
        ("/etc/systemd/system/thor-test-ap.service", TEST_AP),
        ("/etc/systemd/system/thor-test-dhcp.service", TEST_DHCP),
    ]


TEST_AP = """\
[Unit]
Description=Test Wi-Fi networks on the simulated radios
After=sys-subsystem-net-devices-wlan2.device
Wants=sys-subsystem-net-devices-wlan2.device

[Service]
ExecStart=/usr/bin/hostapd /etc/hostapd/thor-test.conf /etc/hostapd/thor-open.conf

[Install]
WantedBy=multi-user.target
"""

# Addresses for whoever joins the test networks.
TEST_DHCP = """\
[Unit]
Description=DHCP on the test Wi-Fi networks
After=thor-test-ap.service
BindsTo=thor-test-ap.service

[Service]
ExecStartPre=/usr/bin/ip addr replace 10.41.1.1/24 dev wlan1
ExecStartPre=/usr/bin/ip addr replace 10.41.2.1/24 dev wlan2
ExecStart=/usr/bin/dnsmasq --keep-in-foreground --conf-file=/dev/null --port=0 --bind-interfaces \\
    --interface=wlan1 --interface=wlan2 --dhcp-range=10.41.1.10,10.41.1.50,1h --dhcp-range=10.41.2.10,10.41.2.50,1h

[Install]
WantedBy=thor-test-ap.service
"""


FSTAB_9P = "thor /mnt/thor 9p trans=virtio,version=9p2000.L,ro,nofail 0 0"


# --- x86_64: the cloud image and cloud-init

SET_KERNEL_VIDEO = (
    "sed -i -E 's/^GRUB_CMDLINE_LINUX=\"([^\"]*)\"/GRUB_CMDLINE_LINUX=\"net.ifnames=0 "
    + KERNEL_VIDEO
    + "\"/' /etc/default/grub && grub-mkconfig -o /boot/grub/grub.cfg"
)


def user_data():
    files = [(p, c, "root") for p, c in system_files()] + [(p, c, USER) for p, c in session_files()]
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
        "mounts": [FSTAB_9P.split()],
        "write_files": [
            {"path": path, "content": content, "owner": f"{owner}:{owner}", "defer": owner != "root"}
            for path, content, owner in files
        ],
        "runcmd": [
            ["mkdir", "-p", "/mnt/thor"],
            ["chown", "-R", f"{USER}:{USER}", f"/home/{USER}"],
            ["loginctl", "enable-linger", USER],
            ["systemctl", "enable", "NetworkManager", "thor-test-ap", "thor-test-dhcp"],
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

        def log_message(self, *_):
            pass

    server = socketserver.TCPServer(("127.0.0.1", 0), Seed)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


# --- aarch64: Arch Linux ARM's root filesystem, a first-boot service

def firstboot_script():
    """Runs once on the machine's first boot: keys, packages, the user and the session; then reboots."""
    home_files = "\n".join(
        f"mkdir -p $(dirname {p})\ncat > {p} <<'THOR_EOF'\n{c}THOR_EOF" for p, c in session_files()
    )
    return f"""#!/bin/bash
set -ex
pacman-key --init
pacman-key --populate archlinuxarm
pacman -Syu --noconfirm --needed sudo rust gcc pkgconf {' '.join(PACKAGES)}
id {USER} || useradd -m -G wheel,video,input,audio -s /bin/bash {USER}
echo '{USER} ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/{USER}
install -d -m 700 -o {USER} -g {USER} /home/{USER}/.ssh
echo '{ssh_key()}' > /home/{USER}/.ssh/authorized_keys
{home_files}
chown -R {USER}:{USER} /home/{USER}
chmod 600 /home/{USER}/.ssh/authorized_keys
loginctl enable-linger {USER}
systemctl enable NetworkManager thor-test-ap thor-test-dhcp
mkdir -p /mnt/thor
grep -q /mnt/thor /etc/fstab || echo '{FSTAB_9P}' >> /etc/fstab
touch /var/lib/thor-firstboot
systemctl reboot
"""


FIRSTBOOT_UNIT = """\
[Unit]
Description=The virtual Thor's first boot: packages, user, session
Wants=network-online.target
After=network-online.target
ConditionPathExists=!/var/lib/thor-firstboot

[Service]
Type=oneshot
ExecStart=/usr/local/bin/thor-firstboot
StandardOutput=journal+console

[Install]
WantedBy=multi-user.target
"""


def arm_image():
    """The root filesystem as an ext4 disk image, made without root: under fakeroot, files keep their
    owners from the tarball, and mkfs.ext4 -d writes them so."""
    need("fakeroot", "mkfs.ext4", "tar")
    with tempfile.TemporaryDirectory(dir=A.state) as work:
        extra = Path(work) / "extra"
        for path, content in system_files() + [
            ("/usr/local/bin/thor-firstboot", firstboot_script()),
            ("/etc/systemd/system/thor-firstboot.service", FIRSTBOOT_UNIT),
        ]:
            target = extra / path.lstrip("/")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
        root = Path(work) / "root"
        root.mkdir()
        script = f"""set -e
tar -xpf '{A.base}' -C '{root}' --warning=no-unknown-keyword
cp -r '{extra}'/. '{root}'/
chown -R 0:0 '{root}/etc/systemd/system' '{root}/usr/local/bin' '{root}/etc/modules-load.d' '{root}/etc/udev/rules.d'
chmod 755 '{root}/usr/local/bin/thor-firstboot'
ln -sf /etc/systemd/system/thor-firstboot.service '{root}/etc/systemd/system/multi-user.target.wants/thor-firstboot.service'
mkfs.ext4 -q -F -L root -d '{root}' '{A.disk}' {DISK_SIZE}
"""
        print("making the disk image (fakeroot)", flush=True)
        run(["fakeroot", "sh", "-c", script])


def arm_boot_files():
    """The kernel and initramfs, read out of the disk image (so updates in the machine are used)."""
    need("debugfs")
    files = {}
    # The fallback initramfs (all modules) once the machine has one; the shipped image has only the
    # default one.
    for names, out in ((["Image"], "Image"), (["initramfs-linux-fallback.img", "initramfs-linux.img"], "initramfs.img")):
        target = A.state / out
        for name in names:
            target.unlink(missing_ok=True)
            subprocess.run(["debugfs", "-R", f"dump /boot/{name} {target}", str(A.disk)], check=True, capture_output=True)
            if target.exists() and target.stat().st_size > 0:
                break
        else:
            sys.exit(f"no {' or '.join(names)} in the disk image's /boot")
        files[out] = target
    return files


# --- commands

def image(_args):
    need("qemu-img")
    download()
    if A.disk.exists():
        sys.exit(f"{A.disk} exists: `tools/vm.py --arch {A.name} reset` first")
    if A.arm:
        arm_image()
    else:
        # A disk on top of the image (the image stays as downloaded).
        run(["qemu-img", "create", "-f", "qcow2", "-F", "qcow2", "-b", A.base, A.disk, DISK_SIZE])
    A.provisioned.unlink(missing_ok=True)
    print(f"ready: tools/vm.py --arch {A.name} run")


def qemu_command(args, seed_port):
    common = [
        "-smp", "4", "-m", "4G",
        "-nic", f"user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:{A.ssh_port}-:22",
        "-virtfs", f"local,path={REPO},mount_tag=thor,security_model=none,readonly=on",
        # Two screens: Virtual-1 (top) and Virtual-2 (bottom).
        "-device", f"virtio-gpu-pci,id=video0,max_outputs=2,xres={TOP[0]},yres={TOP[1]}",
        "-device", "virtio-keyboard-pci",
        # The host mouse as an absolute pointer on each screen's window, and a touchscreen on the bottom.
        "-device", "virtio-tablet-pci,display=video0,head=0",
        "-device", "qemu-xhci",
        "-device", "usb-tablet,display=video0,head=1",
        "-device", "virtio-multitouch-pci,display=video0,head=1",
        "-serial", f"file:{A.state / 'serial.log'}",
        # QEMU's monitor, for commands from here (`wake`).
        "-monitor", f"unix:{A.state / 'monitor.sock'},server,nowait",
        # Each screen is an SDL window (offscreen when headless).
        "-display", "sdl",
    ]
    if args.pad:
        vendor, product = args.pad.split(":")
        common += ["-device", f"usb-host,vendorid=0x{vendor},productid=0x{product}"]
    if getattr(args, "sd", None):
        # An SD card image to build (/dev/vdb), and the downloads, read-only, at the tag "cache".
        common += ["-drive", f"file={args.sd},if=virtio,format=raw",
                   "-virtfs", f"local,path={CACHE},mount_tag=cache,security_model=none,readonly=on"]
    if A.arm:
        boot = arm_boot_files()
        return [
            # A reboot stops the machine: the kernel is read out of the disk at each start (an update in
            # the machine would otherwise run the old kernel with the new modules).
            "qemu-system-aarch64", "-machine", "virt", "-cpu", "max", "-accel", "tcg,thread=multi", "-no-reboot",
            "-kernel", boot["Image"], "-initrd", boot["initramfs.img"],
            "-append", f"root=/dev/vda rw console=ttyAMA0 {KERNEL_VIDEO}",
            "-drive", f"file={A.disk},if=virtio,format=raw",
        ] + common
    cmd = [
        # No default VGA card (one GPU: the virtio one with both screens) and no VMware mouse.
        "qemu-system-x86_64", "-enable-kvm", "-machine", "q35,vmport=off", "-vga", "none", "-cpu", "host",
        "-drive", f"file={A.disk},if=virtio",
    ] + common
    if seed_port is not None:
        cmd += ["-smbios", f"type=1,serial=ds=nocloud;s=http://10.0.2.2:{seed_port}/"]
    return cmd


def start(args):
    need("qemu-system-aarch64" if A.arm else "qemu-system-x86_64")
    if not A.disk.exists():
        sys.exit(f"no machine yet: tools/vm.py --arch {A.name} image")
    first = not A.provisioned.exists()
    seed = serve_seed() if first and not A.arm else None
    if first:
        slow = " (emulated ARM: a long while)" if A.arm else ""
        print(f"first start: provisioning{slow}; the machine reboots once when done", flush=True)
    env = dict(os.environ, SDL_VIDEODRIVER="offscreen") if args.headless else None
    while True:
        qemu = subprocess.Popen([str(c) for c in qemu_command(args, seed)], env=env)
        if first:
            threading.Thread(target=mark_provisioned, args=(qemu,), daemon=True).start()
        try:
            code = qemu.wait()
        except KeyboardInterrupt:
            qemu.terminate()
            return qemu.wait()
        # ARM: the first boot ends in a reboot (no-reboot stops the machine): start again, with the new kernel.
        if A.arm and first and not A.provisioned.exists() and arm_firstboot_done():
            print("the first boot is done: starting again", flush=True)
            continue
        return code


def arm_firstboot_done():
    out = subprocess.run(["debugfs", "-R", "stat /var/lib/thor-firstboot", str(A.disk)], capture_output=True, text=True)
    return "Inode:" in out.stdout


def mark_provisioned(qemu):
    """Once the machine answers ssh with its session running, the first start is done."""
    while qemu.poll() is None:
        result = subprocess.run(ssh_args(["systemctl", "--user", "is-active", "graphical-session.target", "clamshell.service"], quiet=True),
                                capture_output=True, text=True)
        if result.returncode == 0 or "activating" in result.stdout or (A.arm and ssh_ok()):
            A.provisioned.touch()
            print("provisioned: the machine's session runs", flush=True)
            return
        time.sleep(10)


def ssh_ok():
    """The user can log in (on ARM, before the program is built, the service can't run yet)."""
    return subprocess.run(ssh_args(["test", "-f", f"/home/{USER}/.config/sway/config"], quiet=True),
                          capture_output=True).returncode == 0


def ssh_args(command, quiet=False):
    args = ["ssh", "-p", str(A.ssh_port), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=3"]
    if quiet:
        args += ["-o", "BatchMode=yes"]
    return args + [f"{USER}@127.0.0.1"] + list(command)


def ssh(args):
    os.execvp("ssh", ssh_args(args.command))


def build(_args):
    if not A.arm:
        run(["cargo", "build", "--release"], cwd=REPO / "clamshell")
        return
    # Natively in the machine, out of the read-only repository into the user's own target dir.
    run(ssh_args([f"cd /mnt/thor/clamshell && CARGO_TARGET_DIR=$HOME/target cargo build --release --locked"]))


# --- an SD card image for the real Thor

ROCKNIX_RELEASES = "https://api.github.com/repos/ROCKNIX/distribution/releases/latest"


def rocknix():
    """ROCKNIX's latest SM8550 release, checked, with its KERNEL and SYSTEM unpacked; returns that target dir."""
    release = json.load(urllib.request.urlopen(ROCKNIX_RELEASES))
    assets = {a["name"]: a["browser_download_url"] for a in release["assets"]}
    name = next(n for n in assets if n.startswith("ROCKNIX-SM8550.aarch64-") and n.endswith(".tar"))
    folder = CACHE / "rocknix"
    folder.mkdir(parents=True, exist_ok=True)
    tar = folder / name
    target = folder / name.removesuffix(".tar") / "target"
    if (target / "KERNEL").exists() and (target / "SYSTEM").exists():
        return target
    expected = urllib.request.urlopen(assets[name + ".sha256"]).read().decode().split()[0]
    if not tar.exists() or digest(tar, "sha256") != expected:
        print(f"downloading {name}")
        urllib.request.urlretrieve(assets[name], tar)
        if digest(tar, "sha256") != expected:
            sys.exit("checksum mismatch: download again")
    run(["tar", "-xf", tar, "-C", folder, "--wildcards", "*/target/KERNEL", "*/target/SYSTEM"])
    return target


def thor_image(args):
    if not A.arm:
        sys.exit("the Thor's image is built in the aarch64 machine: tools/vm.py --arch aarch64 thor-image")
    if not A.provisioned.exists():
        sys.exit("the aarch64 machine isn't provisioned yet: tools/vm.py --arch aarch64 run (once)")
    target = rocknix()
    sd = A.state / "thor-sd.img"
    sd.unlink(missing_ok=True)
    with open(sd, "wb") as f:
        f.truncate(args.size << 30)
    print(f"starting the machine with {sd.name} attached")
    machine = threading.Thread(target=start, args=(argparse.Namespace(headless=True, pad=None, sd=sd),), daemon=True)
    machine.start()
    while subprocess.run(ssh_args(["true"], quiet=True), capture_output=True).returncode != 0:
        time.sleep(5)
    inside = f"""set -e
sudo mkdir -p /mnt/cache
mountpoint -q /mnt/cache || sudo mount -t 9p -o trans=virtio,version=9p2000.L,ro cache /mnt/cache
cd /mnt/thor/clamshell && CARGO_TARGET_DIR=$HOME/target cargo build --release --locked
sudo bash /mnt/thor/tools/thor-image.sh /dev/vdb /mnt/cache/{target.relative_to(CACHE)} $HOME/target/release/clamshell '{ssh_key()}'
sudo poweroff"""
    result = subprocess.run(ssh_args([inside]))
    machine.join(timeout=120)
    if result.returncode not in (0, 255):
        sys.exit(f"the build failed ({result.returncode})")
    print(f"\n{sd}: write it to an SD card (dd, bs=4M); ROCKNIX's ABL (in its release, 3rdparty/bootloader) boots it")


def sync(_args):
    for path, content in session_files():
        subprocess.run(ssh_args(["mkdir -p $(dirname " + path + ") && cat > " + path]), input=content, text=True, check=True)
        print(f"wrote {path}")
    reload = "systemctl --user daemon-reload; SWAYSOCK=$(ls /run/user/$(id -u)/sway-ipc.*.sock) swaymsg reload"
    subprocess.run(ssh_args([reload]), check=False)


def wake(_args):
    """Wakes the machine from sleep (as the power button would)."""
    with socket.socket(socket.AF_UNIX) as sock:
        sock.connect(str(A.state / "monitor.sock"))
        sock.sendall(b"system_wakeup\n")
        time.sleep(0.5)
    print("woken")


def restart(_args):
    run(ssh_args(["systemctl", "--user", "restart", "clamshell.service"]))


def reset(_args):
    for path in (A.disk, A.provisioned, A.state / "Image", A.state / "initramfs.img"):
        path.unlink(missing_ok=True)
    print(f"the {A.name} machine is gone; the download stays (tools/vm.py --arch {A.name} image for a new one)")


def main():
    global A
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--arch", choices=["x86_64", "aarch64"], default="x86_64")
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("image").set_defaults(func=image)
    p = sub.add_parser("run")
    p.add_argument("--headless", action="store_true")
    p.add_argument("--pad", metavar="VENDOR:PRODUCT")
    p.set_defaults(func=start)
    p = sub.add_parser("ssh")
    p.add_argument("command", nargs=argparse.REMAINDER)
    p.set_defaults(func=ssh)
    sub.add_parser("build").set_defaults(func=build)
    sub.add_parser("restart").set_defaults(func=restart)
    sub.add_parser("wake").set_defaults(func=wake)
    sub.add_parser("sync").set_defaults(func=sync)
    sub.add_parser("reset").set_defaults(func=reset)
    p = sub.add_parser("thor-image")
    p.add_argument("--size", type=int, default=12, help="the image's size in GB (default 12)")
    p.set_defaults(func=thor_image)
    args = parser.parse_args()
    A = Arch(args.arch)
    sys.exit(args.func(args) or 0)


if __name__ == "__main__":
    main()
