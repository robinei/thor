//! The names ClamShell gives sway workspaces and systemd scopes. A dock app instance is a scope we
//! started (`clamshell-app-<key>-<secs>-<n>.scope`); all of an app's instances' windows are on the app's
//! workspace (`clamshell-app-<key>`), as its tabs. A game shelf started has a scope and a workspace of the
//! same name (`clamshell-game-<secs>-<n>`); a game Steam started has a workspace only
//! (`clamshell-game-steam-<appid>`), its windows (`steam_app_<appid>`) moved there from the landing
//! workspace (`clamshell-game`) where a sway rule puts them. Shelf's window has `clamshell-shelf`, the
//! second screen `clamshell-bottom`. `<secs>-<n>` is the start time and a counter, so names are unique
//! across restarts and sort in the order things were started.

use std::fmt::{self, Write};
use std::str::FromStr;

/// Every name of ours starts with this.
const PREFIX: &str = "clamshell-";
const SCOPE: &str = ".scope";
/// Steam's app id for a game's windows: `steam_app_<appid>`.
pub const STEAM_APP_PREFIX: &str = "steam_app_";

/// A workspace (and, for apps and games we started, a scope) of ours.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Instance {
    /// Shelf's window.
    Shelf,
    /// The second screen.
    Bottom,
    /// Where Steam's game windows land before they are moved to their game's workspace.
    Landing,
    /// A dock app's instance: the app's key (see [`crate::apps::App::key`]), the start time, a counter.
    App { key: String, secs: u64, n: u64 },
    /// A game shelf started.
    Game { secs: u64, n: u64 },
    /// A game Steam started, by Steam's app id.
    Steam(u32),
}

/// A name of ours, borrowed: [`Instance`] without allocating the app's key, for checks while drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Name<'a> {
    Shelf,
    Bottom,
    Landing,
    App {
        key: &'a str,
        secs: u64,
        n: u64,
    },
    /// An app's workspace.
    AppSpace(&'a str),
    Game {
        secs: u64,
        n: u64,
    },
    Steam(u32),
}

impl<'a> Name<'a> {
    fn parse(name: &'a str) -> Option<Name<'a>> {
        let rest = name.strip_prefix(PREFIX)?;
        match rest {
            "shelf" => return Some(Name::Shelf),
            "bottom" => return Some(Name::Bottom),
            "game" => return Some(Name::Landing),
            _ => {}
        }
        if let Some(rest) = rest.strip_prefix("app-") {
            // (A key has no '-': an app's workspace is its key alone.)
            if !rest.is_empty() && !rest.contains('-') {
                return Some(Name::AppSpace(rest));
            }
            let mut parts = rest.rsplitn(3, '-');
            let n = parts.next()?.parse().ok()?;
            let secs = parts.next()?.parse().ok()?;
            let key = parts.next().filter(|k| !k.is_empty())?;
            return Some(Name::App { key, secs, n });
        }
        let rest = rest.strip_prefix("game-")?;
        if let Some(appid) = rest.strip_prefix("steam-") {
            return appid.parse().ok().map(Name::Steam);
        }
        let (secs, n) = rest.split_once('-')?;
        Some(Name::Game { secs: secs.parse().ok()?, n: n.parse().ok()? })
    }
}

impl Instance {
    /// The instance `name` (a workspace's or a scope's, without `.scope`) stands for, if it is ours (an
    /// app's workspace is none: its instances share it).
    pub fn parse(name: &str) -> Option<Instance> {
        Some(match Name::parse(name)? {
            Name::AppSpace(_) => return None,
            Name::Shelf => Instance::Shelf,
            Name::Bottom => Instance::Bottom,
            Name::Landing => Instance::Landing,
            Name::App { key, secs, n } => Instance::App { key: key.to_owned(), secs, n },
            Name::Game { secs, n } => Instance::Game { secs, n },
            Name::Steam(appid) => Instance::Steam(appid),
        })
    }

    /// The app or game whose scope `unit` (`<name>.scope`) is, if we started it.
    pub fn from_unit(unit: &str) -> Option<Instance> {
        Instance::parse(unit.strip_suffix(SCOPE)?).filter(|i| matches!(i, Instance::App { .. } | Instance::Game { .. }))
    }

    /// Whether `unit` is the scope of a game we started (without allocating).
    pub fn is_game_unit(unit: &str) -> bool {
        unit.strip_suffix(SCOPE).and_then(Name::parse).is_some_and(|n| matches!(n, Name::Game { .. }))
    }

    /// The key of the dock app whose instance or workspace `name` is (without allocating).
    pub fn app_key_of(name: &str) -> Option<&str> {
        match Name::parse(name)? {
            Name::App { key, .. } | Name::AppSpace(key) => Some(key),
            _ => None,
        }
    }

    /// Dock app `key`'s workspace, its instances' windows' tabs.
    pub fn app_workspace(key: &str) -> String {
        format!("{PREFIX}app-{key}")
    }

    /// Whether `name` is one of ours (a workspace or scope ClamShell names, known kind or not).
    pub fn is_ours(name: &str) -> bool {
        name.starts_with(PREFIX)
    }

    /// Its name: its scope's (without `.scope`), and but for an app's instance its workspace's.
    pub fn name(&self) -> String {
        self.to_string()
    }

