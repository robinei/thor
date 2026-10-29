//! What an entry's windows are known by when it runs in another program's processes, which makes them its
//! windows wherever they open: the mark a game launcher gives its games' processes, or the app id of an
//! app its runner hands over (such as Waydroid's). ClamShell's state follows launchers with them
//! (clamshell-state's `session::launchers`, which also reads a process's mark).
//!
//! The launchers' marks, the strongest first:
//!
//! - Steam: `SteamAppId` in the environment, which Steam gives its games. The mark is `steam_app_<id>`,
//!   which is also the X11 class of Steam's Proton games; the entry is Steam's game.
//! - Heroic: `HEROIC_APP_NAME` and `HEROIC_APP_RUNNER` in the environment, which Heroic gives its games
//!   for its users' scripts. The mark is `heroic:<runner>:<name>`; the entry is the desktop file whose
//!   command opens `heroic://launch?appName=<name>&runner=<runner>`, as Heroic's shortcuts do.
//! - Lutris: the `lutris-wrapper` process Lutris runs a game under, whose first argument is the game's
//!   name. The mark is `lutris:<name>`; the entry is the desktop file whose command opens
//!   `lutris:rungameid/` and whose name that is, as Lutris's shortcuts are. The wrapper's arguments are
//!   Lutris's own rather than meant to be read, and names can repeat: a wrapper not like this, or two such
//!   entries of one name, mark nothing, and the game's windows are Lutris's.

use std::fmt;
use std::sync::Arc;

use crate::{Entry, Launch, Library, ids};

/// Steam's app id for a game's windows, and the mark of its processes: `steam_app_<id>`.
pub const STEAM: &str = "steam_app_";
pub const HEROIC: &str = "heroic:";
pub const LUTRIS: &str = "lutris:";

/// A handed-over entry's app id: its desktop file's name.
pub fn handed_over_app(g: &Entry) -> Option<&str> {
    ids::desktop_file(&g.id).filter(|_| g.runner.as_ref().is_some_and(|r| r.handover))
}

/// The runner and the game's name in a Heroic launch URL: `heroic://launch?appName=<name>&runner=<runner>`.
pub fn heroic_game(arg: &str) -> Option<(&str, &str)> {
    let query = arg.strip_prefix("heroic://launch?")?;
    let value = |key: &str| query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='));
    Some((value("runner")?, value("appName")?))
}

/// Whether `g` is a Lutris shortcut: its command opens `lutris:rungameid/`.
pub fn is_lutris(g: &Entry) -> bool {
    matches!(&g.launch, Launch::Program { argv, .. } if argv.iter().any(|a| a.starts_with("lutris:rungameid/")))
}

