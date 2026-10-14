//! Pegasus Frontend's library format, read the way Pegasus reads it (docs/pegasus-format.md in the GPD
//! repo): `game_dirs.txt`, the metadata files at the top of each game dir (collections, `game:` entries,
//! launch commands, details, `assets.*`), files found by a collection's extensions, and box art in
//! `media/<file name>/boxFront.*`. Missing files are skipped, as Pegasus does with `verify-files`.
//!
//! Not implemented (unused in this setup, see the spec): `directories:`, collection `files:`, regexes,
//! `ignore-*`, global `metafiles/`, URI files, assets other than box art, `rating`.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::metafile::{self, Entry};
use crate::{Collection, Game, Launch};

/// The dirs listed in `game_dirs.txt` under `config_dir` that exist (lines starting with `#` skipped,
/// duplicates dropped).
pub fn read_game_dirs(config_dir: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(config_dir.join("game_dirs.txt")) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let dir = clean(&absolute(Path::new(line)));
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) }
}

/// The path with `.` and `..` resolved lexically (no symlinks), as Qt's `cleanPath`.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn is_metafile(name: &str) -> bool {
    matches!(name, "metadata.pegasus.txt" | "metadata.txt") || name.ends_with(".metadata.pegasus.txt") || name.ends_with(".metadata.txt")
}

