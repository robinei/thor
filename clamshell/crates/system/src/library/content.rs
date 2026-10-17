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
//! - Titles: the name at the top of `dir`, an episode's file name, without the extension (a file's).
//! - Box art: `<name>.jpg` (or .jpeg, .png, .webp) beside a file, or `poster`, `cover` or `folder` in a
//!   folder; else the group's.
//! - `icon`: the tab's icon; sources in the same tab need it once.
//! - `metadata`: where to fetch covers, screenshots and the rest for its entries (`libretro:<system>`,
//!   `tmdb:movie`, `tmdb:tv`); `keys`: the services' keys (see [`crate::meta`]).
//!
//! `~/` at the start of `dir` or of a launch argument is the home dir. A source whose dir is missing is
//! skipped; so is a file that doesn't parse (logged).

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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub tab: String,
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

/// The config at `file`; none if it is missing or doesn't parse (logged).
pub fn read_config(file: &Path) -> Config {
    let Ok(text) = fs::read_to_string(file) else {
        return Config::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        log::warn!("library: {}: {e}", file.display());
        Config::default()
    })
}

/// A source's entries: appended to `games` (groups before their items), the indices of its top-level
/// ones returned.
pub fn scan(source: &Source, home: Option<&Path>, games: &mut Vec<Game>) -> Vec<usize> {
    let dir = expand(&source.dir, home);
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
        let title = if path.is_file() && path.parent() == Some(dir.as_path()) { stem(&top) } else { top.clone() };
        let id = format!("{}:{}", source.id, path.strip_prefix(&dir).unwrap_or(&path).to_string_lossy());
        let art = art(&path, &dir);
        let index = if path.is_dir() && !source.items.is_empty() {
            match group(&path, &dir, &source.id, &source.items, &launch, None, games) {
                Some(g) => g,
                None => continue,
            }
        } else {
            games.push(Game { launch: launch.of(&path), ..Game::new(&id, &title) });
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
    games.push(Game { parent, box_front: art.clone(), ..Game::new(&id(folder), &name) });
    let listing = listing(folder);
    let mut members = Vec::new();
    for (_, path) in listing.iter().filter(|(n, p)| p.is_dir() && !n.starts_with('.')) {
        if let Some(g) = group(path, dir, source, items, launch, Some(index), games) {
            members.push(g);
        }
    }
    for (file, path) in listing.iter().filter(|(_, p)| p.is_file()) {
        if !items.iter().any(|p| !hidden(file, p) && pattern::matches(p, file)) {
            continue;
        }
        let box_front = art_beside(path).or_else(|| art.clone());
        games.push(Game { parent: Some(index), box_front, launch: launch.of(path), ..Game::new(&id(path), &stem(file)) });
        members.push(games.len() - 1);
    }
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

fn folder_art(folder: &Path) -> Option<PathBuf> {
    FOLDER_ART.iter().flat_map(|name| IMAGES.iter().map(move |ext| folder.join(format!("{name}.{ext}")))).find(|p| p.is_file())
}

/// A file name without its extension.
fn stem(name: &str) -> String {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem).to_owned()
}

/// `~/...` against `home`.
fn expand(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
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
        assert_eq!(episode.box_front, Some(root.join("Show/poster.jpg")), "the series' art");
        assert_eq!(episode.parent, Some(show.items[0]));
        assert!(games[tops[0]].is_group(), "a film's folder too");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_config_that_doesnt_parse_is_none() {
        let file = std::env::temp_dir().join(format!("clamshell-library-{}.json", std::process::id()));
        fs::write(&file, r#"{ "sources": [ { "tab": "Games" } ] }"#).unwrap();
        assert!(read_config(&file).sources.is_empty());
        let _ = fs::remove_file(&file);
        assert_eq!(expand("~/Games", Some(Path::new("/h"))), PathBuf::from("/h/Games"));
    }
}
