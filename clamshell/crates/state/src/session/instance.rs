//! The names ClamShell gives home groups, systemd units and slices.
//!
//! A launch of a library entry, whether a game or an app, is a service we started
//! (`clamshell-run-<key>-<secs>-<n>.service`), its output in the journal as the unit's. A program found
//! running that we take as an entry's is a scope of the same kind of name (`….scope`), since a service
//! can't take in processes that already run. Either is in the entry's slice
//! (`app-graphical-clamshell-run-<key>.slice`), which is in the session's app slice; Quit and Force kill
//! end the slice. All the entry's windows are on its workspace (`clamshell-run-<key>`), as tabs. `<key>` is
//! the entry's key ([`clamshell_library::key_of`]), made of letters, digits and `_`. `<secs>-<n>` is the
//! start time and a counter, so names are unique across restarts and sort in the order things were
//! started.
//!
//! An entry that runs in another program's processes, such as a game in Steam's or an Android app in
//! Waydroid's, isn't in a scope of ours, but its windows belong to its entry's home group too.

use std::fmt::{self, Write};
use std::str::FromStr;

/// Every name of ours starts with this.
const PREFIX: &str = "clamshell-";
const RUN: &str = "clamshell-run-";
/// The prefix of an entry's slice, which is named by the entry's key. The slice is in the session's slice
/// for apps, `app-graphical.slice`, which thor-session stops as the session ends.
pub const SLICE_PREFIX: &str = "app-graphical-clamshell-run-";
const SERVICE: &str = ".service";
const SCOPE: &str = ".scope";
/// Steam's app id for a game's windows: `steam_app_<id>`.
pub const STEAM_APP_PREFIX: &str = "steam_app_";

/// A workspace of ours, or, for a launch, a scope and the workspace its windows are on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Instance {
    /// Shelf, which has no scope.
    Shelf,
    /// A launch of an entry: its key, the start time, a counter.
    Run { key: String, secs: u64, n: u64 },
}

impl Instance {
    /// The instance that `name`, a workspace's or a scope's without `.scope`, stands for, if it is ours. An
    /// entry's workspace gives none, since its launches share it.
    pub fn parse(name: &str) -> Option<Instance> {
        if name.strip_prefix(PREFIX)? == "shelf" {
            return Some(Instance::Shelf);
        }
        let (key, secs, n) = Instance::parse_run(name)?;
        Some(Instance::Run { key: key.to_owned(), secs, n })
    }

    /// A launch's key, start time and counter, from its name.
    fn parse_run(name: &str) -> Option<(&str, u64, u64)> {
        let mut parts = name.strip_prefix(RUN)?.rsplitn(3, '-');
        let n = parts.next()?.parse().ok()?;
        let secs = parts.next()?.parse().ok()?;
        let key = parts.next().filter(|k| !k.is_empty() && !k.contains('-'))?;
        Some((key, secs, n))
    }

    /// A unit's name without `.service` or `.scope`.
    pub fn unit_name(unit: &str) -> Option<&str> {
        unit.strip_suffix(SERVICE).or_else(|| unit.strip_suffix(SCOPE))
    }

    /// The launch whose unit `unit` (its service, or the scope of a program taken in) is, if it is ours.
    pub fn from_unit(unit: &str) -> Option<Instance> {
        Instance::parse(Instance::unit_name(unit)?).filter(|i| matches!(i, Instance::Run { .. }))
    }

    /// Whether `unit` is a launch's ([`Instance::from_unit`]), found without allocating.
    pub fn is_launch_unit(unit: &str) -> bool {
        Instance::unit_name(unit).and_then(Instance::parse_run).is_some()
    }

    /// The file name, in the runtime dir, of the control socket of entry `key`'s launches. It is a
    /// command's `{socket}`.
    pub fn socket_of(key: &str) -> String {
        format!("{RUN}{key}.sock")
    }

    /// The entry key in `name`, which is a launch's unit, with or without its suffix, or an entry's
    /// workspace. It doesn't allocate.
    pub fn key_of(name: &str) -> Option<&str> {
        let rest = name.strip_prefix(RUN)?;
        let rest = Instance::unit_name(rest).unwrap_or(rest);
        let key = rest.split('-').next()?;
        (!key.is_empty()).then_some(key)
    }

    /// The workspace of the entry with `key`, which holds its launches' windows as tabs.
    pub fn workspace_of(key: &str) -> String {
        format!("{RUN}{key}")
    }

