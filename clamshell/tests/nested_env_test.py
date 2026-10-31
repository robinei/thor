"""Deadline fixtures for the nested harness, without a compositor or graphics tools."""
import os
from pathlib import Path
import signal
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import nested_env
from nested_env import Nested


class Deadlines(unittest.TestCase):
    def harness(self, directory):
        n = Nested.__new__(Nested)
        n.directory = Path(directory)
        n.processes = []
        n.expected_exits = set()
        n.host = {**os.environ, "PATH": directory + os.pathsep + os.environ["PATH"]}
        return n

    def test_stalled_probes_are_stopped_and_report_the_active_wait(self):
        for probe in [lambda n: n.image(), lambda n: n.shot("stalled"), lambda n: n.control("window", "events")]:
            with self.subTest(probe=probe), tempfile.TemporaryDirectory() as directory:
                n = self.harness(directory)
                pid_file = n.directory / "pid"
                stalled = n.directory / "grim"
                stalled.write_text(f"#!{sys.executable}\nimport os, time\nfrom pathlib import Path\nPath({str(pid_file)!r}).write_text(str(os.getpid()))\ntime.sleep(30)\n")
                stalled.chmod(0o755)
                start = time.monotonic()
                try:
                    with patch.object(nested_env, "BIN", stalled):
                        with self.assertRaisesRegex(RuntimeError, "timed out: stalled fixture"):
                            n.wait(lambda: probe(n), "stalled fixture", seconds=1)
                    self.assertLess(time.monotonic() - start, 3)
                    self.assertIsNone(n._waiting)
                    pid = int(pid_file.read_text())
                    with self.assertRaises(ProcessLookupError):
                        os.kill(pid, 0)
                finally:
                    if pid_file.exists():
                        try:
                            os.kill(int(pid_file.read_text()), signal.SIGKILL)
                        except ProcessLookupError:
                            pass

    def test_late_success_is_rejected_and_wait_context_is_restored(self):
        with tempfile.TemporaryDirectory() as directory:
            n = self.harness(directory)
            def late():
                time.sleep(0.05)
                return True
            with self.assertRaisesRegex(RuntimeError, "timed out: late fixture"):
                n.wait(late, "late fixture", seconds=0.01)
            self.assertIsNone(n._waiting)
            self.assertEqual(n.wait(lambda: "ready", "ready fixture"), "ready")
            self.assertIsNone(n._waiting)

    def test_a_nested_wait_cannot_extend_the_outer_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            n = self.harness(directory)
            start = time.monotonic()
            with self.assertRaisesRegex(RuntimeError, "timed out: inner fixture"):
                n.wait(lambda: n.wait(lambda: False, "inner fixture", seconds=30), "outer fixture", seconds=0.05)
            self.assertLess(time.monotonic() - start, 1)
            self.assertIsNone(n._waiting)


if __name__ == "__main__":
    unittest.main()
