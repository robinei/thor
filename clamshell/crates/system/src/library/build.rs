//! Building the library from library.json's sources: the folders made in the tabs, and each kind of
//! source scanned, placed and checked.

use super::*;

/// The folders made in the tabs, by tab and path, and library.json's places with their ids, icons and
/// pictures. Image file paths in the places are relative to `config_dir`, or to the home dir with `~/`.
pub(super) struct Folders<'a> {
    pub(super) made: HashMap<(String, String), usize>,
    pub(super) places: &'a [content::Placed],
    pub(super) home: Option<&'a Path>,
    pub(super) config_dir: Option<&'a Path>,
}

impl Folders<'_> {
    /// Puts `entries`, from a scan of `dir`, in the folder at `path` in `tab`, where `A/B` is B inside A.
    /// Folders that don't exist yet are made. Their art comes from a place declared there, or else from
    /// `dir`. Returns the folder at the top of the tab if it is new, for the caller to add to the tab.
    pub(super) fn put(&mut self, games: &mut Vec<Entry>, tab: &str, path: &str, dir: &Path, entries: &[usize]) -> Option<usize> {
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

    /// A new folder at `at` in `tab`, titled `title`. If a place is declared there, it gives the folder's
    /// id (`place:<id>`), icon and picture. Otherwise the id is the path (`folder:<tab>/<at>`).
    pub(super) fn folder(&self, tab: &str, at: &str, title: &str) -> Entry {
        let path = format!("{tab}/{at}");
        let Some(place) = self.places.iter().find(|p| p.path.trim().trim_matches('/') == path) else {
            return Entry::new(&format!("{}{path}", ids::FOLDER), title);
        };
        let file = |f: &str| match (f.strip_prefix("~/"), self.home, self.config_dir) {
            (Some(rest), Some(home), _) => home.join(rest),
            (None, _, Some(dir)) if !f.starts_with('/') => dir.join(f),
            _ => PathBuf::from(f),
        };
        let mut g = Entry::new(&format!("{}{}", ids::PLACE, place.id), title);
        g.box_front = place.picture.as_deref().map(file);
        match place.icon.as_deref() {
            // Either an image file or one of the UI's icons by name.
            Some(icon) if icon.contains(['/', '.']) => g.icon = Some(file(icon)),
            Some(icon) => g.glyph = Some(icon.into()),
            None => {}
        }
        g
    }
}

/// The library while it is read: its entries and tabs, the folders made, where each entry's metadata
/// comes from, the problems, the sources that are away, and the mounted removable filesystems.
pub(super) struct Builder<'a> {
    pub(super) config: &'a content::Config,
    /// The declared places with their paths.
    pub(super) places: &'a [content::Placed],
    pub(super) runners: &'a [runners::Runner],
    pub(super) sources: &'a Sources,
    pub(super) games: Vec<Entry>,
    pub(super) collections: Vec<Collection>,
    pub(super) folders: Folders<'a>,
    pub(super) hows: Vec<(usize, How)>,
    pub(super) problems: Vec<String>,
    pub(super) away: Vec<String>,
    pub(super) mounts: Vec<crate::files::mounts::Mount>,
}

