# thor (working name)

[![check](https://github.com/robinei/thor/actions/workflows/check.yml/badge.svg)](https://github.com/robinei/thor/actions/workflows/check.yml)

Linux on the AYN Thor, a handheld with two screens. This repository holds ClamShell, the program that
runs the device's session, and the system around it: Arch Linux ARM with our own packages, built on a PC
and kept current by pacman. `docs/thor-gameplan.md` says what to do when the device arrives and what is
left to test on it.

The aim is a full Linux PC in a handheld that folds shut, with nothing cut down for the device: desktop
programs, emulators, Steam (its ARM build) with Windows games through Valve's ARM Proton, which
translates x86 code with FEX, Android apps through Waydroid, files and media. What is adapted is how it
is used. The interface is made for two small screens, a controller and touch, rather than a mouse and a
monitor: the top screen holds the games and the program in front, and the second screen holds the
controls, the on-screen keyboard and touchpad, and windows moved down to it.

Not aims (decided, so they are not reopened by accident):

- More than one on-screen keyboard layout. It is a PC-style US layout with an Fn layer, which covers
  what is typed on the device (searches, passwords, names); other languages are for a physical keyboard,
  which keeps its own layout.
- Running under another compositor. ClamShell is the compositor (`crates/compositor`): `clamshell --drm`
  on the device's screens, `clamshell --nested` in a window on a desktop for development.
- More than one virtual controller for games. Games see one Xbox 360 pad; remapping and the gyro are
  ClamShell's keymaps.
- Themes, for now. There is one look, made for the OLED screens: black, grey text, colour only where it
  means something.
- Animations, for now. Screens change at once, without transitions (a flung list still scrolls on and
  slows); every frame drawn costs power, and an interface that answers immediately needs no transition
  to show what happened.
- Translations. The interface is in English, as the keyboard is.
- Installing or buying software. ClamShell lists what is installed; installing stays with Steam, pacman
  and programs' own installers.
- Devices other than the Thor. ClamShell runs on a desktop for development, but the system, its packages
  and its image are made for the Thor.
- Google's apps in Waydroid. Android is set up without them, so apps that need Google Play's services or
  certification don't run.
- Several users or profiles, for now. There is one user, with one library and one set of settings.
- Editing the library from the interface, for now. The library is what `library.json` declares, written
  by hand; ClamShell reads it and doesn't rewrite it.
- Reading older ClamShell formats. Nobody else runs it yet, so formats change outright.

How it is built is in `docs/architecture.md`, and how work on it is done in `AGENTS.md`; `docs/README.md`
lists the other design notes. How each part behaves is in its module's documentation
(`cargo doc --open --document-private-items` in `clamshell/`) and in its tests.

## How it is made

Most of the code is written by Claude (Anthropic's model) in Claude Code sessions, with its author
deciding the design, reviewing the work and setting the rules it follows (`AGENTS.md`). A commit's
`Claude-Session` line names the session it came from. What holds the code to account is checkable:

- every commit passes `tools/check.sh` (formatting, clippy, the tests, the docs), on CI too;
- the snapshot test pins each screen's pixels, the pad's focus moves and the effects of scripted
  scenarios, and a randomized test checks the state's invariants after every event;
- `docs/state-machines.md` describes each feature's states and transitions, with the test that pins each
  one;
- reviews by agents new to the code, their findings and the decisions taken on them are kept in
  `docs/history/`.

## ClamShell

`clamshell/` is one process with one in-memory state for shelf (the game list on the top screen), the
second screen (the bar and its tabs), the input path (the controller, the pad mouse, the touchpad, the
on-screen keyboard), and the games' settings and keymaps. It is also the Wayland compositor the games
and programs show their windows in. Its crates, the cycle they make and the rules the code follows are in
`docs/architecture.md`.

It is made to be dependable, light and quick, to start and while it runs. Measured on a desktop PC
(the Thor's numbers will come with the device):

- **Starting:** both screens are drawn about 80 ms after it starts, when it tells systemd it is ready. A
  watchdog restarts it if its main loop ever stops. As it is the compositor, a restart of it closes the
  session's apps.
- **Running:** about 40 MB resident, most of it the two screens' buffers, and about 0.2 % of a CPU over
  hours of use. A frame takes 0.1-0.6 ms to draw; when an event changes nothing visible, checking costs
  about 0.02 ms and nothing is drawn or sent to the compositor. Handling an event or drawing a frame
  allocates no memory.

Beside the Cargo workspace are `android/` (the Waydroid session's script and its clipboard bridge, which the
package installs) and `tests/` (the nested compositor and picture tests with their test programs, the device
profiles' test, and the snapshots' references). The settings of the programs the session runs (foot, mpv,
RetroArch) are in `packages/thor-session/root/`.

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
`crates/system/src/library/meta/`.

What ClamShell keeps (play time, favourites, settings, keymaps, window rules, the dock's pins, where shelf
was, brightness) is in JSON files in `~/.local/share/clamshell`, readable and editable by hand; which
file holds what: `crates/system/src/store.rs`.

## Building and running

ClamShell is a Cargo workspace in `clamshell/` (Rust 1.95 or later); `cargo build` there builds it.

    cd clamshell
    cargo run -- --nested                       # the session in a window of this desktop: both screens
    cargo run --features dev -- --snapshot DIR  # every screen as PNG files at the Thor's size, no Wayland
    cargo run -- ctl help                       # the control pipe's commands (a running ClamShell takes them)
    ../tools/nested-session.sh                  # as on the device: the nested session as a user unit
                                                # (clamshell-nested.service), with this desktop's library

On the device, `clamshell --drm` runs on the screens from the first console (`thor-session`); `clamshell`
without arguments prints its usage.

In shelf, Y makes the selected entry a favourite (the Favourites tab) or not, and X pins it to the dock
or unpins it; a pinned folder (a series) opens in shelf from the dock.

On a desktop, keys stand in for the pad while no window has the keyboard: arrows move the focus, Enter or
Space is A, Backspace B, Tab View, Q and E the shoulders, F is Y, P is X, C Start, Home and End the
triggers. Whatever has the keyboard, F12 is the Thor's Home button (a tap shows shelf), F11 moves the window
in front down to the second screen and F10 brings it back up. A controller's Guide button is Home too.

`clamshell ctl screenshot`, or Home+R1, saves a screenshot of both screens. `clamshell ctl window events
FILE` writes the windows and the last 256 window events as JSON, which
`clamshell_compositor::trace::Recording::replay` checks against a state fixture with the same library,
rules and launch context.

## Checking

- `tools/check.sh`: formatting, clippy with warnings as errors (the `dev` and release builds too), the
  tests (the snapshots among them), and the documentation; about a minute. Every commit passes it:
  `git config core.hooksPath tools/hooks` makes it the pre-commit hook. A plain `cargo test` in
  `clamshell/` runs only the binary's tests.
- Snapshots: the tests walk a scenario through every screen (`clamshell/src/dev/snapshot.rs`) and
  compare three references in `clamshell/tests/`: the screens' pixel hashes, where the pad's focus goes
  on each, and the scenario's effects. On a difference the failure prints the text diff and the paths of
  the screens drawn, with the pad's moves drawn over a screen whose focus changed. When and how to update
  the references is in `AGENTS.md`, "Checking".
- `clamshell/tests/nested-compositor.py` (after `cargo build` and `cargo build -p clamshell-compositor
  --examples`): ClamShell nested in a headless sway, which only hosts its window, with real clients:
  Wayland and X11 windows, Xwayland started on demand and after a crash, menus, the clipboard, the
  cursor, notifications, screenshots, moves between the screens. `--gpu`, on a desktop with a render
  node and a Vulkan driver (and Wayland-capable `vkcube`), requires native dmabuf submission. It prints
  the directory with its screenshots and protocol logs.
- `clamshell/tests/real-apps.py` (after `cargo build`): the apps installed on this computer (RetroArch,
  Firefox, Wine's Notepad, Lutris, Heroic; the missing skipped) started in nested ClamShell, each checked
  to open in front, draw, cause no compositor error and end when ClamShell closes its window. `--gpu` as
  above.
- `clamshell/tests/picture-scaling.py` (after `cargo build` and `cargo build -p clamshell-compositor
  --example pattern-game`): test games launched from shelf, their pictures checked to fill the top screen
  at each Resolution and Scaling. Both need sway (as the host), Xwayland and grim; the first also foot,
  dbus-daemon and gdbus, the second a C compiler and SDL 3.
- `tools/vm/vm-test.sh`: the VM suite end to end, about twenty minutes, for what nothing else can check.
  The virtual Thor is `tools/vm/vm.py` (QEMU/KVM, both screens, a fake controller with a gyro and rumble,
  simulated Wi-Fi and Bluetooth; `tools/vm/vm.py --help`), and its checks `tools/vm/vm-checks.py`
  (`--list`). Its session is the device's, `clamshell.service` (the VM's build, `CLAMSHELL_DEVICE=vm`)
  started by the repository's thor-session: `vm.py restart` ends it for the autologin to start again, and
  as the device's session it sleeps when idle (`vm.py wake`).

## The device's system

Arch Linux ARM and the Thor's own packages (`packages/`: `thor`, the metapackage that is the whole
system, and what it holds: `linux-thor`, `thor-system`, `thor-session`, `clamshell`, `thor-ucm`,
`thor-sensors`, `thor-keyring`; and `thor-firmware`, which is never published), kept current with
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
