//! Volume and brightness, set from the bar's sliders: the output volume through PipeWire (`wpctl`), the
//! screens' brightness on every backlight (the Thor has one per panel) through logind, which lets the
//! session's user set it without root. While a finger slides, requests come faster than they can be
//! carried out; only the latest of each is.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::Receiver;
use std::thread;

use zbus::blocking::{Connection, Proxy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Volume(u8),
    Mute(bool),
    Brightness(u8),
}

/// Where the backlights are; CLAMSHELL_BACKLIGHTS: another folder laid out the same way (tests, a desktop
/// without one).
fn backlights_dir() -> PathBuf {
    std::env::var_os("CLAMSHELL_BACKLIGHTS").map_or_else(|| PathBuf::from("/sys/class/backlight"), PathBuf::from)
}

fn backlights() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(backlights_dir()).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    dirs.sort();
    dirs
}

fn read_number(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The first backlight's brightness, 0-100; `None` without a backlight.
pub fn brightness() -> Option<u8> {
    let dir = backlights().into_iter().next()?;
    let max = read_number(&dir.join("max_brightness"))?.max(1);
    let now = read_number(&dir.join("brightness"))?;
    Some((f64::from(now) * 100.0 / f64::from(max)).round().clamp(0.0, 100.0) as u8)
}

/// A level 0-100 as a backlight's raw value: never 0, which turns some panels off.
fn raw(percent: u8, max: u32) -> u32 {
    ((f64::from(max) * f64::from(percent.min(100)) / 100.0).round() as u32).clamp(1, max.max(1))
}

/// Carries out the commands as they come, the latest of each kind when several are waiting.
pub fn spawn(commands: Receiver<Command>) -> io::Result<()> {
    thread::Builder::new().name("controls".into()).spawn(move || {
        let mut logind = None;
        while let Ok(first) = commands.recv() {
            let (mut volume, mut mute, mut brightness) = (None, None, None);
            for command in std::iter::once(first).chain(commands.try_iter()) {
                match command {
                    Command::Volume(v) => volume = Some(v),
                    Command::Mute(m) => mute = Some(m),
                    Command::Brightness(b) => brightness = Some(b),
                }
            }
            if let Some(m) = mute {
                wpctl(&["set-mute", "@DEFAULT_AUDIO_SINK@", if m { "1" } else { "0" }]);
            }
            if let Some(v) = volume {
                wpctl(&["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{v}%")]);
            }
            if let Some(b) = brightness {
                set_brightness(b, &mut logind);
            }
        }
    })?;
    Ok(())
}

fn wpctl(args: &[&str]) {
    match Process::new("wpctl").args(args).status() {
        Ok(s) if s.success() => {}
        Ok(s) => log::warn!("wpctl {}: {s}", args.join(" ")),
        Err(e) => log::warn!("wpctl: {e}"),
    }
}

/// Every backlight to `percent`: through logind's session (kept connected), else straight to sysfs
/// (which needs write access).
fn set_brightness(percent: u8, logind: &mut Option<Connection>) {
    for dir in backlights() {
        let (Some(name), Some(max)) = (dir.file_name().and_then(|n| n.to_str()), read_number(&dir.join("max_brightness"))) else {
            continue;
        };
        let value = raw(percent, max);
        if logind.is_none() {
            *logind = Connection::system().map_err(|e| log::warn!("brightness: system bus: {e}")).ok();
        }
        let through_logind = logind.as_ref().map(|conn| {
            Proxy::new(conn, "org.freedesktop.login1", "/org/freedesktop/login1/session/auto", "org.freedesktop.login1.Session")
                .and_then(|session| session.call::<_, _, ()>("SetBrightness", &("backlight", name, value)))
        });
        match through_logind {
            Some(Ok(())) => {}
            other => {
                if let Some(Err(e)) = other {
                    log::debug!("brightness through logind: {e}");
                }
                if let Err(e) = std::fs::write(dir.join("brightness"), value.to_string()) {
                    log::warn!("brightness {name}: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_as_raw_values_never_turn_the_panel_off() {
        assert_eq!(raw(100, 4095), 4095);
        assert_eq!(raw(50, 255), 128);
        assert_eq!(raw(0, 255), 1);
        assert_eq!(raw(200, 10), 10);
    }
}
