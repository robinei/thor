# Robustness plan

From `critique-2026-10.md` (2026-10-22): points 4-6 (hand-written code, parsed tool output, silent
fallbacks) and 9 (the comment style). Each step is a commit or a few that pass `tools/check.sh`.

## 1. Services over D-Bus instead of their command-line tools

The `dbus` crate is already used; these tools are front ends to services it can call directly, with typed
replies and real errors.

- [x] `timedatectl` → `org.freedesktop.timedate1` (`Timezone`, `NTP`; `SetTimezone`, `SetNTP`).
- [x] `udisksctl` → `org.freedesktop.UDisks2` (`Filesystem.Mount`/`Unmount`, `Drive.PowerOff`).
- [x] `systemctl` (archives: `is-active`, stopping those left before) → `org.freedesktop.systemd1`, as
      scopes and the freezer already were. `systemd-run` stays: it starts programs (a launch's scope
      around `exec`, an archive's service) and nothing it prints is read.
- [x] `wpctl get-volume` → the default sink's volume and mute from `pw-dump`'s JSON, which the mixer reads
      already.

Staying: `pw-dump` (JSON), `grim` (only its exit status), `fc-match` (a path, once), `bsdtar`, and the
programs started rather than read (`systemd-run`, `nice`, `pkexec`, `fusermount3`, `foot`).

## 2. Crates for hand-written code

- [x] MD5 and CRC32 (`clamshell_library::hash`) → `md5` and `crc32fast`. FNV stays (a few lines).
- [x] Percent encoding (`os/text`) → `percent-encoding`.
- [x] Icon lookup: looked at `freedesktop-icons`, kept ours. The crate brings about 20 crates, reads only
      the XDG dirs from the environment (the tray needs a program's own theme path, and the tests pass
      temporary folders), and ours covers how apps install their icons.
- [x] sway's IPC: looked at `swayipc`, kept ours. The framing is about 60 lines; the replies are already
      typed with serde, reading only the fields used, and tested.

Staying, with reasons:

- HTTP through `curl`: a Rust client brings a TLS stack (rustls, ring, webpki roots) for a few requests a
  day; curl is on every install. Its failures get logged (step 3).
- The sensor hub's protobuf and QMI framing: a handful of fields of one undocumented message; protobuf
  codegen would add a build step for that.
- The Waydroid Wayland proxy's wire format: a proxy that filters messages, which the Wayland crates don't
  offer.
- Steam's VDF and desktop files: small, tested, and the parsers' corner cases are covered.

## 3. No silent failures

Policy: don't crash unless there is no way on; a failure that is handled by falling back is logged.
`warn` for what shouldn't happen, `debug` for the expected (an optional file that isn't there, a device
that isn't present). Fallbacks on indexes valid by construction stay quiet, as core's crate doc says.

- [x] No helper: `.inspect_err(|e| log::warn!("reading {}: {e}", path.display())).ok()` says it in place.
      A failure that repeats on a timer (a reading every few seconds) logs at `debug`, so the log isn't
      flooded.
- [x] system: discarded `Result`s (`.ok()`, `let _ =`, `if let Ok` with no else, `unwrap_or*` on a
      `Result`) logged.
- [x] ui, binary, fan, library: the same.
- [x] core: fallbacks on `Option`s that a bug could reach: `debug_assert!` or a log.

Found on the way: a folder copy skipped entries it couldn't list and reported success, losing them on a
cut across filesystems (now it fails); a poisoned lock stopped metadata fetching for good (now recovered).
Open, as they change behaviour:

- [x] The control pipe's reader thread ends for good if the pipe can't be reopened (`clamshell ctl` stops
      working until a restart).
- [x] The fan daemon takes an unreadable `pwm1` as 0, which feeds the curve.
- [x] Emptying the Trash says "Emptied the Trash" when some entries couldn't be removed.
- [ ] `content::scan` expands a source's dir a second time after its caller did.

## 4. Comments a person would write

Plain sentences with articles, one idea each; no colon chains or parentheses stacked in parentheses.
Rewrite what is hard to read and leave what reads well; don't add comments or lengthen them.

- [x] core
- [x] system
- [x] ui
- [x] binary, library, fan

About 3,000 comments rewritten, the rest left; about 45 found contradicting the code and corrected (one, a
long press locking a keyboard modifier, has a commit of its own). Only comment lines changed.
