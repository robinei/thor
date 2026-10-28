# Plan, 2026-10-28: Library, Settings and Setup as built-in screens

From a discussion of what the bottom screen has had to hold, how library.json is edited, and a file
picker. Nothing here is built yet.

## Names

- **Library** is the top screen that browses what the device has (shelf until now). In code it is
  `Browser` (`ShelfEvent` → `BrowserEvent`, `ShelfId` → `BrowserId`): `Library` stays the data, the
  `clamshell-library` crate and library.json.
- **Settings** is a top screen of its own; the bottom screen's Settings tab keeps the panel for the
  program in front.
- **Setup** is the first start's wizard.
- In code the three are `builtin::{browser, settings, setup}`; `setup` takes in today's Device view
  (`TopView` goes).

## Built-ins as entries

Library and Settings are library entries, `builtin:library` and `builtin:settings`, launched as
`Launch::Builtin(..)` and placed by library.json like anything else (a `builtin` source). Each runs while
it is open: on the dock while it runs, pinnable and a favourite like any entry, in front as a group with no
windows, which ClamShell draws; the pad goes to ClamShell's UI while it is in front. They have no runner
and no settings of their own.

Library always runs and can't be quit: something is in front when nothing else is. So its tile is always
on the dock, and the starter pins it at 0. `InFront::Shelf` and `Front::Shelf` go; the front falls back to
Library's group.

Setup is a built-in that isn't an entry: nothing places it and it can't be pinned.

## Setup

Shown at the first start, and again only until it is completed: while the helper reports no password,
and while the wizard isn't marked done (power off during Wi-Fi brings it back at Wi-Fi). Each step in turn
on the top screen; the bottom screen shows only the keyboard while Setup shows (no bar, no tabs).

1. **Password**, twice, 8 characters at least. Not skippable.
2. **Wi-Fi**: the network list, from the radios state the Wi-Fi tab uses; the passphrase typed below.
   Skippable.
3. **Time zone**: the list, with the one suggested from the network selected; typing narrows it.
   Skippable.
4. Done.

Off the device (no helper, as in the nested session) the password step shows dimmed as unavailable and
Continue is allowed.

**Security.** The first password is set without the current one by `thor-account`, which any process of
`thor` can run; Volume Up within 20 s is what proves a person is there (`security.md`). With the password
step unskippable and nothing of the user's started before it, no user program runs while the account has
no password, so the confirmation goes: from the helper (thor-system), from ClamShell, from `security.md`.
This holds only if nothing running as `thor` starts before Setup completes besides ClamShell and the
image's own services: launches, "Start with ClamShell" entries and the maintenance pass are held by a
condition the launch path checks, derived from Setup showing. A forgotten password needs root to recover
(a recovery boot or reflashing), as now.

## Settings

Takes what is crammed into the bottom screen's Settings tab and the Device view: the keymap and curve
editors; changing the password, remote login; stick, gyro and refresh calibration; the time zone; the
window rules; device profiles; and the library's editors (below). The bottom screen keeps the panel for
the program in front, the levels, the radios and the bar.

## library.json

Places are normalised: an `id`, a `name`, and `in`, the parent's id. A rename changes `name`, a move
`in`; pins and favourites stay keyed by id (`place:snes`). The editor makes ids (a slug of the first name,
kept after); a hand-written file names them. A dir source's files default to its runner's `match`, with
`also` adding to them (`*.zip`, which no runner opens by name); `match` still overrides. Sources that make
their own folders (browsed, removable) give `in` and a `name` (`{label}`). Built-ins are a source type.

```json
"places": [
  { "id": "games", "name": "Games", "icon": "gamepad" },
  { "id": "snes", "name": "SNES", "in": "games", "picture": "snes.png" },
  { "id": "files", "name": "Files", "icon": "folder" },
  { "id": "tools", "name": "Tools", "type": "tools", "icon": "tools" }
],
"sources": [
  { "type": "dir", "id": "snes", "dir": "~/Roms/snes", "place": "snes", "runner": "snes", "also": ["*.zip"] },
  { "type": "browse", "dir": "~", "in": "files", "name": "Home" },
  { "type": "removable", "in": "files", "name": "{label}" },
  { "type": "builtin", "id": "settings", "place": "tools" }
]
```

