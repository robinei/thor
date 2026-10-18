//! Content dirs, as library.json lists them: each source a dir, name patterns for what in it is an entry,
//! the command that starts one (`{path}` in it the entry's file or folder), and the tab it goes in.
//!
//! ```json
//! { "sources": [
//!     { "tab": "Games", "icon": "gamepad", "id": "snes", "dir": "~/Games/snes", "match": ["*.sfc", "*.smc"],
//!       "launch": ["retroarch", "-L", "/usr/lib/libretro/snes9x_libretro.so", "{path}"] },
//!     { "tab": "TV", "icon": "tv", "id": "tv", "dir": "~/TV", "match": ["*/"], "items": ["*.mkv", "*.mp4"],
//!       "launch": ["mpv", "--fs", "{path}"], "metadata": "tmdb:tv" }
//! ],
//!   "keys": { "tmdb": "...", "steamgriddb": "..." } }
//! ```
//!
//! - `match`: patterns (see clamshell-library's `pattern`) against paths in `dir`. An entry per name at the
//!   top of `dir`: a file, or a folder (matched itself, `*/`, or by a file in it, `*/*.cue`); where several
//!   files of a folder match, the first pattern wins, then the first name.
//! - `items`: a matched folder is a group: in it and every folder under it, the files matching these
//!   patterns are entries and the folders with any are groups (a series' seasons, their episodes).
//! - Ids: `<id>:<path in dir>`, so moving the whole dir (a new `dir`) keeps them; renaming or moving
//!   inside it doesn't.
//! - Titles: the name at the top of `dir`, an episode's file name, without the extension (a file's). In a
//!   season's folder (`Season 2`, `S02`, `Series 2`; `Specials` for season 0), a file named for an
//!   episode of that season (`017 S02E18 Title`) is titled by its number there: `18. Title` (`1–2.` for a
//!   double one).
//! - Box art: `<name>.jpg` (or .jpeg, .png, .webp) beside a file, or `poster`, `cover` or `folder` in a
//!   folder (any case: `Cover.jpg`); else the group's, but for a video mpv plays (its frame is its
//!   picture).
//! - `icon`: the tab's icon; sources in the same tab need it once.
//! - `desktop`, `steam`: rules (see [`Rule`]) for the desktop files' apps and games, and for Steam's
//!   installed games: the first one matching an entry (by its id, its name, and a desktop file's
//!   categories: globs, any case) puts it in a tab (`group`: in a folder of it), or hides it; a rule
//!   matching nothing in particular takes the rest. Without one, a desktop file's game (`Categories=Game`,
//!   not `Emulator`) goes in Games, its app in Apps; a Steam game in Games. A desktop file's id is its
//!   name without `.desktop`, a Steam game's its app id. `{ "name": "Battle.net", "tab": "Apps" }`,
//!   `{ "id": "net.lutris.*", "tab": "Games", "group": "Launchers" }`, `{ "id": "*uninstall*", "hide": true }`;
//!   for Steam `{ "name": "Dark Souls*", "group": "Steam/Souls" }`, `{ "group": "Steam" }`.
//! - `browse`: dirs listed as they are opened, not scanned (no metadata, not in Recent), each the folder
//!   `group` (its last part) in `tab`; their files open by the first source whose `match` or `items`
//!   (their last part) fits the name, any case, else `xdg-open`; `trash`: the Trash's `files` dir.
//!   `{ "tab": "Files", "group": "Home", "dir": "~" }`, `{ "tab": "Files", "group": "Trash", "dir":
//!   "${XDG_DATA_HOME:-~/.local/share}/Trash/files", "trash": true }`.
//! - `removable`: rules for the removable filesystems mounted (udisks2's, under `/run/media/$USER`),
//!   browsed as `browse`'s dirs: the first matching one (by `label`, `uuid`, `fs`: globs, any case) puts
//!   one in `tab` as the folder `group` (`{label}` in it its label, else its UUID), or hides it; without
//!   one, Files, `{label}`. `{ "label": "SD*", "tab": "Files", "group": "SD card" }`.
//! - `group`: a folder in the tab for the source's entries (`SNES`, or `Handhelds/GBA` for one in
//!   another), so one tab can hold folders each with its own launch command; sources naming the same
//!   folder share it. The folder's art: `poster`, `cover` or `folder` in the source's dir.
//! - `metadata`: where to fetch covers, screenshots and the rest for its entries (`libretro:<system>`,
//!   `tmdb:movie`, `tmdb:tv`); `keys`: the services' keys (see [`crate::meta`]).
//!
//! `~/` at the start of `dir` or of a launch argument is the home dir; in `dir`, `$VAR`, `${VAR}` and
//! `${VAR:-default}` are environment variables (`${XDG_DATA_HOME:-~/.local/share}/roms`). A source whose dir
//! is missing is skipped, as one naming a variable unset without a default (shown); so is a file that
//! doesn't parse (logged).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use clamshell_library::pattern::{self, hidden};
use clamshell_library::{Game, Launch, sort};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub keys: crate::meta::Keys,
    /// Where the desktop files' apps and games go (the first rule matching one; without one, Games for a
    /// game, Apps for the rest).
    #[serde(default)]
    pub desktop: Vec<Rule>,
    /// Where Steam's installed games go (the first rule matching one; without one, Games).
    #[serde(default)]
    pub steam: Vec<Rule>,
    /// Dirs browsed (listed as they are opened) rather than scanned.
    #[serde(default)]
    pub browse: Vec<BrowseRoot>,
    /// Where mounted removable filesystems go (the first rule matching one; without one, Files, a folder
    /// named by its label).
    #[serde(default)]
    pub removable: Vec<MountRule>,
}

