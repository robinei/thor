//! The library from disk: library.json's content dirs (see [`content`]), installed Steam games, the
//! desktop files' games and apps; play time and favourites as kept (see [`crate::store`]). The model is
//! clamshell-library's.

pub mod content;
pub mod runners;
pub mod steam;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clamshell_library::{BrowseKind, Browsed, Collection, Details, Entry, Launch, Library, Meta, Tool, ids};

use crate::meta::{How, Keys, RETRY_MISS, Wanted};

use crate::paths::Paths;

/// Where to read from; `None` skips a source.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// library.json.
    pub config: Option<PathBuf>,
    /// The home dir, for `~/` in library.json.
    pub home: Option<PathBuf>,
    /// Steam's data dir.
    pub steam: Option<PathBuf>,
    /// ClamShell's state dir, for play time and favourites.
    pub state: Option<PathBuf>,
    /// The art cache, for what was fetched about entries and videos' lengths.
    pub art: Option<PathBuf>,
    /// The XDG data dirs, the installed apps' desktop files in their `applications` (see
    /// [`crate::desktop`]).
    pub data_dirs: Vec<PathBuf>,
    /// Where mpv keeps videos' places.
    pub watch_later: Vec<PathBuf>,
}

impl Sources {
    /// ClamShell's paths, and Steam's data dir under `home` unless `steam` is off.
    pub fn new(paths: &Paths, home: Option<&std::path::Path>, steam: bool) -> Sources {
        Sources {
            config: paths.library.clone(),
            home: home.map(std::path::Path::to_path_buf),
            steam: home.filter(|_| steam).and_then(steam::default_root),
            state: paths.state_dir.clone(),
            art: paths.art.clone(),
            data_dirs: crate::desktop::data_dirs(),
            watch_later: paths.watch_later.clone(),
        }
    }
}

/// The folders made in the tabs, by tab and path; library.json's places (their ids, icons, pictures),
/// image files in them from `config_dir` (`~/` the home dir).
struct Folders<'a> {
    made: HashMap<(String, String), usize>,
    places: &'a [content::Place],
    home: Option<&'a Path>,
    config_dir: Option<&'a Path>,
}

impl Folders<'_> {
    /// Puts `entries` (a scan's, from `dir`) in the folder at `path` in `tab` (`A/B`: B in A), made where it
    /// isn't there yet (its art from a place declared there, else from `dir`); the folder at the top of the
    /// tab if it is new (to add to it).
    fn put(&mut self, games: &mut Vec<Entry>, tab: &str, path: &str, dir: &Path, entries: &[usize]) -> Option<usize> {
        let mut new_top = None;
        let mut parent: Option<usize> = None;
        let mut at = String::new();
        for part in path.split('/').map(str::trim).filter(|p| !p.is_empty()) {
            if !at.is_empty() {
                at.push('/');
            }
            at.push_str(part);
            let folder = match self.made.get(&(tab.to_owned(), at.clone())) {
                Some(&f) => f,
                None => {
                    let f = games.len();
                    games.push(Entry { parent, ..self.folder(tab, &at, part) });
                    self.made.insert((tab.to_owned(), at.clone()), f);
                    match parent.and_then(|p| games.get_mut(p)) {
                        Some(p) => p.items.push(f),
                        None => new_top = Some(f),
                    }
                    f
                }
            };
            parent = Some(folder);
        }
        let folder = parent?;
        for &e in entries {
            if let Some(g) = games.get_mut(e) {
                g.parent = Some(folder);
            }
        }
        if let Some(g) = games.get_mut(folder) {
            g.items.extend_from_slice(entries);
            if g.box_front.is_none() {
                g.box_front = content::folder_art(dir);
            }
        }
        new_top
    }

    /// A new folder at `at` in `tab`, titled `title`: a place declared there gives its id (`place:<id>`),
    /// icon and picture; else its id is its path (`folder:<tab>/<at>`).
    fn folder(&self, tab: &str, at: &str, title: &str) -> Entry {
        let path = format!("{tab}/{at}");
        let Some(place) = self.places.iter().find(|p| p.path.trim().trim_matches('/') == path) else {
            return Entry::new(&format!("folder:{path}"), title);
        };
        let file = |f: &str| match (f.strip_prefix("~/"), self.home, self.config_dir) {
            (Some(rest), Some(home), _) => home.join(rest),
            (None, _, Some(dir)) if !f.starts_with('/') => dir.join(f),
            _ => PathBuf::from(f),
        };
        let mut g = Entry::new(&format!("place:{}", place.id), title);
        g.box_front = place.picture.as_deref().map(file);
        match place.icon.as_deref() {
            // (An image file, or one of the UI's icons by name.)
            Some(icon) if icon.contains(['/', '.']) => g.icon = Some(file(icon)),
            Some(icon) => g.glyph = Some(icon.into()),
            None => {}
        }
        g
    }
}

/// The programs ClamShell runs itself (not a runner's: those are the runners' problems), and what goes
/// without each.
const NEEDED: [(&str, &str); 8] = [
    ("systemd-run", "nothing can be launched"),
    ("ratarmount", "archives can't be browsed"),
    ("fusermount3", "archives can't be browsed"),
    ("bsdtar", "archives can't be extracted"),
    ("grim", "no screenshots"),
    ("udisksctl", "removable storage can't be ejected"),
    ("wpctl", "no volume in the bar"),
    ("curl", "no covers or details fetched"),
];

