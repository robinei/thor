# Library: places, runners, sources

Everything the Library starts is an item opened by a runner: `Zelda.sfc` by SNES (RetroArch with snes9x),
`Alien.mkv` by Video (mpv), `README.md` by Text, Spotify by Desktop (its desktop file's command), Hades by
Steam. library.json declares three things, and ClamShell shows nothing it doesn't declare:

- `places`: where things show (tabs, folders in them), their icons and pictures.
- `runners`: how things open.
- `sources`: where items come from, and which place they go to.

Settings come in three levels: All, the runner, the item. The format in full, with every field:
`clamshell/crates/library/src/config.rs` and `clamshell/crates/system/src/library/content/`; a whole library: `packages/thor-session/root/usr/share/thor/home/.config/clamshell/library.json`.

| Item | Content | Runner |
|---|---|---|
| A SNES game | `Zelda.sfc` | SNES |
| A film | `Alien.mkv` | Video (mpv) |
| A file in the Files tab | `README.md` | Text |
| A desktop app | Spotify (its desktop file) | Desktop |
| A Steam game | Hades (its app id) | Steam |

## Runners: how things open

An ordered list in library.json, every runner declared there: nothing opens by a runner it doesn't list.
The first runner whose patterns match a file's name opens it. A runner has:

- `id`: what its settings, environment and own keymap are kept by, never shown (without one: its name in
  lowercase, spaces as `-`).
- `name`: its settings tab's title; it can change without losing the settings.
- `type`, for the two that find their items themselves (below); else:
- `match`: the file names it opens, one name each (`*.sfc`; any case).
- `launch`: the command, `{path}` the file (`{paths}`, an argument of its own: all the files, when it is
  started with several; `{socket}`: a control socket's path, its entry's).
- `keymap`: a keymap of ClamShell's it comes with; `item_settings` (on unless `false`): whether its items
  have settings of their own.
- `enqueue`: what to write to the control socket to add a file to a launch while it runs (`{path}` the
  file, as a JSON string's contents); its items then have Enqueue (below). It needs `{socket}` in `launch`,
  and must be mpv's JSON form with `{path}` inside a string: in its text form a file named `a; run ...`
  would run a program (otherwise a problem, and the runner has no Enqueue).
- `pause`, `resume`: commands that pause and resume all its entries at once, for entries sharing one
  container (Waydroid's); used when its In the background setting is Pauses hidden (see
  process-control.md).
- `awake`: a command that succeeds while its app on screen keeps the screen on in a way the compositor
  doesn't see (Android's keep-screen-on for a video); run before the screens dim.
- `bios`: the BIOS files its emulator needs and the folder it looks in (`{ "dir": "~/.config/retroarch/system",
  "files": ["scph5500.bin"] }`); each one missing is a problem.

A runner says nothing about where things are or show: that is the sources' and places' (below).

```json
"runners": [
  { "type": "desktop", "id": "desktop", "name": "Desktop", "match": ["*.desktop"] },
  { "type": "steam", "id": "steam", "name": "Steam" },
  { "id": "snes", "name": "SNES", "match": ["*.sfc", "*.smc"], "launch": ["retroarch", "-f", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{path}"] },
  { "id": "text", "name": "Text", "match": ["*.txt", "*.md"], "launch": ["foot", "less", "{path}"], "keymap": "viewer" },
  { "id": "other", "name": "Other files", "match": ["*"], "launch": ["xdg-open", "{path}"] }
]
```

The starter library.json (`packages/thor-session/root/usr/share/thor/home/.config/clamshell/library.json`, put in the home where there is none)
declares ClamShell's set: Desktop, Steam, one per system, mostly RetroArch's cores (NES, SNES, Nintendo
64, Game Boy and Color, Game Boy Advance, Nintendo DS, Mega Drive, Master System, Game Gear, PC Engine,
Saturn, PlayStation, Dreamcast, arcade with MAME), PSP (PPSSPP), ScummVM, Video and Music (mpv), Images
(imv), PDF (zathura), Text (`less` in foot), last Other files (`xdg-open`, any name). Each system's games
are scanned from `~/Roms/<system>`, by ES-DE's folder names (`snes`, `megadrive`, `psx`, ...), so a
collection from another frontend can be copied in as it is; the folders are made with the starter
library. A system's folder with nothing in it shows dimmed, saying where its files go, and lists the BIOS
files its runner lacks (`bios`). A runner whose program isn't on the PATH is shown as a problem (it stays:
the program may come).

