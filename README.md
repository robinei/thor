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
- Several users, for now. There is one user, with one library and one set of settings.
- Reading older ClamShell formats. Nobody else runs it yet, so formats change outright.

How it is built is in `docs/architecture.md`, and how work on it is done in `AGENTS.md`; `docs/README.md`
lists the other design notes. How each part behaves is in its module's documentation
(`cargo doc --workspace --no-deps --document-private-items --open` in `clamshell/`) and in its tests.

## How it is made

Most of the code is written by Claude (Anthropic's model) in Claude Code sessions, with its author
deciding the design, reviewing the work and setting the rules it follows (`AGENTS.md`). A commit's
`Claude-Session` line names the session it came from. What holds the code to account is checkable:

- every commit passes `tools/check.sh` (formatting, clippy, the tests, the docs, real clients in the
  nested compositor), on CI too;
- the snapshot test pins each screen's pixels, the pad's focus moves and the effects of scripted
  scenarios, and a randomized test checks the state's invariants after every event;
- `docs/state-machines.md` describes each feature's states and transitions, with the test that pins each
  one;
- reviews by agents new to the code, their findings and the decisions taken on them are kept in
  `docs/history/`.

## ClamShell

`clamshell/` is one process with one in-memory state, and it is also the Wayland compositor (with
Xwayland for X11 programs) the games and programs show their windows in. Its crates, the cycle they make
and the rules the code follows are in `docs/architecture.md`. What it has:

- The top screen shows the program in front, or one of ClamShell's own screens: the Library (the entries
  `library.json` declares, in tabs and folders; also the file picker apps get through
  xdg-desktop-portal), the Device view (Wi-Fi, the time zone, the password and remote login, the
  controllers and their players, calibrating the sticks, the gyro and the refresh rate, editing the
  library), and Setup at the first start (the password, Wi-Fi, the time zone).
- The second screen has a bar of tabs: the settings for the program in front, the dock (pinned and
  running programs), the on-screen keyboard, the touchpad, system information, volume, brightness,
  Bluetooth, Wi-Fi, the battery, the time, and the windows moved down to it.
- Settings at three levels (all, a runner, one entry): the power profile, CPU and GPU caps, the big
  cores, the fan, an FPS cap, the refresh rate, a HUD; the game's picture (its resolution, scaling with
  FSR among the choices, the size of windows, anti-aliasing, sharpening, a tone curve and HDR content's
  peak: `docs/picture.md`); the keymap per player, the pad mouse, rumble and the stick lights; what a
  program does in the background; and for the device as a whole the charge limit (or bypass, the
  battery left alone on the charger), the second screen's mode, and when the screens go off and the
  device sleeps.
- Keymaps, tone curves and stick lights are profiles: named and shared, or a game's own.
- Every controller is grabbed and belongs to one of 1 to 8 players. A player's controllers are merged
  into one virtual Xbox pad for games, through that player's keymap; player 1 also drives ClamShell and
  has the pad mouse.
- Bluetooth pairing shows the code a keyboard asks to have typed.
- A monitor on USB-C mirrors the top screen; while one is connected, its refreshes pace the games.

It is made to be dependable, light and quick. Measured on 2026-10-31 on a Ryzen 9 5900X and
Radeon RX 9070 XT, nested in GPU-backed headless sway at 60 Hz, with both screens at their native
sizes and 1,000 local Library entries ([method and results](docs/measurements/2026-10-31-idle.md)):

- **Starting:** a warm launch reaches systemd readiness in a median 87 ms; both screens are composed
  in about 85 ms. The loop's watchdog lets systemd restart a stalled session. A compositor restart ends
  programs with windows; programs still running without one are found again.
- **Memory:** about 118–132 MiB RSS (87–102 MiB PSS), with GPU allocations accounted separately: about 126–143 MiB
  resident VRAM and 8–28 MiB GTT on this desktop driver. Shared mappings can overlap these accounts.
- **Idle:** about 0.10% of one CPU core with Dock below the Library, or 0.67% with the live System page.
  About 6 and 22 voluntary context switches per second respectively are a wakeup proxy; exact wakeup
  counters were unavailable. The nested backend presents only when something changes.
