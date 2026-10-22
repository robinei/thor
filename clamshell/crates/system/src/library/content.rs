//! library.json: the declared places, the runners that open things (see [`super::runners`]), the sources
//! that say where entries come from and where they go, and the keys for metadata services. ClamShell adds
//! nothing of its own: a tab, runner or source that is left out of the file doesn't exist.
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
//! `places` (see [`Place`]) have these fields:
//! - `id` refers to the place, as in `"place": "@snes"`, or `"@snes/RPG"` for a folder in it. The place's
//!   folder gets the id `place:snes`, which stays the same if its path changes.
//! - `path` is a tab, then folders in it (see [`place`]). A place that isn't declared can be given as a
//!   path too.
//! - `icon` is one of the UI's icons by name. A folder's icon can also be an image file, shown in its row.
//! - `picture` is a folder's image, shown large when the folder is selected.
//! - `recent: false` keeps what is in the place out of Recent, for apps.
//! - `type` makes it one of ClamShell's own tabs (`recent`, `favourites`, `tools`), named by its path.
//!
//! The tabs come in the order they are declared. Tabs that aren't declared come after, in the order they
//! are first named.
//!
//! `sources` (see [`Source`]), by `type`:
//! - `dir` (see [`Scan`]) is a dir that is scanned.
//!   - `runner`, an id, opens its entries. Without it, each file opens with the runner whose patterns its
//!     name matches, and a group opens as its first entry.
//!   - `match` is a list of patterns (see clamshell-library's `pattern`) matched against paths in `dir`;
//!     without it, the runner's are used. Each name at the top of `dir` gives one entry: a file, or a
//!     folder matched itself (`*/`) or through a file in it (`*/*.cue`). Where several files in a folder
//!     match, the first pattern wins, then the first name.
//!   - `items` makes each matched folder a group. In it and every folder under it, the files matching
//!     these patterns are entries and the folders containing any are groups, such as a series' seasons and
//!     their episodes.
//!   - `metadata` says where its entries' covers, screenshots and other details come from:
//!     `libretro:<system>`, `tmdb:movie` or `tmdb:tv`. The services' keys are in `keys` (see
//!     [`crate::library::meta`]).
//!   - `on` names the removable filesystem `dir` is on (see [`On`]); `dir` is then a path inside it. The
//!     source is away while no matching filesystem is mounted.
//!
//!   Ids are `<id>:<path in dir>`, so moving the whole dir and updating `dir` keeps them, but renaming or
//!   moving things inside it doesn't.
//!
//!   An entry's title is its name at the top of `dir`, or an episode's file name, without a file's
//!   extension. In a season's folder (`Season 2`, `S02`, `Series 2`, or `Specials` for season 0), a file
//!   named for an episode of that season, like `017 S02E18 Title`, is titled by its number: `18. Title`,
//!   or `1–2.` for a double episode.
//!
//!   Box art is `<name>.jpg` (or .jpeg, .png, .webp) beside a file, or `poster`, `cover` or `folder` in a
//!   folder, in any case (`Cover.jpg` works). Otherwise an entry uses its group's art, except a video mpv
//!   plays, whose frame is its picture. The folder at the source's place uses the place's picture, or
//!   else a `poster`, `cover` or `folder` image in the scan's dir.
//! - `browse` is a dir that is listed when it is opened instead of being scanned, so it has no metadata
//!   and isn't in Recent. It gets its own folder at its `place`, titled by the last part. `trash` marks it
//!   as the Trash's `files` dir.
//! - `desktop` and `steam` take the apps and games from desktop files and Steam's installed games, which
//!   are opened by a runner of that type. The sources of each type take entries in order: each takes the
//!   remaining ones it matches (see [`Rule`]) and puts them at its `place`, or leaves them out with
//!   `hide`. Matching is by id, name and a desktop file's categories, as globs in any case; a rule with
//!   none of them matches everything. Entries no source takes are left out. `runner` is the runner of the
//!   type that opens them, by default the first. `metadata` is `steamgriddb`, or `steam` for a Steam
//!   source. A desktop file's id is its name without `.desktop`, and a Steam game's is its app id.
//!   Examples, in order:
//!   `{ "type": "desktop", "id": "net.lutris.*", "place": "@apps" }`,
//!   `{ "type": "desktop", "categories": "Game", "place": "@games" }`,
//!   `{ "type": "desktop", "id": "*uninstall*", "hide": true }`,
//!   `{ "type": "desktop", "place": "@apps" }` for the rest.
//! - `removable` takes the mounted removable filesystems (udisks2's, under `/run/media/$USER`). They are
//!   browsed like `browse` dirs and taken in the same way as `desktop` entries, matched by `label`, `uuid`
//!   and `fs` (see [`MountRule`]). Each goes at its source's `place`, where `{label}` is replaced by the
//!   filesystem's label, or its UUID if it has none.
//!
//! `~` and `~/` at the start of `dir` or of a launch argument mean the home dir. In `dir`, `$VAR`,
//! `${VAR}` and `${VAR:-default}` are environment variables, as in `${XDG_DATA_HOME:-~/.local/share}/roms`.
//! A source whose dir is missing is skipped. So is one naming an unset variable with no default, which is
//! shown as a problem. A file that doesn't parse is skipped too, and logged.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use clamshell_library::pattern::{self, hidden};
use clamshell_library::titles::{episode_title, season_of, stem, untagged};
use clamshell_library::{Entry, Launch, sort};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The declared places, with their ids (referred to as `@id`), icons and pictures. They also set the
    /// tabs' order and declare ClamShell's own tabs.
    #[serde(default)]
    pub places: Vec<Place>,
    #[serde(default)]
    pub runners: Vec<super::runners::Runner>,
    /// Where entries come from, and where they go.
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub keys: crate::library::meta::Keys,
}