    /// The workspace its windows are on: its app's for an app's instance, else its own name's.
    pub fn workspace(&self) -> String {
        match self {
            Instance::App { key, .. } => Instance::app_workspace(key),
            _ => self.name(),
        }
    }

    /// Whether workspace `name` is the one its windows are on, compared without allocating.
    pub fn on(&self, name: &str) -> bool {
        match self {
            Instance::App { key, .. } => name.strip_prefix(PREFIX).and_then(|r| r.strip_prefix("app-")) == Some(key.as_str()),
            _ => self.is(name),
        }
    }

    /// Its scope's unit name, for the apps and games we start in one.
    pub fn unit(&self) -> Option<String> {
        matches!(self, Instance::App { .. } | Instance::Game { .. }).then(|| format!("{self}{SCOPE}"))
    }

    /// Its place in the order things were started: the start time and the counter.
    pub fn order(&self) -> (u64, u64) {
        match self {
            Instance::App { secs, n, .. } | Instance::Game { secs, n } => (*secs, *n),
            _ => (0, 0),
        }
    }

    /// The dock app key, for an app's instance.
    pub fn app_key(&self) -> Option<&str> {
        match self {
            Instance::App { key, .. } => Some(key),
            _ => None,
        }
    }

    /// Whether `name` is this instance's name, compared without allocating (for drawing).
    pub fn is(&self, name: &str) -> bool {
        let mut rest = Rest(Some(name));
        let _ = write!(rest, "{self}");
        rest.0 == Some("")
    }
}

/// Steam's id of the game a window with this app id belongs to (`steam_app_<id>`).
pub fn steam_app_id(app_id: &str) -> Option<u32> {
    app_id.strip_prefix(STEAM_APP_PREFIX).and_then(|n| n.parse().ok())
}

/// What is left of a name to compare against, as [`Instance::is`] writes the instance's name piece
/// by piece; `None` once a piece didn't match.
struct Rest<'a>(Option<&'a str>);

impl Write for Rest<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 = self.0.and_then(|rest| rest.strip_prefix(s));
        Ok(())
    }
}

impl fmt::Display for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PREFIX)?;
        match self {
            Instance::Shelf => f.write_str("shelf"),
            Instance::Bottom => f.write_str("bottom"),
            Instance::Landing => f.write_str("game"),
            Instance::App { key, secs, n } => write!(f, "app-{key}-{secs}-{n}"),
            Instance::Game { secs, n } => write!(f, "game-{secs}-{n}"),
            Instance::Steam(appid) => write!(f, "game-steam-{appid}"),
        }
    }
}

/// A name that isn't one of ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotOurs;

impl FromStr for Instance {
    type Err = NotOurs;

    fn from_str(name: &str) -> Result<Instance, NotOurs> {
        Instance::parse(name).ok_or(NotOurs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_back() {
        let all = [
            Instance::Shelf,
            Instance::Bottom,
            Instance::Landing,
            Instance::App { key: "org_foot".into(), secs: 1234, n: 7 },
            Instance::Game { secs: 99, n: 1 },
            Instance::Steam(620),
        ];
        for i in all {
            assert_eq!(Instance::parse(&i.name()), Some(i.clone()), "{i}");
            assert!(i.is(&i.name()) && !i.is("clamshell-") && !i.is(&format!("{i}-1")), "{i}");
        }
        assert_eq!(Instance::Game { secs: 99, n: 1 }.name(), "clamshell-game-99-1");
        assert_eq!(Instance::Steam(620).name(), "clamshell-game-steam-620");
        assert_eq!(Instance::Game { secs: 99, n: 1 }.unit().as_deref(), Some("clamshell-game-99-1.scope"));
        assert_eq!(Instance::Steam(620).unit(), None, "Steam's games have no scope of ours");
        assert_eq!(Instance::parse("clamshell-app-x-y-1"), None);
        assert_eq!(Instance::parse("1"), None);
        assert_eq!(Instance::from_unit("clamshell-app-terminal-1-0.scope").and_then(|i| i.app_key().map(str::to_owned)).as_deref(), Some("terminal"));
        assert_eq!(Instance::from_unit("clamshell-shelf.scope"), None);
        assert!(Instance::is_game_unit("clamshell-game-5-0.scope") && !Instance::is_game_unit("clamshell-app-x-5-0.scope"));
        assert!(Instance::App { key: "x".into(), secs: 5, n: 10 }.order() > Instance::App { key: "x".into(), secs: 5, n: 9 }.order());
        let app = Instance::App { key: "terminal".into(), secs: 1, n: 0 };
        assert_eq!(app.workspace(), "clamshell-app-terminal");
        assert!(app.on("clamshell-app-terminal") && !app.on(&app.name()) && !app.on("clamshell-app-terminal2"));
        assert_eq!(Instance::parse("clamshell-app-terminal"), None, "an app's workspace is no instance");
        assert_eq!(Instance::app_key_of("clamshell-app-terminal"), Some("terminal"));
        assert_eq!(steam_app_id("steam_app_620"), Some(620));
        assert_eq!("clamshell-shelf".parse(), Ok(Instance::Shelf));
    }
}
