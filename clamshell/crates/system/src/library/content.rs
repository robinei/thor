//! library.json: the declared places, the runners that open things (see [`super::runners`]), the sources
//! that say where entries come from and where they go, and the keys for metadata services. ClamShell adds
//! nothing of its own: a tab, runner or source that is left out of the file doesn't exist.
//!
//! ```json
//! { "places": [
//!     { "id": "recent", "name": "Recent", "type": "recent", "icon": "clock" },
//!     { "id": "games", "name": "Games", "icon": "gamepad" },
//!     { "id": "snes", "name": "SNES", "in": "games", "icon": "snes-icon.png", "picture": "snes.png" },
//!     { "id": "apps", "name": "Apps", "icon": "apps", "recent": false },
//!     { "id": "files", "name": "Files", "icon": "folder" },
//!     { "id": "tools", "name": "Tools", "type": "tools", "icon": "tools" }
//!   ],
//!   "runners": [
//!     { "type": "desktop", "id": "desktop", "name": "Desktop", "match": ["*.desktop"] },
//!     { "id": "snes", "name": "SNES", "match": ["*.sfc", "*.smc"],
//!       "launch": ["retroarch", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{path}"] },
//!     { "id": "other", "name": "Other files", "match": ["*"], "launch": ["xdg-open", "{path}"] }
//!   ],
//!   "sources": [
//!     { "type": "dir", "id": "snes", "dir": "~/Roms/snes", "place": "snes", "runner": "snes",
//!       "also": ["*.zip"], "metadata": "libretro:..." },
//!     { "type": "desktop", "categories": "Game", "place": "games" },
//!     { "type": "desktop", "place": "apps" },
//!     { "type": "browse", "dir": "~", "in": "files", "name": "Home" },
//!     { "type": "removable", "in": "files", "name": "{label}" },
//!     { "type": "builtin", "id": "device", "place": "tools" }
//!   ],
//!   "keys": { "tmdb": "...", "steamgriddb": "..." } }
//! ```
//!
//! `places` (see [`Place`]) are the tabs and the folders in them, each by its `id`:
//! - `name` is its title. `in` is the id of the place it is in; without it, it is a tab. Renaming changes
//!   only `name` and moving only `in`, so what is kept by the id (a folder's pins and favourites, as
//!   `place:<id>`, and sources' places) stays.
//! - `icon` is one of the UI's icons by name. A folder's icon can also be an image file, shown in its row.
//! - `picture` is a folder's image, shown large when the folder is selected.
//! - `recent: false` keeps what is in the place out of Recent, for apps.
//! - `type` makes it one of ClamShell's own tabs (`recent`, `favourites`, `tools`).
//!
//! The tabs come in the order they are declared.
//!
//! `sources` (see [`Source`]), by `type`, each naming the place its entries go to by its id:
//! - `dir` (see [`Scan`]) is a dir that is scanned.
//!   - `runner`, an id, opens its entries. Without it, each file opens with the runner whose patterns its
//!     name matches, and a group opens as its first entry.
//!   - `match` is a list of patterns (see clamshell-library's `pattern`) matched against paths in `dir`;
//!     without it, the runner's are used. `also` adds patterns to either, such as `*.zip`, which no runner
//!     opens by its name. Each name at the top of `dir` gives one entry: a file, or a folder matched itself
//!     (`*/`) or through a file in it (`*/*.cue`). Where several files in a folder match, the first
//!     pattern wins, then the first name.
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
//!   and isn't in Recent. It is a folder of its own, `name`, in the place `in`. `trash` marks it as the
//!   Trash's `files` dir.
//! - `desktop` and `steam` take the apps and games from desktop files and Steam's installed games, which
//!   are opened by a runner of that type. The sources of each type take entries in order: each takes the
//!   remaining ones it matches (see [`Rule`]) and puts them at its `place`, or leaves them out with
//!   `hide`. Matching is by id, name and a desktop file's categories, as globs in any case; a rule with
//!   none of them matches everything. Entries no source takes are left out. `runner` is the runner of the
//!   type that opens them, by default the first. `metadata` is `steamgriddb`, or `steam` for a Steam
//!   source. A desktop file's id is its name without `.desktop`, and a Steam game's is its app id.
//!   Examples, in order:
//!   `{ "type": "desktop", "id": "net.lutris.*", "place": "apps" }`,
//!   `{ "type": "desktop", "categories": "Game", "place": "games" }`,
//!   `{ "type": "desktop", "id": "*uninstall*", "hide": true }`,
//!   `{ "type": "desktop", "place": "apps" }` for the rest.
//! - `removable` takes the mounted removable filesystems (udisks2's, under `/run/media/$USER`). They are
//!   browsed like `browse` dirs and taken in the same way as `desktop` entries, matched by `label`, `uuid`
//!   and `fs` (see [`MountRule`]). Each is a folder of its own in the place `in`, titled by `name`, where
//!   `{label}` is the filesystem's label, or its UUID if it has none (the default name).
//! - `builtin` places one of ClamShell's own screens, by its `id` (`library`, `device`). One not placed
//!   is still there (the Library is always on the dock), in no place.
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