impl Builder<'_> {
    /// Reports tabs with a picture or an image icon. A tab shows only an icon from the UI's set, because
    /// something in it is always selected.
    pub(super) fn check_tabs(&mut self) {
        for p in self.places.iter().filter(|p| !p.path.trim().trim_matches('/').contains('/')) {
            if p.picture.is_some() || p.icon.as_deref().is_some_and(|i| i.contains(['/', '.'])) {
                self.problems.push(format!("{}: a tab has an icon of the UI's (gamepad, film, ...), no picture", p.path));
            }
        }
    }

    /// The path of the place declared as `id`. If there is none, the problem is recorded and none is
    /// returned.
    pub(super) fn resolve(&mut self, place: &str) -> Option<String> {
        content::resolve(place, self.places).map_err(|e| self.problems.push(e)).ok()
    }

    /// Puts `entries` at the resolved `place`: in its tab, or in a folder in it that takes its art from
    /// `dir`.
    pub(super) fn put(&mut self, place: &str, dir: &Path, entries: Vec<usize>) {
        let (tab, path) = content::place(place);
        // A folder is made with nothing in it too: shown dimmed, saying what goes there.
        let entries = match path {
            "" => entries,
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

    /// Where a source's `metadata` says its entries' covers and details come from. Problems with it are
    /// recorded.
    pub(super) fn how(&mut self, id: &str, metadata: Option<&str>) -> Option<How> {
        let m = metadata?;
        let Some(how) = How::parse(m) else {
            self.problems.push(format!("{id}: metadata \"{m}\" isn't libretro:<system>, tmdb:movie, tmdb:tv, steamgriddb or steam"));
            return None;
        };
        // Without the key nothing is asked. That isn't recorded as a miss, so it is fetched once the key is
        // there.
        let keys = &self.config.keys;
        match how {
            How::Tmdb(_) if keys.tmdb.is_none() => self.problems.push(format!("{id}: tmdb needs keys.tmdb")),
            How::SteamGridDb if keys.steamgriddb.is_none() => self.problems.push(format!("{id}: steamgriddb needs keys.steamgriddb")),
            how => return Some(how),
        }
        None
    }

    /// Scans a dir source and puts its entries at its place. Each opens with the source's runner, or the
    /// runner matching its file name.
    pub(super) fn scan(&mut self, source: &content::Scan) {
        let runners = self.runners;
        let home = self.sources.home.as_deref();
        // The runner the source names opens all its entries, and its patterns are used unless the source
        // has its own. Otherwise each entry opens with the runner matching its file name.
        let named = match source.runner.as_deref().map(|id| runners.iter().find(|r| r.id == id).ok_or(id)) {
            Some(Err(id)) => return self.problems.push(format!("{}: no runner {id}", source.id)),
            Some(Ok(r)) => Some(r),
            None => None,
        };
        let mut patterns = match named {
            Some(r) if source.patterns.is_empty() => r.patterns.clone(),
            _ => source.patterns.clone(),
        };
        patterns.extend(source.also.iter().cloned());
        let open = |path: &Path| match named {
            Some(r) => Some((r.command(path, home), r.of(home))),
            None => runners::open(runners, path, home),
        };
        // The dir is either on a removable filesystem, away while no matching one is mounted, or as given.
        let dir = match &source.on {
            Some(on) => match self.mounts.iter().find(|m| content::on_matches(on, &m.label, &m.uuid, &m.fs)) {
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
        let entries = content::scan(&scan, &mut self.games, &open);
        if let Some(how) = self.how(&source.id, source.metadata.as_deref()) {
            // libretro has games, not folders of them.
            let games = &self.games;
            let fits = |i: &usize| !matches!(how, How::Libretro(_)) || games.get(*i).is_some_and(|g| !g.is_group());
            let fitting: Vec<usize> = entries.iter().copied().filter(fits).collect();
            self.hows.extend(fitting.into_iter().map(|i| (i, how.clone())));
        }
        let Some(place) = self.resolve(&source.place) else {
            return;
        };
        let empty = entries.is_empty();
        self.put(&place, &dir, entries);
        // The folder says where its files go while it has none, and which BIOS files its runner lacks.
        let (tab, path) = content::place(&place);
        let missing = named.map(|r| self.missing_bios(r)).unwrap_or_default();
        if let Some(folder) = self.folders.made.get(&(tab.to_owned(), path.to_owned())).and_then(|&f| self.games.get_mut(f)) {
            let summary = &mut folder.details.summary;
            if empty {
                summary.push_str(&format!("Nothing here yet: its files go in {}.", dir.display()));
            }
            for (file, bios) in missing {
                if !summary.is_empty() {
                    summary.push('\n');
                }
                summary.push_str(&format!("Needs {file} in {}.", bios.display()));
            }
        }
    }

    /// A browse source: puts its dir's own folder in its place, to be listed when it is opened.
    pub(super) fn browse(&mut self, root: &content::BrowseRoot) {
        let Some(within) = self.resolve(&root.within) else {
            return;
        };
        let place = format!("{}/{}", within.trim().trim_matches('/'), root.name.trim());
        let dir = match content::expand_dir(&root.dir, self.sources.home.as_deref()) {
            Ok(dir) if dir.is_dir() || root.trash => dir,
            Ok(dir) => return self.problems.push(format!("{place}: no folder {}", dir.display())),
            Err(name) => return self.problems.push(format!("{place}: ${name} isn't set")),
        };
        // The Trash's folder may not exist yet.
        if root.trash
            && let Err(e) = std::fs::create_dir_all(&dir)
        {
            log::warn!("library: {}: {e}", dir.display());
        }
        let kind = if root.trash { BrowseKind::Trash } else { BrowseKind::Root };
        self.browsed_root(&place, browsed_root(&format!("{}{place}", ids::BROWSE), dir, kind));
    }

    /// Adds the mounted removable filesystems, each taken by the first removable source matching it.
    pub(super) fn removable(&mut self) {
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
            let Some(rule) = rules.iter().find(|r| content::mount_matches(r, &m.label, &m.uuid, &m.fs)) else {
                continue;
            };
            let Some(within) = rule.within.as_deref().filter(|_| !rule.hide) else {
                continue;
            };
            let label = if m.label.is_empty() { &m.uuid } else { &m.label };
            let Some(within) = self.resolve(within) else {
                continue;
            };
            let place = format!("{within}/{}", rule.name.as_deref().unwrap_or("{label}").replace("{label}", label));
            let id = format!("{}media/{}", ids::BROWSE, if m.uuid.is_empty() { &m.device } else { &m.uuid });
            self.browsed_root(&place, browsed_root(&id, m.dir, BrowseKind::Removable(m.device)));
        }
    }

    /// Adds a browsed dir's folder at `place`. The last part of `place` is the folder's title, and the rest
    /// is where it goes.
    pub(super) fn browsed_root(&mut self, place: &str, mut root: Entry) {
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

    /// Adds Steam's installed games and the apps and games from desktop files, games first. Each is taken
    /// by the first source of its type that matches it, which puts it at its place, sets its runner and
    /// says where its metadata comes from. It is left out if that source has `hide` or none matches.
    pub(super) fn installed(&mut self) {
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
        // Report each of these problems only once.
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
            for d in crate::library::desktop::entries(&self.sources.data_dirs) {
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
            // The runner is the one the source names, or else the first of its type.
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
            // A desktop runner's own command, if it has one, replaces the desktop file's.
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

    /// Puts each folder's entries, from its sources and its subfolders, and each tab's entries in list
    /// order. Tabs without entries are dropped.
    pub(super) fn order(&mut self) {
        let games = &self.games;
        let mut sorted: Vec<(usize, Vec<usize>)> = Vec::new();
        for &f in self.folders.made.values() {
            let mut items = games.get(f).map(|g| g.items.clone()).unwrap_or_default();
            items.sort_by(|&a, &b| clamshell_library::sort::list_order(games, a, b));
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
            c.entries.sort_by(|&a, &b| clamshell_library::sort::list_order(games, a, b));
        }
    }

    /// The declared place that is tab `name`, if any.
    pub(super) fn tab_place(&self, name: &str) -> Option<&content::Placed> {
        self.places.iter().find(|p| p.path.trim().trim_matches('/') == name)
    }

    /// Marks what is left out of Recent: anything in a place, or a folder of one, with `recent: false`.
    /// Also sets the tabs' icons from their places.
    pub(super) fn mark_recent(&mut self) {
        let places = &self.places;
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
                let declared = self.games.get(i).and_then(|g| g.id.strip_prefix(ids::PLACE)).and_then(|id| places.iter().find(|p| p.id == id));
                let recent = declared.and_then(|p| p.recent).unwrap_or(inherited);
                if let Some(g) = self.games.get_mut(i) {
                    g.app = !recent;
                    stack.extend(g.items.iter().map(|&item| (item, recent)));
                }
            }
        }
    }

    /// One of ClamShell's own tabs, as a place declares it.
    pub(super) fn own(&self, kind: content::OwnTab) -> Option<clamshell_library::OwnTab> {
        let p = self.places.iter().find(|p| p.kind == Some(kind))?;
        Some(clamshell_library::OwnTab { name: p.path.trim().trim_matches('/').to_owned(), icon: p.icon.clone() })
    }

    /// Records a problem for each program ClamShell runs itself that isn't installed, saying what doesn't
    /// work without it.
    pub(super) fn needed(&mut self) {
        for (program, for_what) in NEEDED {
            if !runners::on_path(program) {
                self.problems.push(format!("{program} isn't installed: {for_what}"));
            }
        }
    }

    /// Places one of ClamShell's own screens, as a `builtin` source says.
    pub(super) fn builtin(&mut self, source: &content::BuiltinPlace) {
        let Some(builtin) = clamshell_library::BuiltinApp::ENTRIES.into_iter().find(|app| app.name() == source.id.trim()) else {
            let names: Vec<&str> = clamshell_library::BuiltinApp::ENTRIES.map(clamshell_library::BuiltinApp::name).into();
            return self.problems.push(format!("builtin {}: not one of ClamShell's screens ({})", source.id, names.join(", ")));
        };
        if self.games.iter().any(|g| g.launch == Launch::Builtin(builtin)) {
            return self.problems.push(format!("builtin {}: placed twice", source.id));
        }
        let Some(place) = self.resolve(&source.place) else {
            return;
        };
        self.games.push(builtin_entry(builtin));
        let i = self.games.len() - 1;
        self.put(&place, Path::new(""), vec![i]);
    }

    /// Adds ClamShell's own screens no source placed, in no place: they are entries all the same, the
    /// Library always on the dock.
    pub(super) fn unplaced_builtins(&mut self) {
        for builtin in clamshell_library::BuiltinApp::ENTRIES {
            if !self.games.iter().any(|g| g.launch == Launch::Builtin(builtin)) {
                self.games.push(builtin_entry(builtin));
            }
        }
    }

    /// Records a problem for each BIOS file a runner needs that isn't where its emulator looks.
    pub(super) fn bios(&mut self) {
        let runners = self.runners;
        for r in runners {
            if let Some(Err(var)) = r.bios.as_ref().map(|b| content::expand_dir(&b.dir, self.sources.home.as_deref())) {
                self.problems.push(format!("{}: its BIOS folder needs ${var}", r.title()));
            }
            for (file, dir) in self.missing_bios(r) {
                self.problems.push(format!("{}: its BIOS {file} isn't in {}", r.title(), dir.display()));
            }
        }
    }

    /// The BIOS files runner `r` needs that aren't where its emulator looks, each with that folder.
    pub(super) fn missing_bios(&self, r: &runners::Runner) -> Vec<(String, PathBuf)> {
        let Some(bios) = &r.bios else { return Vec::new() };
        let Ok(dir) = content::expand_dir(&bios.dir, self.sources.home.as_deref()) else { return Vec::new() };
        bios.files.iter().filter(|f| !dir.join(f).is_file()).map(|f| (f.clone(), dir.clone())).collect()
    }

    /// Adds ClamShell's own tools in their tab, if a place declares it, after what sources put there. The
    /// rescan entry's details list the library's current problems.
    pub(super) fn tools(&mut self) {
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
        let tools = first..self.games.len();
        match self.collections.iter_mut().find(|c| c.name == tab.name) {
            Some(c) => c.entries.extend(tools),
            None => self.collections.push(Collection { icon: tab.icon, ..Collection::new(&tab.name, tools.collect()) }),
        }
    }
}

/// The entry of one of ClamShell's own screens.
pub(super) fn builtin_entry(builtin: clamshell_library::BuiltinApp) -> Entry {
    let details = Details { summary: builtin.about().into(), ..Details::default() };
    Entry { launch: Launch::Builtin(builtin), app: true, details, ..Entry::new(builtin.id(), builtin.title()) }
}

/// The first of `rules` matching an entry with this id, name and categories.
pub(super) fn first_match<'r>(rules: &[&'r content::Rule], id: &str, name: &str, categories: &[String]) -> Option<&'r content::Rule> {
    rules.iter().copied().find(|r| content::rule_matches(r, id, name, categories))
}
