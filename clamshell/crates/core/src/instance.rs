//! The names ClamShell gives sway workspaces, systemd scopes and slices. A launch of a library entry (a
//! game, an app: alike) is a scope we started (`clamshell-run-<key>-<secs>-<n>.scope`) in the entry's
//! slice (`clamshell-run-<key>.slice`, which Quit and Force kill end); all the entry's windows are on its
//! workspace (`clamshell-run-<key>`), as tabs. `<key>` is the entry's ([`clamshell_library::key_of`]:
//! letters, digits and `_`). A game Steam runs isn't in a scope of ours (it runs in Steam's), but its
//! windows go to its entry's workspace too. Shelf's window has `clamshell-shelf`, the second screen
//! `clamshell-bottom`; Steam's game windows land on `clamshell-game` (a sway rule puts them there).
//! `<secs>-<n>` is the start time and a counter, so names are unique across restarts and sort in the
//! order things were started.

use std::fmt::{self, Write};
use std::str::FromStr;

/// Every name of ours starts with this.
const PREFIX: &str = "clamshell-";
const RUN: &str = "clamshell-run-";
const SCOPE: &str = ".scope";
/// Steam's app id for a game's windows: `steam_app_<id>`.
pub const STEAM_APP_PREFIX: &str = "steam_app_";

/// A workspace of ours, or (a launch) a scope and the workspace its windows are on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Instance {
    /// Shelf's window.
    Shelf,
    /// The second screen.
    Bottom,
    /// Where Steam's game windows land before they are moved to their entry's workspace.
    Landing,
    /// A launch of an entry: its key, the start time, a counter.
    Run { key: String, secs: u64, n: u64 },
}

impl Instance {
    /// The instance `name` (a workspace's or a scope's, without `.scope`) stands for, if it is ours (an
    /// entry's workspace is none: its launches share it).
    pub fn parse(name: &str) -> Option<Instance> {
        match name.strip_prefix(PREFIX)? {
            "shelf" => return Some(Instance::Shelf),
            "bottom" => return Some(Instance::Bottom),
            "game" => return Some(Instance::Landing),
            _ => {}
        }
        let mut parts = name.strip_prefix(RUN)?.rsplitn(3, '-');
        let n = parts.next()?.parse().ok()?;
        let secs = parts.next()?.parse().ok()?;
        let key = parts.next().filter(|k| !k.is_empty() && !k.contains('-'))?;
        Some(Instance::Run { key: key.to_owned(), secs, n })
    }

    /// The launch whose scope `unit` (`<name>.scope`) is, if we started it.
    pub fn from_unit(unit: &str) -> Option<Instance> {
        Instance::parse(unit.strip_suffix(SCOPE)?).filter(|i| matches!(i, Instance::Run { .. }))
    }

    /// The entry key in `name`: a launch's scope (with or without `.scope`), or an entry's workspace
    /// (without allocating).
    pub fn key_of(name: &str) -> Option<&str> {
        let rest = name.strip_prefix(RUN)?;
        let rest = rest.strip_suffix(SCOPE).unwrap_or(rest);
        let key = rest.split('-').next()?;
        (!key.is_empty()).then_some(key)
    }

    /// The workspace of the entry with `key`: its launches' windows, as tabs.
    pub fn workspace_of(key: &str) -> String {
        format!("{RUN}{key}")
    }

    /// The slice of the entry with `key`: its launches' scopes (all its processes).
    pub fn slice_of(key: &str) -> String {
        format!("{RUN}{key}.slice")
    }

    /// Whether `name` is one of ours (a workspace or scope ClamShell names, known kind or not).
    pub fn is_ours(name: &str) -> bool {
        name.starts_with(PREFIX)
    }

    /// Its name: its scope's (without `.scope`) for a launch, else its workspace's.
    pub fn name(&self) -> String {
        self.to_string()
    }

    /// The workspace its windows are on: its entry's for a launch, else its own name's.
    pub fn workspace(&self) -> String {
        match self {
            Instance::Run { key, .. } => Instance::workspace_of(key),
            _ => self.name(),
        }
    }

    /// Whether workspace `name` is the one its windows are on, compared without allocating.
    pub fn on(&self, name: &str) -> bool {
        match self {
            Instance::Run { key, .. } => name.strip_prefix(RUN) == Some(key.as_str()),
            _ => self.is(name),
        }
    }

    /// Its scope's unit name, for a launch.
    pub fn unit(&self) -> Option<String> {
        matches!(self, Instance::Run { .. }).then(|| format!("{self}{SCOPE}"))
    }

    /// Its slice's unit name, for a launch.
    pub fn slice(&self) -> Option<String> {
        self.key().map(Instance::slice_of)
    }

    /// Its place in the order things were started: the start time and the counter.
    pub fn order(&self) -> (u64, u64) {
        match self {
            Instance::Run { secs, n, .. } => (*secs, *n),
            _ => (0, 0),
        }
    }

    /// The entry key, for a launch.
    pub fn key(&self) -> Option<&str> {
        match self {
            Instance::Run { key, .. } => Some(key),
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
        match self {
            Instance::Shelf => write!(f, "{PREFIX}shelf"),
            Instance::Bottom => write!(f, "{PREFIX}bottom"),
            Instance::Landing => write!(f, "{PREFIX}game"),
            Instance::Run { key, secs, n } => write!(f, "{RUN}{key}-{secs}-{n}"),
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
        let run = Instance::Run { key: "zelda_3fa2b1".into(), secs: 1234, n: 7 };
        for i in [Instance::Shelf, Instance::Bottom, Instance::Landing, run.clone()] {
            assert_eq!(Instance::parse(&i.name()), Some(i.clone()), "{i}");
            assert!(i.is(&i.name()) && !i.is("clamshell-") && !i.is(&format!("{i}-1")), "{i}");
        }
        assert_eq!(run.name(), "clamshell-run-zelda_3fa2b1-1234-7");
        assert_eq!(run.unit().as_deref(), Some("clamshell-run-zelda_3fa2b1-1234-7.scope"));
        assert_eq!(run.slice().as_deref(), Some("clamshell-run-zelda_3fa2b1.slice"));
        assert_eq!(run.workspace(), "clamshell-run-zelda_3fa2b1");
        assert!(run.on("clamshell-run-zelda_3fa2b1") && !run.on(&run.name()) && !run.on("clamshell-run-zelda_3fa2b12"));
        assert_eq!(Instance::parse("clamshell-run-zelda_3fa2b1"), None, "an entry's workspace is no instance");
        assert_eq!(Instance::key_of("clamshell-run-zelda_3fa2b1"), Some("zelda_3fa2b1"));
        assert_eq!(Instance::key_of("clamshell-run-zelda_3fa2b1-1234-7.scope"), Some("zelda_3fa2b1"));
        assert_eq!(Instance::key_of("clamshell-shelf"), None);
        assert_eq!(Instance::Landing.unit(), None, "only a launch has a scope");
        assert_eq!(Instance::from_unit("clamshell-run-x_1-5-0.scope").and_then(|i| i.key().map(str::to_owned)).as_deref(), Some("x_1"));
        assert_eq!(Instance::from_unit("clamshell-shelf.scope"), None);
        assert_eq!(Instance::parse("1"), None);
        assert!(Instance::Run { key: "x".into(), secs: 5, n: 10 }.order() > Instance::Run { key: "x".into(), secs: 5, n: 9 }.order());
        assert_eq!(steam_app_id("steam_app_620"), Some(620));
        assert_eq!("clamshell-shelf".parse(), Ok(Instance::Shelf));
    }
}