/// Gives `mark` each mark that [`Library::is_marked`] or a handed-over app id might match `g` by: what
/// the index files `g` under.
pub(crate) fn each_mark(g: &Entry, mut mark: impl FnMut(fmt::Arguments<'_>)) {
    if let Some(app) = handed_over_app(g) {
        mark(format_args!("{app}"));
        return;
    }
    match &g.launch {
        Launch::Steam { appid } => mark(format_args!("{STEAM}{appid}")),
        Launch::Program { argv, .. } => {
            for (runner, name) in argv.iter().filter_map(|a| heroic_game(a)) {
                mark(format_args!("{HEROIC}{runner}:{name}"));
            }
            if is_lutris(g) {
                mark(format_args!("{LUTRIS}{}", g.title));
            }
        }
        Launch::None | Launch::Tool(_) | Launch::Builtin(_) => {}
    }
}

impl Library {
    /// What entry `g`'s windows are known by when it runs in another program's processes. For a
    /// launcher's game it is the mark the launcher gives its processes ([`Library::launcher_mark`]),
    /// Steam's `steam_app_<id>` also its windows' app id. For an entry whose runner hands its apps over,
    /// such as Waydroid's, it is its windows' app id, the desktop file's name. It is none for the rest,
    /// which run in units of ours.
    pub fn mark_of(&self, g: &Entry) -> Option<Arc<str>> {
        handed_over_app(g).map(Into::into).or_else(|| self.launcher_mark(g).map(Into::into))
    }

    /// Whether a window with app id `app_id`, whose process is marked `game` (by the launcher that started
    /// it), is marked as [`Library::mark_of`] `g`, without building the mark. A handed-over app is known
    /// by its app id alone, a launcher's game by either.
    pub fn has_mark(&self, g: &Entry, app_id: &str, game: Option<&str>) -> bool {
        match handed_over_app(g) {
            Some(app) => app_id == app,
            None => [Some(app_id), game].into_iter().flatten().any(|mark| self.is_marked(g, mark)),
        }
    }

    /// The first entry that [`Library::has_mark`] says a window with app id `app_id` and process mark
    /// `game` is marked as.
    pub fn entry_by_mark(&self, app_id: &str, game: Option<&str>) -> Option<&Entry> {
        let entries = self.entries();
        let by_app_id = self.index().mark(entries, app_id, |g| match handed_over_app(g) {
            Some(app) => app == app_id,
            None => self.is_marked(g, app_id),
        });
        let by_game = game.and_then(|game| self.index().mark(entries, game, |g| handed_over_app(g).is_none() && self.is_marked(g, game)));
        let first = match (by_app_id, by_game) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        entries.get(first?)
    }

    /// The mark of entry `g`'s game, if a launcher starts it (see [`crate::marks`]).
    pub fn launcher_mark(&self, g: &Entry) -> Option<String> {
        match &g.launch {
            Launch::Steam { appid } => Some(format!("{STEAM}{appid}")),
            Launch::Program { argv, .. } => match argv.iter().find_map(|a| heroic_game(a)) {
                Some((runner, name)) => Some(format!("{HEROIC}{runner}:{name}")),
                None => self.lutris_unique(g).then(|| format!("{LUTRIS}{}", g.title)),
            },
            Launch::None | Launch::Tool(_) | Launch::Builtin(_) => None,
        }
    }

    /// Whether [`Library::launcher_mark`] of `g` is `mark`, without building it.
    pub fn is_marked(&self, g: &Entry, mark: &str) -> bool {
        match &g.launch {
            Launch::Steam { appid } => mark.strip_prefix(STEAM).and_then(|n| n.parse::<u32>().ok()) == Some(*appid),
            Launch::Program { argv, .. } => match mark.strip_prefix(HEROIC) {
                Some(game) => argv.iter().filter_map(|a| heroic_game(a)).any(|game_of| game.split_once(':') == Some(game_of)),
                None => mark.strip_prefix(LUTRIS) == Some(g.title.as_str()) && self.lutris_unique(g),
            },
            Launch::None | Launch::Tool(_) | Launch::Builtin(_) => false,
        }
    }

    /// Whether `g` is a Lutris shortcut, and the only one of its name. The others of its name are among
    /// the entries filed under its mark, so only those are compared.
    fn lutris_unique(&self, g: &Entry) -> bool {
        is_lutris(g) && !self.index().any_marked(self.entries(), format_args!("{LUTRIS}{}", g.title), |o| o.id != g.id && o.title == g.title && is_lutris(o))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn shortcut(id: &str, title: &str, argv: &[&str]) -> Entry {
        Entry { launch: Launch::program(argv.iter().map(|a| (*a).to_owned()).collect()), ..Entry::new(id, title) }
    }

    fn library(entries: Vec<Entry>) -> Library {
        Library::new(Vec::new(), entries)
    }

    #[test]
    fn an_entry_is_marked_by_its_launchers_shortcut() {
        let heroic = shortcut("desktop:Pattern Game", "Pattern Game", &["xdg-open", "heroic://launch?appName=patterngame&runner=sideload"]);
        let lutris = shortcut("desktop:net.lutris.pattern-game-1", "Pattern Game", &["env", "LUTRIS_SKIP_INIT=1", "lutris", "lutris:rungameid/1"]);
        let steam = Entry { launch: Launch::Steam { appid: 620 }, ..Entry::new("steam:620", "Portal 2") };
        let other = shortcut("desktop:foot", "Terminal", &["foot"]);
        let all = library(vec![heroic.clone(), lutris.clone(), steam.clone(), other.clone()]);
        for (g, mark) in
            [(&heroic, Some("heroic:sideload:patterngame")), (&lutris, Some("lutris:Pattern Game")), (&steam, Some("steam_app_620")), (&other, None)]
        {
            assert_eq!(all.launcher_mark(g).as_deref(), mark, "{}", g.id);
            if let Some(mark) = mark {
                assert!(all.is_marked(g, mark), "{}", g.id);
                assert_eq!(all.entry_by_mark("other", Some(mark)).map(|e| &e.id), Some(&g.id), "{}", g.id);
            }
        }
        assert!(!all.is_marked(&heroic, "heroic:gog:patterngame") && !all.is_marked(&steam, "steam_app_6200"));
        // Two Lutris shortcuts of one name: neither is known by it.
        let twin = shortcut("desktop:net.lutris.pattern-game-2", "Pattern Game", &["lutris", "lutris:rungameid/2"]);
        let twins = library(vec![lutris.clone(), twin]);
        assert_eq!(twins.launcher_mark(&lutris), None);
        assert!(!twins.is_marked(&lutris, "lutris:Pattern Game"));
        assert_eq!(twins.entry_by_mark("lutris:Pattern Game", Some("lutris:Pattern Game")), None);
    }

    #[test]
    fn has_mark_agrees_with_mark_of() {
        let steam = Entry { launch: Launch::Steam { appid: 620 }, ..Entry::new("steam:620", "Portal 2") };
        let handed = Entry {
            runner: Some(crate::RunnerOf { handover: true, ..crate::RunnerOf::new("android", "Android", false) }),
            ..Entry::new("desktop:org.app", "App")
        };
        let heroic = shortcut("desktop:X", "X", &["xdg-open", "heroic://launch?appName=x&runner=gog"]);
        let ours = Entry::new("desktop:foot", "Terminal");
        let library = library(vec![steam.clone(), handed.clone(), heroic.clone(), ours.clone()]);
        let marks = ["steam_app_620", "steam_app_62", "org.app", "foot", "heroic:gog:x", ""];
        for g in [&steam, &handed, &heroic, &ours] {
            for mark in marks {
                // A handed-over app is known by its app id alone; a launcher's game by either.
                let expected = library.mark_of(g).as_deref() == Some(mark);
                assert_eq!(library.has_mark(g, mark, None), expected, "{} app id {mark}", g.id);
                assert_eq!(library.has_mark(g, "other", Some(mark)), expected && g.id != handed.id, "{} process {mark}", g.id);
            }
        }
    }
}
