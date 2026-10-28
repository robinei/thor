# Plan, 2026-10-28: Library, Device and Setup as built-in screens

From a discussion of the top screen's own screens, how library.json is edited, a file picker, and what
the image ships. Built, as the steps at the end say; what changed on the way is noted there.

## What goes on which screen

The bottom screen stays as it is: what belongs to a status icon is under it, and what is tuned while
playing (the settings for the program in front, the keymap and curve editors, the levels) sits beside the
game it changes. Nothing moves up. The top screen's own screens are for what is never used in a game,
needs room and is rare: the device's setup, and editing the library.

## Names

- **Library** is the top screen that browses what the device has (shelf until now). In code it is
  `LibraryView` (`LibraryViewEvent`, `LibraryViewId`, `library_view`), "the Library" in prose: `Library`
  stays the data, the `clamshell-library` crate and library.json.
- **Device** is today's Device view: password, remote login, stick, gyro and refresh calibration, Wi-Fi and
  the time zone (opening their tabs).
- **Setup** is the first start's wizard.
- In code the three are `builtin::{library_view, device, setup}`; `TopView` stays, as Setup or the
  Library, the Device view being an entry shown in the Library's place.

## Built-ins as entries

Library and Device are library entries, `builtin:library` and `builtin:device`, launched as
`Launch::Builtin(..)` and placed by library.json like anything else (a `builtin` source). Each runs while
it is open: on the dock while it runs, pinnable and a favourite like any entry, in front as a group with no
windows, which ClamShell draws; the pad goes to ClamShell's UI while it is in front. They have no runner
and no settings of their own.

Library always runs: something is in front when nothing else is. Its tile has no Quit, Force kill or
Unpin; it can be moved. The starter pins it at 0. Home always goes to Library. `InFront::Shelf` and
`Front::Shelf` go; the front falls back to Library's group.

Setup is a built-in that isn't an entry: nothing places it and it can't be pinned.

## Setup

Shown at the first start, and again only until it is completed: while the helper reports no password,
and while the wizard isn't marked done (power off during Wi-Fi brings it back at Wi-Fi). A device that
already has a password counts as done. Each step in turn on the top screen; the bottom screen shows only
the keyboard while Setup shows (no bar, no tabs).

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
image's own services: every launch path (the Library, the dock, "Start with ClamShell", the control pipe
and the CLI) is held by a condition derived from Setup showing, and the maintenance pass needs the
password anyway. A forgotten password needs root to recover (a recovery boot or reflashing), as now.

## library.json

Places are normalised: an `id`, a `name`, and `in`, the parent's id. A rename changes `name`, a move
`in`; pins and favourites stay keyed by id (`place:snes`). The editor makes ids (a slug of the first name,
kept after); a hand-written file names them. A dir source's files default to its runner's `match`, with
`also` adding to them (`*.zip`, which no runner opens by name); `match` still overrides. A source's `id`
is needed only for `dir` sources, whose entries' ids come from it. Sources that make their own folders
(browsed, removable) give `in` and a `name` (`{label}`). Built-ins are a source type. Desktop and Steam
runners have no `match`, but the desktop runner's `*.desktop`, for files opened from Files. Sources stay
one ordered list of all types. A runner may name the BIOS files its emulator needs and where it looks
for them (`bios`), shown as a problem on its folder while one is missing.

```json
"places": [
  { "id": "games", "name": "Games", "icon": "gamepad" },
  { "id": "snes", "name": "SNES", "in": "games", "picture": "snes.png" },
  { "id": "files", "name": "Files", "icon": "folder" },
  { "id": "tools", "name": "Tools", "type": "tools", "icon": "tools" }
],
"runners": [
  { "id": "psx", "name": "PlayStation", "match": ["*.chd", "*.cue", "*.pbp", "*.m3u"],
    "launch": ["retroarch", "-f", "-L", "/usr/lib/libretro/mednafen_psx_hw_libretro.so", "{path}"],
    "bios": { "dir": "~/.config/retroarch/system", "files": ["scph5500.bin", "scph5501.bin", "scph5502.bin"] } }
],
"sources": [
  { "type": "dir", "id": "snes", "dir": "~/Roms/snes", "place": "snes", "runner": "snes", "also": ["*.zip"] },
  { "type": "browse", "dir": "~", "in": "files", "name": "Home" },
  { "type": "removable", "in": "files", "name": "{label}" },
  { "type": "builtin", "id": "device", "place": "tools" }
]
```

No compatibility with the current format: the starter file and the desktop's are rewritten.

