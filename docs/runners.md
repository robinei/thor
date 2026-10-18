# Runners, places and settings

Everything shelf starts is content opened by a runner: `Zelda.sfc` by SNES (RetroArch with snes9x),
`Alien.mkv` by Video (mpv), `README.md` by Text, Spotify by Desktop (its desktop file's command), Hades by
Steam. library.json says how things open (runners) and where they show (places); settings come in three
levels: All, the runner, the item.

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
- `launch`: the command, `{path}` the file.
- `keymap`: a keymap of ClamShell's it comes with; `item_settings` (on unless `false`): whether its items
  have settings of their own.

A runner says nothing about where things are or show: that is the scans' (and the other places', below).

```json
"runners": [
  { "type": "desktop", "id": "desktop", "name": "Desktop", "match": ["*.desktop"] },
  { "type": "steam", "id": "steam", "name": "Steam" },
  { "id": "snes", "name": "SNES", "match": ["*.sfc", "*.smc"], "launch": ["retroarch", "-f", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{path}"] },
  { "id": "text", "name": "Text", "match": ["*.txt", "*.md"], "launch": ["foot", "less", "{path}"], "keymap": "viewer" },
  { "id": "other", "name": "Other files", "match": ["*"], "launch": ["xdg-open", "{path}"] }
]
```

The starter library.json (`tools/device/library.json`, installed on the device where there is none)
declares ClamShell's set: Desktop, Steam, one per RetroArch system (NES, SNES, Nintendo 64, Game Boy, Game
Boy Advance, Nintendo DS, Mega Drive, PlayStation, Dreamcast), Video and Music (mpv), Images (imv), PDF
(zathura), Text (`less` in foot), last Other files (`xdg-open`, any name). A runner whose program isn't on
the PATH is shown as a problem (it stays: the program may come).

A file is opened the same way wherever it is: a `.sfc` opened from the Files tab is SNES's, scanned or
not. A file that is a scanned entry's opens as that entry.

Two types of runner find their items themselves, each item with its own command: `"type": "desktop"` the
desktop files' apps and games (and a `.desktop` file opened from the Files tab, by its `Exec`), `"type":
"steam"` Steam's installed games (by app id). They take a name, a keymap, `item_settings`, but no command.
Without one, those items aren't in the library. Where their items show is the `desktop` and `steam` rules'
(Places).

## Places: where things show

Where an item shows, and what can be done to it there:

- `scans`: dirs whose entries go in the library, each with an `id` (entries' ids are `<id>:<path in
  dir>`), `dir`, `place`, its `match` and `items` for the dir's layout, `metadata` (where its entries'
  covers and details come from: `libretro:<system>`, `tmdb:movie`, `tmdb:tv`) and, optionally, `runner`:
  the runner that opens all its entries (else each opens by the runner its file's name matches). The
  SNES scan names SNES, since its `*.zip` files would be Other files' by their name; the TV scan names
  none, its episodes Video's by theirs. Several scans may fill one place (SNES on the card and on an SD
  card). `{ "id": "snes", "dir": "~/Games/snes", "place": "@snes", "runner": "snes", "match": ["*.sfc",
  "*.zip"], "metadata": "libretro:Nintendo - Super Nintendo Entertainment System" }`.
- `browse`: dirs listed as they are opened (see files-tab.md), each its own folder at its place.
- The rules for desktop files (`desktop`), Steam's games (`steam`) and removable storage (`removable`):
  the first matching one puts an item at its place, or hides it.

`places` declares places: an `id` (`"place": "@snes"` elsewhere, `"@snes/RPG"` a folder in it; the folder's
id is `place:snes`, so pins, favourites and where a tab was outlive a new path), the `path`, an `icon` and a
folder's `picture`. A tab has only an icon, one of the UI's (`gamepad`): something in it is always selected.
A folder's icon (an image file, or one of the UI's) is what its row shows; its picture is what the details
show large when it is selected (a console, in a system's folder). The declared tabs come in that order.
A place not declared still works as a path (`folder:Games/SNES` its folder's id).

They stay separate lists: their matchers differ (a desktop file's categories, a Steam app id, a
filesystem's label). A place decides nothing about opening.

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

- The runner: A opens (Play, Open, Run). The content: Continue (a folder of videos).
- The place: browsed dirs have the file actions (Copy, Cut, Paste, Rename, Move to Trash, New folder);
  the Trash Restore and Delete, its folder Empty Trash; a removable filesystem's folder Eject. A scan's
  entries have none (the scan owns those files).
- A stable identity: Favourite and Pin, for items whose id outlives a rescan (scanned entries, desktop
  apps, Steam games), not browsed files.

## The dock

A tile per item started: `README.md`, Spotify, Zelda, never the runner. The runner shows as the tile's
picture where the item has none of its own (a window `xdg-open` handed to an app: that app's icon) and as
the settings tab. An item running shows again rather than starting twice; each file opened is its own
tile and workspace. A running item keeps the title and picture it had at its launch, so its tile stays if
its file is renamed or deleted meanwhile.

## Launches

Each launch is a systemd scope in its item's slice: whatever it starts stays in it (forking and
detaching don't leave a cgroup), so Quit (the windows closed, then the slice stopped) and Force kill reach
it all, windows or not. What gets out is what a program has another manager start: D-Bus activation, an
instance already running elsewhere, Steam's games (in Steam's scope; followed by their windows). A file's
launch that hands over and ends runs on by its windows (see files-tab.md); Quit closes them, Force kill
can't reach them. On the Thor nothing in the image starts that way.

## Order of work

1. Runners replace sources: the format, the built-in runners, the Other files catch-all; files opened
   by the same matching everywhere.
2. Running items keep their title and picture.
3. Settings in three levels (folders dropped), the page's tabs named by the runner.
4. Keymap slots per level; the built-in runners' keymaps.
5. Actions from runner, content, place and identity.
6. The image: imv, zathura and its PDF backend.
