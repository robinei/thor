# Robustness plan

From `critique-2026-10.md` (2026-10-22): points 4-6 (hand-written code, parsed tool output, silent
fallbacks) and 9 (the comment style). Each step is a commit or a few that pass `tools/check.sh`.

## 1. Services over D-Bus instead of their command-line tools

The `dbus` crate is already used; these tools are front ends to services it can call directly, with typed
replies and real errors.

- [x] `timedatectl` → `org.freedesktop.timedate1` (`Timezone`, `NTP`; `SetTimezone`, `SetNTP`).
- [ ] `udisksctl` → `org.freedesktop.UDisks2` (`Filesystem.Mount`/`Unmount`, `Drive.PowerOff`).
- [ ] `systemd-run` and `systemctl` → `org.freedesktop.systemd1` (`StartTransientUnit`, `StopUnit`, the
      unit's `FreezerState`).
- [ ] `wpctl get-volume` → the default sink's volume and mute from `pw-dump`'s JSON, which the mixer reads
      already.

Staying: `pw-dump` (JSON), `grim` (only its exit status), `fc-match` (a path, once), `bsdtar`, and the
programs started rather than read (`nice`, `pkexec`, `fusermount3`, `foot`).

## 2. Crates for hand-written code

- [ ] MD5 and CRC32 (`clamshell_library::hash`) → `md5` and `crc32fast`. FNV stays (a few lines).
- [ ] Percent encoding (`os/text`) → `percent-encoding`.
- [ ] Icon lookup (`os/xdg::find_icon`: three themes and two layouts guessed) → `freedesktop-icons`, which
      follows the icon theme spec (`index.theme`, inheritance, the user's theme), if its dependencies are
      reasonable.
- [ ] sway's IPC (`sway/ipc`) → `swayipc` (blocking, typed replies and events), if it fits how the sway
      thread polls.

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

- [ ] A helper for the common case (a `Result` turned into an `Option` or a default, with the failure
      logged and what was being done).
- [ ] system: discarded `Result`s (`.ok()`, `let _ =`, `if let Ok` with no else, `unwrap_or*` on a
      `Result`) logged.
- [ ] ui, binary, fan, library: the same.
- [ ] core: fallbacks on `Option`s that a bug could reach: `debug_assert!` or a log.

## 4. Comments a person would write

Plain sentences with articles, one idea each; no colon chains or parentheses stacked in parentheses.
Rewrite what is hard to read and leave what reads well; don't add comments or lengthen them.

- [ ] core
- [ ] system
- [ ] ui
- [ ] binary, library, fan
