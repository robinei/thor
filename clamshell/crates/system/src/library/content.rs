//! library.json: the places declared, the runners (how things open: see [`super::runners`]), the sources
//! (where entries come from, and where they go), the services' keys. Nothing is ClamShell's but what it
//! declares: a tab, a runner, a source left out isn't there.
//!
//! ```json
//! { "places": [
//!     { "id": "recent", "path": "Recent", "type": "recent", "icon": "clock" },
//!     { "id": "games", "path": "Games", "icon": "gamepad" },
//!     { "id": "apps", "path": "Apps", "icon": "apps", "recent": false },
//!     { "id": "tools", "path": "Tools", "type": "tools", "icon": "tools" },
//!     { "id": "snes", "path": "Games/SNES", "icon": "snes-icon.png", "picture": "snes.png" }
//!   ],
//!   "runners": [
//!     { "type": "desktop", "id": "desktop", "name": "Desktop", "match": ["*.desktop"] },
//!     { "id": "snes", "name": "SNES", "match": ["*.sfc", "*.smc"],
//!       "launch": ["retroarch", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{path}"] },
//!     { "id": "other", "name": "Other files", "match": ["*"], "launch": ["xdg-open", "{path}"] }
//!   ],
//!   "sources": [
//!     { "type": "dir", "id": "snes", "dir": "~/Games/snes", "place": "@snes", "runner": "snes",
//!       "match": ["*.sfc", "*.zip"], "metadata": "libretro:..." },
//!     { "type": "desktop", "categories": "Game", "place": "@games" },
//!     { "type": "desktop", "place": "@apps" },
//!     { "type": "browse", "dir": "~", "place": "@files/Home" }
//!   ],
//!   "keys": { "tmdb": "...", "steamgriddb": "..." } }
//! ```
//!
//! `places` (see [`Place`]): an `id` to refer to a place by (`"place": "@snes"`, `"@snes/RPG"` a folder in
//! it; its folder's id `place:snes`, which outlives a new path), its `path` (a tab, then folders in it: see
//! [`place`]; a place not declared works as a path too), an `icon` (a tab's one of the UI's by name; a
//! folder's also an image file, shown in its row), a folder's `picture` (shown large when it is selected),
//! `recent: false` (what is in it isn't in Recent: apps), `type` for ClamShell's own tabs (`recent`,
//! `favourites`, `tools`: its path their name). The tabs come in the order declared (those not declared
//! after, as first named).
//!
//! `sources` (see [`Source`]), by `type`:
//! - `dir` (see [`Scan`]): a dir scanned. `runner` (an id) opens its entries, else each opens by the runner
//!   its file's name matches (a group as its first entry). `match`: patterns (see clamshell-library's
//!   `pattern`) against paths in `dir` (else its runner's); an entry per name at the top of `dir`: a file,
//!   or a folder (matched itself, `*/`, or by a file in it, `*/*.cue`); where several files of a folder
//!   match, the first pattern wins, then the first name. `items`: a matched folder is a group: in it and
//!   every folder under it, the files matching these patterns are entries and the folders with any are
//!   groups (a series' seasons, their episodes). `metadata`: where its entries' covers, screenshots and the
//!   rest come from (`libretro:<system>`, `tmdb:movie`, `tmdb:tv`; `keys`: the services' keys, see
//!   [`crate::meta`]). Ids: `<id>:<path in dir>`, so moving the whole dir (a new `dir`) keeps them;
//!   renaming or moving inside it doesn't. Titles: the name at the top of `dir`, an episode's file name,
//!   without the extension (a file's); in a season's folder (`Season 2`, `S02`, `Series 2`; `Specials` for
//!   season 0), a file named for an episode of that season (`017 S02E18 Title`) is titled by its number
//!   there: `18. Title` (`1–2.` for a double one). Box art: `<name>.jpg` (or .jpeg, .png, .webp) beside a
//!   file, or `poster`, `cover` or `folder` in a folder (any case: `Cover.jpg`); else the group's, but for
//!   a video mpv plays (its frame is its picture). A folder's art: its place's picture, else `poster`,
//!   `cover` or `folder` in the scan's dir. `on`: the removable filesystem `dir` is in (see [`On`]),
//!   `dir` then a path in it; the source is away while none matching is mounted.
//! - `browse`: a dir listed as it is opened, not scanned (no metadata, not in Recent), its own folder at
//!   its `place` (its last part the folder's title); `trash`: the Trash's `files` dir.
//! - `desktop`, `steam`: the desktop files' apps and games, Steam's installed games (opened by a runner of
//!   that type). Each source of the type takes, in order, those left that it matches (see [`Rule`]: by id,
//!   name, a desktop file's categories; globs, any case; none: all), putting them at its `place`, or leaving
//!   them out (`hide`); what none takes is left out. `runner`: the runner of the type that opens them (else
//!   the first); `metadata`: `steamgriddb`, a Steam source's `steam`. A desktop file's id is its name without
//!   `.desktop`, a Steam game's its app id. `{ "type": "desktop", "id": "net.lutris.*", "place": "@apps" }`,
//!   `{ "type": "desktop", "categories": "Game", "place": "@games" }`, `{ "type": "desktop", "id":
//!   "*uninstall*", "hide": true }`, `{ "type": "desktop", "place": "@apps" }` (the rest).
//! - `removable`: the removable filesystems mounted (udisks2's, under `/run/media/$USER`), browsed as
//!   `browse`'s dirs, taken the same way (see [`MountRule`]: by `label`, `uuid`, `fs`), each at its source's
//!   `place` (`{label}` in it the filesystem's label, else its UUID).
//!
//! `~` and `~/` at the start of `dir` or of a launch argument is the home dir; in `dir`, `$VAR`, `${VAR}`
//! and `${VAR:-default}` are environment variables (`${XDG_DATA_HOME:-~/.local/share}/roms`). A source
//! whose dir is missing is skipped, as one naming a variable unset without a default (shown); so is a file
//! that doesn't parse (logged).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use clamshell_library::pattern::{self, hidden};
use clamshell_library::{Entry, Launch, sort};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Places declared: their ids (to refer to as `@id`), icons and pictures; the tabs' order; ClamShell's
    /// own tabs.
    #[serde(default)]
    pub places: Vec<Place>,
    #[serde(default)]
    pub runners: Vec<super::runners::Runner>,
    /// Where entries come from, and where they go.
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub keys: crate::meta::Keys,
}