- **Drawing:** a whole screen costs 0.12–0.43 ms of CPU time; recording and finding it unchanged costs
  0.004–0.028 ms. Warmed steady-state events and frame drawing allocate no memory.

These process measurements exclude the host compositor and apps. The Thor's DRM and power figures
will come from the device.

In `clamshell/`, beside the crates, are `android/` (the Waydroid session's script and its clipboard bridge,
which the package installs) and `tests/` (the nested compositor, real apps and picture tests with their
test programs, the device profiles' test, and the snapshots' references). The settings of the programs
the session runs (foot, mpv, RetroArch) are in `packages/thor-session/root/`.

## The library

The Library comes from `~/.config/clamshell/library.json`, and nothing is there but what it declares.
The file can be written by hand, or edited in the Library's edit mode (from the Device view), which
writes it back:

- `places`: the tabs, in their order, and the folders in them, each with an `id` that sources and other
  places name it by, a `name`, and `in`, the id of the place a folder is in (a place without one is a
  tab); an icon, and a folder's picture (shown large when it is selected). Recent, Favourites and Tools
  are places of a `type`. `"recent": false` keeps what is in a place out of Recent (apps).
- `runners`: how things open, each with an id (its settings are kept by it), a name (shown), the file
  names it opens (`match`), its command (`launch`, `{path}` the file) and the BIOS files its emulator
  needs (`bios`); a runner of `type` `desktop` or `steam` opens desktop files and Steam's games by their
  own commands.