/// Where entries come from, by `type`, and where they go. The types are a scanned dir (`dir`), a browsed
/// dir (`browse`), the apps and games from desktop files (`desktop`), Steam's installed games (`steam`)
/// and the mounted removable filesystems (`removable`).
///
/// For the last three, the sources of a type take their entries in turn. Each takes the remaining ones it
/// matches (see [`Rule`], [`MountRule`]) and puts them at its place, or leaves them out with `hide`.
/// Entries no source takes are left out.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    Dir(Scan),
    Browse(BrowseRoot),
    Desktop(Rule),
    Steam(Rule),
    Removable(MountRule),
}

/// A declared place.
///
/// - `id` refers to it elsewhere, as `"place": "@snes"` or `"@snes/RPG"`. Its folder's id is
///   `place:snes`, which stays the same if the path changes.
/// - `path` is like `Games/SNES`; a single part is a tab.
/// - `icon` is one of the UI's icons by name, such as `gamepad`. A folder's icon can also be an image
///   file, shown in its row.
/// - `picture` is a folder's image file, shown large when it is selected. A tab has none, because
///   something in it is always selected.
/// - `recent: false` keeps what is in it out of Recent, for apps. Its folders inherit this unless they
///   set it themselves.
/// - `type` makes it one of ClamShell's own tabs (`recent`, `favourites`, `tools`), named by its path.
///
/// Image file paths starting with `~/` are in the home dir; others are relative to library.json's dir.
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
    /// The entries played last, except those in a place with `recent: false`.
    Recent,
    Favourites,
    /// ClamShell's own tools, such as rescanning the library and fetching what is missing again.
    Tools,
}

/// `place` with a declared place's id (`@snes`, `@snes/RPG`) replaced by its path. Errs if the id isn't
/// declared.
pub fn resolve(place: &str, places: &[Place]) -> Result<String, String> {
    let Some(reference) = place.trim().strip_prefix('@') else {
        return Ok(place.to_owned());
    };
    let (id, rest) = reference.split_once('/').unwrap_or((reference, ""));
    let declared = places.iter().find(|p| p.id == id).ok_or_else(|| format!("{place}: no place {id} declared"))?;
    Ok(if rest.is_empty() { declared.path.clone() } else { format!("{}/{rest}", declared.path.trim_end_matches('/')) })
}

/// A browsed dir. It gets its own folder at `place`, titled by the last part (`Files/Home`,
/// `Files/Places/Root`), and is listed when it is opened. `trash` marks it as the Trash's `files` dir,
/// whose entries can be restored or deleted.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowseRoot {
    pub place: String,
    pub dir: String,
    #[serde(default)]
    pub trash: bool,
}