/// The Tools tab's entries: id, title, the tool, what it does.
const TOOLS: [(&str, &str, Tool, &str); 3] = [
    (
        "tool:device",
        "Device",
        Tool::Device,
        "Calibrates the controller's sticks and gyro and the top screen's refresh rate; sets the password and remote login; Wi-Fi and the time zone.",
    ),
    (
        "tool:rescan",
        "Rescan library",
        Tool::Rescan,
        "Reads library.json, its folders, Steam and the desktop files again, then fetches the covers and details still missing.",
    ),
    ("tool:refetch", "Fetch missing art again", Tool::Refetch, "Tries again for the entries whose covers and details weren't found, then rescans."),
];

/// The order of a tab's or a folder's entries: folders first, then by title; browsed dirs' own folders
/// after the other folders, as library.json and the mounts have them.
fn list_order(games: &[Entry], a: usize, b: usize) -> std::cmp::Ordering {
    let key = |i: usize| {
        games.get(i).map(|g| match g.browsed.as_ref().map(|b| &b.kind) {
            Some(BrowseKind::Root | BrowseKind::Trash | BrowseKind::Removable(_)) => (false, true, None),
            _ => (!g.is_group(), false, Some((&g.sort_key, &g.title))),
        })
    };
    key(a).cmp(&key(b))
}

/// A browsed dir's own folder.
fn browsed_root(id: &str, dir: PathBuf, kind: BrowseKind) -> Entry {
    let glyph = match kind {
        BrowseKind::Trash => "trash",
        BrowseKind::Removable(_) => "card",
        _ => "folder",
    };
    Entry { file: Some(dir), browsed: Some(Browsed { folder: true, kind, ..Browsed::default() }), glyph: Some(glyph.into()), ..Entry::new(id, "") }
}

/// The first frame (a video's, in order, all the way down) in folder `i`.
fn first_frame(library: &Library, i: usize) -> Option<PathBuf> {
    let g = library.entries.get(i)?;
    g.items.iter().find_map(|&item| match library.entries.get(item) {
        Some(e) if e.is_group() => first_frame(library, item),
        Some(e) => e.details.shot.clone().filter(|_| e.mpv_file().is_some()),
        None => None,
    })
}

/// The library, and what to fetch for it.
#[derive(Debug, Default)]
pub struct Loaded {
    pub library: Library,
    /// The entries to fetch metadata for (none fetched, the source changed, a miss long ago, or its
    /// images gone from the cache).
    pub wanted: Vec<Wanted>,
    pub keys: Keys,
}

/// Reads library.json's sources into its places (see [`content`]): each entry, its runner and where its
/// metadata comes from; ClamShell's own tabs as declared; the tabs in their order. Then each entry's
/// plays, whether it is a favourite, and what was fetched about it.
pub fn load(sources: &Sources) -> Loaded {
    let (config, config_problem) = sources.config.as_deref().map(content::read_config).unwrap_or_default();
    let (runners, runner_problems) = runners::in_force(&config.runners);
    let config_dir = sources.config.as_deref().and_then(Path::parent);
    let mut b = Builder {
        config: &config,
        runners: &runners,
        sources,
        games: Vec::new(),
        collections: Vec::new(),
        folders: Folders { made: HashMap::new(), places: &config.places, home: sources.home.as_deref(), config_dir },
        hows: Vec::new(),
        problems: config_problem.into_iter().chain(runner_problems).collect(),
        away: Vec::new(),
        mounts: if config.sources.iter().any(|s| matches!(s, content::Source::Removable(_) | content::Source::Dir(content::Scan { on: Some(_), .. }))) {
            crate::files::media_mounts()
        } else {
            Vec::new()
        },
    };
    b.check_tabs();
    for source in &config.sources {
        match source {
            content::Source::Dir(scan) => b.scan(scan),
            content::Source::Browse(root) => b.browse(root),
            // (Each kind of these at once, its sources in turn.)
            content::Source::Desktop(_) | content::Source::Steam(_) | content::Source::Removable(_) => {}
        }
    }
    b.removable();
    b.installed();
    b.needed();
    b.order();
    b.mark_recent();
    b.tools();
    let runner_keymaps = runners.iter().filter_map(|r| Some((clamshell_library::runner_key(&r.id), r.keymap.clone()?))).collect();
    let order: Vec<String> = config.places.iter().map(|p| p.path.trim().trim_matches('/')).filter(|p| !p.contains('/')).map(str::to_owned).collect();
    let (recent, favourites) = (b.own(content::OwnTab::Recent), b.own(content::OwnTab::Favourites));
    let Builder { games, collections, hows, problems, away, .. } = b;
    let mut library = Library { collections, entries: games, problems, away, runner_keymaps, order, recent, favourites, ..Library::default() };
    let wanted = known(&mut library, hows, sources);
    library.rebuild_derived();
    Loaded { library, wanted, keys: config.keys }
}

