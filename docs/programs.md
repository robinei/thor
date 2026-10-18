# Programs, places and settings (design)

Everything shelf starts is content opened by a program: `Zelda.sfc` by SNES (RetroArch with snes9x),
`Alien.mkv` by mpv, `README.md` by Text. A desktop app or a Steam game is a program with no content of its
own. library.json says how things open (programs) and where they show (places); settings come in three
levels: All, the program, the content.

| Item | Content | Program |
|---|---|---|
| A SNES game | `Zelda.sfc` | SNES |
| A film | `Alien.mkv` | Films (mpv) |
| A file in the Files tab | `README.md` | Text |
| A desktop app | none | the app (Spotify) |
| A Steam game | none | the game |

## Programs: how things open

An ordered list; the first program whose patterns match a file's name opens it. A program has:

- `name`: shown as its settings tab's title; its settings are kept by it (renaming one starts its settings
  afresh).
- `match` (and `items`): name patterns, as sources have them now.
- `launch`: the command, `{path}` the file.
- Its settings, keymap and environment (see Settings).
- `scan`, optionally: a dir whose matching entries go in the library (`dir`, `tab`, `group`,
  `metadata`), as a source does now. Sources become programs that scan.

```json
"programs": [
  { "name": "SNES", "match": ["*.sfc", "*.smc"], "launch": ["retroarch", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{path}"],
    "scan": { "dir": "~/Games/snes", "tab": "Games", "group": "SNES", "metadata": "libretro:Nintendo - Super Nintendo Entertainment System" } },
  { "name": "Text", "match": ["*.txt", "*.md", "*.log"], "launch": ["foot", "less", "{path}"] }
]
```

ClamShell's own programs come after library.json's: one per RetroArch system whose core is installed;
mpv for video and audio; Images (imv); PDF (zathura); Text (`less` in foot); last, Other files, `xdg-open`
for anything. library.json's come first, so one of its own wins over a built-in for the same names.

A file is opened the same way wherever it is: a `.sfc` opened from the Files tab is SNES's, scanned or not.
A file that is a scanned entry's opens as that entry.

Each desktop file is a program of its own (its name, its command); so is each Steam game. Neither has
content.

## Places: where things show

Where an item shows, and what can be done to it there:

- A program's `scan`: its entries in its tab and group.
- `browse`: dirs listed as they are opened (see files-tab.md).
- The rules for desktop files (`desktop`), Steam's games (`steam`) and removable storage (`removable`):
  the first matching one puts an item in a tab and group, or hides it.

They stay separate lists: their matchers differ (a desktop file's categories, a Steam app id, a
filesystem's label). A place decides nothing about opening.

## Settings

Three levels, each overriding the one before: All, the program, the content. The settings page has a tab
for each the item has: `All | SNES | A Link to the Past`; `All | Spotify` for an app or a Steam game (no
content); `All | Text` for a file from the Files tab (its own settings would be tied to a file that can be
renamed or deleted at any time). A file that is a library entry opens as that entry, with its tabs.

Folders are not a level. What folder settings were for (a system's games alike) is the program's; a
folder is where things show, not how they run.

### Keymaps

Each level has a keymap slot: Inherit (the level above's; the default), Own (a keymap kept at that
level, edited in place), or a named keymap. Named keymaps are only for sharing one keymap between programs
or items.

ClamShell's programs come with their own keymaps filled in: Text, PDF and Images with B to quit and the
D-pad to scroll. The one named keymap ClamShell ships is RetroArch (its hotkeys, B to its menu), which
every system's program uses; a system that needs another (Nintendo 64's C buttons on the right stick) has
its own.

## Actions

The details pane's rows each come from one of three things:

- The program: A opens (Play, Open, Run). The content: Continue (a folder of videos).
- The place: browsed dirs have the file actions (Copy, Cut, Paste, Rename, Move to Trash, New folder);
  the Trash Restore and Delete, its folder Empty Trash; a removable filesystem's folder Eject. A scan's
  entries have none (the scan owns those files).
- A stable identity: Favourite and Pin, for items whose id outlives a rescan (scanned entries, desktop
  apps, Steam games), not browsed files.

## The dock

A tile per item started: `README.md`, Spotify, Zelda, never the program. The program shows as the tile's
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

1. Programs replace sources: the format, the built-in programs, the Other files catch-all; files opened
   by the same matching everywhere.
2. Running items keep their title and picture.
3. Settings in three levels (folders dropped), the page's tabs named by the program.
4. Keymap slots per level; the built-in programs' keymaps; RetroArch's named one.
5. Actions from program, content, place and identity.
6. The image: imv, zathura and its PDF backend.