/// A removable source. The remaining mounted removable filesystems that match all of its `label`, `uuid`
/// and `fs` go at `place` as their own folders, or are left out with `hide`. The fields are globs in any
/// case, and an unset one matches anything. The folder is titled by the last part of `place`, where
/// `{label}` is replaced by the filesystem's label, or its UUID if it has none.
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

/// A desktop or Steam source. The remaining desktop file entries or Steam games that match all of its
/// `id`, `name` and `categories` go at `place`, a tab or a folder in one (`@games/Launchers`), or are left
/// out with `hide`. The fields are globs in any case, and an unset one matches anything. `categories`
/// matches if any of an entry's categories does; Steam games have none.
///
/// `runner`, an id, is the runner that opens them; by default it is the first runner of the type.
/// `metadata` is where their covers and details come from: `steamgriddb`, or `steam` (the Steam store)
/// for a Steam source.
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

/// A dir whose entries go in the library.
///
/// `id` names the entries as `<id>:<path in dir>`, and `place` says where they show. `match` and `items`
/// say what is an entry: `match` is matched against paths in `dir`, one part per folder level
/// (`*/*.cue`), and `items` against a matched folder's own entries. `metadata` says where their covers and
/// details come from. `runner`, an id, opens them all; without it, each opens with the runner its file
/// name matches (see [`super::runners`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scan {
    pub id: String,
    pub dir: String,
    /// Where its entries go: a tab (`Games`) or a folder in one (`Games/SNES`, `@snes`).
    pub place: String,
    #[serde(default, rename = "match")]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub metadata: Option<String>,
    #[serde(default)]
    pub runner: Option<String>,
    /// The removable filesystem `dir` is on: the first mounted one matching this. `dir` is then a path
    /// inside it (`roms/snes`), and the source is away while none is mounted.
    #[serde(default)]
    pub on: Option<On>,
}

/// A removable filesystem, matched by its `label`, `uuid` and `fs`. These are globs in any case, and an
/// unset one matches anything.
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

/// A place's tab and the path of folders in it, which is empty for the tab's top. `Games/Handhelds/GBA`
/// gives Games and `Handhelds/GBA`. Slashes at the ends and spaces around the parts are ignored.
pub fn place(place: &str) -> (&str, &str) {
    let place = place.trim().trim_matches('/');
    match place.split_once('/') {
        Some((tab, path)) => (tab.trim(), path.trim().trim_matches('/')),
        None => (place, ""),
    }
}