/// Where entries come from (`type`), and where they go: a dir scanned (`dir`), one browsed (`browse`), the
/// desktop files' apps and games (`desktop`), Steam's installed games (`steam`), the removable
/// filesystems mounted (`removable`). Of the last three, the sources of a type take their entries in
/// turn: each the ones left that it matches (see [`Rule`], [`MountRule`]), putting them at its place or
/// leaving them out (`hide`); what none takes is left out.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    Dir(Scan),
    Browse(BrowseRoot),
    Desktop(Rule),
    Steam(Rule),
    Removable(MountRule),
}

/// A place declared: its `id` (`"place": "@snes"`, `"@snes/RPG"` elsewhere; its folder's id, `place:snes`,
/// which outlives a new path), its `path` (`Games/SNES`; one part: a tab), its `icon` (one of the UI's by
/// name, `gamepad`; a folder's also an image file, its row's) and a folder's `picture` (an image file,
/// shown large when it is selected; a tab has none: something in it is always selected). Image files: `~/`
/// the home dir, else from library.json's dir. `recent: false`: what is in it (and in its folders, unless
/// they say) isn't in Recent (apps). `type`: one of ClamShell's own tabs (`recent`, `favourites`,
/// `tools`), its path its name.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Place {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
    #[serde(default)]
    pub recent: Option<bool>,
    #[serde(default, rename = "type")]
    pub kind: Option<OwnTab>,
}

/// One of ClamShell's own tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnTab {
    /// The entries played last (not those in a place with `recent: false`).
    Recent,
    Favourites,
    /// ClamShell's own tools (rescanning the library, fetching what is missing again).
    Tools,
}

/// `place` with a declared place's id (`@snes`, `@snes/RPG`) as its path; Err: an id not declared.
pub fn resolve(place: &str, places: &[Place]) -> Result<String, String> {
    let Some(reference) = place.trim().strip_prefix('@') else {
        return Ok(place.to_owned());
    };
    let (id, rest) = reference.split_once('/').unwrap_or((reference, ""));
    let declared = places.iter().find(|p| p.id == id).ok_or_else(|| format!("{place}: no place {id} declared"))?;
    Ok(if rest.is_empty() { declared.path.clone() } else { format!("{}/{rest}", declared.path.trim_end_matches('/')) })
}

/// A dir browsed: its own folder at `place` (its last part the folder's title: `Files/Home`,
/// `Files/Places/Root`), listed when it is opened; `trash`: it is the Trash's `files` dir (its entries
/// restored, deleted).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowseRoot {
    pub place: String,
    pub dir: String,
    #[serde(default)]
    pub trash: bool,
}

/// A removable source: the mounted removable filesystems (those left) matching all of its `label`,
/// `uuid` and `fs` (globs, any case; none: all) go at `place` as their folder (its last part the folder's
/// title; `{label}` in it their label, or their UUID without one), or are left out (`hide`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountRule {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub fs: Option<String>,
    #[serde(default)]
    pub place: Option<String>,
    #[serde(default)]
    pub hide: bool,
}

impl MountRule {
    /// Whether it matches a filesystem with this label, UUID and type.
    pub fn matches(&self, label: &str, uuid: &str, fs: &str) -> bool {
        On { label: self.label.clone(), uuid: self.uuid.clone(), fs: self.fs.clone() }.matches(label, uuid, fs)
    }
}