No compatibility with the current format: the starter file and the desktop's are rewritten.

## Editing the library

Three editors, which together are the three lists of library.json:

- **Places: an edit mode of the Library screen**, not a separate tree. The tabs and folders are the
  tree; the Files tab's verbs mean the same for places (Rename, New place, Cut and Paste to move), plus
  Icon, Picture, Remove and Add source. Empty places show; folders a source makes show dimmed and are
  edited in Sources; ClamShell's own tabs can be renamed, given an icon and moved, not removed. Remove
  only an empty place (no children, nothing filling it); otherwise the details say what uses it. Each
  change is written as it is made. Entered from a place's long-press menu or from Settings; B leaves.
- **Sources**, a list in Settings, in order (the first that matches takes an entry), each with the
  entries it takes now. A source's screen chooses its runner, suggested from the files found (the
  runner whose patterns match most of them first), and shows live what it will list and leave out.
- **Runners**, a list in Settings. A source's runner list ends in "New runner…", which opens the runner's
  screen as a step and returns with it chosen. Runners are shared (Video opens films, TV and the Files
  tab's videos), so a source chooses one rather than defining its own.

A place, source or runner that names one gone shows dimmed as a problem. The editors write the whole
file; a change by hand while one is open is read again.

### Mockups

Library in edit mode, a place selected:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│   ◷ Recent   ★ Favourites   ▣ Games   ▤ Films   ▢ TV   ▭ Files   ⚙ Tools  +  │
│                                                          editing the library │
│  ▸ NES                 12       SNES                                         │
│  ▸ SNES               143  ◀    in Games                                     │
│  ▸ Nintendo 64          0                                                    │
│  ▸ Game Boy            31       Filled by                                    │
│  ▸ PlayStation          8         SNES scan · ~/Roms/snes · 143              │
│  ▸ Steam               46                                                    │
│    + New place                  Rename          Icon                         │
│    · Desktop games      9       Cut             Picture                      │
│                                 Add source…     Remove                       │
└──────────────────────────────────────────────────────────────────────────────┘
```

A new place, empty:

```
│    Game Cube            0  ◀    Game Cube                                    │
│                                 in Games                                     │
│                                 Nothing fills this place yet.                │
│                                 Add source…     Rename      Icon             │
│                                 Picture         Cut         Remove           │
```

Add source…: its kind, then (for files in a folder) the folder in Library's pick mode:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ⚙ Settings  ›  Sources  ›  New source for Game Cube                         │
│    Files in a folder          ◀   Scans a folder for files and lists each    │
│    Apps                           as an entry: ROMs, films, music.           │
│    Steam games                                                               │
│    A browsable folder                                                        │
│    Removable storage                                                         │
│    Library or Settings                                                       │
└──────────────────────────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────────────────────────┐
│  Choose a folder for Game Cube                                     ▭ Files   │
│  ‹ Roms                                                                      │
│  ▸ gc                      ◀    gc                                           │
│  ▸ snes                         ~/Roms/gc                                    │
│  ▸ psx                          24 files: 22 .rvz, 2 .iso                    │
│                                 Choose this folder                           │
└──────────────────────────────────────────────────────────────────────────────┘
```

The source, its runner chosen, what it will list:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ⚙ Settings  ›  Sources  ›  Game Cube                                        │
│    Folder         ~/Roms/gc                  Will list 22 of 24 files        │
│    Opened by      Game Cube (Dolphin)  ◀       Metroid Prime.rvz             │
│    Files          *.rvz *.iso  (the runner's)  Wind Waker.rvz                │
│    Also           *.zip                        ...                           │
│    Pictures from  libretro: GameCube         Left out                        │
│    Removable      no                           notes.txt, cover.png          │
│                         Done      Cancel                                     │
└──────────────────────────────────────────────────────────────────────────────┘
```

A new runner, when none fits:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ⚙ Settings  ›  Sources  ›  Game Cube  ›  New runner                         │
│    Name           Game Cube                                                  │
│    Opens          *.rvz *.iso *.gcm          from the files in ~/Roms/gc     │
│    Command        dolphin-emu -b -e {path}  ◀                                │
│    Keymap         Default                                                    │
│    More           enqueue, pause, awake…                                     │
│    ✓ dolphin-emu is installed                                                │
│                         Done      Cancel                                     │
└──────────────────────────────────────────────────────────────────────────────┘
```

## What the image ships

The image is meant to work without editing anything: the editors are for changing it, not for setting
it up.

- **Players and emulators**: RetroArch with its cores, mpv, imv, zathura (as now), plus standalone
  emulators where they are better than a core on the Thor's aarch64 (GameCube and Wii, PSP, 3DS, PS2 are
  the candidates). Each needs checking: built for aarch64 (or packaged by us), its licence lets us
  redistribute it (some emulators' don't), and how it takes a file and the pad.
- **A runner for each**, in the starter library.json.
- **A Roms folder**, `~/Roms/<system>`, with the short system names other frontends use (`nes`, `snes`,
  `n64`, `gb`, `gbc`, `gba`, `nds`, `megadrive`, `psx`, `ps2`, `psp`, `dreamcast`, `gc`, `wii`, `3ds`), so
  a collection from elsewhere drops in. Made empty at the first start. An empty system's place shows
  dimmed, saying where its files go. Systems needing a BIOS name the file and folder missing as a
  problem.
- **The menu**: Recent, Favourites, Games (a folder per system, Steam, the desktop's games), Films, TV,
  Music, Files (Home, Roms, removable storage), Apps, Tools (Settings and ClamShell's tools).
- **The dock's first pins**: Library, Firefox, foot. Pins are the user's saved state, so the image's are
  a starter that the first start takes when nothing is saved. Firefox isn't in the image yet.

## The file picker

Library's pick mode: a request (a file or a folder, a filter, a folder to start in, who asked) shows only
the Files tab's places, folders always and files that match; the details' actions become Choose, and B or
Home cancels. ClamShell's own screens use it first (a source's folder, a place's picture).

Then for apps, through xdg-desktop-portal: ClamShell serves `org.freedesktop.impl.portal.FileChooser`
(a `.portal` file and `portals.conf` for the session), and OpenFile, SaveFile and SaveFiles open pick mode
over the asking app's group. Each toolkit uses the portal only when told to (GTK and Qt through the
environment, which launches would set; Firefox by its own setting), and some apps never do: the step
checks the apps we care about. Save needs a name, typed on the bottom keyboard, and asking before
overwriting.

## Steps

Each ends in commits that pass `tools/check.sh` and the randomized test; the VM suite where the step
touches the session or the outside.

- [ ] 1. **Rename** shelf to Library (`Browser` in code), in the UI, the code and the docs.
- [ ] 2. **Setup as a wizard**: password, Wi-Fi, time zone; the bottom screen locked to the keyboard; the
  launch gate; Volume Up removed from `thor-account` (a thor-system release) and from `security.md`.
- [ ] 3. **Built-ins as entries**: Library always running and pinned at 0 by the starter; `InFront::Shelf`
  gone.
- [ ] 4. **The Settings screen**, taking the keymap and curve editors and the rest of the Device view.
- [ ] 5. **The format**: places by `id`, `name`, `in`; the `match` default and `also`; built-in sources.
- [ ] 6. **What the image ships**: emulators and runners, `~/Roms`, the menu, the first pins, Firefox.
- [ ] 7. **The editors**: Library's edit mode, Sources, Runners; the picker for ClamShell's own screens.
- [ ] 8. **The portal**: Open, then Save.
