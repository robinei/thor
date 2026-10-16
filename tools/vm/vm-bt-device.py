#!/usr/bin/env python3
"""A Bluetooth device for the virtual Thor, on the second of btvirt's two virtual controllers (hci1): run
as root in the machine by `tools/vm/vm.py bluetooth`. Standard library only.

It takes hci1 away from bluetoothd (which has MaxControllers=1, so it keeps hci0 only: opening hci1's
HCI user channel and closing it again makes the kernel announce it anew, and bluetoothd then ignores it),
sets it up through the kernel's management interface (btmgmt: powered, connectable, discoverable,
bondable, no input or output, so pairing is "just works" and the kernel accepts it by itself), and then
answers as a small HID device: an SDP server with one HID record (a vendor-defined report, so no input
device appears in the machine), and the HID control and interrupt channels, which it accepts and holds.
That is enough for BlueZ on hci0 to find it, pair, connect (its input profile) and forget it.
"""

import ctypes
import os
import select
import socket
import struct
import subprocess
import sys
import time

INDEX = 1
NAME = "ThorTestDevice"
PSM_SDP, PSM_HID_CONTROL, PSM_HID_INTERRUPT = 1, 17, 19
AF_BLUETOOTH, BTPROTO_HCI, HCI_CHANNEL_USER = 31, 1, 1


def log(*words):
    print(*words, flush=True)


def btmgmt(*args):
    out = subprocess.run(["btmgmt", "--index", str(INDEX), *args], capture_output=True, text=True)
    log("btmgmt", *args, "->", (out.stdout + out.stderr).strip().splitlines()[-1:] or "")


def take_from_bluetoothd():
    """Opens and closes hci1's user channel: the kernel removes the index and adds it again, which
    bluetoothd (at MaxControllers=1, with hci0) ignores."""
    btmgmt("power", "off")
    libc = ctypes.CDLL(None, use_errno=True)
    fd = libc.socket(AF_BLUETOOTH, socket.SOCK_RAW, BTPROTO_HCI)
    address = struct.pack("<HHH", AF_BLUETOOTH, INDEX, HCI_CHANNEL_USER)
    if libc.bind(fd, address, len(address)) != 0:
        sys.exit(f"hci{INDEX}: no user channel ({os.strerror(ctypes.get_errno())})")
    libc.close(fd)
    time.sleep(0.5)


def address_of(index):
    info = subprocess.run(["btmgmt", "--index", str(index), "info"], capture_output=True, text=True).stdout
    return next(line.split()[1] for line in info.splitlines() if line.strip().startswith("addr "))


# --- SDP data elements

def uint8(v):
    return bytes([0x08, v])


def uint16(v):
    return b"\x09" + struct.pack(">H", v)


def uint32(v):
    return b"\x0a" + struct.pack(">I", v)


def uuid16(v):
    return b"\x19" + struct.pack(">H", v)


def boolean(v):
    return bytes([0x28, 1 if v else 0])


def text(s):
    data = s if isinstance(s, bytes) else s.encode()
    return bytes([0x25, len(data)]) + data


def seq(*items):
    body = b"".join(items)
    if len(body) < 256:
        return bytes([0x35, len(body)]) + body
    return b"\x36" + struct.pack(">H", len(body)) + body


