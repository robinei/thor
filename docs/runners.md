# Runners, places and settings

Everything shelf starts is content opened by a runner: `Zelda.sfc` by SNES (RetroArch with snes9x),
`Alien.mkv` by mpv, `README.md` by Text. A desktop app or a Steam game is a runner with no content of its
own. library.json says how things open (runners) and where they show (places); settings come in three
levels: All, the runner, the content.

| Item | Content | Runner |
|---|---|---|
| A SNES game | `Zelda.sfc` | SNES |
| A film | `Alien.mkv` | Video (mpv) |
| A file in the Files tab | `README.md` | Text |
| A desktop app | none | the app (Spotify) |
| A Steam game | none | the game |

## Runners: how things open

An ordered list; the first runner whose patterns match a file's name opens it. A runner has:

- `id`: what its settings, environment and own keymap are kept by, never shown (without one: its name in
  lowercase, spaces as `-`).
- `name`: its settings tab's title; it can change without losing the settings.
- `match`: the file names it opens, one name each (`*.sfc`; any case).
- `launch`: the command, `{path}` the file.
- Its settings, keymap and environment (see Settings).
- `scan`, optionally: dirs whose matching entries go in the library, each with an `id` (entries' ids
  are `<id>:<path in dir>`), `dir`, `tab`, `group`, `icon` (the tab's), its own `match` and `items` for
  the dir's layout (else the runner's patterns) and `metadata` (else the runner's). What used to be
  sources are runners that scan; one runner may scan several dirs (Video: films and series).

```json
"runners": [
  { "id": "snes", "scan": [{ "id": "snes", "dir": "~/Games/snes", "tab": "Games", "group": "SNES", "match": ["*.sfc", "*.smc", "*.zip"] }] },
  { "id": "video", "scan": [{ "id": "tv", "dir": "~/TV", "tab": "TV", "match": ["*/"], "items": ["*.mkv"], "metadata": "tmdb:tv" }] },
  { "id": "text", "name": "Kate", "match": ["*.txt", "*.md", "*.log"], "launch": ["kate", "{path}"] }
]
```

ClamShell's own runners come after library.json's, those whose program (and core) is installed: one per
RetroArch system; Video and Music (mpv); Images (imv); PDF (zathura); Text (`less` in foot); last, Other
files, `xdg-open` for any name. Their ids: `nes`, `snes`, `n64`, `gb`, `gba`, `nds`, `md`, `psx`, `dc`,
`video`, `music`, `images`, `pdf`, `text`, `other`. A runner in library.json with one of those ids takes
what it leaves out (name, patterns, command, metadata, keymap) from it and takes its place: `{ "id": "snes", "scan": [...] }` scans
with ClamShell's SNES. A scan's own patterns (the SNES scan's `*.zip`) find entries in its dir but open
nothing elsewhere: a `.zip` in the Files tab is Other files'.

A file is opened the same way wherever it is: a `.sfc` opened from the Files tab is SNES's, scanned or not.
A file that is a scanned entry's opens as that entry.

Each desktop file is a runner of its own (its name, its command); so is each Steam game. Neither has
content.

## Places: where things show

Where an item shows, and what can be done to it there:

- A runner's `scan`: its entries in its tab and group.
- `browse`: dirs listed as they are opened (see files-tab.md).
- The rules for desktop files (`desktop`), Steam's games (`steam`) and removable storage (`removable`):
  the first matching one puts an item in a tab and group, or hides it.

They stay separate lists: their matchers differ (a desktop file's categories, a Steam app id, a
filesystem's label). A place decides nothing about opening.

## Settings

Three levels, each overriding the one before: All, the runner, the content. The settings page has a tab
for each the item has: `All | SNES | A Link to the Past`; `All | Spotify` for an app or a Steam game (no
content); `All | Text` for a file from the Files tab (its own settings would be tied to a file that can be
renamed or deleted at any time). A file that is a library entry opens as that entry, with its tabs.

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