## Editing the library

All of it in an edit mode of the Library screen: its tabs and folders edited where they are, and Sources
and Runners as tabs at the end of the tab row. Entered from a tab's or folder's long-press menu or from
Device; B leaves. Each
change is written as it is made; the editors write the whole file, and a change made by hand while one is
open is read again. A folder, source or runner that names one gone shows dimmed as a problem.

The UI says tab and folder, never place: "New tab", "New folder", "Nothing fills this folder yet".
"Places" is only library.json's and the code's word for both.

- **Tabs and folders**: the tree, edited where they are. The Files tab's verbs mean the same here
  (Rename, New folder, Cut and Paste to move), plus New tab, Icon, Picture, Remove and Add source. Empty
  tabs and folders show; folders a source makes show dimmed and are edited in Sources; ClamShell's own
  tabs can be renamed, given an icon and moved, not removed. Remove only an empty one (no children, nothing
  filling it); otherwise the details say what uses it.
- **Sources**: in order (the first that matches takes an entry), each with the entries it takes now; moved
  as on the dock, the counts following. A source's screen chooses its runner, suggested from the files
  found (the runner whose patterns match most of them first), and shows live what it will list and leave
  out.
- **Runners**: in order (for a file from Files, the first whose patterns match opens it), each with what
  it opens, its command and who uses it. A source's runner list ends in "New runner…", which opens the
  runner's screen as a step and returns with it chosen. Runners are shared (Video opens films, TV and the
  Files tab's videos), so a source chooses one rather than defining its own.

Text fields are typed on the bottom keyboard, which gains a row of keys for what the field takes (`{path}`,
`{paths}`, `{socket}`, `/`, `~`): the editor's `Purpose`, generalised.

Folders are chosen in Library's pick mode (below).

### Mockups

Library in edit mode, a folder selected:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ◷ Recent  ★ Favourites  ▣ Games  ▤ Films  ▭ Files  ⚙ Tools  +  │ Sources  Runners │
│                                                          editing the library │
│  ▸ NES                 12       SNES                                         │
│  ▸ SNES               143  ◀    in Games                                     │
│  ▸ Nintendo 64          0                                                    │
│  ▸ Game Boy            31       Filled by                                    │
│  ▸ PlayStation          8         SNES scan · ~/Roms/snes · 143              │
│  ▸ Steam               46                                                    │
│    + New folder                 Rename          Icon                         │
│    · Desktop games      9       Cut             Picture                      │
│                                 Add source…     Remove                       │
└──────────────────────────────────────────────────────────────────────────────┘
```

A new folder, empty:

```
│    Game Cube            0  ◀    Game Cube                                    │
│                                 in Games                                     │
│                                 Nothing fills this folder yet.               │
│                                 Add source…     Rename      Icon             │
│                                 Picture         Cut         Remove           │
```

Add source…: its kind, then (for files in a folder) the folder in Library's pick mode:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  Sources  ›  New source for Game Cube                                        │
│    Files in a folder          ◀   Scans a folder for files and lists each    │
│    Apps                           as an entry: games, films, music.          │
│    Steam games                                                               │
│    A browsable folder                                                        │
│    Removable storage                                                         │
│    Library or Device                                                         │
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
│  Sources  ›  Game Cube                                                       │
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
│  Sources  ›  Game Cube  ›  New runner                                        │
│    Name           Game Cube                                                  │
│    Opens          *.rvz *.iso *.gcm          from the files in ~/Roms/gc     │
│    Command        dolphin-emu -b -e {path}  ◀                                │
│    Keymap         Default                                                    │
│    More           enqueue, pause, awake, BIOS…                               │
│    ✓ dolphin-emu is installed                                                │
│                         Done      Cancel                                     │
└──────────────────────────────────────────────────────────────────────────────┘
```

The Sources tab:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ... ⚙ Tools  +  │ Sources  Runners                     editing the library  │
│    Steam games          → Games/Steam          46   SNES scan                │
│    Desktop: waydroid.*  → Apps    (Android)    12   ~/Roms/snes → Games/SNES │
│    Desktop: Game        → Games                 9   opened by SNES           │
│    Desktop: all others  → Apps                 38   143 entries              │
│    SNES scan            → Games/SNES   ◀      143                            │
│    Films scan           → Films                71   Open    Move            │
│  · GC scan              → Games/Game Cube       0   Remove                   │
│    + New source                                     the folder is empty      │
└──────────────────────────────────────────────────────────────────────────────┘
```

The Runners tab:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ... ⚙ Tools  +  │ Sources  Runners                     editing the library  │
│    Desktop              *.desktop                   SNES                     │
│    Steam                                            *.sfc *.smc              │
│    NES                  *.nes                       retroarch -f -L …snes9x… │
│    SNES                 *.sfc *.smc    ◀            used by: SNES scan       │
│    Game Cube            *.rvz *.iso *.gcm           Open   Move   Remove     │
│    Video                *.mkv *.mp4 …                                        │
│  · PSP                  *.iso *.cso                 ppsspp isn't installed   │
│    Other files          *                                                    │
└──────────────────────────────────────────────────────────────────────────────┘
```