/// The library as it is read: its entries and tabs, the folders made, where each entry's metadata comes
/// from, what is wrong, the sources away; the removable filesystems mounted.
struct Builder<'a> {
    config: &'a content::Config,
    runners: &'a [runners::Runner],
    sources: &'a Sources,
    games: Vec<Entry>,
    collections: Vec<Collection>,
    folders: Folders<'a>,
    hows: Vec<(usize, How)>,
    problems: Vec<String>,
    away: Vec<String>,
    mounts: Vec<crate::files::Mount>,
}

impl Builder<'_> {
    /// A tab shows its icon only, one of the UI's: something in it is always selected.
    fn check_tabs(&mut self) {
        for p in self.config.places.iter().filter(|p| !p.path.trim().trim_matches('/').contains('/')) {
            if p.picture.is_some() || p.icon.as_deref().is_some_and(|i| i.contains(['/', '.'])) {
                self.problems.push(format!("{}: a tab has an icon of the UI's (gamepad, film, ...), no picture", p.path));
            }
        }
    }

    /// A place as library.json gives it (`@id` a declared one's path), or what is wrong with it (said).
    fn resolve(&mut self, place: &str) -> Option<String> {
        content::resolve(place, &self.config.places).map_err(|e| self.problems.push(e)).ok()
    }

    /// `entries` at `place` (resolved): in its tab, or in a folder of it (made from `dir`'s art).
    fn put(&mut self, place: &str, dir: &Path, entries: Vec<usize>) {
        let (tab, path) = content::place(place);
        let entries = match path {
            "" => entries,
            _ if entries.is_empty() => entries,
            path => self.folders.put(&mut self.games, tab, path, dir, &entries).into_iter().collect(),
        };
        let c = match self.collections.iter().position(|c| c.name == tab) {
            Some(i) => self.collections.get_mut(i),
            None => {
                self.collections.push(Collection::new(tab, Vec::new()));
                self.collections.last_mut()
            }
        };
        if let Some(c) = c {
            c.entries.extend(entries);
        }
    }

    /// Where a source's `metadata` says its entries' covers and details come from; what is wrong with it.
    fn how(&mut self, id: &str, metadata: Option<&str>) -> Option<How> {
        let m = metadata?;
        let Some(how) = How::parse(m) else {
            self.problems.push(format!("{id}: metadata \"{m}\" isn't libretro:<system>, tmdb:movie, tmdb:tv, steamgriddb or steam"));
            return None;
        };
        // (Without the key nothing is asked: not a miss, so it is fetched once the key is there.)
        let keys = &self.config.keys;
        match how {
            How::Tmdb(_) if keys.tmdb.is_none() => self.problems.push(format!("{id}: tmdb needs keys.tmdb")),
            How::SteamGridDb if keys.steamgriddb.is_none() => self.problems.push(format!("{id}: steamgriddb needs keys.steamgriddb")),
            how => return Some(how),
        }
        None
    }

    /// A dir source: its entries scanned, each opened by its runner (or its file's name's), at its place.
    fn scan(&mut self, source: &content::Scan) {
        let runners = self.runners;
        let home = self.sources.home.as_deref();
        // The runner it names opens all its entries (its patterns theirs, unless it has its own); else
        // each its file's name's.
        let named = match source.runner.as_deref().map(|id| runners.iter().find(|r| r.id == id).ok_or(id)) {
            Some(Err(id)) => return self.problems.push(format!("{}: no runner {id}", source.id)),
            Some(Ok(r)) => Some(r),
            None => None,
        };
        let patterns = match named {
            Some(r) if source.patterns.is_empty() => r.patterns.clone(),
            _ => source.patterns.clone(),
        };
        let open = |path: &Path| match named {
            Some(r) => Some((r.command(path, home), r.of(home))),
            None => runners::open(runners, path, home),
        };
        // Its dir: on a removable filesystem (away while none matching is mounted), or as it says.
        let dir = match &source.on {
            Some(on) => match self.mounts.iter().find(|m| on.matches(&m.label, &m.uuid, &m.fs)) {
                Some(m) => m.dir.join(source.dir.trim_start_matches('/')),
                None => {
                    self.problems.push(format!("{}: its card or stick isn't in", source.id));
                    self.away.push(format!("{}:", source.id));
                    return;
                }
            },
            None => match content::expand_dir(&source.dir, home) {
                Ok(dir) => dir,
                Err(name) => return self.problems.push(format!("{}: ${name} isn't set", source.id)),
            },
        };
        if !dir.is_dir() {
            self.problems.push(format!("{}: no folder {}", source.id, dir.display()));
            self.away.push(format!("{}:", source.id));
        }
        let scan = content::Scan { patterns, dir: dir.to_string_lossy().into_owned(), on: None, ..source.clone() };
        let entries = content::scan(&scan, home, &mut self.games, &open);
        if let Some(how) = self.how(&source.id, source.metadata.as_deref()) {
            // (libretro has games, not folders of them.)
            let games = &self.games;
            let fits = |i: &usize| !matches!(how, How::Libretro(_)) || games.get(*i).is_some_and(|g| !g.is_group());
            let fitting: Vec<usize> = entries.iter().copied().filter(fits).collect();
            self.hows.extend(fitting.into_iter().map(|i| (i, how.clone())));
        }
        if let Some(place) = self.resolve(&source.place) {
            self.put(&place, &dir, entries);
        }
    }

    /// A browse source: its dir's own folder at its place, listed when it is opened.
    fn browse(&mut self, root: &content::BrowseRoot) {
        let Some(place) = self.resolve(&root.place) else {
            return;
        };
        let place = place.trim().trim_matches('/').to_owned();
        let dir = match content::expand_dir(&root.dir, self.sources.home.as_deref()) {
            Ok(dir) if dir.is_dir() || root.trash => dir,
            Ok(dir) => return self.problems.push(format!("{place}: no folder {}", dir.display())),
            Err(name) => return self.problems.push(format!("{place}: ${name} isn't set")),
        };
        // (The Trash's folder may not be made yet.)
        if root.trash {
            let _ = std::fs::create_dir_all(&dir);
        }
        let kind = if root.trash { BrowseKind::Trash } else { BrowseKind::Root };
        self.browsed_root(&place, browsed_root(&format!("{}{place}", ids::BROWSE), dir, kind));
    }

    /// The removable filesystems mounted, each taken by the first removable source matching it.
    fn removable(&mut self) {
        let config = self.config;
        let rules: Vec<&content::MountRule> = config
            .sources
            .iter()
            .filter_map(|s| match s {
                content::Source::Removable(rule) => Some(rule),
                _ => None,
            })
            .collect();
        for m in std::mem::take(&mut self.mounts) {
            let Some(rule) = rules.iter().find(|r| r.matches(&m.label, &m.uuid, &m.fs)) else {
                continue;
            };
            let Some(place) = rule.place.as_deref().filter(|_| !rule.hide) else {
                continue;
            };
            let name = if m.label.is_empty() { &m.uuid } else { &m.label };
            let Some(place) = self.resolve(place).map(|p| p.replace("{label}", name)) else {
                continue;
            };
            let id = format!("{}media/{}", ids::BROWSE, if m.uuid.is_empty() { &m.device } else { &m.uuid });
            self.browsed_root(&place, browsed_root(&id, m.dir, BrowseKind::Removable(m.device)));
        }
    }

    /// A browsed dir's folder at `place`: its last part the folder's title, the rest where it is.
    fn browsed_root(&mut self, place: &str, mut root: Entry) {
        let (tab, path) = content::place(place);
        if path.is_empty() {
            return self.problems.push(format!("{place}: a browsed dir's place needs a tab and its folder's title (Files/Home)"));
        }
        let (within, name) = path.rsplit_once('/').unwrap_or(("", path));
        name.clone_into(&mut root.title);
        root.sort_key = clamshell_library::sort::key(name);
        self.games.push(root);
        let i = vec![self.games.len() - 1];
        let place = if within.is_empty() { tab.to_owned() } else { format!("{tab}/{within}") };
        self.put(&place, Path::new(""), i);
    }

    /// Steam's installed games and the desktop files' apps and games, each taken by the first source of
    /// its type matching it: at its place (or left out: `hide`, or none matching), opened by its runner,
    /// its metadata from where it says. (The games first, then the apps.)
    fn installed(&mut self) {
        let config = self.config;
        let of_type = |want: fn(&content::Source) -> Option<&content::Rule>| config.sources.iter().filter_map(want).collect::<Vec<_>>();
        let steam = of_type(|s| match s {
            content::Source::Steam(r) => Some(r),
            _ => None,
        });
        let desktop = of_type(|s| match s {
            content::Source::Desktop(r) => Some(r),
            _ => None,
        });
        let mut placed: Vec<(&content::Rule, Entry)> = Vec::new();
        // (Its problems once each.)
        let first_problem = self.problems.len();
        if !steam.is_empty() {
            for game in self.sources.steam.as_deref().map(steam::games).unwrap_or_default() {
                let appid = match game.launch {
                    Launch::Steam { appid } => appid.to_string(),
                    _ => String::new(),
                };
                if let Some(rule) = first_match(&steam, &appid, &game.title, &[]) {
                    placed.push((rule, game));
                }
            }
        }
        if !desktop.is_empty() {
            for d in crate::desktop::entries(&self.sources.data_dirs) {
                if let Some(rule) = first_match(&desktop, &d.id, &d.name, &d.categories) {
                    placed.push((rule, d.game));
                }
            }
        }
        placed.sort_by_key(|(_, g)| g.app);
        for (rule, mut game) in placed {
            let Some(place) = rule.place.as_deref().filter(|_| !rule.hide) else {
                continue;
            };
            // Its runner: the one the source names, else the first of its type.
            let kind = if matches!(game.launch, Launch::Steam { .. }) { runners::Kind::Steam } else { runners::Kind::Desktop };
            let runner = match rule.runner.as_deref() {
                Some(id) => self.runners.iter().find(|r| r.id == id && r.kind == Some(kind)),
                None => runners::of_kind(self.runners, kind),
            };
            let Some(runner) = runner else {
                let what = if kind == runners::Kind::Steam { "steam" } else { "desktop" };
                let named = rule.runner.as_deref().map_or_else(String::new, |id| format!(" ({id} isn't one)"));
                self.problems.push(format!("a {what} source needs a runner of type {what} to open its entries{named}"));
                continue;
            };
            game.runner = Some(runner.of(self.sources.home.as_deref()));
            // (A desktop runner's own command, if it has one, instead of the desktop file's.)
            if let Some(launch) = ids::desktop_file(&game.id).and_then(|id| runner.wrap(id, &game.launch, self.sources.home.as_deref())) {
                game.launch = launch;
            }
            let how = match (rule.metadata.as_deref(), &game.launch) {
                (Some("steam"), Launch::Steam { appid }) => Some(How::Steam(*appid)),
                (metadata, _) => self.how(&game.id, metadata),
            };
            let Some(place) = self.resolve(place) else {
                continue;
            };
            self.games.push(game);
            let i = self.games.len() - 1;
            self.hows.extend(how.map(|how| (i, how)));
            self.put(&place, Path::new(""), vec![i]);
        }
        let mut mine = self.problems.split_off(first_problem);
        mine.sort();
        mine.dedup();
        self.problems.extend(mine);
    }

    /// A folder's entries (from its sources, and its folders) and each tab's in list order; the tabs
    /// without entries left out.
    fn order(&mut self) {
        let games = &self.games;
        let mut sorted: Vec<(usize, Vec<usize>)> = Vec::new();
        for &f in self.folders.made.values() {
            let mut items = games.get(f).map(|g| g.items.clone()).unwrap_or_default();
            items.sort_by(|&a, &b| list_order(games, a, b));
            sorted.push((f, items));
        }
        for (f, items) in sorted {
            if let Some(g) = self.games.get_mut(f) {
                g.items = items;
            }
        }
        self.collections.retain(|c| !c.entries.is_empty());
        let games = &self.games;
        for c in &mut self.collections {
            c.entries.sort_by(|&a, &b| list_order(games, a, b));
        }
    }

    /// The declared place that is tab `name`, if any.
    fn tab_place(&self, name: &str) -> Option<&content::Place> {
        self.config.places.iter().find(|p| p.path.trim().trim_matches('/') == name)
    }

    /// What is in Recent: not what is in a place (or a folder of one) saying `recent: false`; the tabs'
    /// icons, as their places give them.
    fn mark_recent(&mut self) {
        let places = &self.config.places;
        for c in 0..self.collections.len() {
            let Some(name) = self.collections.get(c).map(|c| c.name.clone()) else {
                continue;
            };
            let place = self.tab_place(&name);
            let (recent, icon) = (place.and_then(|p| p.recent).unwrap_or(true), place.and_then(|p| p.icon.clone()));
            let Some(coll) = self.collections.get_mut(c) else {
                continue;
            };
            coll.icon = icon;
            let mut stack: Vec<(usize, bool)> = coll.entries.iter().map(|&i| (i, recent)).collect();
            while let Some((i, inherited)) = stack.pop() {
                let declared = self.games.get(i).and_then(|g| g.id.strip_prefix("place:")).and_then(|id| places.iter().find(|p| p.id == id));
                let recent = declared.and_then(|p| p.recent).unwrap_or(inherited);
                if let Some(g) = self.games.get_mut(i) {
                    g.app = !recent;
                    stack.extend(g.items.iter().map(|&item| (item, recent)));
                }
            }
        }
    }

    /// One of ClamShell's own tabs, as a place declares it.
    fn own(&self, kind: content::OwnTab) -> Option<clamshell_library::OwnTab> {
        let p = self.config.places.iter().find(|p| p.kind == Some(kind))?;
        Some(clamshell_library::OwnTab { name: p.path.trim().trim_matches('/').to_owned(), icon: p.icon.clone() })
    }

    /// The programs ClamShell runs itself that aren't installed: a problem each (what goes without).
    fn needed(&mut self) {
        for (program, for_what) in NEEDED {
            if !runners::on_path(program) {
                self.problems.push(format!("{program} isn't installed: {for_what}"));
            }
        }
    }

    /// ClamShell's own tools, in their tab if a place declares it. The rescan's details list what is
    /// wrong with the library (as read now).
    fn tools(&mut self) {
        let Some(tab) = self.own(content::OwnTab::Tools) else {
            return;
        };
        let first = self.games.len();
        for (id, title, tool, summary) in TOOLS {
            let mut summary = summary.to_owned();
            if tool == Tool::Rescan && !self.problems.is_empty() {
                summary.push_str("\n\nWhat is wrong now:");
                for p in &self.problems {
                    summary.push_str("\n· ");
                    summary.push_str(p);
                }
            }
            self.games.push(Entry { launch: Launch::Tool(tool), app: true, details: Details { summary, ..Details::default() }, ..Entry::new(id, title) });
        }
        self.collections.push(Collection { icon: tab.icon, ..Collection::new(&tab.name, (first..self.games.len()).collect()) });
    }
}