/// A dir browsed: a folder `group` in `tab` (its last part the dir's own folder: `Home`, `Places/Root`),
/// listed when it is opened; `trash`: it is the Trash's `files` dir (its entries restored, deleted).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowseRoot {
    pub tab: String,
    pub group: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub dir: String,
    #[serde(default)]
    pub trash: bool,
}

/// A rule for mounted removable filesystems: those matching all of its `label`, `uuid` and `fs` (globs,
/// any case) go in `tab` (with `icon`) as the folder `group` (`{label}` in it their label, or their UUID
/// without one), or are left out (`hide`).
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
    pub tab: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub hide: bool,
}

impl MountRule {
    /// Whether it matches a filesystem with this label, UUID and type.
    pub fn matches(&self, label: &str, uuid: &str, fs: &str) -> bool {
        let glob = |pattern: &str, value: &str| pattern::matches(&pattern.to_lowercase(), &value.to_lowercase());
        self.label.as_deref().is_none_or(|p| glob(p, label))
            && self.uuid.as_deref().is_none_or(|p| glob(p, uuid))
            && self.fs.as_deref().is_none_or(|p| glob(p, fs))
    }
}

/// A rule for desktop files' entries or Steam's games: those matching all of its `id`, `name` and
/// `categories` (globs, any case; `categories` matches if any of an entry's does; Steam's games have
/// none) go in `tab` (with `icon`; in its folder `group`, if given), or are left out (`hide`).
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
    pub tab: Option<String>,
    /// A folder in the tab (as a source's `group`: `Launchers`, `Tools/Wine`).
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub hide: bool,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub tab: String,
    /// The folder in the tab its entries go in (`SNES`; `Handhelds/GBA` one in another); none: the tab
    /// itself.
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub id: String,
    pub dir: String,
    #[serde(rename = "match")]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub items: Vec<String>,
    pub launch: Vec<String>,
    #[serde(default)]
    pub metadata: Option<String>,
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