    /// The slice of the entry with `key`, which holds its launches' units and so all its processes.
    pub fn slice_of(key: &str) -> String {
        format!("{SLICE_PREFIX}{key}.slice")
    }

    /// Whether `unit` is a systemd service that a program runs in as part of the service, such as a portal
    /// or the polkit agent. Such a program's windows and processes belong to the service and aren't moved.
    /// ClamShell's own unit doesn't count, because what ClamShell starts runs there until it is moved out,
    /// nor do our launches'. ClamShell's unit is `clamshell.service` on the device and
    /// `clamshell-nested.service` on the desktop.
    pub fn is_service(unit: &str) -> bool {
        unit.ends_with(SERVICE) && unit != "clamshell.service" && unit != "clamshell-nested.service" && !Instance::is_launch_unit(unit)
    }

    /// Whether `name` is one of ours: a workspace or scope that ClamShell names, of a known kind or not.
    pub fn is_ours(name: &str) -> bool {
        name.starts_with(PREFIX)
    }

    /// Its name: for a launch, its unit's without the suffix, or else its workspace's.
    pub fn name(&self) -> String {
        self.to_string()
    }

    /// The workspace its windows are on: its entry's for a launch, or else the one with its own name.
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

    /// The service a launch runs in.
    pub fn unit(&self) -> Option<String> {
        matches!(self, Instance::Run { .. }).then(|| format!("{self}{SERVICE}"))
    }

    /// The scope a program found running is taken into, for a launch.
    pub fn scope(&self) -> Option<String> {
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

    /// Whether `name` is this instance's name, compared without allocating so it can be used while drawing.
    pub fn is(&self, name: &str) -> bool {
        let mut rest = Rest(Some(name));
        let _ = write!(rest, "{self}");
        rest.0 == Some("")
    }
}

/// What is left of a name to compare against, as [`Instance::is`] writes the instance's name piece by
/// piece. It is `None` once a piece didn't match.
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
        for i in [Instance::Shelf, run.clone()] {
            assert_eq!(Instance::parse(&i.name()), Some(i.clone()), "{i}");
            assert!(i.is(&i.name()) && !i.is("clamshell-") && !i.is(&format!("{i}-1")), "{i}");
        }
        assert_eq!(run.name(), "clamshell-run-zelda_3fa2b1-1234-7");
        assert_eq!(run.unit().as_deref(), Some("clamshell-run-zelda_3fa2b1-1234-7.service"));
        assert_eq!(run.scope().as_deref(), Some("clamshell-run-zelda_3fa2b1-1234-7.scope"));
        assert_eq!(run.slice().as_deref(), Some("app-graphical-clamshell-run-zelda_3fa2b1.slice"));
        assert_eq!(run.workspace(), "clamshell-run-zelda_3fa2b1");
        assert!(run.on("clamshell-run-zelda_3fa2b1") && !run.on(&run.name()) && !run.on("clamshell-run-zelda_3fa2b12"));
        assert_eq!(Instance::parse("clamshell-run-zelda_3fa2b1"), None, "an entry's workspace is no instance");
        assert_eq!(Instance::key_of("clamshell-run-zelda_3fa2b1"), Some("zelda_3fa2b1"));
        assert_eq!(Instance::key_of("clamshell-run-zelda_3fa2b1-1234-7.scope"), Some("zelda_3fa2b1"));
        assert_eq!(Instance::key_of("clamshell-shelf"), None);
        assert_eq!(Instance::Shelf.unit(), None, "only a launch has a unit");
        assert_eq!(Instance::from_unit("clamshell-run-x_1-5-0.service").and_then(|i| i.key().map(str::to_owned)).as_deref(), Some("x_1"));
        assert_eq!(Instance::from_unit("clamshell-run-x_1-5-0.scope").and_then(|i| i.key().map(str::to_owned)).as_deref(), Some("x_1"), "taken in");
        assert!(!Instance::is_service("clamshell-run-x_1-5-0.service"), "a launch is no system service");
        assert_eq!(Instance::from_unit("clamshell-shelf.scope"), None);
        assert_eq!(Instance::parse("1"), None);
        assert!(Instance::Run { key: "x".into(), secs: 5, n: 10 }.order() > Instance::Run { key: "x".into(), secs: 5, n: 9 }.order());
        assert_eq!("clamshell-shelf".parse(), Ok(Instance::Shelf));
    }
}