A command typed, the keyboard with the field's keys:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│    Command        dolphin-emu -b -e {path}▌                                  │
│                   ✓ dolphin-emu is installed                                 │
└──────────────────────────────────────────────────────────────────────────────┘
        ┌────────────────────────────────────────────────┐
        │ {path} {paths} {socket}    -   /   ~   "       │
        │  q  w  e  r  t  y  u  i  o  p                  │
        │   a  s  d  f  g  h  j  k  l                    │
        │  ⇧  z  x  c  v  b  n  m  ⌫                     │
        │        space              Done                 │
        └────────────────────────────────────────────────┘
```

## What the image ships

The image works without editing anything; the editors are for changing it.

Emulators, from a survey of what runs on the Thor's SM8550 (ROCKNIX's build for the Odin 2, the same
chip, is the reference) and of Arch Linux ARM's aarch64 repos:

- **Shipped already**, as they are: RetroArch's cores for NES (nestopia), SNES (snes9x), N64
  (mupen64plus-next), Game Boy (gambatte), GBA (mgba), DS (melonds), Mega Drive (genesis-plus-gx), PS1
  (beetle-psx-hw), Dreamcast (flycast).
- **Added from the repos**: Master System and Game Gear (genesis-plus-gx), PC Engine (beetle-pce-fast),
  Saturn (kronos), arcade (the MAME core), PSP (PPSSPP), ScummVM.
- **Packaged by us**, one at a time, each tried on the device: Dolphin (GameCube, Wii), Azahar (3DS),
  ARMSX2 (PS2; its Linux release is new), the DOSBox Pure core (DOS), the FBNeo core (arcade). Dolphin,
  Azahar and ARMSX2 need controller configs that we ship and put in the user's config.
- **Left out**: Wii U (Cemu: heavy games are marginal), Vita (Vita3K installs games and launches them by
  title id, which a runner doesn't fit), Switch, DuckStation (its licence forbids packaging).

snes9x, genesis-plus-gx and FBNeo are non-commercial: fine for a free image, as Arch ships them.

- **A runner for each**, in the starter library.json, with its BIOS files where it needs them.
- **The Roms folder**, `~/Roms/<system>`, with ES-DE's folder names (`nes`, `snes`, `n64`, `gb`, `gbc`,
  `gba`, `nds`, `3ds`, `megadrive`, `mastersystem`, `gamegear`, `pcengine`, `saturn`, `psx`, `ps2`, `psp`,
  `dreamcast`, `gc`, `wii`, `arcade`, `dos`, `scummvm`), so a collection from elsewhere drops in. Made
  empty at the first start. An empty system's folder shows dimmed, saying where its files go.
- **The menu**: Recent, Favourites, Games (a folder per system, Steam, the desktop's games), Films, TV,
  Music, Files (Home, Roms, removable storage), Apps, Tools (Device and ClamShell's tools).
- **The dock's first pins**: Library, Firefox, foot. Pins are the user's saved state, so the image's are
  a starter that the first start takes when nothing is saved. Firefox isn't in the image yet.

## The file picker

Library's pick mode: a request (a file or a folder, a filter, a folder to start in, who asked) shows only
the Files tab's places, folders always and files that match; the details' actions become Choose, and B or
Home cancels. ClamShell's own screens use it first (a source's folder, a folder's picture).

Then for apps, through xdg-desktop-portal:

- ClamShell serves `org.freedesktop.impl.portal.FileChooser` (OpenFile, SaveFile, SaveFiles) as
  `org.freedesktop.impl.portal.desktop.clamshell`, registered by a `.portal` file; `clamshell-portals.conf`
  sends only the file chooser to it (`org.freedesktop.impl.portal.FileChooser=clamshell`), the rest
  staying with GTK's. xdg-desktop-portal takes the first backend listed with the interface whether it runs
  or not (1.22): there is no fallback while ClamShell is down, when nothing could show a chooser anyway.
- Answers are `file://` URIs; unsandboxed apps need nothing more. A request waits for the user with no
  timeout, so it is answered asynchronously, off the compositor's loop; several may be open; the portal
  closing one (the app cancelled or went away) closes the picker. For Save, ClamShell asks before
  overwriting, and returns the filter chosen.