/// A desktop or Steam source: the desktop files' entries or Steam's games (those left) matching all of
/// its `id`, `name` and `categories` (globs, any case; `categories` matches if any of an entry's does;
/// Steam's games have none; none: all) go at `place` (a tab, or a folder in one: `@games/Launchers`), or
/// are left out (`hide`); `runner` (an id; else the first runner of the type) opens them, `metadata`
/// (`steamgriddb`; a Steam source's `steam`, its store) is where their covers and details come from.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub categories: Option<String>,
    #[serde(default)]
    pub place: Option<String>,
    #[serde(default)]
    pub hide: bool,
    #[serde(default)]
    pub runner: Option<String>,
    #[serde(default)]
    pub metadata: Option<String>,
}

impl Rule {
    /// Whether it matches an entry with this id, name and categories.
    pub fn matches(&self, id: &str, name: &str, categories: &[String]) -> bool {
        let glob = |pattern: &str, value: &str| pattern::matches(&pattern.to_lowercase(), &value.to_lowercase());
        self.id.as_deref().is_none_or(|p| glob(p, id))
            && self.name.as_deref().is_none_or(|p| glob(p, name))
            && self.categories.as_deref().is_none_or(|p| categories.iter().any(|c| glob(p, c)))
    }
}

/// A dir whose entries go in the library: `id` names them (`<id>:<path in dir>`), `place` says where they
/// show, `match` (paths in `dir`, a part per folder level: `*/*.cue`) and `items` (a matched folder's own
/// entries) say what is an entry, `metadata` where their covers and details come from; `runner` (an id)
/// opens them all, else each opens by the runner its file's name matches (see [`super::runners`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scan {
    pub id: String,
    pub dir: String,
    /// Where its entries go: a tab (`Games`), or a folder in one (`Games/SNES`, `@snes`).
    pub place: String,
    #[serde(default, rename = "match")]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub metadata: Option<String>,
    #[serde(default)]
    pub runner: Option<String>,
    /// The removable filesystem `dir` is on (the first mounted matching this): then `dir` is a path in it
    /// (`roms/snes`), and the source is away while none is mounted.
    #[serde(default)]
    pub on: Option<On>,
}

/// A removable filesystem, by its `label`, `uuid` and `fs` (globs, any case; none: any).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct On {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub fs: Option<String>,
}

impl On {
    /// Whether it matches a filesystem with this label, UUID and type.
    pub fn matches(&self, label: &str, uuid: &str, fs: &str) -> bool {
        let glob = |pattern: &str, value: &str| pattern::matches(&pattern.to_lowercase(), &value.to_lowercase());
        self.label.as_deref().is_none_or(|p| glob(p, label))
            && self.uuid.as_deref().is_none_or(|p| glob(p, uuid))
            && self.fs.as_deref().is_none_or(|p| glob(p, fs))
    }
}

/// How a scanned file opens: its command and its runner, if a runner opens it.
pub type Open<'a> = dyn Fn(&Path) -> Option<(Launch, clamshell_library::RunnerOf)> + 'a;

/// A place's tab and the path of folders in it (none: the tab's top): `Games/Handhelds/GBA` is Games and
/// `Handhelds/GBA`; slashes at its ends and spaces around its parts don't count.
pub fn place(place: &str) -> (&str, &str) {
    let place = place.trim().trim_matches('/');
    match place.split_once('/') {
        Some((tab, path)) => (tab.trim(), path.trim().trim_matches('/')),
        None => (place, ""),
    }
}

/// The config at `file`; none if it is missing, or if it doesn't parse (then what is wrong with it).
pub fn read_config(file: &Path) -> (Config, Option<String>) {
    let Ok(text) = fs::read_to_string(file) else {
        return (Config::default(), None);
    };
    match serde_json::from_str(&text) {
        Ok(config) => (config, None),
        Err(e) => {
            log::warn!("library: {}: {e}", file.display());
            (Config::default(), Some(format!("library.json: {e}")))
        }
    }
}