A file is opened the same way wherever it is: a `.sfc` opened from the Files tab is SNES's, scanned or
not. A file that is a scanned entry's opens as that entry.

Two types of runner find their items themselves, each item with its own command: `"type": "desktop"` the
desktop files' apps and games (and a `.desktop` file opened from the Files tab, by its `Exec`), `"type":
"steam"` Steam's installed games (by app id). They take a name, a keymap, `item_settings`, but no command;
except that a desktop runner may have one, replacing its entries' own: `{id}` the desktop file's name,
`{exec}` its command (as arguments of their own). The starter's Android runner launches Waydroid's apps
by it (see android.md). A desktop runner's `handover` (off unless said): its launches hand their app over
to something running outside them and end, and its entries are known by their windows' app id (below);
its `stop`, the command that ends one (`{id}` the app id).
A desktop or Steam source is opened by the runner of its type it names (`runner`), else the first; there
may be several (one for desktop games, one for apps, each with its own settings). Where their items show is
the `desktop` and `steam` sources' (below).

## Places and sources: where things show

`places` declares the tabs and the folders in them, each by its `id` (`"place": "snes"` elsewhere), with
its `name` and `in`, the id of the place it is in (none: a tab). A rename changes only `name` and a move
only `in`, so what is kept by the id stays: sources' places, and a folder's pins, favourites and where a
tab was (its id is `place:snes`). Each has an `icon` and a folder a `picture`. A tab has only an icon, one
of the UI's (`gamepad`): something in it is always selected. A folder's icon (an image file, or one of the
UI's) is what its row shows; its picture is what the details show large when it is selected (a console,
in a system's folder). `"recent": false` keeps what is in a place (and its folders, unless they say) out
of Recent: apps. ClamShell's own tabs are places of a `type`: `recent`, `favourites`, `tools`. The tabs
come in the order declared. Nothing has a place or an icon of ClamShell's choosing.

```json
"places": [
  { "id": "games", "name": "Games", "icon": "gamepad" },
  { "id": "snes", "name": "SNES", "in": "games", "picture": "snes.png" },
  { "id": "files", "name": "Files", "icon": "folder" },
  { "id": "tools", "name": "Tools", "type": "tools", "icon": "tools" }
]
```

`sources` says where entries come from and where they go, each by `type`:

- `dir`: a dir scanned, with an `id` (entries' ids are `<id>:<path in dir>`), `dir`, `place`, its `match`
  and `items` for the dir's layout, `metadata` (where its entries' covers and details come from:
  `libretro:<system>`, `tmdb:movie`, `tmdb:tv`) and, optionally, `runner`: the runner that opens all its
  entries (else each opens by the runner its file's name matches). Without `match`, the files are its
  runner's (its patterns); `also` adds patterns to either. The SNES scan names SNES and adds `*.zip`
  (`"also": ["*.zip"]`), which would be Other files' by its name; the TV scan names none, its episodes
  Video's by theirs. Several may fill one place (SNES on internal storage and on an SD card). `on`: the removable
  filesystem its `dir` is on (`{ "label": "SD*" }`; `dir` then a path in it, `roms/snes`): while none
  matching is mounted the source is away (its entries' plays and settings kept), and it comes back when
  one is (a mount reads the library again).
- `browse`: a dir listed as it is opened (see files-tab.md), a folder of its own, `name`, in the place
  `in`.
- `desktop`, `steam`, `removable`: the desktop files' apps and games, Steam's installed games, the
  removable filesystems mounted (browsed). The sources of a type take their entries in turn: each, in
  order, the ones still left that it matches (desktop and Steam by id, name, a desktop file's categories;
  removable by `label`, `uuid`, `fs`; a source matching nothing in particular takes all that are left),
  putting them at its `place` (a removable source: as a folder of its own, `name`, default `{label}`, in
  the place `in`) or leaving them out (`hide`). An entry is in one place at most, the first
  matching source's; what none takes is left out. A desktop or Steam source names its `runner` (else the
  first of its type) and its `metadata` (`steamgriddb`, which needs `keys.steamgriddb`; a Steam source's
  `steam`, the store; none: nothing fetched). Games in Games and the rest in Apps is three sources in the
  starter library.json: `{ "type": "desktop", "categories": "Emulator", "place": "apps" }`,
  `{ "type": "desktop", "categories": "Game", "place": "games" }`, `{ "type": "desktop", "place": "apps" }`.
- `builtin`: one of ClamShell's own screens, by its `id` (`library`, `device`), at its `place`. One not
  placed is there all the same, in no place (the Library is always on the dock).

A source decides nothing about opening: that is the runners'.

## Settings

Three levels, each overriding the one before: All, the runner, the item. The settings page has a tab for
each the item has: `All | SNES | A Link to the Past`, `All | Desktop | Spotify`, `All | Steam | Hades`; only
`All | Text` for a file from the Files tab (its own settings would be tied to a file that can be renamed or
deleted at any time), for a folder, and for an item of a runner with `"item_settings": false`. A file that
is a library entry opens as that entry, with its tabs.

Folders are not a level. What folder settings were for (a system's games alike) is the runner's; a
folder is where things show, not how they run.

### Keymaps

Each level has a keymap slot, the Keymap setting: Inherit (the level above's; the default), Own (a keymap
kept at that level, edited in place), or a named keymap. Named keymaps are only for sharing one keymap
between runners or items.

A runner can come with a keymap of ClamShell's (`"keymap": "viewer"`), made its own the first time it is
seen without a keymap chosen and without one of its own: ClamShell's Text, PDF and Images have `viewer`
(the D-pad the arrow keys, A Space, B `q` to quit, the shoulders Page Up and Page Down), since `less`,
zathura and imv read the keyboard, not the pad. RetroArch and mpv read the pad themselves: Default suits
them, so ClamShell ships no keymap for them. Choosing another keymap for a runner keeps it; resetting the
runner's settings brings ClamShell's back at the next start.

### Steam's games: the launch hook

A launch's environment is the same however the game starts (`settings::launch_env`): the variables of the
settings acting at launch (the Frame rate cap's FIFO, x86 emulation's preset), then
`SDL_GAMECONTROLLER_IGNORE_DEVICES` (the controllers ClamShell grabbed), then the Env tile's for All, the
runner and the item, a later one replacing an earlier one of the same name.

ClamShell doesn't start a Steam game's process: `steam -applaunch` hands the request to the client. So
the variables reach the game through Steam's launch hook. Steam runs every game through
`steam-launch-wrapper` (`steamrtarm64/` on the ARM client), as `steam-launch-wrapper -- reaper SteamLaunch
AppId=<id> -- <Proton or the game>`, whether it is started from the Library or Steam's own window, its
install scripts too. `steam-setup --links` (each start of `steam`) points that link at
`/usr/lib/clamshell/steam-launch-wrapper` (`clamshell/steam/`), which:

- takes the app id from `AppId=<id>` among its arguments, else `SteamAppId`;
- runs `clamshell --launch-env steam:<id>`, which needs no running ClamShell: it reads the settings'
  files, finds the game's runner from library.json's Steam sources and runners (the first source taking
  the game, by id or name, and its runner), joins Steam's own `SDL_GAMECONTROLLER_IGNORE_DEVICES` (the
  controllers Steam Input maps) with the grabbed ones (`$XDG_RUNTIME_DIR/clamshell/grabbed`, written by
  the running ClamShell), and prints `KEY=value` lines;
- exports them and execs Valve's `linuxarm64/steam-launch-wrapper` with its arguments unchanged
  (`CLAMSHELL_STEAM_WRAPPER` names another: on an x86 client `ubuntu12_32/` or `steamrt64/`).

So a Steam game's settings are the saved ones, read from the files when Steam starts it, while
ClamShell's own launches take them from the running state. The two agree once a change is saved, which
the store thread does as the change is made (a slider when it is let go), well before Steam has started
a game. They differ only if the save failed: then Steam's games get the settings saved last, the store
logs the failure at `warn` and says so on a notice ("settings/ not saved"), and the change would be lost for every game at ClamShell's next start
anyway. The hook doesn't ask the running ClamShell instead: that would need a running ClamShell, a
query on the control pipe with a time limit, and the files as the fallback all the same.

On a failure the game starts as it is, and nothing is silent: the reason goes to the journal
(`journalctl -t clamshell-steam`) and a notice says "<game> started without its settings"
(`clamshell ctl launch-env failed steam:<id>`). ClamShell's programs run without Steam's
`LD_LIBRARY_PATH` and `LD_PRELOAD`, which are for the game.

Precedence: All, the runner, the game, then the Env tile, then what Steam itself sets. The user's Steam
launch options (`VAR=x %command%`) are set before the hook runs (Steam puts them in front of
`steam-launch-wrapper`), so a variable ClamShell also sets is replaced by ClamShell's; the others pass
unchanged. To set one of ClamShell's variables otherwise for a game, use its Env tile, not Steam's
launch options.

Checked on a desktop's x86 client (2026-10-30, Proton Experimental in Steam Linux Runtime 4,
`ubuntu12_32/steam-launch-wrapper` linked to the hook for the test): Steam called the hook with the app
id for the install script and for the game, and the game's process in the container
(`container=pressure-vessel`) had the cap's variables, `FEX_APP_CONFIG` and the Env tile's variables.
The ARM client's path is a device check (thor-gameplan.md).

### x86 emulation

FEX, the x86 emulator inside Proton ARM64 (ARM64EC), is set by a preset file named by `FEX_APP_CONFIG`,
which every FEX reads (Armada's way): Proton's own (nothing set), Fast (TSO off, half barriers off, x87
reduced precision, multiblock) or Safe (TSO with vector and memcpy TSO, full x87 precision, no
multiblock), Armada's two presets. ClamShell writes them to `~/.cache/clamshell/fex/` at its start, since
Valve's container replaces `/usr` but shares the home. The tile is lit wherever the launch's environment
reaches the program: whether an entry runs x86 code isn't known (a Steam game may be native arm64), and
the variable does nothing where FEX doesn't run. A Heroic or Lutris game runs in its launcher's
processes, with the launcher's launch-time settings and environment, so for the game's own level this
tile, the cap's, the Vulkan driver's and the Env tile are dimmed, as for an app handed over; the
launcher's own entry is where they are set. Steam's games get theirs from Steam's launch hook. Single FEX options
(`FEX_SMCCHECKS=full`, ...) go in the Env tile.

### Vulkan driver

The Vulkan driver tile selects Mesa system or Mesa newest at the next start, globally, per runner or
per game. Newest uses the pinned, Adreno-only `turnip-devel` package, built without LLVM. Its manifest
stays outside the loader's search path, so other programs keep their system driver. ClamShell's
launches and Steam's hook both set `VK_DRIVER_FILES` for the selected driver before the Env tile's
variables; an explicit Env value wins.

The menu shows installed versions. Newest is dimmed when the package or either of its driver files is
missing, but stays configurable. Package readings never change the saved choice. Each launch checks
again: if Newest has disappeared, it logs the fallback and uses the loader's defaults. Android apps
handed to Waydroid do not get the launch environment; the tile says "no effect" there. Whether the
private driver path reaches Proton ARM64 through pressure-vessel remains a device check.

## Actions

The details pane's rows each come from one of three things:

- The runner: A opens (Play, Open, Run); Enqueue, if it declares `enqueue`. The content: Continue (a
  folder of videos).
- The place: browsed dirs have the file actions (Copy, Cut, Paste, Rename, Move to Trash, New folder);
  the Trash Restore and Delete, its folder Empty Trash; a removable filesystem's folder Eject. A scan's
  entries have none (the scan owns those files).
- A stable identity: Favourite and Pin, for items whose id outlives a rescan (scanned entries, desktop
  apps, Steam games), not browsed files.

### Enqueue

Enqueue adds what the details show to what its runner plays: the launch of that runner most recently in
front gets it by its control socket (a notice says to what); with none running, it is launched with it. A
file is itself; a folder its playlist (an `.m3u` or `.m3u8` in it, so nothing is queued twice), else its
files of that runner all the way down, in list order; a Files tab folder not opened yet goes as the
folder (the player reads it), and has Enqueue if one of its first files is of such a runner. In the Files tab, the entries selected, in the order
they were picked. A folder launched this way is its own dock tile; a file alone opens as A opens it. The
starter library.json's Video and Music:

```json
{ "id": "music", "name": "Music", "match": ["*.mp3", "*.flac"], "launch": ["mpv", "--input-ipc-server={socket}", "--force-window=immediate", "{paths}"], "enqueue": "{\"command\": [\"loadfile\", \"{path}\", \"append-play\"]}" }
```

## The dock

A tile per item started: `README.md`, Spotify, Zelda, never the runner. The runner shows as the tile's
picture where the item has none of its own (a window `xdg-open` handed to an app: that app's icon) and as
the settings tab. An item running shows again rather than starting twice; each file opened is its own
tile and group of windows. A running item keeps the title and picture it had at its launch, so its tile stays if
its file is renamed or deleted meanwhile.

Programs' tray icons (StatusNotifierItem over D-Bus: Steam's, and any other program's) are on the dock
too. ClamShell is the session's tray watcher where nothing else is (the Thor), else reads the one there (a
desktop around the nested session). An icon is the running entry's whose unit its process is in. A
program ClamShell didn't start runs as an entry found by its icon: the desktop file named as the icon's
id (`desktop:steam`), else one of its own named as the icon. Like any running entry it has its group:
its windows (those of the icon's process, and of processes it started) are in it wherever they open. A tile with an icon has a mark in its top left corner (three bars) that opens the program's menu;
the long-press menu has it as Menu. A tap shows the program's windows, or (none open) asks it to show
itself (Steam started silently), and its window is shown. Quit closes its windows, then asks
the icon's process to end (SIGTERM); Force kill kills it. A window's entry is found by its app id first
(a Steam game's, though it runs in Steam's unit), then its unit, its process, its group.
`clamshell --tray` lists the icons and their menus; `cargo run -p clamshell-system --example fake_tray`
is a program with one, for testing (activated, it opens a terminal).

## Launches

Each launch is a systemd service in its item's slice: whatever it starts stays in it (forking and
detaching don't leave a cgroup), so Quit (the windows closed, then the slice stopped) and Force kill reach
it all, windows or not. What gets out is what a program has another manager start: D-Bus activation, an
instance already running elsewhere. A file's launch that hands over and ends runs on by its windows (see
files-tab.md); Quit closes them, Force kill can't reach them. On the Thor nothing in the image starts that
way.

Two kinds of entry run in another program's processes on purpose, and are known by a mark of their
windows:

- Launchers' games, by the mark the launcher gives the game's processes, read from a window's process:
  Steam's (`SteamAppId`; also the window's app id `steam_app_<id>`), in Steam's unit; Heroic's (`HEROIC_APP_NAME`
  and `HEROIC_APP_RUNNER`, matching the `heroic://launch?appName=…&runner=…` of its shortcut); Lutris's (the
  game's name in the `lutris-wrapper` process it runs under, matching the name of a `lutris:rungameid/`
  shortcut; two shortcuts of one name are known by none). Heroic and Lutris, when they already run, take a
  game's launch from its shortcut, which ends, and start the game in their own unit. A Heroic game is
  launched as a launch of Heroic's entry, without Heroic's window (see process-control.md), so that Heroic,
  which runs on after the game, is an entry of its own. A launcher's game waits up to 2 minutes for its
  first window. A launch from the Library shows only the game; if the game's window hasn't come 30 s after
  the launch was asked for, the launcher's window comes in front until it does, since a launcher still
  busy that long is usually showing a prompt, its progress or an error. A launcher with no window (Steam
  started `-silent`, only in its tray) shows nothing and isn't asked to. See `clamshell/crates/state/src/session/launchers.rs`.
- The entries of a runner with `handover`, by their windows' app id, their desktop file's name: Waydroid's
  apps, in the Waydroid session.

Alike for both:

- Their windows are found by their mark and put in their entry's group as they open; one opening while
  the entry doesn't run (a game started in Steam's, Heroic's or Lutris's own window, an Android app opened
  by another) starts it, shown.
- Such an entry runs once: launching it again shows it.
- It runs on by its windows after its launch ends, until the last closes; its play counts until then.
- Force kill ends Steam's launcher for a game and the processes under it, and runs a `handover` runner's
  `stop`. Quit closes the windows (Android ends the app's task).
