//! The names ClamShell gives its systemd units and slices.
//!
//! A launch of a library entry, whether a game or an app, is a service we started
//! (`clamshell-run-<key>-<secs>-<n>.service`), its output in the journal as the unit's. A program found
//! running that we take as an entry's is a scope of the same kind of name (`….scope`), since a service
//! can't take in processes that already run. Either is in the entry's slice
//! (`app-graphical-clamshell-run-<key>.slice`), which is in the session's app slice; Quit and Force kill
//! end the slice. `<key>` is the entry's key ([`clamshell_library::key_of`]), made of letters, digits and
//! `_`. `<secs>-<n>` is the start time and a counter, so names are unique across restarts and sort in the
//! order things were started.

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
pub(crate) const STEAM_APP_PREFIX: &str = "steam_app_";

/// A launch of an entry, which names its unit: the entry's key, the start time and a counter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LaunchUnit {
    pub key: String,
    pub secs: u64,
    pub n: u64,
}

impl LaunchUnit {
    /// The launch whose name (its unit's without the suffix) is `name`, if it is ours.
    pub fn parse(name: &str) -> Option<LaunchUnit> {
        let (key, secs, n) = LaunchUnit::parse_run(name)?;
        Some(LaunchUnit { key: key.to_owned(), secs, n })
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
    pub(crate) fn unit_name(unit: &str) -> Option<&str> {
        unit.strip_suffix(SERVICE).or_else(|| unit.strip_suffix(SCOPE))
    }

    /// The launch whose unit `unit` (its service, or the scope of a program taken in) is, if it is ours.
    pub fn from_unit(unit: &str) -> Option<LaunchUnit> {
        LaunchUnit::parse(LaunchUnit::unit_name(unit)?)
    }

    /// Whether `unit` is a launch's ([`LaunchUnit::from_unit`]), found without allocating.
    pub fn is_launch_unit(unit: &str) -> bool {
        LaunchUnit::unit_name(unit).and_then(LaunchUnit::parse_run).is_some()
    }

    /// The file name, in the runtime dir, of the control socket of entry `key`'s launches. It is a
    /// command's `{socket}`.
    pub fn socket_of(key: &str) -> String {
        format!("{RUN}{key}.sock")
    }

    /// The entry key in `name`, a launch's unit with or without its suffix. It doesn't allocate.
    pub fn key_of(name: &str) -> Option<&str> {
        let rest = name.strip_prefix(RUN)?;
        let rest = LaunchUnit::unit_name(rest).unwrap_or(rest);
        let key = rest.split('-').next()?;
        (!key.is_empty()).then_some(key)
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
        unit.ends_with(SERVICE) && unit != "clamshell.service" && unit != "clamshell-nested.service" && !LaunchUnit::is_launch_unit(unit)
    }

    /// Whether `name` is one of ours: a unit or slice that ClamShell names, of a known kind or not.
    pub fn is_ours(name: &str) -> bool {
        name.starts_with(PREFIX)
    }

    /// Its name: its unit's without the suffix.
    pub fn name(&self) -> String {
        self.to_string()
    }

    /// The service it runs in.
    pub fn service(&self) -> String {
        format!("{self}{SERVICE}")
    }

    /// The scope a program found running is taken into.
    pub fn scope(&self) -> String {
        format!("{self}{SCOPE}")
    }

    /// Its entry's slice.
    pub fn slice(&self) -> String {
        LaunchUnit::slice_of(&self.key)
    }

    /// Whether `name` is this unit's name, compared without allocating so it can be used while drawing.
    pub fn is(&self, name: &str) -> bool {
        let mut rest = Rest(Some(name));
        let _ = write!(rest, "{self}");
        rest.0 == Some("")
    }
}

/// What is left of a name to compare against, as [`LaunchUnit::is`] writes the unit's name piece by
/// piece. It is `None` once a piece didn't match.
struct Rest<'a>(Option<&'a str>);

impl Write for Rest<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 = self.0.and_then(|rest| rest.strip_prefix(s));
        Ok(())
    }
}

impl fmt::Display for LaunchUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{RUN}{}-{}-{}", self.key, self.secs, self.n)
    }
}

/// A name that isn't one of ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotOurs;

impl FromStr for LaunchUnit {
    type Err = NotOurs;

    fn from_str(name: &str) -> Result<LaunchUnit, NotOurs> {
        LaunchUnit::parse(name).ok_or(NotOurs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_back() {
        let run = LaunchUnit { key: "zelda_3fa2b1".into(), secs: 1234, n: 7 };
        assert_eq!(LaunchUnit::parse(&run.name()), Some(run.clone()), "{run}");
        assert!(run.is(&run.name()) && !run.is("clamshell-") && !run.is(&format!("{run}-1")), "{run}");
        assert_eq!(run.name(), "clamshell-run-zelda_3fa2b1-1234-7");
        assert_eq!(run.service(), "clamshell-run-zelda_3fa2b1-1234-7.service");
        assert_eq!(run.scope(), "clamshell-run-zelda_3fa2b1-1234-7.scope");
        assert_eq!(run.slice(), "app-graphical-clamshell-run-zelda_3fa2b1.slice");
        assert_eq!(LaunchUnit::parse("clamshell-run-zelda_3fa2b1"), None, "a key alone is no unit");
        assert_eq!(LaunchUnit::key_of("clamshell-run-zelda_3fa2b1-1234-7"), Some("zelda_3fa2b1"));
        assert_eq!(LaunchUnit::key_of("clamshell-run-zelda_3fa2b1-1234-7.scope"), Some("zelda_3fa2b1"));
        assert_eq!(LaunchUnit::key_of("clamshell-shelf"), None);
        assert_eq!(LaunchUnit::from_unit("clamshell-run-x_1-5-0.service").map(|i| i.key).as_deref(), Some("x_1"));
        assert_eq!(LaunchUnit::from_unit("clamshell-run-x_1-5-0.scope").map(|i| i.key).as_deref(), Some("x_1"), "taken in");
        assert!(!LaunchUnit::is_service("clamshell-run-x_1-5-0.service"), "a launch is no system service");
        assert_eq!(LaunchUnit::from_unit("clamshell-shelf.scope"), None);
        assert_eq!(LaunchUnit::parse("1"), None);
        assert_eq!("clamshell-shelf".parse::<LaunchUnit>(), Err(NotOurs));
    }
}