L2CAP, HIDP, HID_SERVICE, PUBLIC_BROWSE = 0x0100, 0x0011, 0x1124, 0x1002
HANDLE = 0x00010001
# A vendor-defined report of one byte: hidraw only, no input device.
HID_DESCRIPTOR = bytes.fromhex("0600ff0901a10115002600ff7508950109018102c0")
RECORD_UUIDS = {L2CAP, HIDP, HID_SERVICE, PUBLIC_BROWSE}
RECORD = seq(
    uint16(0x0000), uint32(HANDLE),
    uint16(0x0001), seq(uuid16(HID_SERVICE)),
    uint16(0x0004), seq(seq(uuid16(L2CAP), uint16(PSM_HID_CONTROL)), seq(uuid16(HIDP))),
    uint16(0x0005), seq(uuid16(PUBLIC_BROWSE)),
    uint16(0x0006), seq(uint16(0x656E), uint16(0x006A), uint16(0x0100)),
    uint16(0x0009), seq(seq(uuid16(HID_SERVICE), uint16(0x0101))),
    uint16(0x000D), seq(seq(seq(uuid16(L2CAP), uint16(PSM_HID_INTERRUPT)), seq(uuid16(HIDP)))),
    uint16(0x0100), text(NAME),
    uint16(0x0201), uint16(0x0111),
    uint16(0x0202), uint8(0x00),
    uint16(0x0203), uint8(0x00),
    uint16(0x0204), boolean(True),
    uint16(0x0205), boolean(True),
    uint16(0x0206), seq(seq(uint8(0x22), text(HID_DESCRIPTOR))),
    uint16(0x0207), seq(seq(uint16(0x0409), uint16(0x0100))),
    uint16(0x020B), uint16(0x0100),
    uint16(0x020C), uint16(0x0C80),
    uint16(0x020D), boolean(True),
    uint16(0x020E), boolean(False),
)

BASE_UUID_TAIL = bytes.fromhex("00001000800000805F9B34FB")


def uuids_in(data):
    """The UUIDs of a search pattern (a sequence of UUIDs), as 16- or 32-bit numbers where they can be."""
    found, i = set(), 0
    if data[:1] == b"\x35":
        i = 2
    elif data[:1] == b"\x36":
        i = 3
    while i < len(data):
        kind = data[i]
        if kind == 0x19:
            found.add(struct.unpack(">H", data[i + 1:i + 3])[0])
            i += 3
        elif kind == 0x1A:
            found.add(struct.unpack(">I", data[i + 1:i + 5])[0])
            i += 5
        elif kind == 0x1C:
            full = data[i + 1:i + 17]
            found.add(struct.unpack(">I", full[:4])[0] if full[4:] == BASE_UUID_TAIL else full)
            i += 17
        else:
            break
    return found


def pattern_length(data):
    if data[:1] == b"\x35":
        return 2 + data[1]
    if data[:1] == b"\x36":
        return 3 + struct.unpack(">H", data[1:3])[0]
    return 0


def sdp_answer(request):
    """One SDP request, answered: our one record for any search it matches; errors for the rest."""
    if len(request) < 5:
        return None
    pdu, tid = request[0], request[1:3]
    params = request[5:]
    if pdu == 0x02:  # ServiceSearchRequest
        n = pattern_length(params)
        handles = [HANDLE] if uuids_in(params[:n]) <= RECORD_UUIDS else []
        body = struct.pack(">HH", len(handles), len(handles)) + b"".join(struct.pack(">I", h) for h in handles) + b"\x00"
        reply = 0x03
    elif pdu == 0x04:  # ServiceAttributeRequest
        body = struct.pack(">H", len(RECORD)) + RECORD + b"\x00"
        reply = 0x05
    elif pdu == 0x06:  # ServiceSearchAttributeRequest
        n = pattern_length(params)
        lists = seq(RECORD) if uuids_in(params[:n]) <= RECORD_UUIDS else seq()
        body = struct.pack(">H", len(lists)) + lists + b"\x00"
        reply = 0x07
    else:
        body, reply = struct.pack(">H", 0x0003), 0x01  # an error: invalid syntax
    return bytes([reply]) + tid + struct.pack(">H", len(body)) + body


HCI_CHANNEL_CONTROL, HCI_DEV_NONE = 3, 0xFFFF
MGMT_EV_USER_CONFIRM_REQUEST, MGMT_OP_USER_CONFIRM_REPLY = 0x000F, 0x001C