/// A scan's entries: appended to `games` (groups before their items, each file opened as `open` says, a
/// group by its first entry's runner), the indices of its top-level ones returned.
pub fn scan(source: &Scan, home: Option<&Path>, games: &mut Vec<Entry>, open: &Open) -> Vec<usize> {
    let Ok(dir) = expand_dir(&source.dir, home) else {
        return Vec::new();
    };
    if !dir.is_dir() {
        log::info!("library: {}: no dir {}", source.id, dir.display());
        return Vec::new();
    }
    // The first match of each name at the top, pattern by pattern.
    let mut taken = HashSet::new();
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for p in &source.patterns {
        let (parts, folders) = pattern::parts(p);
        for path in matches(&dir, &parts, folders) {
            let Some(top) = path.strip_prefix(&dir).ok().and_then(|r| r.components().next()).and_then(|c| c.as_os_str().to_str()).map(str::to_owned) else {
                continue;
            };
            if taken.insert(top.clone()) {
                found.push((top, path));
            }
        }
    }
    let mut tops = Vec::new();
    for (top, path) in found {
        let title = untagged(&if path.is_file() && path.parent() == Some(dir.as_path()) { stem(&top) } else { top.clone() });
        let id = format!("{}:{}", source.id, path.strip_prefix(&dir).unwrap_or(&path).to_string_lossy());
        let art = art(&path, &dir);
        let index = if path.is_dir() && !source.items.is_empty() {
            match group(&path, &dir, &source.id, &source.items, open, None, games) {
                Some(g) => g,
                None => continue,
            }
        } else {
            let (launch, runner) = opened(open, &path);
            games.push(Entry { launch, runner, file: Some(path.clone()), ..Entry::new(&id, &title) });
            games.len() - 1
        };
        if let Some(g) = games.get_mut(index) {
            title.clone_into(&mut g.title);
            g.sort_key = sort::key(&title);
            g.box_front = art;
        }
        tops.push(index);
    }
    tops
}

/// `path`'s command and runner as `open` says; nothing (and none) if no runner opens it.
fn opened(open: &Open, path: &Path) -> (Launch, Option<clamshell_library::RunnerOf>) {
    open(path).map_or((Launch::None, None), |(launch, runner)| (launch, Some(runner)))
}

/// The paths under `dir` that `parts` match, a part per level (the last a folder if `folders`, else a
/// file), in name order.
fn matches(dir: &Path, parts: &[&str], folders: bool) -> Vec<PathBuf> {
    let Some((part, rest)) = parts.split_first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, path) in listing(dir) {
        if hidden(&name, part) || !pattern::matches(part, &name) {
            continue;
        }
        if !rest.is_empty() {
            if path.is_dir() {
                out.extend(matches(&path, rest, folders));
            }
        } else if path.is_dir() == folders {
            out.push(path);
        }
    }
    out
}

/// A folder's names and paths, sorted as titles are.
fn listing(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut entries: Vec<(String, PathBuf)> =
        fs::read_dir(dir).map(|d| d.flatten().filter_map(|e| Some((e.file_name().to_str()?.to_owned(), e.path()))).collect()).unwrap_or_default();
    entries.sort_by_cached_key(|(name, _)| (sort::key(name), name.clone()));
    entries
}

/// The group of `folder` (in `dir`, the source's): its subfolders with entries first, then its files
/// matching `items`; added to `games` with its items, its index returned. None if nothing in it matches.
fn group(folder: &Path, dir: &Path, source: &str, items: &[String], open: &Open, parent: Option<usize>, games: &mut Vec<Entry>) -> Option<usize> {
    let id = |path: &Path| format!("{source}:{}", path.strip_prefix(dir).unwrap_or(path).to_string_lossy());
    let name = folder.file_name()?.to_string_lossy().into_owned();
    let index = games.len();
    let art = art(folder, dir).or_else(|| parent.and_then(|p| games.get(p)).and_then(|g| g.box_front.clone()));
    // (A folder's entries in its outermost folder's colour: one colour for a series while it has no art.)
    let mut own = Entry { parent, box_front: art.clone(), ..Entry::new(&id(folder), &name) };
    if let Some(p) = parent.and_then(|p| games.get(p)) {
        own.color = p.color;
    }
    let color = own.color;
    games.push(own);
    let listing = listing(folder);
    let season = season_of(&name);
    let mut members = Vec::new();
    for (_, path) in listing.iter().filter(|(n, p)| p.is_dir() && !n.starts_with('.')) {
        if let Some(g) = group(path, dir, source, items, open, Some(index), games) {
            members.push(g);
        }
    }
    let mut files = Vec::new();
    for (file, path) in listing.iter().filter(|(_, p)| p.is_file()) {
        if !items.iter().any(|p| !hidden(file, p) && pattern::matches(p, file)) {
            continue;
        }
        // (A video keeps its own picture, its frame: not its folder's art.)
        let (launch, runner) = opened(open, path);
        let video = Entry { launch: launch.clone(), file: Some(path.clone()), ..Entry::new("", "") }.mpv_file().is_some();
        let box_front = art_beside(path).or_else(|| art.clone().filter(|_| !video));
        games.push(Entry {
            parent: Some(index),
            box_front,
            color,
            launch,
            runner,
            file: Some(path.clone()),
            ..Entry::new(&id(path), &season.and_then(|n| episode_title(&stem(file), n)).unwrap_or_else(|| untagged(&stem(file))))
        });
        files.push(games.len() - 1);
    }
    // The files by their titles (an episode's number, where it has one), after the folders.
    files.sort_by(|&a, &b| {
        let key = |i: usize| games.get(i).map(|g| (&g.sort_key, &g.title));
        key(a).cmp(&key(b))
    });
    members.extend(files);
    if members.is_empty() {
        games.truncate(index);
        return None;
    }
    // (Its runner: its first entry's. Its playlist: an `.m3u` in it, enqueued instead of its entries.)
    let runner = members.iter().find_map(|&m| games.get(m).and_then(|g| g.runner.clone()));
    let playlist = listing.iter().find(|(n, p)| p.is_file() && [".m3u", ".m3u8"].iter().any(|e| n.to_lowercase().ends_with(e))).map(|(_, p)| p.clone());
    if let Some(g) = games.get_mut(index) {
        g.items = members;
        g.runner = runner;
        g.playlist = playlist;
    }
    Some(index)
}

