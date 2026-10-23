# thor (working name)

[![check](https://github.com/robinei/thor/actions/workflows/check.yml/badge.svg)](https://github.com/robinei/thor/actions/workflows/check.yml)

Linux on the AYN Thor, a handheld with two screens. This repository holds ClamShell, the program that
runs the device's session, and the system around it: Arch Linux ARM with our own packages, built on a PC
and kept current by pacman. `docs/thor-gameplan.md` says what to do when the device arrives and what is
left to test on it.

This README is kept short while things change. How each part behaves is in its module's documentation
(`cargo doc --open --document-private-items` in `clamshell/`) and in its tests.

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
| `crates/fan` | the fan daemon `clamshell-fan`: a system service on the device, the fan's speed from the temperatures in the mode the Fan setting picks (its curves in `crates/fan/src/curve.rs`); and its library `clamshell_fan_mode`, the modes as ClamShell writes them, which core uses |
| `src/` | the program around them: the command line, the environment, the threads, the control pipe, the snapshots |

No panics by construction: `unwrap`, `expect`, `panic!` and slice indexing are denied outside tests.
Where a lookup can't fail by construction it falls back quietly; where a bug could reach the fallback, a
`debug_assert!` catches it in tests.

Beside the workspace are the configuration files the packages install with it: `sway/` (ClamShell's part
of sway's config), `foot/`, `mako/`, `mpv/` and `retroarch/` (those programs' settings for the device),
and `android/` (the Waydroid session's script and its clipboard bridge). `tests/` holds the nested sway
test and the snapshots' reference hashes.

## The library

Shelf comes from `~/.config/clamshell/library.json`, and nothing is there but what it declares:

- `places`: the tabs (in their order, with their icons; Recent, Favourites and Tools are places of a
  `type`) and folders in them, each with an id to refer to it by (`"@snes"`), and a folder's icon (its
  row's) and picture (shown large when it is selected). `"recent": false` keeps what is in a place out of
  Recent (apps).
- `runners`: how things open, each with an id (its settings are kept by it), a name (shown), the file
  names it opens and its command (`{path}` the file); a `desktop` and a `steam` runner open desktop files
  and Steam's games, by their own commands.
- `sources`: where entries come from, and where they go: `dir` (a dir scanned: what is an entry, its
  metadata, the runner that opens its entries if not by their names; `on` a removable filesystem, for
  ROMs on an SD card), `browse` (a dir listed as it is
  opened: the Files tab), `desktop` and `steam` (the installed apps and games: each taken by the first
  source of its type that matches it),
  `removable` (SD cards and USB sticks as they are mounted).

```json
{ "places": [
    { "id": "recent", "path": "Recent", "type": "recent", "icon": "clock" },
    { "id": "games", "path": "Games", "icon": "gamepad" },
    { "id": "apps", "path": "Apps", "icon": "apps", "recent": false },
    { "id": "snes", "path": "Games/SNES", "icon": "snes-icon.png", "picture": "snes.png" }
  ],
  "runners": [
    { "type": "desktop", "id": "desktop", "name": "Desktop", "match": ["*.desktop"] },
    { "id": "snes", "name": "SNES", "match": ["*.sfc", "*.smc"], "launch": ["retroarch", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{path}"] },
    { "id": "video", "name": "Video", "match": ["*.mkv", "*.mp4"], "launch": ["mpv", "{path}"] }
  ],
  "sources": [
    { "type": "dir", "id": "snes", "dir": "~/Games/snes", "place": "@snes", "runner": "snes", "match": ["*.sfc", "*.smc", "*.zip"],
      "metadata": "libretro:Nintendo - Super Nintendo Entertainment System" },
    { "type": "dir", "id": "tv", "dir": "~/TV", "place": "TV", "match": ["*/"], "items": ["*.mkv", "*.mp4"], "metadata": "tmdb:tv" },
    { "type": "desktop", "name": "*Uninstall*", "hide": true },
    { "type": "desktop", "categories": "Game", "place": "@games" },
    { "type": "desktop", "place": "@apps" }
] }
```

The starter library.json (`packages/thor-session/root/usr/share/thor/home/.config/clamshell/library.json`) declares ClamShell's set: Recent, Favourites,
Games, Films, TV, Files, Apps, Tools; a runner per RetroArch system, Video and Music (mpv), Images (imv),
PDF (zathura), Text (`less`), last Other files (`xdg-open`); a scan per system and for films and series,
Steam, the desktop files, Home, Root, the Trash and removable storage. In every list, folders come first.
A file opened from the Files tab opens by the first runner whose patterns match its name. The design:
`docs/runners.md`.
The rules (ids, titles, art, icons): `crates/system/src/library/content.rs`.

Covers, screenshots, descriptions, developers and years are fetched in the background after start
(`clamshell --fetch-metadata` does it without the UI): a scan's `"metadata": "libretro:<system>"`
(libretro's thumbnails and database, by the file's checksum or by name), `"tmdb:movie"` or `"tmdb:tv"` (needs `"keys": { "tmdb": ... }`); Steam's
games from the store; the desktop files' games from SteamGridDB (needs `keys.steamgriddb`). Art beside a
file wins. libretro's lists are kept in `~/.cache/clamshell/libretro` for 30 days. The rules:
`crates/system/src/meta.rs`.

What ClamShell keeps (play time, favourites, settings, keymaps, window rules, the dock's pins, where shelf
was, brightness) is in JSON files in `~/.local/share/clamshell`, readable and editable by hand; which
file holds what: `crates/system/src/store.rs`.

## Running

    cd clamshell
    cargo run                                   # two windows at half the Thor's size: top and bottom
    cargo run --features dev -- --snapshot DIR  # every screen as PNG files at the Thor's size, no Wayland
    cargo run -- ctl help                       # the control pipe's commands (a running ClamShell takes them)
    tools/nested-session.sh                     # as on the device: a nested sway in two windows (the Thor's
                                                # screens at their size), with this desktop's library

In shelf, Y makes the selected entry a favourite (the Favourites tab) or not, and X pins it to the dock
or unpins it; a pinned folder (a series) opens in shelf from the dock.

On a desktop, keys stand in for the pad: arrows move the focus, Enter is A, Backspace B, Tab View, Q and
E the shoulders, F is Y, P is X.

## Checking

- `tools/check.sh`: formatting, clippy with warnings as errors (the `dev` build too), the tests (the
  snapshots among them), and the documentation; about a minute. Every commit passes it:
  `git config core.hooksPath tools/hooks` makes it the pre-commit hook.
- Snapshots: the tests draw every screen and compare hashes of their pixels with
  `clamshell/tests/snapshots.txt`; on a difference the files drawn are kept and the failure says where.
  After a change meant to change what is drawn, look at the files (`cargo run --features dev --
  --snapshot DIR`), then write the new hashes with `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell
  snapshots`.
- `clamshell/tests/nested-sway.sh` (after `cargo build --release`): a headless sway with ClamShell in it,
  driven through the control pipe; about a minute.
- `tools/vm/vm-test.sh`: the VM suite end to end, about twenty minutes; before pushing. The virtual Thor is
  `tools/vm/vm.py` (QEMU/KVM, both screens, a fake controller with a gyro and rumble, simulated Wi-Fi and
  Bluetooth; `tools/vm/vm.py --help`), and its checks `tools/vm/vm-checks.py` (`--list`).

## The device's system

Arch Linux ARM and the Thor's own packages (`packages/`: `thor`, the metapackage that is the whole
system, and what it holds: `linux-thor`, `thor-system`, `thor-session`, `clamshell`, `thor-ucm`,
`thor-keyring`; and `thor-firmware`, which is never published), kept current with
`pacman -Syu` from the repository `[thor]`, the GitHub release `packages`. What each holds:
`docs/thor-gameplan.md`, "Our image and packages". On the device, `.claude/commands/maintain.md` is the
playbook for a maintenance pass with Claude Code, which the dock's Maintenance app starts: news,
updates, the boot, the firmware, pacnew files and health.

From a checkout, on an Arch Linux PC, everything is built here and nothing needs the virtual machines:

    tools/host-check.sh kernel packages image  # what this PC lacks, and the pacman line that adds it
    tools/image/image.sh                       # the Thor's SD card image: the packages (the kernel
                                               # among them), then pacstrap through qemu-user (sudo)
    tools/image/image.sh vm                    # the same system for x86_64, to try the first boot:
    tools/vm/vm.py thor-image                  #   in QEMU, a window per screen (--fresh: from scratch)
    tools/image/upgrade-test.sh                # a kernel update tried on a copy of the Thor's image

The steps on their own: `tools/repo/build.sh [PACKAGE...]` (the packages, from the last commit: ClamShell
cross-compiled against `tools/repo/sysroot.sh`'s sysroot of Arch Linux ARM's packages, linux-thor built
from Armada's pinned commit), `tools/repo/follow-armada.sh [COMMIT]` (linux-thor moved to another of
Armada's commits), `tools/repo/repo.sh publish` (the
repository uploaded with `gh`: only with the signing key, `~/.config/thor-repo/gnupg`, which isn't in the
repository; without it the packages are indexed unsigned and images still build). Arch Linux ARM's
public keys are in `tools/image/keyrings`.

## License

Ours: MIT or Apache-2.0, at your choice (`LICENSE-MIT`, `LICENSE-APACHE`). Not ours, under their own:

- Noto Sans (`clamshell/crates/ui/fonts`, built into ClamShell): SIL Open Font License 1.1 (`OFL.txt`).
- The Rust crates ClamShell's programs are built with: their licenses (all permissive: MIT, Apache-2.0,
  BSD, Zlib, Unicode-3.0 and the like), carried by the clamshell package in
  `/usr/share/licenses/clamshell/THIRD-PARTY` (`tools/repo/third-party-licenses.py`).
- Kernel patches of ours (`packages/linux-thor`): GPL-2.0, as Linux. The linux-thor package (Armada's
  kernel: Linux, GPL-2.0) carries the kernel's license files and is published with its corresponding
  source beside it: its source package, `linux-thor-VERSION.src.tar.gz` (`makepkg --allsource`: the
  PKGBUILD, kernel.org's tarball, Armada's repository at the commit, our patches and config).
- thor-ucm: ROCKNIX's Thor profiles, additions to alsa-ucm-conf, under its BSD-3-Clause license (carried).
- thor-firmware: AYN's and Qualcomm's firmware from ROCKNIX's release, not redistributed by us (built
  where it is installed, never published).
- Arch Linux ARM's public keys (`tools/image/keyrings`): theirs.

Configuration values and facts taken from ROCKNIX, Armada, Thorch and Kettle Linux (named where used) are
written here in our own words and files.