/// The config at `file`. It is the default if the file is missing or doesn't parse; in the second case,
/// what is wrong with it is returned too.
pub fn read_config(file: &Path) -> (Config, Option<String>) {
    let Ok(text) = fs::read_to_string(file).inspect_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            log::debug!("library: {}: {e}", file.display());
        } else {
            log::warn!("library: {}: {e}", file.display());
        }
    }) else {
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

/// Scans `source` and appends its entries to `games`, groups before their items. Each file opens as `open`
/// says, and a group with its first entry's runner. Returns the indices of the top-level entries.
pub fn scan(source: &Scan, home: Option<&Path>, games: &mut Vec<Entry>, open: &Open) -> Vec<usize> {
    let dir = match expand_dir(&source.dir, home) {
        Ok(dir) => dir,
        Err(name) => {
            log::warn!("library: {}: ${name} isn't set", source.id);
            return Vec::new();
        }
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

/// `path`'s command and runner as `open` says, or no launch and no runner if no runner opens it.
fn opened(open: &Open, path: &Path) -> (Launch, Option<clamshell_library::RunnerOf>) {
    open(path).map_or((Launch::None, None), |(launch, runner)| (launch, Some(runner)))
}

/// The paths under `dir` that `parts` match, one part per level, in name order. The last part matches a
/// folder if `folders` is set, and a file otherwise.
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
    let mut entries: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .inspect_err(|e| log::debug!("library: {}: {e}", dir.display()))
        .map(|d| d.flatten().filter_map(|e| Some((e.file_name().to_str()?.to_owned(), e.path()))).collect())
        .unwrap_or_default();
    entries.sort_by_cached_key(|(name, _)| (sort::key(name), name.clone()));
    entries
}

/// Adds the group for `folder`, which is in the source's `dir`, to `games` along with its items, and
/// returns its index. Its subfolders with entries come first, then its files matching `items`. None if
/// nothing in it matches.
fn group(folder: &Path, dir: &Path, source: &str, items: &[String], open: &Open, parent: Option<usize>, games: &mut Vec<Entry>) -> Option<usize> {
    let id = |path: &Path| format!("{source}:{}", path.strip_prefix(dir).unwrap_or(path).to_string_lossy());
    let name = folder.file_name()?.to_string_lossy().into_owned();
    let index = games.len();
    let art = art(folder, dir).or_else(|| parent.and_then(|p| games.get(p)).and_then(|g| g.box_front.clone()));
    // A folder's entries take its outermost folder's colour, so a series has one colour while it has no art.
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
        // A video keeps its own picture, its frame, rather than its folder's art.
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
    // Sort the files by title, which starts with the episode number where there is one. They go after the
    // folders.
    files.sort_by(|&a, &b| {
        let key = |i: usize| games.get(i).map(|g| (&g.sort_key, &g.title));
        key(a).cmp(&key(b))
    });
    members.extend(files);
    if members.is_empty() {
        games.truncate(index);
        return None;
    }
    // The group's runner is its first entry's. Its playlist is an `.m3u` in it, enqueued instead of its
    // entries.
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

/// An entry's art. A folder uses its own; a file uses the art beside it, or else its folder's if that
/// folder is inside the source's `dir`.
fn art(path: &Path, dir: &Path) -> Option<PathBuf> {
    picture_of(path).or_else(|| path.parent().filter(|p| *p != dir && p.starts_with(dir) && path.is_file()).and_then(folder_art))
}

/// A file's or folder's own picture, whether scanned or browsed. For a folder, it is a `poster`, `cover`
/// or `folder` image in it. For a file, it is `<name>.<image>` beside it, or else the file itself if it is
/// an image.
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
    // Names match in any case, so `Cover.jpg` counts.
    let files: Vec<(String, PathBuf)> = fs::read_dir(folder)
        .inspect_err(|e| log::debug!("library: {}: {e}", folder.display()))
        .map(|d| d.flatten().filter_map(|e| Some((e.file_name().to_str()?.to_lowercase(), e.path()))).collect())
        .unwrap_or_default();
    FOLDER_ART
        .iter()
        .flat_map(|name| IMAGES.iter().map(move |ext| format!("{name}.{ext}")))
        .find_map(|wanted| files.iter().find(|(n, p)| *n == wanted && p.is_file()).map(|(_, p)| p.clone()))
}

/// `~` and `~/...` against `home`.
pub(crate) fn expand(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        (None, Some(home)) if path == "~" => home.to_path_buf(),
        _ => PathBuf::from(path),
    }
}

/// Expands a dir as library.json gives it: environment variables first (see [`expand_vars`]), then `~/`.
/// Errs with the name of a variable that has no value and no default.
pub(crate) fn expand_dir(dir: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    expand_vars(dir, &|name| std::env::var(name).ok()).map(|d| expand(&d, home))
}

/// Expands `$NAME`, `${NAME}` and `${NAME:-default}` in `text` using `lookup`. The default is used when
/// the variable is unset or empty, and is expanded itself. A `$` followed by anything else stays as it is.
/// Errs with the name of a variable that has no value and no default.
pub(crate) fn expand_vars(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let name_len =
        |s: &str| s.char_indices().find(|&(i, c)| !(c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))).map_or(s.len(), |(i, _)| i);
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(rest.get(..at).unwrap_or(""));
        let after = rest.get(at + 1..).unwrap_or("");
        if let Some(inner) = after.strip_prefix('{') {
            // Find the matching brace, since a default may contain `${...}` itself.
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
        let root = clamshell_core::kit::testdir::TempDir::new("content");
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
        let root = clamshell_core::kit::testdir::TempDir::new("series");
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
        let root = clamshell_core::kit::testdir::TempDir::new("seasons");
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
