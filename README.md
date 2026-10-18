# thor (working name)

Linux on the AYN Thor: ClamShell, the program that runs the device's session, plus the setup around it
(Arch Linux ARM, packages built on the device). What to do when the device arrives, and what is left to
test on it: `docs/thor-gameplan.md`.

This README is kept short while things change; how each part behaves is in its module's documentation
(`cargo doc --open` in `clamshell/`) and its tests.

## ClamShell

`clamshell/` is one process with one in-memory state for shelf (the game list on the top screen), the
second screen (the bar and its tabs), the input path (the controller, the pad mouse, the touchpad, the
on-screen keyboard), and the games' settings and keymaps. Rust, a Cargo workspace:

| Crate | What |
|---|---|
| `crates/core` | the state and its rules: events in, dirty screens and effects out; no I/O |
| `crates/system` | threads that watch the system and send events, and carry out effects (sway, systemd, NetworkManager, BlueZ, evdev/uinput, sysfs); the game library from disk |
| `crates/library` | the library's model (entries, groups, tabs), sorting, name patterns, and the parts of Steam's formats that need no disk |
| `crates/ui` | Wayland, CPU drawing (tiny-skia, fontdue), the views, pad navigation; the font built in (Noto Sans, SIL Open Font License 1.1: `crates/ui/fonts/OFL.txt`) |
| `src/` | the program around them: the command line, the environment, the threads, the control pipe, the snapshots |

No panics by construction: `unwrap`, `expect`, `panic!` and slice indexing are denied outside tests.

## The library

Shelf's tabs come from `~/.config/clamshell/library.json`, installed Steam games, and desktop files
(`Categories=Game` in Games, the rest in Apps). Everything opens by a runner: a name, the file names it
opens, its command (`{path}` the file), and dirs it scans into the library (each into a tab, maybe a folder
of it). ClamShell has its own runners (one per RetroArch system with its core installed, Video and Music in
mpv, Images in imv, PDF in zathura, Text in `less`, and last Other files with `xdg-open`; Desktop and Steam
for the desktop files' apps and Steam's games, each by its own command). A runner's `id`
keeps its settings (its `name` is only shown); one in library.json with the id of one of ClamShell's
(`snes`, `video`, `text`: see `docs/runners.md`) takes what it leaves out from it:

```json
{ "runners": [
    { "id": "snes", "scan": [{ "id": "snes", "dir": "~/Games/snes", "tab": "Games", "icon": "gamepad", "group": "SNES",
      "match": ["*.sfc", "*.smc", "*.zip"] }] },
    { "id": "video", "scan": [{ "id": "tv", "dir": "~/TV", "tab": "TV", "icon": "tv", "match": ["*/"], "items": ["*.mkv", "*.mp4"],
      "metadata": "tmdb:tv" }] },
    { "id": "text", "name": "Kate", "match": ["*.txt", "*.md"], "launch": ["kate", "{path}"] }
] }
```

A scan's `match` (paths in its dir, a part per folder level: `*/*.cue`) says what is an entry, else the
runner's patterns; `items` makes each matched folder a group that opens in shelf (a series, its seasons,
their episodes); `group` puts its entries in a folder of the tab (`"SNES"`, or `"Handhelds/GBA"` for one in
another). A file opened from the Files tab opens by the first runner whose patterns match its name. The
design: `docs/runners.md`.

Desktop files' apps and games go in Games (`Categories=Game`) or Apps unless `desktop` rules say otherwise:
the first rule matching an entry's desktop id, name or categories (globs, any case) puts it in a tab or
hides it (`group` puts it in a folder of the tab, as a scan's does), e.g.
`{ "id": "net.lutris.*", "tab": "Games", "group": "Launchers" }`, `{ "name": "*Uninstall*", "hide": true }`. Steam's installed games go
in Games unless `steam` rules say otherwise, matched by name or app id (`id`) the same way:
`"steam": [{ "name": "Dark Souls*", "group": "Steam/Souls" }, { "group": "Steam" }]`. In every list,
folders come first.
The rules (ids, titles, art, icons): `crates/system/src/library/content.rs`.

Covers, screenshots, descriptions, developers and years are fetched in the background after start
(`clamshell --fetch-metadata` does it without the UI): a scan's or runner's `"metadata": "libretro:<system>"` (ClamShell's
RetroArch runners have theirs)
(libretro's thumbnails and database, by the file's checksum or by name), `"tmdb:movie"` or `"tmdb:tv"` (needs `"keys": { "tmdb": ... }`); Steam's
games from the store; the desktop files' games from SteamGridDB (needs `keys.steamgriddb`). Art beside a
file wins. libretro's lists are kept in `~/.cache/clamshell/libretro` for 30 days. The rules:
`crates/system/src/meta.rs`.

What ClamShell keeps (play time, favourites, settings, keymaps, window rules, the dock's pins, where shelf
was, brightness) is in one SQLite database, `~/.local/share/clamshell/clamshell.db`; its tables:
`crates/system/src/store.rs`.

## Running

    cd clamshell
    cargo run                         # two windows at half the Thor's size: top and bottom
    cargo run -- --snapshot DIR       # every screen as PNG files at the Thor's size, no Wayland
    cargo run -- ctl help             # the control pipe's commands (a running ClamShell takes them)
    tools/nested-session.sh           # as on the device: a nested sway in two windows (the Thor's screens
                                      # at their size), ClamShell in it with this desktop's library

In shelf, Y makes the selected entry a favourite (the Favourites tab) or not, and X pins it to the dock
or unpins it; a pinned folder (a series) opens in shelf from the dock.

On a desktop, keys stand in for the pad: arrows move the focus, Enter is A, Backspace B, Tab View, Q and
E the shoulders, F is Y, P is X.

## Checking

- `tools/check.sh`: formatting, clippy with warnings as errors, and the tests (the snapshots among them),
  about a minute. Every commit passes it: `git config core.hooksPath tools/hooks` makes it the
  pre-commit hook.
- Snapshots: the tests draw every screen and compare hashes with `clamshell/tests/snapshots.txt`. After
  a change meant to change what is drawn, look at the files (`--snapshot DIR`), then write the new
  hashes with `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`.
- `clamshell/tests/nested-sway.sh` (after `cargo build --release`): a headless sway with ClamShell in it,
  driven through the control pipe; about a minute.
- `tools/vm/vm-test.sh`: the VM suite end to end, about twenty minutes; before pushing. The virtual Thor is
  `tools/vm/vm.py` (QEMU/KVM, both screens, a fake controller with a gyro and rumble, simulated Wi-Fi and
  Bluetooth; `tools/vm/vm.py --help`), and its checks `tools/vm/vm-checks.py` (`--list`).
