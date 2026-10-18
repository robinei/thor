# Files tab

A tab, made by library.json like the others, for browsing the file system: Home, Root, the Trash, and
removable storage as it comes and goes. Not scanned up front: a folder is listed when it is opened, and
listed again whenever it changes while it is open.

## library.json

```json
"sources": [
  { "type": "browse", "place": "@files/Home", "dir": "~" },
  { "type": "browse", "place": "@files/Root", "dir": "/" },
  { "type": "browse", "place": "@files/Trash", "dir": "${XDG_DATA_HOME:-~/.local/share}/Trash/files", "trash": true },
  { "type": "removable", "place": "@files/{label}", "rules": [
      { "label": "SD*", "place": "@files/SD card" },
      { "fs": "vfat", "place": "@files/USB/{label}" },
      { "uuid": "1234-ABCD", "hide": true } ] }
]
```

- `browse`: a source per dir, its own folder at `place` (its last part the folder's title; `Files/Places/Root`
  puts it in a folder `Places`). Nothing in it is scanned, fetched or counted beforehand.
- `dir` (here and in scans): `~` and `~/` at the start; `$VAR`, `${VAR}`, `${VAR:-default}` anywhere
  (the default itself expanded). An unset variable without a default skips the entry, shown as a problem.
- `removable`: the removable filesystems mounted under `/run/media` (udisks2's mounts; udiskie mounts what
  is put in), each at the place of the first of its rules matching it (`label`, `uuid`, `fs`: globs, any
  case; `{label}` in a place replaced by its label, else its UUID), else at the source's `place`; a rule
  may hide one. Without a removable source, none shows. A mount or unmount reads the library again, so a
  folder comes and goes, and scans on it come back.
- Browsed dirs' folders come after the tab's other folders, in the order library.json and the mounts give
  them.

## Entries

A browsed entry is a library entry like the rest (the same rows, details, focus and navigation), marked
as browsed: its id is its folder's id, `/`, its name; it has no metadata, isn't in Recent, and can't be a
favourite or pinned (those are kept by id across rescans, which a listing isn't). Hidden names (`.name`)
are left out. A picture (`.jpg`, `.png`, `.webp`) is its own thumbnail; the rest show a folder or file
icon. The details show a file's size and age.

A file that is a library entry's (a ROM, a film the sources scanned) opens as that entry: its plays,
Recent, where mpv left it. Any other file opens with the first `sources` entry whose `match` or `items`
(their last part, any case) fits its name (an `.sfc` in RetroArch, an `.mkv` in mpv, wherever it is), else
`xdg-open`; either way as a launch of its own, in its own slice and workspace, on the dock while it runs.

`xdg-open` often hands the file to a program not ours (an app's running instance, D-Bus activation) and
ends at once, so nothing of the launch is left to follow. Such a launch runs on by its windows, as a game
Steam runs does: a new window of no launch of ours that comes up within 15 seconds is moved to its
workspace (never left on shelf's), and it ends when that window closes (or when none came). Its dock tile
shows the picture of the app whose window it is (the desktop file named by the window's app id, or whose
program it is), for display only.

## Actions

The details pane has the entry's file actions instead of a game's (Favourite, Pin, Continue): rows to tap
or reach with the D-pad, Y and X as shortcuts.

- A: Open.
- Y: Copy. Cut. Rename (the on-screen keyboard's editor). Move to Trash.
- X: Paste into the open folder (shown while something is copied or cut). New folder.
- In the Trash: Restore, Delete. On the Trash's folder: Empty Trash. On a removable filesystem's folder:
  Eject (`udisksctl unmount`, then `power-off` where the device allows it).
- In an empty folder the details show the folder itself, with Paste and New folder.

Delete and Empty Trash ask first: the row turns into "… · press again". A paste never replaces anything: a
name that is taken gets " (2)". A move within a filesystem is a rename; across filesystems a copy then a
delete. A copy shows its progress under the actions; every operation ends in a notice (what was done, or
what went wrong).

The Trash follows the freedesktop spec: `files/` and `info/<name>.trashinfo` (original path, deletion
date). Something on another filesystem than the home Trash goes to that filesystem's `.Trash-<uid>` (the
Files tab shows the home Trash only).

## Inside

- **Listings:** opening a browsed folder sends `Effect::Browse(folder)`; the browse thread lists it and
  sends `FilesEvent::Listed`, then watches it (inotify) and lists it again when it changes; leaving it
  (out of it, another tab) drops the watch. The shelf splices a listing into the library
  (`Library::set_listing`): entries there before keep their place in the library's list (and their own
  listings), those gone free theirs for the next.
- **Rescans** replace the library; the shelf keeps the listings it was sent and applies them to the new
  library (those whose folder is still there), so a rescan keeps the place in a browsed folder.
- **Operations** go to the files thread (`Effect::Files`), one after another.
- **Mounts:** the browse thread reads `/proc/self/mounts` as it polls; a change under `/run/media` asks
  for a rescan. Labels and UUIDs come from `/dev/disk/by-label` and `by-uuid`.

## Not done

- Selecting several entries at once.
- Removable filesystems' own Trash folders in the Files tab.
- Showing hidden files.