/// Image extensions looked for, in order.
const IMAGES: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
/// A folder's own art, by name.
const FOLDER_ART: [&str; 3] = ["poster", "cover", "folder"];

/// An entry's art: a folder's own; a file's beside it, else its folder's (one inside the source's `dir`).
fn art(path: &Path, dir: &Path) -> Option<PathBuf> {
    picture_of(path).or_else(|| path.parent().filter(|p| *p != dir && p.starts_with(dir) && path.is_file()).and_then(folder_art))
}

/// A file's or folder's own picture, wherever it is (scanned or browsed): a folder's `poster`, `cover` or
/// `folder` image in it; a file's `<name>.<image>` beside it, else the file itself if it is an image.
pub fn picture_of(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        return folder_art(path);
    }
    let image = path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGES.contains(&e.to_lowercase().as_str()));
    art_beside(path).filter(|p| p != path).or_else(|| image.then(|| path.to_path_buf()))
}

/// `<name>.<image>` beside a file.
fn art_beside(file: &Path) -> Option<PathBuf> {
    IMAGES.iter().map(|ext| file.with_extension(ext)).find(|p| p.is_file())
}

pub(crate) fn folder_art(folder: &Path) -> Option<PathBuf> {
    // (By name in any case: `Cover.jpg` too.)
    let files: Vec<(String, PathBuf)> =
        fs::read_dir(folder).map(|d| d.flatten().filter_map(|e| Some((e.file_name().to_str()?.to_lowercase(), e.path()))).collect()).unwrap_or_default();
    FOLDER_ART
        .iter()
        .flat_map(|name| IMAGES.iter().map(move |ext| format!("{name}.{ext}")))
        .find_map(|wanted| files.iter().find(|(n, p)| *n == wanted && p.is_file()).map(|(_, p)| p.clone()))
}

/// The season a folder is, by its name: `Season 2`, `S02`, `Series 2`; `Specials` is season 0.
fn season_of(folder: &str) -> Option<u32> {
    let name = folder.trim().to_lowercase();
    if name == "specials" {
        return Some(0);
    }
    let number = ["season", "series", "s"].iter().find_map(|w| name.strip_prefix(w))?;
    number.trim().parse().ok()
}

/// An episode's title in season `season`'s folder, from its file name: `017 S01E18 Title` (season 1) is
/// `18. Title`, `S05E01E02 Title` is `1–2. Title`; none if the name has no episode of that season.
fn episode_title(stem: &str, season: u32) -> Option<String> {
    let chars: Vec<char> = stem.chars().collect();
    let digits = |from: usize| -> (u32, usize) {
        let end = (from..chars.len()).find(|&i| !chars.get(i).is_some_and(char::is_ascii_digit)).unwrap_or(chars.len());
        (chars.get(from..end).unwrap_or(&[]).iter().collect::<String>().parse().unwrap_or(u32::MAX), end)
    };
    for at in 0..chars.len() {
        if !matches!(chars.get(at), Some('S' | 's')) || at > 0 && chars.get(at - 1).is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        let (s, after_s) = digits(at + 1);
        if after_s == at + 1 || !matches!(chars.get(after_s), Some('E' | 'e')) {
            continue;
        }
        let (first, mut end) = digits(after_s + 1);
        if end == after_s + 1 || s != season {
            continue;
        }
        // A double episode: E01E02, E01-E02, E01-02.
        let mut last = first;
        let next = if chars.get(end) == Some(&'-') { end + 1 } else { end };
        let next = if matches!(chars.get(next), Some('E' | 'e')) {
            next + 1
        } else if next > end {
            next
        } else {
            usize::MAX
        };
        if next != usize::MAX {
            let (n, e) = digits(next);
            if e > next && n > first {
                (last, end) = (n, e);
            }
        }
        let rest: String = chars.get(end..).unwrap_or(&[]).iter().collect();
        let rest = untagged(rest.trim_start_matches([' ', '-', '.', '_']).trim());
        let number = if last > first { format!("{first}–{last}") } else { first.to_string() };
        return Some(if rest.is_empty() { format!("{number}.") } else { format!("{number}. {rest}") });
    }
    None
}

