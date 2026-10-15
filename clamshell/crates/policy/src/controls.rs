//! Volume and brightness, set from the bar's sliders and the volume and brightness tabs: the output
//! volume, an app's volume and mute, and which output plays through PipeWire (`wpctl`); the screens'
//! brightness (each backlight to the level times its factor) through logind, which lets the session's
//! user set it without root.
//! While a finger slides, requests come faster than they can be carried out; only the latest of each is.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::Receiver;
use std::thread;

use zbus::blocking::{Connection, Proxy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Volume(u8),
    Mute(bool),
    /// Each backlight to `level` times its factor (by name; 100 when not listed), both 0-100.
    Brightness { level: u8, factors: Vec<(String, u8)> },
    /// A PipeWire node (an app's stream): its volume, its mute.
    StreamVolume(u32, u8),
    StreamMute(u32, bool),
    /// Sound to this output (a PipeWire sink).
    Output(u32),
}

impl Command {
    /// Commands with the same key replace each other: only the latest is carried out.
    fn key(&self) -> (u8, Option<&str>, u32) {
        match self {
            Command::Volume(_) => (0, None, 0),
            Command::Mute(_) => (1, None, 0),
            Command::Brightness { .. } => (2, None, 0),
            Command::StreamVolume(id, _) => (4, None, *id),
            Command::StreamMute(id, _) => (5, None, *id),
            Command::Output(_) => (6, None, 0),
        }
    }
}

/// The latest of each kind, in the order they first came (a mute before a volume stays before it).
fn latest(commands: Vec<Command>) -> Vec<Command> {
    let mut kept: Vec<Command> = Vec::new();
    for c in commands {
        match kept.iter_mut().find(|k| k.key() == c.key()) {
            Some(k) => *k = c,
            None => kept.push(c),
        }
    }
    kept
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

fn level(dir: &Path) -> Option<u8> {
    let max = read_number(&dir.join("max_brightness"))?.max(1);
    let now = read_number(&dir.join("brightness"))?;
    Some((f64::from(now) * 100.0 / f64::from(max)).round().clamp(0.0, 100.0) as u8)
}

/// The first backlight's brightness, 0-100; `None` without a backlight.
pub fn brightness() -> Option<u8> {
    level(&backlights().into_iter().next()?)
}

/// The backlights' names.
pub fn backlight_names() -> Vec<String> {
    backlights().iter().filter_map(|dir| Some(dir.file_name()?.to_str()?.to_owned())).collect()
}

/// A level times a factor (both 0-100) as a backlight's raw value: never 0, which turns some panels off.
fn raw(level: u8, factor: u8, max: u32) -> u32 {
    let share = f64::from(level.min(100)) / 100.0 * f64::from(factor.min(100)) / 100.0;
    ((f64::from(max) * share).round() as u32).clamp(1, max.max(1))
}

/// Carries out the commands as they come, the latest of each kind when several are waiting.
pub fn spawn(commands: Receiver<Command>) -> io::Result<()> {
    thread::Builder::new().name("controls".into()).spawn(move || {
        let mut logind = None;
        while let Ok(first) = commands.recv() {
            for command in latest(std::iter::once(first).chain(commands.try_iter()).collect()) {
                let flag = |on: bool| if on { "1" } else { "0" };
                match command {
                    Command::Mute(m) => wpctl(&["set-mute", "@DEFAULT_AUDIO_SINK@", flag(m)]),
                    Command::Volume(v) => wpctl(&["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{v}%")]),
                    Command::StreamMute(id, m) => wpctl(&["set-mute", &id.to_string(), flag(m)]),
                    Command::StreamVolume(id, v) => wpctl(&["set-volume", &id.to_string(), &format!("{v}%")]),
                    Command::Output(id) => wpctl(&["set-default", &id.to_string()]),
                    Command::Brightness { level, factors } => {
                        for dir in backlights() {
                            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                            let factor = factors.iter().find(|(n, _)| n == name).map_or(100, |(_, f)| *f);
                            set_backlight(&dir, level, factor, &mut logind);
                        }
                    }
                }
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

/// A backlight to `level` times `factor`: through logind's session (kept connected), else straight to
/// sysfs (which needs write access).
fn set_backlight(dir: &Path, level: u8, factor: u8, logind: &mut Option<Connection>) {
    let (Some(name), Some(max)) = (dir.file_name().and_then(|n| n.to_str()), read_number(&dir.join("max_brightness"))) else {
        return;
    };
    let value = raw(level, factor, max);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_of_each_is_carried_out() {
        let commands = vec![
            Command::Mute(false),
            Command::Volume(10),
            Command::StreamVolume(80, 5),
            Command::Volume(20),
            Command::StreamVolume(81, 7),
            Command::StreamVolume(80, 6),
            Command::Brightness { level: 10, factors: vec![] },
            Command::Brightness { level: 20, factors: vec![("a".into(), 50)] },
        ];
        assert_eq!(
            latest(commands),
            vec![
                Command::Mute(false),
                Command::Volume(20),
                Command::StreamVolume(80, 6),
                Command::StreamVolume(81, 7),
                Command::Brightness { level: 20, factors: vec![("a".into(), 50)] }
            ]
        );
    }

    #[test]
    fn levels_as_raw_values_never_turn_the_panel_off() {
        assert_eq!(raw(100, 100, 4095), 4095);
        assert_eq!(raw(50, 100, 255), 128);
        assert_eq!(raw(50, 50, 400), 100, "the level times the factor");
        assert_eq!(raw(0, 100, 255), 1);
        assert_eq!(raw(5, 5, 255), 1, "never off");
        assert_eq!(raw(200, 200, 10), 10);
    }
}