/// The first of `rules` matching an entry with this id, name and categories.
fn first_match<'r>(rules: &[&'r content::Rule], id: &str, name: &str, categories: &[String]) -> Option<&'r content::Rule> {
    rules.iter().copied().find(|r| r.matches(id, name, categories))
}

/// What is kept about `library`'s entries: their plays, whether each is a favourite, what was
/// fetched about them (where its images are still there), mpv's videos' places, lengths and frames; and
/// what to fetch (`hows`: where each entry's metadata comes from).
fn known(library: &mut Library, hows: Vec<(usize, How)>, sources: &Sources) -> Vec<Wanted> {
    let mut metas = std::collections::HashMap::new();
    let mut probes = std::collections::HashMap::new();
    if let Some(art) = sources.art.as_deref() {
        let cache = crate::store::Cache::open(art);
        metas = cache.metas();
        probes = cache.probes();
    }
    if let Some(state) = sources.state.as_deref() {
        let store = crate::store::Store::open(state);
        let plays = store.plays();
        let favourites = store.favourites();
        for g in &mut library.entries {
            if let Some(p) = plays.get(&*g.id) {
                g.plays = *p;
            }
            g.favourite = favourites.contains(&*g.id);
        }
    }
    let now = clamshell_core::unix_secs();
    let mut wanted = Vec::new();
    for (i, how) in hows {
        let Some(g) = library.entries.get(i) else {
            continue;
        };
        let fetched = metas.get(&*g.id);
        let gone = |p: &Option<std::path::PathBuf>| p.as_ref().is_some_and(|p| !p.exists());
        let want = match fetched {
            None => true,
            Some(f) => {
                f.source != how.source()
                    || gone(&f.meta.cover)
                    || gone(&f.meta.details.shot)
                    || (f.meta == Meta::default() && now.saturating_sub(f.at) > RETRY_MISS)
            }
        };
        if want {
            wanted.push(Wanted { id: g.id.clone(), key: g.key.clone(), title: g.title.clone(), file: g.file.clone(), how, art: g.box_front.is_some() });
        } else if let Some(f) = fetched {
            library.apply_meta(i, &f.meta);
        }
    }
    // mpv's videos: where mpv left each, its length and a frame of it (read once, then kept).
    for g in &mut library.entries {
        let Some(file) = g.mpv_file().map(std::path::Path::to_path_buf) else {
            continue;
        };
        let kept = crate::mpv::position(&sources.watch_later, &file);
        (g.playback.position, g.playback.at) = (kept.map(|(s, _)| s), kept.map_or(0, |(_, at)| at));
        let probe = probes.get(&*g.id).filter(|p| p.frame.as_ref().is_none_or(|f| f.exists()));
        match probe {
            Some(p) => {
                g.playback.duration = p.seconds;
                if g.details.shot.is_none() {
                    g.details.shot.clone_from(&p.frame);
                }
            }
            None => wanted.push(Wanted { id: g.id.clone(), key: g.key.clone(), title: g.title.clone(), file: Some(file), how: How::Duration, art: true }),
        }
    }
    // A folder of videos without art: the frame of its first video that has one.
    for i in 0..library.entries.len() {
        let bare = library.entries.get(i).is_some_and(|g| g.is_group() && g.box_front.is_none() && g.details.shot.is_none());
        if bare
            && let Some(frame) = first_frame(library, i)
            && let Some(g) = library.entries.get_mut(i)
        {
            g.details.shot = Some(frame);
        }
    }
    wanted
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_first_and_steam_can_have_its_own() {
        let root = clamshell_core::testdir::TempDir::new("steam-group");
        let steam = root.join("steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(steam.join("steamapps/libraryfolders.vdf"), format!("\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}", steam.display())).unwrap();
        let manifest = |id: u32, name: &str| format!("\"AppState\" {{ \"appid\" \"{id}\" \"name\" \"{name}\" \"StateFlags\" \"4\" }}");
        std::fs::write(steam.join("steamapps/appmanifest_10.acf"), manifest(10, "Arma")).unwrap();
        std::fs::write(steam.join("steamapps/appmanifest_20.acf"), manifest(20, "Zork")).unwrap();
        std::fs::create_dir_all(root.join("snes")).unwrap();
        std::fs::write(root.join("snes/Mario.sfc"), "").unwrap();
        // (The Steam sources.)
        let config = |steam: &str| {
            format!(
                r#"{{ "runners": [{{ "name": "Test SNES", "match": ["*.sfc"], "launch": ["retroarch", "{{path}}"] }}, {{ "type": "steam", "id": "steam", "name": "Steam" }}],
                    "sources": [{{ "type": "dir", "place": "Games/SNES", "id": "snes", "dir": "{}" , "match": ["*.sfc"] }}, {steam}] }}"#,
                root.join("snes").display()
            )
        };
        let games_tab = |json: &str| {
            std::fs::write(root.join("library.json"), json).unwrap();
            let lib = load(&Sources { config: Some(root.join("library.json")), steam: Some(steam.clone()), ..Sources::default() }).library;
            let tab = lib.collections.iter().find(|c| c.name == "Games").unwrap();
            tab.entries.iter().map(|&i| lib.entries[i].title.clone()).collect::<Vec<_>>()
        };
        assert_eq!(games_tab(&config(r#"{ "type": "steam", "place": "Games" }"#)), ["SNES", "Arma", "Zork"], "the folder first, before Arma");
        assert_eq!(games_tab(&config(r#"{ "type": "steam", "place": "Games/Steam" }"#)), ["SNES", "Steam"], "Steam's games in their folder");
        // By name or app id, into folders, or hidden; the rest by the source matching anything; each taken by
        // the first matching it (Zork by z*, not by the last).
        let rules = r#"{ "type": "steam", "name": "z*", "place": "Games/Steam/Text" }, { "type": "steam", "id": "10", "hide": true },
                       { "type": "steam", "place": "Games/Steam" }"#;
        std::fs::write(steam.join("steamapps/appmanifest_30.acf"), manifest(30, "Celeste")).unwrap();
        std::fs::write(root.join("library.json"), config(rules)).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), steam: Some(steam.clone()), ..Sources::default() }).library;
        let titles = |ids: &[usize]| ids.iter().map(|&i| lib.entries[i].title.clone()).collect::<Vec<_>>();
        let folder = |title: &str| lib.entries.iter().find(|g| g.title == title).unwrap();
        assert_eq!(titles(&folder("Steam").items), ["Text", "Celeste"], "Arma hidden");
        assert_eq!(titles(&folder("Text").items), ["Zork"]);
        assert_eq!(folder("Zork").runner.as_ref().map(|r| &*r.key), Some("runner:steam"), "Steam's games: Steam's");
    }

    #[test]
    fn sources_name_their_runner_and_metadata_and_wait_for_their_card() {
        let root = clamshell_core::testdir::TempDir::new("named");
        let apps = root.join("data/applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("celeste.desktop"), "[Desktop Entry]\nType=Application\nName=Celeste\nExec=celeste\nCategories=Game;\n").unwrap();
        std::fs::write(apps.join("foot.desktop"), "[Desktop Entry]\nType=Application\nName=Foot\nExec=foot\nCategories=System;\n").unwrap();
        let steam = root.join("steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(steam.join("steamapps/libraryfolders.vdf"), format!("\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}", steam.display())).unwrap();
        std::fs::write(steam.join("steamapps/appmanifest_10.acf"), "\"AppState\" { \"appid\" \"10\" \"name\" \"Arma\" \"StateFlags\" \"4\" }").unwrap();
        let config = r#"{ "runners": [{ "type": "desktop", "id": "apps", "name": "Apps" }, { "type": "desktop", "id": "desktop-games", "name": "Desktop games" },
            { "type": "steam", "name": "Steam" }],
          "sources": [
            { "type": "desktop", "categories": "Game", "place": "Games", "runner": "desktop-games", "metadata": "steamgriddb" },
            { "type": "desktop", "place": "Apps" },
            { "type": "steam", "place": "Games", "metadata": "steam" },
            { "type": "dir", "id": "sd", "dir": "roms", "place": "Games", "match": ["*"], "on": { "label": "NO SUCH CARD" } }
          ] }"#;
        std::fs::write(root.join("library.json"), config).unwrap();
        let loaded =
            load(&Sources { config: Some(root.join("library.json")), data_dirs: vec![root.join("data")], steam: Some(steam.clone()), ..Sources::default() });
        let lib = &loaded.library;
        let runner = |title: &str| lib.entries.iter().find(|g| g.title == title).and_then(|g| g.runner.as_ref()).map(|r| r.key.to_string());
        assert_eq!(runner("Celeste").as_deref(), Some("runner:desktop-games"), "the runner it names");
        assert_eq!(runner("Foot").as_deref(), Some("runner:apps"), "else the first of its type");
        assert!(lib.problems.iter().any(|p| p.ends_with("steamgriddb needs keys.steamgriddb")), "{:?}", lib.problems);
        assert!(loaded.wanted.iter().any(|w| w.how == How::Steam(10)), "Steam's store, as it says");
        assert!(!loaded.wanted.iter().any(|w| w.how == How::SteamGridDb), "not without the key");
        assert!(lib.problems.iter().any(|p| p == "sd: its card or stick isn't in") && lib.away.contains(&"sd:".to_owned()), "away, not gone");
    }

    #[test]
    fn desktop_rules_move_or_hide_entries() {
        let rule = |json: &str| serde_json::from_str::<content::Rule>(json).unwrap();
        let cats = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(rule(r#"{ "name": "battle.NET", "place": "Apps" }"#).matches("net.lutris.battlenet-1", "Battle.net", &cats(&["Game"])), "any case");
        assert!(rule(r#"{ "id": "*lutris*" }"#).matches("net.lutris.Lutris", "Lutris", &[]));
        assert!(rule(r#"{ "categories": "Emu*" }"#).matches("x", "RetroArch", &cats(&["Game", "Emulator"])));
        assert!(!rule(r#"{ "id": "*lutris*", "name": "Steam" }"#).matches("net.lutris.Lutris", "Lutris", &[]), "all of them");
        assert!(rule("{}").matches("any", "Any", &[]), "nothing to match: every entry");
        let root = clamshell_core::testdir::TempDir::new("rules");
        let apps = root.join("data/applications");
        std::fs::create_dir_all(&apps).unwrap();
        let write = |id: &str, name: &str, cats: &str| {
            std::fs::write(apps.join(format!("{id}.desktop")), format!("[Desktop Entry]\nType=Application\nName={name}\nExec={id}\nCategories={cats}\n"))
                .unwrap();
        };
        write("celeste", "Celeste", "Game;");
        write("lutris", "Lutris", "Game;");
        write("uninstall-foo", "Uninstall Foo", "Utility;");
        write("foot", "Foot", "System;");
        let config = r#"{ "places": [{ "id": "apps", "path": "Apps", "recent": false }],
          "runners": [{ "type": "desktop", "name": "Desktop" }], "sources": [
            { "type": "desktop", "id": "lutris", "place": "Launchers" },
            { "type": "desktop", "id": "celeste", "place": "Games/Platformers" },
            { "type": "desktop", "name": "uninstall*", "hide": true },
            { "type": "desktop", "place": "@apps" }
        ] }"#;
        std::fs::write(root.join("library.json"), config).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), data_dirs: vec![root.join("data")], ..Sources::default() }).library;
        let tab =
            |name: &str| lib.collections.iter().find(|c| c.name == name).map(|c| c.entries.iter().map(|&i| lib.entries[i].title.clone()).collect::<Vec<_>>());
        assert_eq!(tab("Games"), Some(vec!["Platformers".to_owned()]), "in its folder");
        let folder = lib.entries.iter().find(|g| g.title == "Platformers").unwrap();
        assert_eq!(folder.items.iter().map(|&i| lib.entries[i].title.as_str()).collect::<Vec<_>>(), ["Celeste"]);
        assert_eq!(tab("Launchers"), Some(vec!["Lutris".to_owned()]));
        assert_eq!(tab("Apps"), Some(vec!["Foot".to_owned()]), "Uninstall Foo hidden; the rest taken by the last");
        let entry = |title: &str| lib.entries.iter().find(|g| g.title == title).unwrap();
        assert!(entry("Foot").app && !entry("Celeste").app, "Apps says recent: false");
    }

    #[test]
    fn runners_fill_folders_in_a_tab_each_with_its_own_launch() {
        let root = clamshell_core::testdir::TempDir::new("folders");
        for f in ["snes/Zelda.sfc", "snes/Mario.sfc", "snes/cover.png", "gba/Metroid.gba", "pc/Doom.sh"] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let source = |group: &str, id: &str, pattern: &str, emulator: &str| {
            let place = match group {
                "" => "Games".to_owned(),
                g if g.starts_with('@') => g.to_owned(),
                g => format!("Games/{g}"),
            };
            (
                format!(r#"{{ "name": "Test {id}", "match": ["{pattern}"], "launch": ["{emulator}", "{{path}}"] }}"#),
                format!(r#"{{ "type": "dir", "place": "{place}", "id": "{id}", "dir": "{}/{id}", "match": ["{pattern}"] }}"#, root.display()),
            )
        };
        let (snes, gba, pc) = (source("SNES", "snes", "*.sfc", "snes9x"), source("@gba", "gba", "*.gba", "mgba"), source("", "pc", "*.sh", "sh"));
        // (The PC scan names its runner; the others' files open by their names.)
        let pc_scan = pc.1.replace(r#""match""#, r#""runner": "test-pc", "match""#);
        let config = format!(
            r#"{{ "places": [{{ "id": "other", "path": "Other", "icon": "star" }}, {{ "id": "games", "path": "Games", "icon": "film" }},
                {{ "id": "gba", "path": "Games/Handhelds/GBA", "icon": "gba.png", "picture": "~/gba-large.png" }},
                {{ "id": "tools", "path": "Mine", "type": "tools", "icon": "tools" }}],
              "runners": [ {}, {}, {} ], "sources": [ {}, {}, {} ] }}"#,
            snes.0, gba.0, pc.0, snes.1, gba.1, pc_scan
        );
        std::fs::write(root.join("library.json"), config).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), ..Sources::default() }).library;
        // (The test's emulators aren't installed: only that is wrong.)
        assert!(lib.problems.iter().all(|p| p.ends_with("isn't installed")), "{:?}", lib.problems);
        assert_eq!(lib.collections.last().map(|c| c.name.as_str()), Some("Mine"), "ClamShell's tools, named and placed as declared");
        assert_eq!((lib.collections[0].name.as_str(), lib.collections[0].icon.as_deref()), ("Games", Some("film")), "listed: its icon; Other has nothing");
        let games = |ids: &[usize]| ids.iter().map(|&i| lib.entries[i].title.clone()).collect::<Vec<_>>();
        let tab = lib.collections.iter().find(|c| c.name == "Games").unwrap();
        assert_eq!(games(&tab.entries), ["Handhelds", "SNES", "Doom"], "folders first");
        let snes = &lib.entries[tab.entries[1]];
        assert_eq!(games(&snes.items), ["Mario", "Zelda"]);
        assert_eq!(snes.box_front, Some(root.join("snes/cover.png")), "its source's dir's art");
        let zelda = &lib.entries[snes.items[1]];
        assert!(matches!(&zelda.launch, Launch::Program { argv, .. } if argv[0] == "snes9x"));
        assert_eq!(&*zelda.id, "snes:Zelda.sfc", "ids as without the folder");
        let handhelds = &lib.entries[tab.entries[0]];
        let gba = &lib.entries[handhelds.items[0]];
        assert_eq!((gba.title.as_str(), games(&gba.items)), ("GBA", vec!["Metroid".to_owned()]));
        assert_eq!(&*gba.id, "place:gba", "a declared place's id");
        assert_eq!(gba.icon.as_deref(), Some(root.join("gba.png").as_path()), "its icon, from library.json's dir");
        assert_eq!(handhelds.id.as_ref(), "folder:Games/Handhelds", "one not declared: its path");
        assert!(matches!(&lib.entries[gba.items[0]].launch, Launch::Program { argv, .. } if argv[0] == "mgba"));
        assert_eq!(lib.groups_of(gba.items[0]), [tab.entries[0], handhelds.items[0]]);
    }
}