- `sources`: where entries come from, and the place they go to: `dir` (a dir scanned: which files are
  entries, by default those its runner opens, with `also` adding patterns; their metadata; the runner that
  opens them if not by their names; `on` a removable filesystem, for ROMs on an SD card), `browse` (a dir
  listed as it is opened, a folder of its own: the Files tab), `desktop` and `steam` (the installed apps
  and games: each taken by the first source of its type that matches it), `removable` (SD cards and USB
  sticks as they are mounted, each a folder), `builtin` (ClamShell's own screens: the Device view).
- `keys`: the keys of the metadata services that need one.

```json
{ "places": [
    { "id": "recent", "name": "Recent", "type": "recent", "icon": "clock" },
    { "id": "games", "name": "Games", "icon": "gamepad" },
    { "id": "tv", "name": "TV", "icon": "tv" },
    { "id": "apps", "name": "Apps", "icon": "apps", "recent": false },
    { "id": "snes", "name": "SNES", "in": "games", "icon": "snes-icon.png", "picture": "snes.png" }
  ],
  "runners": [
    { "type": "desktop", "id": "desktop", "name": "Desktop", "match": ["*.desktop"] },
    { "id": "snes", "name": "SNES", "match": ["*.sfc", "*.smc"], "launch": ["retroarch", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{path}"] },
    { "id": "video", "name": "Video", "match": ["*.mkv", "*.mp4"], "launch": ["mpv", "{path}"] }
  ],
  "sources": [
    { "type": "dir", "id": "snes", "dir": "~/Roms/snes", "place": "snes", "runner": "snes", "also": ["*.zip"],
      "metadata": "libretro:Nintendo - Super Nintendo Entertainment System" },
    { "type": "dir", "id": "tv", "dir": "~/TV", "place": "tv", "match": ["*/"], "items": ["*.mkv", "*.mp4"], "metadata": "tmdb:tv" },
    { "type": "desktop", "name": "*Uninstall*", "hide": true },
    { "type": "desktop", "categories": "Game", "place": "games" },
    { "type": "desktop", "place": "apps" }
] }
```

The starter library.json (`packages/thor-session/root/usr/share/thor/home/.config/clamshell/library.json`)
declares ClamShell's set: Recent, Favourites, Games (a folder per system, and Steam), Films, TV, Music,
Files, Apps, Tools; the Desktop, Android (Waydroid's apps) and Steam runners, a runner per system
(RetroArch's cores, PPSSPP, ScummVM), Video and Music (mpv), Images (imv), PDF (zathura), Text (`less` in
foot), last Other files (`xdg-open`); a scan per system (`~/Roms/<system>`) and for films, series
(`~/Videos`) and music, Steam, the desktop files, Home, the Roms folder, Root, the Trash, removable
storage, and the Device view in Tools. In every list, folders come first. A file opened from the Files
tab opens by the first runner whose patterns match its name. The design: `docs/runners.md`. The format
as read (`clamshell --library` prints the library it makes): `crates/library/src/config.rs`; the rules
(ids, titles, art, icons): `crates/system/src/library/content/`.

Covers, screenshots, descriptions, developers and years are fetched in the background after start
(`clamshell --fetch-metadata` does it without the UI): a scan's `"metadata": "libretro:<system>"`
(libretro's thumbnails and database, by the file's checksum or by name), `"tmdb:movie"` or `"tmdb:tv"`
(needs `"keys": { "tmdb": ... }`); a Steam source's `"steam"` (the store); a desktop source's
`"steamgriddb"` (needs `keys.steamgriddb`). Art beside a file wins. libretro's lists are kept in `~/.cache/clamshell/libretro` for 30 days. The rules:
`crates/system/src/library/meta/`.

What ClamShell keeps (play time, favourites, settings, keymaps, tone curves, stick lights, the controllers
and their players, the sticks' calibrations, window rules, the dock's pins, where the Library was,
brightness) is in JSON files in `~/.local/share/clamshell`, readable and editable by hand; which
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

In the Library, Y makes the selected entry a favourite (the Favourites tab) or not, and X pins it to the dock
or unpins it; a pinned folder (a series) opens in the Library from the dock.

Keys stand in for the pad while no window has the keyboard: arrows move the focus, Enter or Space is A,
Backspace B, Tab View, Q and E the shoulders, F is Y, P is X, C Start, Home and End the triggers. In the
nested session on a desktop, whatever has the keyboard, F12 stands for the Home button (a tap shows the
Library), F11 moves the window in front down to the second screen and F10 brings it back up; a
controller's Guide button is Home too. (On the device, F10 to F12 reach the window like other keys, and
Home is the controller's.)

`clamshell ctl screenshot`, or Home+R1, saves a screenshot of both screens. `clamshell ctl window events
FILE` writes the windows and the last 256 window events as JSON, which
`clamshell_compositor::trace::Recording::replay` checks against a state fixture with the same library,
rules and launch context.

## Checking

- `tools/check.sh`: formatting, clippy with warnings as errors (the `dev`, device and release builds
  too), the tests (the snapshots among them), the documentation, and real clients in the nested
  compositor (`clamshell/tests/nested-compositor.py`); about two minutes. Every commit passes it:
  `git config core.hooksPath tools/hooks` makes it the pre-commit hook. A plain `cargo test` in
  `clamshell/` runs only the binary's and the fan daemon's tests; `--workspace` runs them all.
- Snapshots: the tests walk a scenario through every screen (`clamshell/src/dev/snapshot.rs`) and
  compare three references in `clamshell/tests/`: the screens' pixel hashes, where the pad's focus goes
  on each, and the scenario's effects. On a difference the failure prints the text diff and the paths of
  the screens drawn, with the pad's moves drawn over a screen whose focus changed. When and how to update
  the references is in `AGENTS.md`, "Checking".
- `clamshell/tests/nested-compositor.py` (after `cargo build` and `cargo build -p clamshell-compositor
  --examples`): ClamShell nested in a headless sway, which only hosts its window, with real clients:
  Wayland and X11 windows, Xwayland started on demand and after a crash, menus, the clipboard, the
  cursor, relative motion and pointer constraints, notifications, screenshots, moves between the screens.
  `--gpu`, on a desktop with a render node and a Vulkan driver (and Wayland-capable `vkcube`), requires
  native dmabuf submission. A failed run keeps its directory with the screenshots and protocol logs, and
  prints it.
- `clamshell/tests/real-apps.py` (after `cargo build`): the apps installed on this computer (RetroArch,
  Firefox, Wine's Notepad, Lutris, Heroic; the missing skipped) started in nested ClamShell, each checked
  to open in front, draw, cause no compositor error and end when ClamShell closes its window. `--gpu` as
  above.
- `clamshell/tests/picture-scaling.py` (after `cargo build` and `cargo build -p clamshell-compositor
  --example pattern-game`): test games launched from the Library, their pictures checked to fill the top screen
  at each Resolution and Scaling. Both need sway (as the host), Xwayland and grim; the first also foot,
  dbus-daemon and gdbus, the second a C compiler and SDL 3.
- `tools/vm/vm-test.sh`: the VM suite end to end, about twenty minutes, for what nothing else can check.
  The virtual Thor is `tools/vm/vm.py` (QEMU/KVM, both screens, a fake controller with a gyro and rumble,
  simulated Wi-Fi and Bluetooth; `tools/vm/vm.py --help`), and its checks `tools/vm/vm-checks.py`
  (`--list`). Its system and session are the device's: thor-system and thor-session, built from the working
  tree and installed with pacman (what differs, and why: `PACKAGED` in vm.py), around `clamshell.service`
  (the VM's build, `CLAMSHELL_DEVICE=vm`) started by thor-session: `vm.py restart` ends it for the autologin to start again, and
  as the device's session it sleeps when idle (`vm.py wake`). Each run's output and the machine's log are
  kept in `~/.local/state/thor/vm-runs/`.

## The device's system

Arch Linux ARM and the Thor's own packages (`packages/`: `thor`, the metapackage that is the whole
system, and what it holds: `linux-thor`, `thor-system`, `thor-session`, `clamshell`, `thor-ucm`,
`thor-sensors`, `thor-keyring`; and `thor-firmware`, which is never published), kept current with
`pacman -Syu` from the repository `[thor]`, the GitHub release `packages`. What each holds:
`docs/thor-gameplan.md`, "Our image and packages". On the device, `.claude/commands/maintain.md` is the
playbook for a maintenance pass with Claude Code, which the dock's Maintenance app starts: news,
updates, the boot, the firmware, pacnew files and health.

From a checkout, on an Arch Linux PC, everything is built here and nothing needs the virtual machines:

    tools/host-check.sh packages image  # what this PC lacks, and the pacman line that adds it
    tools/image/image.sh                       # the Thor's SD card image: the packages (the kernel
                                               # among them), then pacstrap through qemu-user (sudo)
    tools/image/image.sh vm                    # the same system for x86_64, to try the first boot:
    tools/vm/vm.py thor-image                  #   in QEMU, a window per screen (--fresh: from scratch)
    tools/image/upgrade-test.sh                # a kernel update tried on a copy of the Thor's image

The steps on their own: `tools/repo/build.sh [PACKAGE...]` (the packages, from the last commit: ClamShell
cross-compiled against `tools/repo/sysroot.sh`'s sysroot of Arch Linux ARM's packages, Turnip built as an Adreno-only Vulkan driver without LLVM, linux-thor built
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
- The kernel patches in `packages/linux-thor` (ours, and those taken from PB-OS, ROCKNIX and Linux's
  stable releases, each named by its prefix): GPL-2.0, as Linux. The linux-thor package (Armada's
  kernel: Linux, GPL-2.0) carries the kernel's license files and is published with its corresponding
  source beside it: its source package, `linux-thor-VERSION.src.tar.gz` (`makepkg --allsource`: the
  PKGBUILD, kernel.org's tarball, Armada's repository at the commit, our patches and config).
- thor-ucm: ROCKNIX's Thor profiles, additions to alsa-ucm-conf, under its BSD-3-Clause license (carried).
- thor-firmware: AYN's and Qualcomm's firmware from ROCKNIX's release, not redistributed by us (built
  where it is installed, never published).
- Arch Linux ARM's public keys (`tools/image/keyrings`): theirs.

Configuration values and facts taken from ROCKNIX, Armada, PB-OS, Thorch and Kettle Linux (named where used) are
written here in our own words and files.