pub(crate) use clamshell_library::config::{BrowseRoot, BuiltinPlace, Config, MountRule, On, OwnTab, Place, Rule, Scan, Source};

/// A declared place with its path: its tab and the folders in it to it, as `Games/SNES`, worked out from
/// its `name` and the places it is `in` (see [`placed`]).
#[derive(Debug, Clone)]
pub(crate) struct Placed {
    pub(crate) place: Place,
    pub(crate) path: String,
}

impl std::ops::Deref for Placed {
    type Target = Place;

    fn deref(&self) -> &Place {
        &self.place
    }
}

/// The declared places with their paths, worked out from each one's `name` and the places it is in. A
/// place in one that isn't declared, or in itself through others, is a tab; each is a problem returned.
pub(crate) fn placed(places: &[Place]) -> (Vec<Placed>, Vec<String>) {
    let mut problems = Vec::new();
    let mut out = Vec::with_capacity(places.len());
    for place in places {
        let mut names = vec![place.name.trim()];
        let mut within = place.within.as_deref();
        while let Some(id) = within {
            match places.iter().find(|p| p.id == id) {
                Some(_) if names.len() > places.len() => {
                    problems.push(format!("place {}: in itself, through the places it is in", place.id));
                    names.truncate(1);
                    break;
                }
                Some(p) => {
                    names.push(p.name.trim());
                    within = p.within.as_deref();
                }
                None => {
                    problems.push(format!("place {}: in {id}, which isn't declared", place.id));
                    names.truncate(1);
                    break;
                }
            }
        }
        names.reverse();
        out.push(Placed { place: place.clone(), path: names.join("/") });
    }
    (out, problems)
}

/// The path of the place declared as `id` (see [`placed`]). Errs if there is none.
pub(crate) fn resolve(id: &str, places: &[Placed]) -> Result<String, String> {
    places.iter().find(|p| p.id == id.trim()).map(|p| p.path.clone()).ok_or_else(|| format!("no place {id} declared"))
}

/// Whether a desktop or Steam source's `rule` matches an entry with this id, name and categories.
pub(crate) fn rule_matches(rule: &Rule, id: &str, name: &str, categories: &[String]) -> bool {
    let glob = |pattern: &str, value: &str| pattern::matches_ignoring_case(pattern, value);
    rule.id.as_deref().is_none_or(|p| glob(p, id))
        && rule.name.as_deref().is_none_or(|p| glob(p, name))
        && rule.categories.as_deref().is_none_or(|p| categories.iter().any(|c| glob(p, c)))
}

/// Whether a removable source's `rule` matches a filesystem with this label, UUID and type.
pub(crate) fn mount_matches(rule: &MountRule, label: &str, uuid: &str, fs: &str) -> bool {
    on_matches(&On { label: rule.label.clone(), uuid: rule.uuid.clone(), fs: rule.fs.clone() }, label, uuid, fs)
}

/// Whether `on` matches a filesystem with this label, UUID and type.
pub(crate) fn on_matches(on: &On, label: &str, uuid: &str, fs: &str) -> bool {
    let glob = |pattern: &str, value: &str| pattern::matches_ignoring_case(pattern, value);
    on.label.as_deref().is_none_or(|p| glob(p, label)) && on.uuid.as_deref().is_none_or(|p| glob(p, uuid)) && on.fs.as_deref().is_none_or(|p| glob(p, fs))
}

/// How a scanned file opens: its command and its runner, if a runner opens it.
type Open<'a> = dyn Fn(&Path) -> Option<(Launch, clamshell_library::RunnerOf)> + 'a;

/// A place's tab and the path of folders in it, which is empty for the tab's top. `Games/Handhelds/GBA`
/// gives Games and `Handhelds/GBA`. Slashes at the ends and spaces around the parts are ignored.
pub(crate) fn place(place: &str) -> (&str, &str) {
    let place = place.trim().trim_matches('/');
    match place.split_once('/') {
        Some((tab, path)) => (tab.trim(), path.trim().trim_matches('/')),
        None => (place, ""),
    }
}

