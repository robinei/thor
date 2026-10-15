//! Pegasus Frontend's game dirs, read the way Pegasus reads them (docs/pegasus-format.md in the GPD
//! repo): `game_dirs.txt`, the metadata files at the top of each game dir (collections, `game:` entries,
//! launch commands, details, `assets.*`), files found by a collection's extensions, and box art in
//! `media/<file name>/boxFront.*`. Missing files are skipped, as Pegasus does with `verify-files`. The
//! syntax and the keys are clamshell-library's (`metafile`, `pegasus`); this is the disk.
//!
//! Not implemented (unused in this setup, see the spec): `directories:`, collection `files:`, regexes,
//! `ignore-*`, global `metafiles/`, URI files, assets other than box art, `rating`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use clamshell_library::metafile::{self, Entry};
use clamshell_library::pegasus::{self, CollectionKey, GameKey, base_name, clean, hidden};
use clamshell_library::{Collection, Game, Launch, LaunchSpec, sort};

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

#[derive(Debug, Default)]
struct RawCollection {
    name: String,
    short_name: Option<String>,
    sort_by: Option<String>,
    summary: String,
    launch: LaunchSpec,
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
    launch: LaunchSpec,
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

impl RawGame {
    fn new(title: String) -> RawGame {
        RawGame { title, players: 1, ..RawGame::default() }
    }
}

#[derive(Default)]
struct Scan {
    collections: Vec<RawCollection>,
    by_name: HashMap<String, usize>,
    games: Vec<RawGame>,
    /// Which game each file belongs to (the first that names it).
    owner: HashMap<PathBuf, usize>,
}

/// A file named in a metadata file, if it exists.
fn existing(path: PathBuf) -> Option<PathBuf> {
    let path = clean(&path);
    path.exists().then_some(path)
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
        g.launch.inherit(&c.launch);
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
                        coll.launch.basedir.clone_from(&dir);
                        coll.filters.push((dir.clone(), Vec::new()));
                    }
                    cur_coll = Some(c);
                    cur_game = None;
                    file_colls.push(c);
                }
                "game" => {
                    let mut game = RawGame::new(entry.first().to_owned());
                    game.launch.basedir.clone_from(&dir);
                    self.games.push(game);
                    let g = self.games.len() - 1;
                    for &c in &file_colls {
                        self.add_to(g, c);
                    }
                    cur_game = Some(g);
                }
                key => match (cur_game, cur_coll) {
                    (Some(g), _) => {
                        if let Some(key) = GameKey::parse(key) {
                            self.game_key(g, key, &entry, &dir);
                        }
                    }
                    (None, Some(c)) => {
                        if let (Some(key), Some(coll)) = (CollectionKey::parse(key), self.collections.get_mut(c)) {
                            collection_key(coll, key, &entry);
                        }
                    }
                    (None, None) => {}
                },
            }
        }
    }

    fn game_key(&mut self, g: usize, key: GameKey, entry: &Entry, dir: &Path) {
        let Some(game) = self.games.get_mut(g) else {
            return;
        };
        match key {
            GameKey::Files => {
                for file in entry.items().filter_map(|f| existing(dir.join(f))) {
                    if !self.owner.contains_key(&file) {
                        self.owner.insert(file.clone(), g);
                        game.files.push(file);
                    }
                }
            }
            GameKey::BoxFront => game.box_front.extend(entry.items().filter_map(|f| existing(dir.join(f)))),
            GameKey::Launch => game.launch.command = entry.merged(),
            GameKey::Workdir => game.launch.workdir = entry.first().to_owned(),
            GameKey::Developers => game.developers.extend(entry.items().map(str::to_owned)),
            GameKey::Publishers => game.publishers.extend(entry.items().map(str::to_owned)),
            GameKey::Genres => game.genres.extend(entry.items().map(str::to_owned)),
            GameKey::Players => {
                if let Some(n) = pegasus::parse_players(entry.first()) {
                    game.players = n;
                }
            }
            GameKey::Release => game.release = pegasus::parse_release(entry.first()).or(game.release),
            GameKey::Summary => game.summary = entry.text(),
            GameKey::Description => game.description = entry.text(),
            GameKey::SortBy => game.sort_by = Some(entry.first().to_owned()),
            GameKey::Kind => game.tool = entry.first() == "tools",
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
                for path in found.into_iter().filter(|p| exts.contains(&pegasus::extension(p))) {
                    let path = clean(&path);
                    let g = match self.owner.get(&path) {
                        Some(&g) => g,
                        None => {
                            let mut game = RawGame::new(pegasus::pretty_name(&path));
                            game.files.push(path.clone());
                            self.games.push(game);
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
                    if !pegasus::is_image(&f) || !pegasus::is_box_front_name(&base_name(&f)) {
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

fn collection_key(coll: &mut RawCollection, key: CollectionKey, entry: &Entry) {
    match key {
        CollectionKey::ShortName => coll.short_name = Some(entry.first().to_lowercase()),
        CollectionKey::Launch => coll.launch.command = entry.merged(),
        CollectionKey::Workdir => coll.launch.workdir = entry.first().to_owned(),
        CollectionKey::Extensions => {
            if let Some((_, exts)) = coll.filters.last_mut() {
                exts.extend(pegasus::extensions(entry.first()));
            }
        }
        CollectionKey::Summary => coll.summary = entry.text(),
        CollectionKey::SortBy => coll.sort_by = Some(entry.first().to_owned()),
        CollectionKey::Kind => coll.tools = entry.first() == "tools",
    }
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
/// games; the collections' game indices count from 0 in the games returned.
pub fn scan(dirs: &[PathBuf]) -> (Vec<Collection>, Vec<Game>) {
    let mut metafiles: Vec<PathBuf> = dirs
        .iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flat_map(|entries| entries.flatten().map(|e| e.path()))
        .filter(|p| p.is_file() && !hidden(p) && p.file_name().and_then(|n| n.to_str()).is_some_and(pegasus::is_metafile))
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
            *slot = Some(games.len());
        }
        let mut game = Game::new(&file.to_string_lossy(), &g.title);
        game.sort_key = sort::key(g.sort_by.as_deref().unwrap_or(&g.title));
        game.file = Some(file);
        game.launch = if g.launch.command.is_empty() { Launch::None } else { Launch::Command(g.launch) };
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
                sort_key: sort::key(c.sort_by.as_deref().unwrap_or(&c.name)),
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
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A small library on disk: metadata games, scanned files, media art, a missing file.
    #[test]
    fn scans_a_library() {
        let root = std::env::temp_dir().join(format!("clamshell-pegasus-{}", std::process::id()));
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

        let (colls, games) = scan(std::slice::from_ref(&snes));
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
        let launch = LaunchSpec { command: "retroarch -L core.so \"{file.path}\"".into(), workdir: String::new(), basedir: snes.clone() };
        assert_eq!(ct.launch, Launch::Command(launch), "the collection's command, against its dir");
        let _ = fs::remove_dir_all(&root);
    }
}