- The asking window: `wayland:<handle>` through xdg-foreign (v2; smithay has it, ClamShell doesn't use it
  yet, and resolving a handle needs a small public lookup in our smithay fork), `x11:<xid>` through our
  window manager, or none (Wine): then the window in front.
- Reach: GTK4, Firefox, Chromium and Electron, Steam and SDL3 use it by default; GTK3 (its native dialog
  only) with `GTK_USE_PORTAL=1`, Qt with `QT_QPA_PLATFORMTHEME=xdgdesktopportal`, both set for the session;
  Wine and Proton, Java, RetroArch's menu and PPSSPP never do (PPSSPP and SDL's fallback call zenity: a
  stand-in forwarding to the picker would reach them, left out unless it matters).

## Steps

Each ends in commits that pass `tools/check.sh` and the randomized test; the VM suite where the step
touches the session or the outside.

- [x] 1. **Rename** shelf to Library (`LibraryView` in code), in the UI, the code and the docs.
- [x] 2. **Setup as a wizard**: password, Wi-Fi, time zone; the bottom screen locked to the keyboard; the
  launch gate; Volume Up removed from `thor-account` (a thor-system release) and from `security.md`.
- [x] 3. **Built-ins as entries**: Library always running and pinned at 0 by the starter, Device an entry;
  `InFront::Shelf` and `TopView` gone.
  Done as: `Launch::Builtin`, the entries in Tools (the loader puts them there until step 4's `builtin`
  source); the session keeps which built-in shows with no window in front and whether Device runs, so the
  front, its history and Home go by built-ins (`InFront::Builtin`, `Front::Builtin`,
  `WindowCommand::Show`). `TopView` stays, worked out by State. The Library's tile is first on the dock
  whenever it isn't pinned (it always runs), so no starter pin is needed for position 0; once pinned it
  stays pinned, and can be moved. The code wasn't regrouped into `builtin::` modules.
- [x] 4. **The format**: places by `id`, `name`, `in`; the `match` default and `also`; built-in sources;
  runners' `bios`.
  Done as planned. A place's path is worked out from its `in` chain as the library is read; a source
  names a place by its id, and every folder is declared (no `@`, no paths, no undeclared folders). A
  missing BIOS is a library problem for now (the Rescan entry lists it); showing it on the system's folder
  is step 5's. Built-ins no source places are entries all the same, in no place.
- [x] 5. **What the image ships**: the repos' emulators and runners, `~/Roms`, the menu, the first pins,
  Firefox; then each emulator we package.
  Done for the repos' part: Master System, Game Gear, PC Engine, Saturn, arcade (MAME), PSP (PPSSPP's
  `PPSSPPSDL`) and ScummVM beside the cores already shipped, Firefox, `~/Roms/<system>`, Music, and the
  first pins (the Library, Firefox, foot, Steam, Maintenance). An empty system's folder is dimmed with
  where its files go and its missing BIOS. Tabs with nothing in them (Films, TV, Music on a new home)
  are still left out, as before. Packaging Dolphin, Azahar, ARMSX2, DOSBox Pure and FBNeo is still to do.
- [x] 6. **The editors**: Library's edit mode with its tabs and folders, Sources and Runners; the keyboard's field keys;
  the picker for ClamShell's own screens.
  In parts, all done: 6a tabs and folders (Rename, New tab and folder, Cut and Paste, Remove; entered
  from the Device view); 6c Sources and 6d Runners, in one form generic over either; 6e the snippets
  beside the keyboard; 6f the picker (Library's pick mode, the Files tab alone, Choose); 6b icons (the
  UI's, chosen in the details) and folders' pictures (picked). The tab row scrolls when its tabs don't
  fit. Under it all, library.json is shared data (clamshell-library's `config`), carried by the
  library as read and written back by `WriteLibrary`.
- [x] 7. **The portal**: Open, then Save, then the session's environment (`session::portal`, a VM check
  through xdg-desktop-portal). Saving is in the open folder: Save here under the name typed (Save as), or
  over a file chosen, each asking again over a file that is there (Replace). A request is its program's,
  found by the asking process (the request's handle names its connection), and shows as its dialog
  would: over that program when it is in front, set aside when left. The pick starts in the folder the
  app suggests, under the Files tab's folder that has it.