/// The config at `file`. It is the default if the file is missing or doesn't parse; in the second case,
/// what is wrong with it is returned too.
pub(crate) fn read_config(file: &Path) -> (Config, Option<String>) {
    let Ok(text) = fs::read_to_string(file).inspect_err(|e| {
        crate::os::log_unread("library", file, e);
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

/// Writes `config` to `file`, one place, runner or source a line as the starter is written, through a
/// file beside it moved over it.
pub(crate) fn write_config(file: &Path, config: &Config) -> std::io::Result<()> {
    fs::write(file.with_extension("json.new"), config_text(config)?)?;
    fs::rename(file.with_extension("json.new"), file)
}

/// `config` as library.json's text: one place, runner or source a line.
fn config_text(config: &Config) -> std::io::Result<String> {
    let line = |v: &dyn erased::Line| v.line();
    let list = |name: &str, items: Vec<String>| {
        let lines: Vec<String> = items.iter().map(|i| format!("    {i}")).collect();
        format!("  \"{name}\": [\n{}\n  ]", lines.join(",\n"))
    };
    let places = config.places.iter().map(|p| line(p)).collect::<Result<Vec<_>, _>>()?;
    let runners = config.runners.iter().map(|r| line(r)).collect::<Result<Vec<_>, _>>()?;
    let sources = config.sources.iter().map(|s| line(s)).collect::<Result<Vec<_>, _>>()?;
    let keys = line(&config.keys)?;
    Ok(format!("{{\n{},\n{},\n{},\n  \"keys\": {keys}\n}}\n", list("places", places), list("runners", runners), list("sources", sources)))
}

mod erased {
    /// A value as one line of JSON, with spaces inside its braces.
    pub(super) trait Line {
        fn line(&self) -> std::io::Result<String>;
    }

    impl<T: serde::Serialize> Line for T {
        fn line(&self) -> std::io::Result<String> {
            let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
            Ok(spaced(&text))
        }
    }

    /// `{"a":1,"b":[2,3]}` as `{ "a": 1, "b": [2, 3] }`, leaving strings as they are.
    fn spaced(json: &str) -> String {
        let mut out = String::with_capacity(json.len() * 5 / 4);
        let (mut in_string, mut escaped) = (false, false);
        let mut chars = json.chars().peekable();
        while let Some(c) = chars.next() {
            if in_string {
                out.push(c);
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => in_string = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => {
                    in_string = true;
                    out.push(c);
                }
                '{' if chars.peek() == Some(&'}') => out.push('{'),
                '{' => out.push_str("{ "),
                '}' if out.ends_with('{') => out.push('}'),
                '}' => out.push_str(" }"),
                ':' => out.push_str(": "),
                ',' => out.push_str(", "),
                c => out.push(c),
            }
        }
        out
    }
}

/// Scans `source` and appends its entries to `games`, groups before their items. Its dir is a path as is,
/// already expanded by the caller. Each file opens as `open` says, and a group with its first entry's
/// runner. Returns the indices of the top-level entries.
pub(crate) fn scan(source: &Scan, games: &mut Vec<Entry>, open: &Open) -> Vec<usize> {
    let dir = PathBuf::from(&source.dir);
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
    // A symlinked folder isn't followed: a link to `.` or `..` would scan without end.
    let folder_in = |p: &Path| p.symlink_metadata().is_ok_and(|m| m.is_dir());
    for (_, path) in listing.iter().filter(|(n, p)| folder_in(p) && !n.starts_with('.')) {
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
pub(crate) fn picture_of(path: &Path) -> Option<PathBuf> {
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
fn expand_vars(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
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
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(
            root,
            &["Zelda.sfc", "Zelda.png", "Mario.smc", "notes.txt", ".hidden.sfc", "FF7/FF7.m3u", "FF7/FF7 (Disc 1).cue", "FF7/cover.jpg", "Empty/readme"],
        );
        let s = source(r#"{ "place": "Games", "id": "g", "dir": "~/x", "match": ["*.sfc", "*.smc", "*/*.m3u", "*/*.cue"] }"#);
        let mut games = Vec::new();
        let s = Scan { dir: root.to_string_lossy().into_owned(), ..s };
        let tops = scan(&s, &mut games, &by("emu"));
        let found: Vec<(&str, &str)> = tops.iter().map(|&i| (&*games[i].id, games[i].title.as_str())).collect();
        assert_eq!(found, vec![("g:Zelda.sfc", "Zelda"), ("g:Mario.smc", "Mario"), ("g:FF7/FF7.m3u", "FF7")]);
        assert_eq!(games[0].box_front, Some(root.join("Zelda.png")));
        assert_eq!(games[2].box_front, Some(root.join("FF7/cover.jpg")), "its folder's art");
        let path = root.join("FF7/FF7.m3u");
        assert_eq!(games[2].launch, Launch::Program { argv: vec!["emu".into(), path.to_string_lossy().into()], workdir: Some(root.join("FF7")) });
    }

    #[test]
    fn a_dir_with_a_dollar_sign_is_scanned_as_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(root, &["$NOT_SET/Zelda.sfc"]);
        let s = source(r#"{ "place": "Games", "id": "g", "dir": "~/x", "match": ["*.sfc"] }"#);
        let s = Scan { dir: root.join("$NOT_SET").to_string_lossy().into_owned(), ..s };
        let mut games = Vec::new();
        assert_eq!(scan(&s, &mut games, &by("emu")).len(), 1);
    }

    #[test]
    fn a_folder_with_items_is_a_group_of_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(
            root,
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
        let tops = scan(&s, &mut games, &by("mpv"));
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

    /// A symlinked folder in a series isn't followed: a link to its own folder would scan without end.
    #[test]
    fn symlinked_folders_in_a_series_arent_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(root, &["Show/Season 1/1. One.mkv"]);
        std::os::unix::fs::symlink(".", root.join("Show/Season 1/again")).unwrap();
        std::os::unix::fs::symlink("..", root.join("Show/up")).unwrap();
        let s = source(r#"{ "place": "TV", "id": "tv", "dir": "", "match": ["*/"], "items": ["*.mkv"] }"#);
        let s = Scan { dir: root.to_string_lossy().into_owned(), ..s };
        let mut games = Vec::new();
        let tops = scan(&s, &mut games, &by("mpv"));
        assert_eq!(games[tops[0]].items.len(), 1, "the season alone");
        assert_eq!(games.len(), 3, "the show, its season and the episode");
    }

    #[test]
    fn episodes_in_their_seasons_folder_are_titled_by_their_number() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(root, &["Show/Cover.jpg", "Show/Season 1/017 S01E18 Part (Part 1).mkv", "Show/Season 1/S01E02 Two.mkv", "Show/Specials/001 S00E01 Pilot.mkv"]);
        let s = Scan { dir: root.to_string_lossy().into_owned(), ..source(r#"{ "place": "TV", "id": "tv", "dir": "", "match": ["*/"], "items": ["*.mkv"] }"#) };
        let mut games = Vec::new();
        let tops = scan(&s, &mut games, &by("mpv"));
        let show = &games[tops[0]];
        assert_eq!(show.box_front, Some(root.join("Show/Cover.jpg")), "Cover.jpg");
        let titles = |ids: &[usize]| ids.iter().map(|&i| games[i].title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&show.items), ["Season 1", "Specials"]);
        assert_eq!(titles(&games[show.items[0]].items), ["2. Two", "18. Part (Part 1)"], "by number");
        assert_eq!(titles(&games[show.items[1]].items), ["1. Pilot"]);
    }

    /// The starter library.json, read and written back, is the same file, and reads the same.
    #[test]
    fn the_starter_is_written_as_it_is_read() {
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packages/thor-session/root/usr/share/thor/home/.config/clamshell/library.json");
        let (config, problem) = read_config(&starter);
        assert_eq!(problem, None);
        let text = config_text(&config).unwrap();
        assert_eq!(text, fs::read_to_string(&starter).unwrap());
        let reread: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(reread, config);
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
    fn places_are_named_in_the_places_they_are_in() {
        let places: Vec<Place> = serde_json::from_str(
            r#"[{ "id": "games", "name": "Games" }, { "id": "rpg", "name": "RPG", "in": "snes" }, { "id": "snes", "name": "SNES", "in": "games" },
                { "id": "lost", "name": "Lost", "in": "nowhere" }, { "id": "a", "name": "A", "in": "b" }, { "id": "b", "name": "B", "in": "a" }]"#,
        )
        .unwrap();
        let (places, problems) = placed(&places);
        assert_eq!(resolve("snes", &places).unwrap(), "Games/SNES");
        assert_eq!(resolve("rpg", &places).unwrap(), "Games/SNES/RPG", "declared before what it is in");
        assert_eq!(resolve("lost", &places).unwrap(), "Lost", "in one not declared: a tab");
        assert!(resolve("gba", &places).is_err());
        assert_eq!(problems.len(), 3, "{problems:?}");
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