fn hidden(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

/// Everything before the last dot (Qt's completeBaseName).
fn base_name(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name.rfind('.') {
        Some(i) if i > 0 => name.get(..i).unwrap_or(name).to_owned(),
        _ => name.to_owned(),
    }
}

/// A file's name as a title: base name with `_` and `.` as spaces.
fn pretty_name(path: &Path) -> String {
    base_name(path).replace(['_', '.'], " ")
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

#[derive(Debug, Default)]
struct RawCollection {
    name: String,
    short_name: Option<String>,
    sort_by: Option<String>,
    summary: String,
    launch: String,
    workdir: String,
    basedir: PathBuf,
    tools: bool,
    /// One per `collection:` line: its dir and extensions.
    filters: Vec<(PathBuf, Vec<String>)>,
    games: Vec<usize>,
}

#[derive(Debug, Default)]
struct RawGame {
    title: String,
    sort_by: Option<String>,
    files: Vec<PathBuf>,
    launch: String,
    workdir: String,
    basedir: PathBuf,
    developers: Vec<String>,
    publishers: Vec<String>,
    genres: Vec<String>,
    players: u8,
    release: Option<(u16, u8, u8)>,
    summary: String,
    description: String,
    box_front: Vec<PathBuf>,
    tool: bool,
    collections: Vec<usize>,
}

#[derive(Default)]
struct Scan {
    collections: Vec<RawCollection>,
    by_name: HashMap<String, usize>,
    games: Vec<RawGame>,
    owner: HashMap<PathBuf, usize>,
}

impl Scan {
    fn collection(&mut self, name: &str) -> usize {
        if let Some(&i) = self.by_name.get(name) {
            return i;
        }
        self.collections.push(RawCollection { name: name.to_owned(), ..RawCollection::default() });
        let i = self.collections.len() - 1;
        self.by_name.insert(name.to_owned(), i);
        i
    }

    /// Adds a game to a collection, taking its launch settings where the game has none.
    fn add_to(&mut self, game: usize, coll: usize) {
        let (Some(g), Some(c)) = (self.games.get_mut(game), self.collections.get_mut(coll)) else {
            return;
        };
        if g.collections.contains(&coll) {
            return;
        }
        g.collections.push(coll);
        c.games.push(game);
        if g.launch.is_empty() {
            g.launch.clone_from(&c.launch);
        }
        if g.workdir.is_empty() {
            g.workdir.clone_from(&c.workdir);
        }
        if g.basedir.as_os_str().is_empty() {
            g.basedir.clone_from(&c.basedir);
        }
    }

    fn file(&self, path: PathBuf) -> Option<PathBuf> {
        let path = clean(&path);
        path.exists().then_some(path)
    }

    fn read_metafile(&mut self, path: &Path) {
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut cur_coll: Option<usize> = None;
        let mut cur_game: Option<usize> = None;
        let mut file_colls: Vec<usize> = Vec::new();
        for entry in metafile::parse(&text) {
            match entry.key.as_str() {
                "collection" => {
                    let c = self.collection(entry.first());
                    if let Some(coll) = self.collections.get_mut(c) {
                        coll.basedir.clone_from(&dir);
                        coll.filters.push((dir.clone(), Vec::new()));
                    }
                    cur_coll = Some(c);
                    cur_game = None;
                    file_colls.push(c);
                }
                "game" => {
                    let title = entry.first().to_owned();
                    self.games.push(RawGame { title, basedir: dir.clone(), players: 1, ..RawGame::default() });
                    let g = self.games.len() - 1;
                    for &c in &file_colls {
                        self.add_to(g, c);
                    }
                    cur_game = Some(g);
                }
                key => match (cur_game, cur_coll) {
                    (Some(g), _) => self.game_key(g, key, &entry, &dir),
                    (None, Some(c)) => self.collection_key(c, key, &entry, &dir),
                    (None, None) => {}
                },
            }
        }
    }

    fn game_key(&mut self, g: usize, key: &str, entry: &Entry, dir: &Path) {
        let files: Vec<PathBuf> = match key {
            "file" | "files" => entry.items().filter_map(|f| self.file(dir.join(f))).collect(),
            _ => Vec::new(),
        };
        let box_front = (is_box_front_key(key)).then(|| entry.items().filter_map(|f| self.file(dir.join(f))).collect::<Vec<_>>());
        for f in files {
            if self.owner.contains_key(&f) {
                continue;
            }
            self.owner.insert(f.clone(), g);
            if let Some(game) = self.games.get_mut(g) {
                game.files.push(f);
            }
        }
        let Some(game) = self.games.get_mut(g) else {
            return;
        };
        if let Some(art) = box_front {
            game.box_front.extend(art);
            return;
        }
        match key {
            "launch" | "command" => game.launch = entry.merged(),
            "workdir" | "cwd" => game.workdir = entry.first().to_owned(),
            "developer" | "developers" => game.developers.extend(entry.items().map(str::to_owned)),
            "publisher" | "publishers" => game.publishers.extend(entry.items().map(str::to_owned)),
            "genre" | "genres" => game.genres.extend(entry.items().map(str::to_owned)),
            "players" => {
                if let Some(n) = parse_players(entry.first()) {
                    game.players = n;
                }
            }
            "release" => game.release = parse_release(entry.first()).or(game.release),
            "summary" => game.summary = entry.text(),
            "description" => game.description = entry.text(),
            "sorttitle" | "sortname" | "sort_title" | "sort_name" | "sort-title" | "sort-name" | "sortby" | "sort_by"
            | "sort-by" => game.sort_by = Some(entry.first().to_owned()),
            "x-shelf-kind" => game.tool = entry.first() == "tools",
            _ => {}
        }
    }

    fn collection_key(&mut self, c: usize, key: &str, entry: &Entry, dir: &Path) {
        let Some(coll) = self.collections.get_mut(c) else {
            return;
        };
        match key {
            "shortname" => coll.short_name = Some(entry.first().to_lowercase()),
            "launch" | "command" => coll.launch = entry.merged(),
            "workdir" | "cwd" => coll.workdir = entry.first().to_owned(),
            "extension" | "extensions" => {
                if let Some((_, exts)) = coll.filters.last_mut() {
                    exts.extend(entry.first().split(',').map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()));
                }
            }
            "summary" => coll.summary = entry.text(),
            "sortby" | "sort_by" | "sort-by" => coll.sort_by = Some(entry.first().to_owned()),
            "x-shelf-kind" => coll.tools = entry.first() == "tools",
            _ => {
                let _ = dir;
            }
        }
    }

    /// Games for the files a collection's extensions match: direct files of its dir, and everything in
    /// its subdirs except `media` (hidden entries skipped at every level).
    fn run_filters(&mut self) {
        for c in 0..self.collections.len() {
            let filters = self.collections.get(c).map(|coll| coll.filters.clone()).unwrap_or_default();
            for (dir, exts) in filters {
                if exts.is_empty() {
                    continue;
                }
                let mut found = Vec::new();
                collect(&dir, true, &mut found);
                found.sort();
                for path in found.into_iter().filter(|p| exts.contains(&extension(p))) {
                    let path = clean(&path);
                    let g = match self.owner.get(&path) {
                        Some(&g) => g,
                        None => {
                            let title = pretty_name(&path);
                            self.games.push(RawGame { title, files: vec![path.clone()], players: 1, ..RawGame::default() });
                            let g = self.games.len() - 1;
                            self.owner.insert(path, g);
                            g
                        }
                    };
                    self.add_to(g, c);
                }
            }
        }
    }

    /// Box art from `<collection dir>/media/<game file's name or title>/boxFront.*`.
    fn media(&mut self) {
        let mut by_key: HashMap<PathBuf, usize> = HashMap::new();
        for (g, game) in self.games.iter().enumerate() {
            for f in &game.files {
                let dir = f.parent().map(Path::to_path_buf).unwrap_or_default();
                by_key.entry(dir.join(base_name(f))).or_insert(g);
                by_key.entry(dir.join(&game.title)).or_insert(g);
            }
        }
        let dirs: Vec<PathBuf> = self.collections.iter().flat_map(|c| c.filters.iter().map(|(d, _)| d.clone())).collect();
        for dir in dirs {
            for media in ["media", ".media"] {
                let root = dir.join(media);
                let mut files = Vec::new();
                collect_all(&root, &mut files);
                files.sort();
                for f in files {
                    let image = matches!(f.extension().and_then(|e| e.to_str()), Some("png" | "jpg" | "webp" | "apng"));
                    if !image || !is_box_front_name(&base_name(&f)) {
                        continue;
                    }
                    let Some(rel) = f.parent().and_then(|p| p.strip_prefix(&root).ok()) else {
                        continue;
                    };
                    if let Some(&g) = by_key.get(&dir.join(rel))
                        && let Some(game) = self.games.get_mut(g)
                        && !game.box_front.contains(&f)
                    {
                        game.box_front.push(f);
                    }
                }
            }
        }
    }
}

fn is_box_front_name(name: &str) -> bool {
    matches!(name, "boxfront" | "boxFront" | "box_front" | "boxart2D" | "boxart2d")
}

fn is_box_front_key(key: &str) -> bool {
    let name = key.strip_prefix("assets.").or_else(|| key.strip_prefix("asset."));
    name.is_some_and(|n| matches!(n, "boxfront" | "box_front" | "boxart2d"))
}

/// `1-4` gives 4; `2` gives 2; anything else: none.
fn parse_players(v: &str) -> Option<u8> {
    let mut parts = v.splitn(2, '-');
    let a: u8 = parts.next()?.parse().ok()?;
    let b: u8 = match parts.next() {
        Some(b) => b.parse().ok()?,
        None => a,
    };
    Some(a.max(b).max(1))
}

/// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
fn parse_release(v: &str) -> Option<(u16, u8, u8)> {
    let mut parts = v.split('-');
    let y: u16 = parts.next().filter(|p| p.len() == 4)?.parse().ok()?;
    let m: u8 = parts.next().map_or(Some(1), |p| p.parse().ok())?;
    let d: u8 = parts.next().map_or(Some(1), |p| p.parse().ok())?;
    Some((y.max(1), m.clamp(1, 12), d.clamp(1, 31)))
}

/// A dir's direct files (non-hidden), and everything under its subdirs except a top-level `media`.
fn collect(dir: &Path, top: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if hidden(&path) {
            continue;
        }
        if path.is_dir() {
            if top && path.file_name().is_some_and(|n| n == "media") {
                continue;
            }
            // A dir named like a game (`foo.iso`) counts too, as in Pegasus.
            if !top {
                out.push(path.clone());
            }
            collect(&path, false, out);
        } else {
            out.push(path);
        }
    }
}

