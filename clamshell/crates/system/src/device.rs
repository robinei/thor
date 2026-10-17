//! The device as a whole: a screenshot of both screens (grim, through the compositor's screen capture),
//! sleep through logind (suspend-then-hibernate where the system can, a swap file to hibernate to, else
//! suspend; waking is noticed by the clock thread, see [`crate::sleep`]), and notifications through the
//! session's notification daemon (mako, on the main screen).

use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clamshell_core::Event;

use crate::worker::Worker;
use crate::{Sink, command, dbus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Screenshot,
    Sleep,
    Notify { summary: String, body: String, urgent: bool },
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1",
    gen_async = false
)]
trait Manager {
    fn can_suspend_then_hibernate(&self) -> zbus::Result<String>;
    fn suspend_then_hibernate(&self, interactive: bool) -> zbus::Result<()>;
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
}

/// Screenshots go to `screenshots` (a folder, made if needed).
pub fn spawn(screenshots: Option<PathBuf>, sink: impl Sink) -> io::Result<Worker<Command>> {
    Worker::spawn("system", move |commands| {
        // Our last notification, which the next replaces (a lower battery's warning the earlier one's).
        let mut shown = 0;
        for command in commands {
            match command {
                Command::Screenshot => {
                    let result = screenshot(screenshots.as_ref());
                    if sink.send(Event::Screenshot(result)).is_err() {
                        return;
                    }
                }
                Command::Sleep => sleep(),
                Command::Notify { summary, body, urgent } => match notify(shown, &summary, &body, urgent) {
                    Ok(id) => shown = id,
                    Err(e) => log::warn!("notification \"{summary}\": {e}"),
                },
            }
        }
    })
}

/// Both screens (as the compositor lays them out) into one PNG named by the time; its path, or why not.
fn screenshot(dir: Option<&PathBuf>) -> Result<String, String> {
    let dir = dir.ok_or("no folder for screenshots (no HOME)")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let file = dir.join(format!("clamshell-{}.png", timestamp(secs)));
    let path = file.to_string_lossy().into_owned();
    if command::run("grim", &[&path]) {
        log::info!("screenshot: {path}");
        Ok(path)
    } else {
        Err("grim failed (is it installed, and does the compositor allow screen capture?)".into())
    }
}

/// A UTC timestamp for file names: 20261015-103205.
fn timestamp(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Sleep: suspend, then hibernate after a while (logind's HibernateDelaySec) if it can, else suspend.
fn sleep() {
    let slept = dbus::system(None).and_then(|conn| {
        let logind = dbus::default_proxy::<ManagerProxy>(&conn)?;
        if logind.can_suspend_then_hibernate()? == "yes" {
            log::info!("sleep: suspend, then hibernate");
            logind.suspend_then_hibernate(false)
        } else {
            log::info!("sleep: suspend");
            logind.suspend(false)
        }
    });
    if let Err(e) = slept {
        log::warn!("sleep: {e}");
    }
}

/// A notification through the session bus' notification daemon, replacing `replaces` (0: none); its id.
fn notify(replaces: u32, summary: &str, body: &str, urgent: bool) -> zbus::Result<u32> {
    use std::collections::HashMap;
    use zbus::zvariant::Value;
    let conn = zbus::blocking::Connection::session()?;
    // Urgency: 1 normal, 2 critical (stays until dismissed). Timeout -1: the daemon's own.
    let hints = HashMap::from([("urgency", Value::U8(if urgent { 2 } else { 1 }))]);
    let actions: Vec<&str> = Vec::new();
    let reply = conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &("ClamShell", replaces, "", summary, body, actions, hints, -1i32),
    )?;
    reply.body().deserialize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_dates() {
        assert_eq!(timestamp(0), "19700101-000000");
        assert_eq!(timestamp(1_792_060_325), "20261015-103205");
        assert_eq!(timestamp(951_782_400), "20000229-000000", "a leap day");
    }
}