/// A file name without its extension.
fn stem(name: &str) -> String {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem).to_owned()
}

/// A title without the dump tags at its end (`(U) [!]`, `(USA) (Rev 1)`, `(Japan, USA) (En,Ja)`, a
/// translation's ` # SNES`); words in brackets stay (`(Part 1)`), and so does a year (`Cosmos (1980)`).
fn untagged(title: &str) -> String {
    let mut t = title.split_once(" # ").map_or(title, |(t, _)| t).trim_end();
    while let Some(open) = t.strip_suffix([')', ']']).and_then(|rest| rest.rfind(['(', '['])) {
        let square = t.ends_with(']');
        let tag = t.get(open + 1..t.len() - 1).unwrap_or("");
        let rest = t.get(..open).unwrap_or("").trim_end();
        if rest.is_empty() || !(square || dump_tag(tag)) {
            break;
        }
        t = rest;
    }
    t.to_owned()
}

/// Whether a bracketed tag is a ROM dump's (a region, languages, a revision, a stage): `U`, `JU`, `M5`,
/// `V1.1`, `USA`, `Japan, USA`, `En,Ja`, `Rev 1`, `Beta`, `Unl`, `Hack`.
fn dump_tag(tag: &str) -> bool {
    const WORDS: [&str; 26] = [
        "usa",
        "europe",
        "japan",
        "world",
        "asia",
        "korea",
        "brazil",
        "germany",
        "france",
        "spain",
        "italy",
        "australia",
        "canada",
        "china",
        "rev",
        "beta",
        "proto",
        "prototype",
        "demo",
        "sample",
        "unl",
        "hack",
        "pirate",
        "virtual console",
        "en",
        "ja",
    ];
    let short_code = tag.len() <= 5
        && tag.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.' || c == '-')
        && tag.chars().any(|c| c.is_ascii_uppercase());
    let words = tag.split([',', '+']).map(str::trim).all(|w| {
        let w = w.to_ascii_lowercase();
        WORDS.iter().any(|k| w == *k || w.starts_with(&format!("{k} "))) || (w.len() == 2 && w.chars().all(|c| c.is_ascii_alphabetic()))
    });
    short_code || words
}

/// `~` and `~/...` against `home`.
pub(crate) fn expand(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        (None, Some(home)) if path == "~" => home.to_path_buf(),
        _ => PathBuf::from(path),
    }
}

/// A dir as library.json gives it: its environment variables (see [`expand_vars`]), then `~/`. Err: a
/// variable without a value or a default (its name).
pub(crate) fn expand_dir(dir: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    expand_vars(dir, &|name| std::env::var(name).ok()).map(|d| expand(&d, home))
}

