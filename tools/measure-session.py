#!/usr/bin/env python3
"""Measure the optimised nested compositor on a real GPU, in the isolated real-client harness.

Build first: cargo build --manifest-path clamshell/Cargo.toml --profile fast
    tools/measure-session.py --output /tmp/thor-session.json

The fixture is 1,000 local files without metadata lookups, at 1920x1080 + 1240x1080,
60 Hz. The first launch creates saved defaults; subsequent launches reuse them and the
shader cache. Idle samples exclude the host sway and private bus. No owner apps are stopped.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "clamshell/tests"))
import nested_env


def memory(pid):
    return {key: int(value.split()[0]) for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines()
            if re.fullmatch(r"[A-Za-z_]+:\s+\d+ kB", line) for key, value in [line.split(":", 1)]}


def threads(pid):
    found = {}
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:
            status = dict(line.split(":", 1) for line in (task / "status").read_text().splitlines() if ":" in line)
            found[int(task.name)] = {
                "name": (task / "comm").read_text().strip(),
                "cpu_ns": int((task / "schedstat").read_text().split()[0]),
                "voluntary": int(status["voluntary_ctxt_switches"]),
                "involuntary": int(status["nonvoluntary_ctxt_switches"]),
            }
        except FileNotFoundError:
            pass  # A short-lived worker ended during this snapshot.
    return found


def gpu_memory(pid):
    clients = {}
    for file in Path(f"/proc/{pid}/fdinfo").iterdir():
        try:
            fields = dict(line.split(":", 1) for line in file.read_text().splitlines() if ":" in line)
        except FileNotFoundError:
            continue
        if "drm-client-id" not in fields:
            continue
        # Duplicate fds of one DRM client are one allocation account.
        key = (fields.get("drm-pdev", "").strip(), fields["drm-client-id"].strip())
        clients[key] = {k: v.strip() for k, v in fields.items() if k.startswith("drm-")}
    return list(clients.values())


def sample(pid, seconds):
    before = threads(pid)
    gpu_before = gpu_memory(pid)
    began = time.monotonic()
    time.sleep(seconds)
    after = threads(pid)
    elapsed = time.monotonic() - began
    gpu_after = gpu_memory(pid)
    engines = {}
    for end in gpu_after:
        start = next((row for row in gpu_before if (row.get("drm-pdev"), row["drm-client-id"]) == (end.get("drm-pdev"), end["drm-client-id"])), {})
        for key, value in end.items():
            if key.startswith("drm-engine-") and key in start and value.endswith(" ns"):
                delta = int(value.split()[0]) - int(start[key].split()[0])
                engines[key] = engines.get(key, 0) + delta / (elapsed * 1e9) * 100
    if before.keys() != after.keys():
        raise RuntimeError("threads changed during idle sampling; wait for startup work to settle")
    rows = []
    for tid, end in after.items():
        start = before[tid]
        if start["name"] != end["name"]:
            raise RuntimeError("thread identity changed during idle sampling")
        cpu = end["cpu_ns"] - start["cpu_ns"]
        voluntary = end["voluntary"] - start["voluntary"]
        involuntary = end["involuntary"] - start["involuntary"]
        rows.append({"tid": tid, "name": "main" if tid == pid else end["name"],
                     "cpu_ms": cpu / 1e6, "cpu_percent_one_core": cpu / (elapsed * 1e9) * 100,
                     "voluntary_switches": voluntary, "involuntary_switches": involuntary})
    return {"seconds": elapsed, "cpu_percent_one_core": sum(r["cpu_percent_one_core"] for r in rows),
            "voluntary_switches_per_second": sum(r["voluntary_switches"] for r in rows) / elapsed,
            "threads": sorted(rows, key=lambda r: r["cpu_ms"], reverse=True), "memory_kib": memory(pid),
            "drm_clients": gpu_after, "gpu_engine_percent": engines}


class Startup:
    """Observe READY=1 directly and composition log lines within a two-millisecond polling interval."""
    def __init__(self, session):
        self.session = session
        self.notify = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        self.path = session.directory / "measurement-notify"
        self.notify.bind(str(self.path))
        self.notify.settimeout(0.002)
        session.env["NOTIFY_SOCKET"] = str(self.path)
        self.original = session.spawn
        session.spawn = self.spawn
        self.observer = None
        self.stopped = threading.Event()
        self.trials = []
        self.failure = None

    def spawn(self, argv, name, environment, stdin=None):
        if name == "compositor":
            # Discard previous STATUS messages and logs before starting the next clock.
            self.notify.setblocking(False)
            try:
                while self.notify.recv(4096):
                    pass
            except BlockingIOError:
                pass
            self.notify.settimeout(0.002)
            (self.session.directory / "compositor.log").write_text("")
            trial = {"fresh_saved_defaults": not self.trials}
            self.trials.append(trial)
            began = time.monotonic_ns()
            self.observer = threading.Thread(target=self.observe, args=(trial, began), daemon=True)
            self.observer.start()
        return self.original(argv, name, environment, stdin)

    def observe(self, trial, began):
        try:
            while not self.stopped.is_set() and time.monotonic_ns() - began < 20_000_000_000:
                try:
                    message = self.notify.recv(4096)
                    if b"READY=1" in message.splitlines() and "ready_ms" not in trial:
                        trial["ready_ms"] = (time.monotonic_ns() - began) / 1e6
                except socket.timeout:
                    pass
                log = self.session.log()
                for screen in ["Top", "Bottom"]:
                    key = screen.lower() + "_composed_ms"
                    if key not in trial and f"compositor: Screen({screen}) composed" in log:
                        trial[key] = (time.monotonic_ns() - began) / 1e6
                library = re.search(r"rescan: (\d+) entries", log)
                if "library_read_ms" not in trial and library:
                    trial["library_read_ms"] = (time.monotonic_ns() - began) / 1e6
                    trial["library_entries"] = int(library.group(1))
                    if trial["library_entries"] < 1000:
                        raise RuntimeError("the fixture's library was not loaded")
                if all(key in trial for key in ["ready_ms", "top_composed_ms", "bottom_composed_ms", "library_read_ms"]):
                    return
            raise RuntimeError(f"startup observations incomplete: {trial}")
        except Exception as error:
            self.failure = error

    def finish(self):
        self.observer.join(timeout=21)
        if self.failure:
            raise self.failure
        if self.observer.is_alive():
            raise RuntimeError("startup observer did not finish")

    def close(self):
        self.stopped.set()
        if self.observer:
            self.observer.join(timeout=1)
        self.notify.close()


def run(args):
    nested_env.BIN = args.binary.resolve()
    n = nested_env.Nested(gpu=True, prefix="thor-measure-")
    n.env["CLAMSHELL_DEVICE"] = "generic"
    n.env["RUST_LOG"] = "info,tracing=warn"
    if args.render_node:
        n.env["WLR_RENDER_DRM_DEVICE"] = str(args.render_node.resolve())
    n.sway_config.write_text('output HEADLESS-1 resolution 3160x1080@60Hz\ndefault_border none\n'
                             'for_window [title="ClamShell nested"] fullscreen enable\n')
    games = n.home / "games"
    games.mkdir()
    for i in range(1000):
        (games / f"Game {i:04d}.rom").touch()
    config = {"places": [{"id": "games", "name": "Games", "icon": "gamepad"}],
              "runners": [{"id": "test", "name": "Test", "match": ["*.rom"], "launch": ["true"]}],
              "sources": [{"type": "dir", "id": "games", "dir": str(games), "place": "games", "runner": "test"}]}
    (n.directory / "config/clamshell/library.json").write_text(json.dumps(config))
    startup = Startup(n)
    try:
        with n:
            startup.finish()
            for _ in range(args.starts - 1):
                old = n.compositor
                n.expected_exits.add(old)
                os.killpg(old.pid, signal.SIGTERM)
                old.wait(timeout=5)
                n.compositor = n.spawn([str(nested_env.BIN), "--nested"], "compositor", n.host)
                startup.finish()
            n.control("tab", args.tab)
            print("startup trials complete; settling for 10 seconds", flush=True)
            time.sleep(10)
            n.shot("idle")
            result = {"date": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                      "binary": str(nested_env.BIN.relative_to(ROOT)) if nested_env.BIN.is_relative_to(ROOT) else str(nested_env.BIN),
                      "kernel": os.uname().release,
                      "cpu": next(line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
                      "renderer": [line for line in n.log().splitlines() if "GL renderer:" in line or "GL Renderer:" in line],
                      "bottom_tab": args.tab,
                      "fixture": "1000 local ROM entries; no metadata requests or client apps; 3160x1080 at 60 Hz, hardware GPU, generic device profile",
                      "startup": startup.trials,
                      "startup_observation": "monotonic time before Popen; READY=1 datagram and composition log observation (2 ms polling), not physical presentation",
                      "wakeups": "voluntary context switches are a proxy, not scheduler wakeups; kernel.sched_schedstats=" + Path("/proc/sys/kernel/sched_schedstats").read_text().strip(),
                      "samples": []}
            for i in range(args.samples):
                print(f"idle sample {i + 1}/{args.samples}: {args.seconds:g} seconds", flush=True)
                result["samples"].append(sample(n.compositor.pid, args.seconds))
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            shutil.copy(n.directory / "idle.png", args.output.with_suffix(".png"))
            shutil.copy(n.directory / "compositor.log", args.output.with_suffix(".log"))
            print(f"measurements: {args.output}", flush=True)
    finally:
        startup.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "clamshell/target/fast/clamshell")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--render-node", type=Path)
    parser.add_argument("--tab", choices=["sysinfo", "dock"], default="sysinfo")
    parser.add_argument("--starts", type=int, default=10)
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--seconds", type=float, default=30)
    args = parser.parse_args()
    if args.starts < 1 or args.samples < 1 or args.seconds <= 0:
        parser.error("starts, samples and seconds must be positive")
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    run(args)