/// A source's entries: appended to `games` (groups before their items), the indices of its top-level
/// ones returned.
pub fn scan(source: &Source, home: Option<&Path>, games: &mut Vec<Game>) -> Vec<usize> {
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
    let launch = Launcher { args: source.launch.iter().map(|a| expand(a, home).to_string_lossy().into_owned()).collect() };
    let mut tops = Vec::new();
    for (top, path) in found {
        let title = untagged(&if path.is_file() && path.parent() == Some(dir.as_path()) { stem(&top) } else { top.clone() });
        let id = format!("{}:{}", source.id, path.strip_prefix(&dir).unwrap_or(&path).to_string_lossy());
        let art = art(&path, &dir);
        let index = if path.is_dir() && !source.items.is_empty() {
            match group(&path, &dir, &source.id, &source.items, &launch, None, games) {
                Some(g) => g,
                None => continue,
            }
        } else {
            games.push(Game { launch: launch.of(&path), file: Some(path.clone()), ..Game::new(&id, &title) });
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

/// The launch command with `{path}` to fill in.
struct Launcher {
    args: Vec<String>,
}

impl Launcher {
    /// The command for `path`, in its folder.
    fn of(&self, path: &Path) -> Launch {
        let p = path.to_string_lossy();
        let argv = self.args.iter().map(|a| a.replace("{path}", &p)).collect();
        Launch::Program { argv, workdir: path.parent().map(Path::to_path_buf) }
    }
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
fn group(folder: &Path, dir: &Path, source: &str, items: &[String], launch: &Launcher, parent: Option<usize>, games: &mut Vec<Game>) -> Option<usize> {
    let id = |path: &Path| format!("{source}:{}", path.strip_prefix(dir).unwrap_or(path).to_string_lossy());
    let name = folder.file_name()?.to_string_lossy().into_owned();
    let index = games.len();
    let art = art(folder, dir).or_else(|| parent.and_then(|p| games.get(p)).and_then(|g| g.box_front.clone()));
    // (A folder's entries in its outermost folder's colour: one colour for a series while it has no art.)
    let mut own = Game { parent, box_front: art.clone(), ..Game::new(&id(folder), &name) };
    if let Some(p) = parent.and_then(|p| games.get(p)) {
        own.color = p.color;
    }
    let color = own.color;
    games.push(own);
    let listing = listing(folder);
    let season = season_of(&name);
    let mut members = Vec::new();
    for (_, path) in listing.iter().filter(|(n, p)| p.is_dir() && !n.starts_with('.')) {
        if let Some(g) = group(path, dir, source, items, launch, Some(index), games) {
            members.push(g);
        }
    }
    let mut files = Vec::new();
    for (file, path) in listing.iter().filter(|(_, p)| p.is_file()) {
        if !items.iter().any(|p| !hidden(file, p) && pattern::matches(p, file)) {
            continue;
        }
        // (A video keeps its own picture, its frame: not its folder's art.)
        let video = Game { launch: launch.of(path), file: Some(path.clone()), ..Game::new("", "") }.mpv_file().is_some();
        let box_front = art_beside(path).or_else(|| art.clone().filter(|_| !video));
        games.push(Game {
            parent: Some(index),
            box_front,
            color,
            launch: launch.of(path),
            file: Some(path.clone()),
            ..Game::new(&id(path), &season.and_then(|n| episode_title(&stem(file), n)).unwrap_or_else(|| untagged(&stem(file))))
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
    if let Some(g) = games.get_mut(index) {
        g.items = members;
    }
    Some(index)
}

/// Image extensions looked for, in order.
const IMAGES: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
/// A folder's own art, by name.
const FOLDER_ART: [&str; 3] = ["poster", "cover", "folder"];

/// An entry's art: a folder's own; a file's beside it, else its folder's (one inside the source's `dir`).
fn art(path: &Path, dir: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        return folder_art(path);
    }
    art_beside(path).or_else(|| path.parent().filter(|p| *p != dir && p.starts_with(dir)).and_then(folder_art))
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

    fn source(json: &str) -> Source {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn files_and_folders_at_the_top_first_pattern_first() {
        let root = std::env::temp_dir().join(format!("clamshell-content-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        touch(
            &root,
            &["Zelda.sfc", "Zelda.png", "Mario.smc", "notes.txt", ".hidden.sfc", "FF7/FF7.m3u", "FF7/FF7 (Disc 1).cue", "FF7/cover.jpg", "Empty/readme"],
        );
        let s = source(r#"{ "tab": "Games", "id": "g", "dir": "~/x", "match": ["*.sfc", "*.smc", "*/*.m3u", "*/*.cue"], "launch": ["emu", "{path}"] }"#);
        let mut games = Vec::new();
        let s = Source { dir: root.to_string_lossy().into_owned(), ..s };
        let tops = scan(&s, None, &mut games);
        let found: Vec<(&str, &str)> = tops.iter().map(|&i| (&*games[i].id, games[i].title.as_str())).collect();
        assert_eq!(found, vec![("g:Zelda.sfc", "Zelda"), ("g:Mario.smc", "Mario"), ("g:FF7/FF7.m3u", "FF7")]);
        assert_eq!(games[0].box_front, Some(root.join("Zelda.png")));
        assert_eq!(games[2].box_front, Some(root.join("FF7/cover.jpg")), "its folder's art");
        let path = root.join("FF7/FF7.m3u");
        assert_eq!(games[2].launch, Launch::Program { argv: vec!["emu".into(), path.to_string_lossy().into()], workdir: Some(root.join("FF7")) });
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_with_items_is_a_group_of_groups() {
        let root = std::env::temp_dir().join(format!("clamshell-series-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
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
        let s = source(r#"{ "tab": "TV", "id": "tv", "dir": "", "match": ["*/"], "items": ["*.mkv"], "launch": ["mpv", "{path}"] }"#);
        let s = Source { dir: root.to_string_lossy().into_owned(), ..s };
        let mut games = Vec::new();
        let tops = scan(&s, None, &mut games);
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
        let _ = fs::remove_dir_all(&root);
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
        let root = std::env::temp_dir().join(format!("clamshell-seasons-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        touch(&root, &["Show/Cover.jpg", "Show/Season 1/017 S01E18 Part (Part 1).mkv", "Show/Season 1/S01E02 Two.mkv", "Show/Specials/001 S00E01 Pilot.mkv"]);
        let s = Source {
            dir: root.to_string_lossy().into_owned(),
            ..source(r#"{ "tab": "TV", "id": "tv", "dir": "", "match": ["*/"], "items": ["*.mkv"], "launch": ["mpv", "{path}"] }"#)
        };
        let mut games = Vec::new();
        let tops = scan(&s, None, &mut games);
        let show = &games[tops[0]];
        assert_eq!(show.box_front, Some(root.join("Show/Cover.jpg")), "Cover.jpg");
        let titles = |ids: &[usize]| ids.iter().map(|&i| games[i].title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&show.items), ["Season 1", "Specials"]);
        assert_eq!(titles(&games[show.items[0]].items), ["2. Two", "18. Part (Part 1)"], "by number");
        assert_eq!(titles(&games[show.items[1]].items), ["1. Pilot"]);
        let _ = fs::remove_dir_all(&root);
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
        fs::write(&file, r#"{ "sources": [ { "tab": "Games" } ] }"#).unwrap();
        let (config, problem) = read_config(&file);
        assert!(config.sources.is_empty());
        assert!(problem.is_some_and(|p| p.starts_with("library.json: missing field")), "said");
        let _ = fs::remove_file(&file);
        assert_eq!(expand("~/Games", Some(Path::new("/h"))), PathBuf::from("/h/Games"));
        assert_eq!(expand("~", Some(Path::new("/h"))), PathBuf::from("/h"));
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