/// `$NAME`, `${NAME}` and `${NAME:-default}` (the default when it is unset or empty; expanded itself) in
/// `text`, by `lookup`; a `$` before anything else is itself. Err: a variable without a value or a default
/// (its name).
pub(crate) fn expand_vars(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let name_len =
        |s: &str| s.char_indices().find(|&(i, c)| !(c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))).map_or(s.len(), |(i, _)| i);
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(rest.get(..at).unwrap_or(""));
        let after = rest.get(at + 1..).unwrap_or("");
        if let Some(inner) = after.strip_prefix('{') {
            // To the matching brace (a default may hold `${...}` itself).
            let mut depth = 1;
            let end = inner.char_indices().find(|&(_, c)| {
                depth += match c {
                    '{' => 1,
                    '}' => -1,
                    _ => 0,
                };
                depth == 0
            });
            let Some((end, _)) = end else {
                out.push_str(rest.get(at..).unwrap_or(""));
                return Ok(out);
            };
            let body = inner.get(..end).unwrap_or("");
            let (name, default) = match body.split_once(":-") {
                Some((name, default)) => (name, Some(default)),
                None => (body, None),
            };
            match (lookup(name).filter(|v| !v.is_empty()), default) {
                (Some(value), _) => out.push_str(&value),
                (None, Some(default)) => out.push_str(&expand_vars(default, lookup)?),
                (None, None) => return Err(name.to_owned()),
            }
            rest = inner.get(end + 1..).unwrap_or("");
        } else {
            let len = name_len(after);
            if len == 0 {
                out.push('$');
            } else {
                let name = after.get(..len).unwrap_or("");
                out.push_str(&lookup(name).ok_or_else(|| name.to_owned())?);
            }
            rest = after.get(len..).unwrap_or("");
        }
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn touch(root: &Path, files: &[&str]) {
        for f in files {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        }
    }

    fn source(json: &str) -> Scan {
        serde_json::from_str(json).unwrap()
    }

    /// Opens every file with `program`.
    fn by(program: &str) -> impl Fn(&Path) -> Option<(Launch, clamshell_library::RunnerOf)> + '_ {
        move |path: &Path| {
            let launch = Launch::Program { argv: vec![program.into(), path.to_string_lossy().into()], workdir: path.parent().map(Path::to_path_buf) };
            Some((launch, clamshell_library::RunnerOf::new(program, program, true)))
        }
    }

    #[test]
    fn files_and_folders_at_the_top_first_pattern_first() {
        let root = clamshell_core::testdir::TempDir::new("content");
        touch(
            &root,
            &["Zelda.sfc", "Zelda.png", "Mario.smc", "notes.txt", ".hidden.sfc", "FF7/FF7.m3u", "FF7/FF7 (Disc 1).cue", "FF7/cover.jpg", "Empty/readme"],
        );
        let s = source(r#"{ "place": "Games", "id": "g", "dir": "~/x", "match": ["*.sfc", "*.smc", "*/*.m3u", "*/*.cue"] }"#);
        let mut games = Vec::new();
        let s = Scan { dir: root.to_string_lossy().into_owned(), ..s };
        let tops = scan(&s, None, &mut games, &by("emu"));
        let found: Vec<(&str, &str)> = tops.iter().map(|&i| (&*games[i].id, games[i].title.as_str())).collect();
        assert_eq!(found, vec![("g:Zelda.sfc", "Zelda"), ("g:Mario.smc", "Mario"), ("g:FF7/FF7.m3u", "FF7")]);
        assert_eq!(games[0].box_front, Some(root.join("Zelda.png")));
        assert_eq!(games[2].box_front, Some(root.join("FF7/cover.jpg")), "its folder's art");
        let path = root.join("FF7/FF7.m3u");
        assert_eq!(games[2].launch, Launch::Program { argv: vec!["emu".into(), path.to_string_lossy().into()], workdir: Some(root.join("FF7")) });
    }

    #[test]
    fn a_folder_with_items_is_a_group_of_groups() {
        let root = clamshell_core::testdir::TempDir::new("series");
        touch(
            &root,
            &[
                "Show/poster.jpg",
                "Show/Season 2/1. Later.mkv",
                "Show/Season 1/10. Ten.mkv",
                "Show/Season 1/2. Two.mkv",
                "Show/Season 1/2. Two.srt",
                "Show/Extras/notes.txt",
                "Film/Film.mkv",
            ],
        );
        let s = source(r#"{ "place": "TV", "id": "tv", "dir": "", "match": ["*/"], "items": ["*.mkv"] }"#);
        let s = Scan { dir: root.to_string_lossy().into_owned(), ..s };
        let mut games = Vec::new();
        let tops = scan(&s, None, &mut games, &by("mpv"));
        let titles = |ids: &[usize]| ids.iter().map(|&i| games[i].title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&tops), vec!["Film", "Show"]);
        let show = &games[tops[1]];
        assert_eq!(titles(&show.items), vec!["Season 1", "Season 2"], "Extras has nothing: no group");
        let season = &games[show.items[0]];
        assert_eq!(titles(&season.items), vec!["2. Two", "10. Ten"]);
        let episode = &games[season.items[1]];
        assert_eq!(&*episode.id, "tv:Show/Season 1/10. Ten.mkv");
        assert_eq!(season.box_front, Some(root.join("Show/poster.jpg")), "the series' art");
        assert_eq!(episode.box_front, None, "a video: its frame, not the series' art");
        assert_eq!(episode.parent, Some(show.items[0]));
        assert!(games[tops[0]].is_group(), "a film's folder too");
    }

    #[test]
    fn episodes_in_their_seasons_folder_are_titled_by_their_number() {
        assert_eq!((season_of("Season 2"), season_of("S02"), season_of("Series 10"), season_of("Specials")), (Some(2), Some(2), Some(10), Some(0)));
        assert_eq!((season_of("Extras"), season_of("Sxx")), (None, None));
        assert_eq!(episode_title("017 S01E18 A Voice in the Wilderness (Part 1)", 1).as_deref(), Some("18. A Voice in the Wilderness (Part 1)"));
        assert_eq!(episode_title("S01E01 Killeroo", 1).as_deref(), Some("1. Killeroo"));
        assert_eq!(episode_title("001 S00E01 The Gathering", 0).as_deref(), Some("1. The Gathering"));
        assert_eq!(episode_title("Show.S05E01E02.Double.Trouble", 5).as_deref(), Some("1–2. Double.Trouble"));
        assert_eq!(episode_title("S03E07-08 Two Parter", 3).as_deref(), Some("7–8. Two Parter"));
        assert_eq!(episode_title("S02E03 Wrong season", 1), None, "another season's: as named");
        assert_eq!(episode_title("Sunset Boulevard", 1), None);
        let root = clamshell_core::testdir::TempDir::new("seasons");
        touch(&root, &["Show/Cover.jpg", "Show/Season 1/017 S01E18 Part (Part 1).mkv", "Show/Season 1/S01E02 Two.mkv", "Show/Specials/001 S00E01 Pilot.mkv"]);
        let s = Scan { dir: root.to_string_lossy().into_owned(), ..source(r#"{ "place": "TV", "id": "tv", "dir": "", "match": ["*/"], "items": ["*.mkv"] }"#) };
        let mut games = Vec::new();
        let tops = scan(&s, None, &mut games, &by("mpv"));
        let show = &games[tops[0]];
        assert_eq!(show.box_front, Some(root.join("Show/Cover.jpg")), "Cover.jpg");
        let titles = |ids: &[usize]| ids.iter().map(|&i| games[i].title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&show.items), ["Season 1", "Specials"]);
        assert_eq!(titles(&games[show.items[0]].items), ["2. Two", "18. Part (Part 1)"], "by number");
        assert_eq!(titles(&games[show.items[1]].items), ["1. Pilot"]);
    }

    #[test]
    fn titles_lose_their_tags_but_not_a_year() {
        assert_eq!(untagged("7th Saga, The (U) [!]"), "7th Saga, The");
        assert_eq!(untagged("Super Metroid (Japan, USA) (En,Ja)"), "Super Metroid");
        assert_eq!(untagged("Star Ocean # SNES"), "Star Ocean");
        assert_eq!(untagged("Cosmos (1980)"), "Cosmos (1980)");
        assert_eq!(untagged("(Untitled)"), "(Untitled)");
        assert_eq!(untagged("1. Silence"), "1. Silence");
        assert_eq!(untagged("017 S01E18 A Voice in the Wilderness (Part 1)"), "017 S01E18 A Voice in the Wilderness (Part 1)");
        assert_eq!(untagged("Super Battletank - War in the Gulf (U) (V1.1) [!]"), "Super Battletank - War in the Gulf");
        assert_eq!(untagged("Street Fighter II (USA) (Rev 1)"), "Street Fighter II");
        assert_eq!(untagged("Power Drive (U) (Beta) (M5)"), "Power Drive");
        assert_eq!(untagged("Doom (Final)"), "Doom (Final)");
    }

    #[test]
    fn a_config_that_doesnt_parse_is_none() {
        let file = std::env::temp_dir().join(format!("clamshell-library-{}.json", std::process::id()));
        fs::write(&file, r#"{ "sources": [{ "type": "dir", "place": "Games" }] }"#).unwrap();
        let (config, problem) = read_config(&file);
        assert!(config.runners.is_empty());
        assert!(problem.is_some_and(|p| p.starts_with("library.json: missing field")), "said");
        let _ = fs::remove_file(&file);
        assert_eq!(expand("~/Games", Some(Path::new("/h"))), PathBuf::from("/h/Games"));
        assert_eq!(expand("~", Some(Path::new("/h"))), PathBuf::from("/h"));
    }

    #[test]
    fn places_are_referred_to_by_id() {
        let places: Vec<Place> = serde_json::from_str(r#"[{ "id": "snes", "path": "Games/SNES" }]"#).unwrap();
        assert_eq!(resolve("@snes", &places).unwrap(), "Games/SNES");
        assert_eq!(resolve("@snes/RPG", &places).unwrap(), "Games/SNES/RPG");
        assert_eq!(resolve("Games/GBA", &places).unwrap(), "Games/GBA", "a path as it is");
        assert!(resolve("@gba", &places).is_err());
    }

    #[test]
    fn a_place_is_a_tab_and_a_path_in_it() {
        assert_eq!(place("Games/Handhelds/GBA"), ("Games", "Handhelds/GBA"));
        assert_eq!(place(" /Games/ "), ("Games", ""));
        assert_eq!(place("Files/ Home /"), ("Files", "Home"));
    }

    #[test]
    fn dirs_take_environment_variables() {
        let env = |name: &str| match name {
            "XDG_DATA_HOME" => Some("/d".to_owned()),
            "EMPTY" => Some(String::new()),
            _ => None,
        };
        assert_eq!(expand_vars("$XDG_DATA_HOME/Trash", &env).unwrap(), "/d/Trash");
        assert_eq!(expand_vars("${XDG_DATA_HOME}x", &env).unwrap(), "/dx");
        assert_eq!(expand_vars("${NOPE:-~/.local/share}/Trash", &env).unwrap(), "~/.local/share/Trash");
        assert_eq!(expand_vars("${EMPTY:-${XDG_DATA_HOME}}/a", &env).unwrap(), "/d/a", "empty: the default, itself expanded");
        assert_eq!(expand_vars("a$ b$1", &env).unwrap(), "a$ b$1", "not a name: itself");
        assert_eq!(expand_vars("$NOPE/x", &env), Err("NOPE".to_owned()));
        assert_eq!(expand_vars("${NOPE}", &env), Err("NOPE".to_owned()));
    }
}
