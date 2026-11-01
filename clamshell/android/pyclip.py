# A stand-in for pyclip, which Waydroid's clipboard sharing imports (its session is started with this on
# its Python path by clamshell-android): the clipboard through ClamShell, which keeps it
# (`clamshell --clipboard get|set`; CLAMSHELL: ClamShell's program), no other package needed.
#
# It never raises: an exception would end Waydroid's clipboard thread for the session. A ClamShell that
# doesn't answer, or a program gone (replaced by an update), is said on standard error (Waydroid's log),
# and the clipboard reads as empty.
import os
import subprocess
import sys

_CLAMSHELL = os.environ.get("CLAMSHELL", "clamshell")
# How long ClamShell may take to answer, in seconds.
_TIMEOUT = 10


def _run(what, **how):
    try:
        return subprocess.run([_CLAMSHELL, "--clipboard", what], timeout=_TIMEOUT, check=False, **how).stdout or b""
    except (subprocess.SubprocessError, OSError) as e:
        print(f"clamshell pyclip: the clipboard's {what}: {e}", file=sys.stderr)
        return b""


def copy(data):
    if isinstance(data, str):
        data = data.encode()
    _run("set", input=data, stdout=subprocess.DEVNULL)


def paste(text=True, **_):
    out = _run("get", capture_output=True)
    return out.decode(errors="replace") if text else out