def management():
    """The kernel's management interface (what btmgmt and bluetoothd use), to answer pairing requests."""
    libc = ctypes.CDLL(None, use_errno=True)
    fd = libc.socket(AF_BLUETOOTH, socket.SOCK_RAW | socket.SOCK_CLOEXEC, BTPROTO_HCI)
    address = struct.pack("<HHH", AF_BLUETOOTH, HCI_DEV_NONE, HCI_CHANNEL_CONTROL)
    if fd < 0 or libc.bind(fd, address, len(address)) != 0:
        sys.exit(f"no management socket ({os.strerror(ctypes.get_errno())})")
    return Fd(fd)


class Fd:
    """A socket Python's socket module can't wrap (it asks its address), for select, read and write."""

    def __init__(self, fd):
        self.fd = fd

    def fileno(self):
        return self.fd

    def recv(self, n):
        return os.read(self.fd, n)

    def send(self, data):
        return os.write(self.fd, data)


def agent(mgmt):
    """Says yes to "just works" pairing (the confirmation the kernel asks of hci1's user space)."""
    data = mgmt.recv(1024)
    if len(data) < 6:
        return
    event, index, _ = struct.unpack("<HHH", data[:6])
    if event == MGMT_EV_USER_CONFIRM_REQUEST and index == INDEX:
        peer = data[6:13]  # address and its type
        mgmt.send(struct.pack("<HHH", MGMT_OP_USER_CONFIRM_REPLY, INDEX, len(peer)) + peer)
        log("pairing: confirmed")


def listener(address, psm):
    s = socket.socket(AF_BLUETOOTH, socket.SOCK_SEQPACKET, socket.BTPROTO_L2CAP)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((address, psm))
    s.listen(4)
    return s


def serve(address, servers, mgmt):
    peers = {}
    log(f"{NAME} ({address}): SDP and HID listening")
    state("ready")
    while True:
        readable, _, _ = select.select([mgmt] + list(servers) + list(peers), [], [])
        for s in readable:
            if s is mgmt:
                agent(mgmt)
                continue
            if s in servers:
                conn, remote = s.accept()
                peers[conn] = servers[s]
                log(f"psm {servers[s]}: connected from {remote[0]}")
                continue
            psm = peers[s]
            try:
                data = s.recv(4096)
            except OSError:
                data = b""
            if not data:
                log(f"psm {psm}: closed")
                del peers[s]
                s.close()
                continue
            if psm == PSM_SDP:
                answer = sdp_answer(data)
                if answer:
                    s.send(answer)
            elif psm == PSM_HID_CONTROL and data[0] >> 4 in (0x4, 0x5, 0x6, 0x7, 0x9):
                # GET/SET_REPORT, GET/SET_PROTOCOL, DATA on control: "successful" (a handshake).
                if data[0] >> 4 == 0x4:
                    s.send(bytes([0xA1, 0x00]))  # GET_REPORT: a DATA input report
                else:
                    s.send(b"\x00")


STATE = "/run/thor-bt-device"


def state(text):
    """Where it is, for tools/vm/vm.py bluetooth: waiting (for bluetoothd), ready."""
    with open(STATE, "w") as f:
        f.write(text + "\n")


def wait_for_bluetoothd():
    while subprocess.run(["systemctl", "-q", "is-active", "bluetooth"]).returncode != 0:
        time.sleep(0.2)
    time.sleep(1)


def main():
    # The listeners before bluetoothd's: its SDP server listens on every adapter's address, and of two
    # listeners the kernel gives a connection to the older one.
    address = address_of(INDEX)
    servers = {listener(address, psm): psm for psm in (PSM_SDP, PSM_HID_CONTROL, PSM_HID_INTERRUPT)}
    mgmt = management()
    log("waiting for bluetoothd")
    state("waiting")
    wait_for_bluetoothd()
    take_from_bluetoothd()
    for args in (["io-cap", "3"], ["le", "off"], ["power", "on"], ["class", "5", "0x40"], ["name", NAME],
                 ["connectable", "on"], ["bondable", "on"], ["discov", "on"]):
        btmgmt(*args)
    serve(address, servers, mgmt)


if __name__ == "__main__":
    main()
