# A stand-in for pyclip, which Waydroid's clipboard sharing imports (its session is started with this on
# its Python path by clamshell-android): the clipboard through ClamShell, which keeps it
# (`clamshell --clipboard get|set`; CLAMSHELL: ClamShell's program), no other package needed.
import os
import subprocess

_CLAMSHELL = os.environ.get("CLAMSHELL", "clamshell")


def copy(data):
    if isinstance(data, str):
        data = data.encode()
    subprocess.run([_CLAMSHELL, "--clipboard", "set"], input=data, timeout=10, check=False)


def paste(text=True, **_):
    out = subprocess.run([_CLAMSHELL, "--clipboard", "get"], capture_output=True, timeout=10, check=False).stdout
    return out.decode(errors="replace") if text else out