/// Every file under `dir`, recursively (media folders).
fn collect_all(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            collect_all(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Reads the metadata files of `dirs` (in sorted path order, as Pegasus does) into collections and
/// games, `first_game` being the index the first game will get in the library.
pub fn scan(dirs: &[PathBuf], first_game: usize) -> (Vec<Collection>, Vec<Game>) {
    let mut metafiles: Vec<PathBuf> = dirs
        .iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flat_map(|entries| entries.flatten().map(|e| e.path()))
        .filter(|p| p.is_file() && !hidden(p) && p.file_name().and_then(|n| n.to_str()).is_some_and(is_metafile))
        .map(|p| clean(&p))
        .collect();
    metafiles.sort();
    metafiles.dedup();

    let mut scan = Scan::default();
    for path in &metafiles {
        scan.read_metafile(path);
    }
    scan.run_filters();
    scan.media();

    // Games with files and a collection survive; their final indices.
    let mut index = vec![None; scan.games.len()];
    let mut games = Vec::new();
    for (i, g) in scan.games.into_iter().enumerate() {
        let Some(file) = g.files.first().cloned() else {
            continue;
        };
        if g.collections.is_empty() {
            continue;
        }
        if let Some(slot) = index.get_mut(i) {
            *slot = Some(first_game + games.len());
        }
        let launch = if g.launch.is_empty() {
            Launch::None
        } else {
            Launch::Command { command: g.launch, workdir: g.workdir, basedir: g.basedir }
        };
        let mut game = Game::new(&file.to_string_lossy(), &g.title);
        game.sort_key = crate::sort::key(g.sort_by.as_deref().unwrap_or(&g.title));
        game.file = Some(file);
        game.launch = launch;
        game.developers = dedup(g.developers);
        game.publishers = dedup(g.publishers);
        game.genres = dedup(g.genres);
        game.players = g.players;
        game.release = g.release;
        game.summary = g.summary;
        game.description = g.description;
        game.box_front = g.box_front.into_iter().next();
        game.tool = g.tool;
        games.push(game);
    }
    let collections = scan
        .collections
        .into_iter()
        .map(|c| {
            let members: Vec<usize> = c.games.iter().filter_map(|&g| index.get(g).copied().flatten()).collect();
            Collection {
                short_name: c.short_name.unwrap_or_else(|| c.name.to_lowercase()),
                sort_key: crate::sort::key(c.sort_by.as_deref().unwrap_or(&c.name)),
                name: c.name,
                summary: c.summary,
                tools: c.tools,
                derived: false,
                games: members,
            }
        })
        .filter(|c| !c.games.is_empty())
        .collect();
    (collections, games)
}

fn dedup(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn players_and_release() {
        assert_eq!(parse_players("1-4"), Some(4));
        assert_eq!(parse_players("2"), Some(2));
        assert_eq!(parse_players("two"), None);
        assert_eq!(parse_release("1996-12-31"), Some((1996, 12, 31)));
        assert_eq!(parse_release("1997"), Some((1997, 1, 1)));
        assert_eq!(parse_release("97"), None);
    }

    #[test]
    fn clean_paths() {
        assert_eq!(clean(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
    }

    /// A small library on disk: metadata games, scanned files, media art, a missing file.
    #[test]
    fn scans_a_library() {
        let root = std::env::temp_dir().join(format!("sysprog-pegasus-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let snes = root.join("snes");
        fs::create_dir_all(snes.join("media/Chrono Trigger (U)")).unwrap();
        fs::create_dir_all(snes.join("sub")).unwrap();
        for f in ["Chrono Trigger (U).smc", "Super_Metroid.sfc", "notes.txt", ".hidden.smc", "sub/F-Zero.smc"] {
            fs::write(snes.join(f), "").unwrap();
        }
        fs::write(snes.join("media/Chrono Trigger (U)/boxFront.png"), "").unwrap();
        fs::write(
            snes.join("metadata.pegasus.txt"),
            "collection: Super Nintendo\nshortname: SNES\nextensions: smc, SFC\nlaunch: retroarch -L core.so \"{file.path}\"\n\n\
             game: Chrono Trigger\nfile: Chrono Trigger (U).smc\ndeveloper: Square\nplayers: 1\nrelease: 1995-03-11\n\
             description: Time\n  travel.\n\ngame: Missing\nfile: missing.smc\n",
        )
        .unwrap();

        let (colls, games) = scan(std::slice::from_ref(&snes), 0);
        assert_eq!(colls.len(), 1);
        let c = &colls[0];
        assert_eq!((c.name.as_str(), c.short_name.as_str()), ("Super Nintendo", "snes"));
        let titles: Vec<&str> = c.games.iter().map(|&g| games[g].title.as_str()).collect();
        assert_eq!(titles.len(), 3, "{titles:?}");
        assert!(titles.contains(&"Chrono Trigger") && titles.contains(&"Super Metroid") && titles.contains(&"F-Zero"));
        let ct = games.iter().find(|g| g.title == "Chrono Trigger").unwrap();
        assert_eq!(ct.developers, vec!["Square"]);
        assert_eq!(ct.release, Some((1995, 3, 11)));
        assert_eq!(ct.description, "Time travel.");
        assert_eq!(ct.box_front.as_deref(), Some(snes.join("media/Chrono Trigger (U)/boxFront.png").as_path()));
        assert!(matches!(&ct.launch, Launch::Command { command, .. } if command == "retroarch -L core.so \"{file.path}\""));
        let _ = fs::remove_dir_all(&root);
    }
}
