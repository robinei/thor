# Library: places, runners, sources

Everything shelf starts is an item opened by a runner: `Zelda.sfc` by SNES (RetroArch with snes9x),
`Alien.mkv` by Video (mpv), `README.md` by Text, Spotify by Desktop (its desktop file's command), Hades by
Steam. library.json declares three things, and ClamShell shows nothing it doesn't declare:

- `places`: where things show (tabs, folders in them), their icons and pictures.
- `runners`: how things open.
- `sources`: where items come from, and which place they go to.

Settings come in three levels: All, the runner, the item. The format in full, with every field:
`clamshell/crates/system/src/library/content.rs`; a whole library: `packages/thor-session/root/usr/share/thor/home/.config/clamshell/library.json`.

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
  file, as a JSON string's contents); its items then have Enqueue (below). It needs `{socket}` in `launch`.

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
declares ClamShell's set: Desktop, Steam, one per RetroArch system (NES, SNES, Nintendo 64, Game Boy, Game
Boy Advance, Nintendo DS, Mega Drive, PlayStation, Dreamcast), Video and Music (mpv), Images (imv), PDF
(zathura), Text (`less` in foot), last Other files (`xdg-open`, any name). A runner whose program isn't on
the PATH is shown as a problem (it stays: the program may come).

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

`places` declares places: an `id` (`"place": "@snes"` elsewhere, `"@snes/RPG"` a folder in it; the folder's
id is `place:snes`, so pins, favourites and where a tab was outlive a new path), the `path` (a tab, then
folders in it), an `icon` and a folder's `picture`. A tab has only an icon, one of the UI's (`gamepad`):
something in it is always selected. A folder's icon (an image file, or one of the UI's) is what its row
shows; its picture is what the details show large when it is selected (a console, in a system's folder).
`"recent": false` keeps what is in a place (and its folders, unless they say) out of Recent: apps.
ClamShell's own tabs are places of a `type`: `recent`, `favourites`, `tools` (their path their name). The
declared tabs come in that order; a place not declared still works as a path (`folder:Games/SNES` its
folder's id). Nothing has a place or an icon of ClamShell's choosing.

`sources` says where entries come from and where they go, each by `type`:

- `dir`: a dir scanned, with an `id` (entries' ids are `<id>:<path in dir>`), `dir`, `place`, its `match`
  and `items` for the dir's layout, `metadata` (where its entries' covers and details come from:
  `libretro:<system>`, `tmdb:movie`, `tmdb:tv`) and, optionally, `runner`: the runner that opens all its
  entries (else each opens by the runner its file's name matches). The SNES scan names SNES, since its
  `*.zip` files would be Other files' by their name; the TV scan names none, its episodes Video's by
  theirs. Several may fill one place (SNES on internal storage and on an SD card). `on`: the removable
  filesystem its `dir` is on (`{ "label": "SD*" }`; `dir` then a path in it, `roms/snes`): while none
  matching is mounted the source is away (its entries' plays and settings kept), and it comes back when
  one is (a mount reads the library again).
- `browse`: a dir listed as it is opened (see files-tab.md), its own folder at its place.
- `desktop`, `steam`, `removable`: the desktop files' apps and games, Steam's installed games, the
  removable filesystems mounted (browsed). The sources of a type take their entries in turn: each, in
  order, the ones still left that it matches (desktop and Steam by id, name, a desktop file's categories;
  removable by `label`, `uuid`, `fs`; a source matching nothing in particular takes all that are left),
  putting them at its `place` or leaving them out (`hide`). An entry is in one place at most, the first
  matching source's; what none takes is left out. A desktop or Steam source names its `runner` (else the
  first of its type) and its `metadata` (`steamgriddb`, which needs `keys.steamgriddb`; a Steam source's
  `steam`, the store; none: nothing fetched). Games in Games and the rest in Apps is three sources in the
  starter library.json: `{ "type": "desktop", "categories": "Emulator", "place": "@apps" }`,
  `{ "type": "desktop", "categories": "Game", "place": "@games" }`, `{ "type": "desktop", "place": "@apps" }`.

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
tile and workspace. A running item keeps the title and picture it had at its launch, so its tile stays if
its file is renamed or deleted meanwhile.

Programs' tray icons (StatusNotifierItem over D-Bus: Steam's, and any other program's) are on the dock
too. ClamShell is the session's tray watcher where nothing else is (the Thor), else reads the one there (a
desktop around the nested session). An icon is the running entry's whose scope its process is in. A
program ClamShell didn't start runs as an entry found by its icon: the desktop file named as the icon's
id (`desktop:steam`), else one of its own named as the icon. Like any running entry it has its workspace:
its windows (those of the icon's process, and of processes it started) are moved there wherever they
open. A tile with an icon has a mark in its top left corner (three bars) that opens the program's menu;
the long-press menu has it as Menu. A tap shows the program's windows, or (none open) asks it to show
itself (Steam started silently), its window shown on its workspace. Quit closes its windows, then asks
the icon's process to end (SIGTERM); Force kill kills it. A window's entry is found by its app id first
(a Steam game's, though it runs in Steam's scope), then its scope, its process, its workspace.
`clamshell --tray` lists the icons and their menus; `cargo run -p clamshell-system --example fake_tray`
is a program with one, for testing (activated, it opens a terminal).

## Launches

Each launch is a systemd scope in its item's slice: whatever it starts stays in it (forking and
detaching don't leave a cgroup), so Quit (the windows closed, then the slice stopped) and Force kill reach
it all, windows or not. What gets out is what a program has another manager start: D-Bus activation, an
instance already running elsewhere. A file's launch that hands over and ends runs on by its windows (see
files-tab.md); Quit closes them, Force kill can't reach them. On the Thor nothing in the image starts that
way.

Two kinds of entry run in another program's processes on purpose, and are known by their windows' app id:
Steam's games (`steam_app_<id>`, in Steam's scope) and the entries of a runner with `handover` (their
desktop file's name: Waydroid's apps, in the Waydroid session). Alike for both:

- Their windows are found by their app id and put on their entry's workspace as they open; one opening while
  the entry doesn't run (a game started in Steam, an Android app opened by another) starts it, shown.
- Such an entry runs once: launching it again shows it.
- It runs on by its windows after its launch ends, until the last closes; its play counts until then.
- Force kill ends Steam's launcher for a game and the processes under it, and runs a `handover` runner's
  `stop`. Quit closes the windows (Android ends the app's task).
