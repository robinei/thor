//! Our instances as systemd sees them: the user manager's scopes named `clamshell-app-*` and
//! `clamshell-game-*` (D-Bus, org.freedesktop.systemd1), listed at start and again whenever it signals a
//! unit coming or going. This is the truth for what runs: a scope lives as long as any of its processes,
//! whatever its windows do, and survives a restart of ClamShell.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::{Event, Instance};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

use crate::Sink;

const PATTERNS: [&str; 2] = ["clamshell-app-*.scope", "clamshell-game-*.scope"];

/// ListUnitsByPatterns' rows: name, description, load state, active state, sub state, following,
/// object path, job id, job type, job path.
type UnitRow = (String, String, String, String, String, String, OwnedObjectPath, u32, String, OwnedObjectPath);

fn manager(conn: &Connection) -> zbus::Result<Proxy<'static>> {
    Proxy::new(conn, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager")
}

/// Our scopes that run (dock app instances and games), in the order of their names.
fn list(manager: &Proxy) -> zbus::Result<Vec<Instance>> {
    let rows: Vec<UnitRow> = manager.call("ListUnitsByPatterns", &(Vec::<&str>::new(), PATTERNS.to_vec()))?;
    let mut names: Vec<String> = rows.into_iter().filter(|row| !matches!(row.3.as_str(), "inactive" | "failed")).map(|row| row.0).collect();
    names.sort();
    Ok(names.iter().filter_map(|n| Instance::from_unit(n)).collect())
}

/// Sends [`Event::Units`] at start and after every change of our scopes; reconnects if the bus goes.
pub fn spawn(sink: impl Sink) -> io::Result<()> {
    thread::Builder::new().name("units".into()).spawn(move || {
        loop {
            match follow(&sink) {
                Ok(()) => return,
                Err(e) => log::warn!("units: {e}; trying again"),
            }
            thread::sleep(Duration::from_secs(5));
        }
    })?;
    Ok(())
}

fn follow(sink: &impl Sink) -> zbus::Result<()> {
    let conn = Connection::session()?;
    let manager = manager(&conn)?;
    // Without Subscribe, the manager doesn't signal unit changes.
    manager.call::<_, _, ()>("Subscribe", &())?;
    let signals = manager.receive_all_signals()?;
    let mut last = list(&manager)?;
    if !sink.send(Event::Units(last.clone())) {
        return Ok(());
    }
    for message in signals {
        let header = message.header();
        let member = header.member().map(|m| m.as_str().to_owned()).unwrap_or_default();
        if member != "UnitNew" && member != "UnitRemoved" {
            continue;
        }
        let Ok((name, _)) = message.body().deserialize::<(String, OwnedObjectPath)>() else {
            continue;
        };
        if !Instance::is_ours(&name) {
            continue;
        }
        let now = list(&manager)?;
        if now != last {
            last = now.clone();
            if !sink.send(Event::Units(now)) {
                return Ok(());
            }
        }
    }
    Err(zbus::Error::Failure("the bus closed".into()))
}

/// Whether the user manager has this unit loaded (`name` with its suffix).
fn loaded(manager: &Proxy, name: &str) -> bool {
    manager.call::<_, _, OwnedObjectPath>("GetUnit", &(name,)).is_ok()
}

/// Waits for a scope to appear (within `appear`) and then to end; Err if it never appeared. Checks once
/// a second: for a game's play time, a second is exact enough.
pub fn wait(unit: &str, appear: Duration) -> io::Result<()> {
    let conn = Connection::session().map_err(io::Error::other)?;
    let manager = manager(&conn).map_err(io::Error::other)?;
    let asked = Instant::now();
    while !loaded(&manager, unit) {
        if asked.elapsed() > appear {
            return Err(io::Error::other(format!("{unit} didn't start")));
        }
        thread::sleep(Duration::from_millis(200));
    }
    while loaded(&manager, unit) {
        thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::process::Command;

    /// With a user session (a desktop): a scope we start shows in the list, and is gone once it ends.
    #[test]
    fn a_scope_comes_and_goes() {
        let Ok(conn) = Connection::session() else {
            return;
        };
        let manager = manager(&conn).unwrap();
        let instance = Instance::App { key: "unittest".into(), secs: u64::from(std::process::id()), n: 0 };
        let name = instance.name();
        let mut child = Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect", &format!("--unit={name}"), "--", "sleep", "1"])
            .spawn()
            .unwrap();
        let start = Instant::now();
        while !list(&manager).unwrap().contains(&instance) {
            assert!(start.elapsed() < Duration::from_secs(5), "listed");
            thread::sleep(Duration::from_millis(50));
        }
        wait(&instance.unit().unwrap(), Duration::from_secs(1)).unwrap();
        child.wait().unwrap();
        assert!(!list(&manager).unwrap().contains(&instance));
        assert!(wait("clamshell-app-never-0-0.scope", Duration::from_millis(300)).is_err());
    }
}
