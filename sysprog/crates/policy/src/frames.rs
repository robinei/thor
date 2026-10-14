//! A running game's frame rate and frame time, from MangoHud.
//!
//! MangoHud measures inside the game (Vulkan layer, or OpenGL through its wrapper). Games started with
//! [`mangohud_env`] get it with its overlay fully transparent, logging a CSV row every LOG_INTERVAL_MS
//! to a folder in RAM; this thread follows the newest log and reports its latest frame rate and frame
//! time. (MangoHud's `no_display` would hide the overlay too, but stops the measuring with it.)
//!
//! Under gamescope there is also mangoapp's message queue, which gamescope itself feeds; our sway
//! setup doesn't run games inside gamescope, so this reads MangoHud's own log instead.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use sysprog_core::Event;

use crate::Sink;

/// How often MangoHud logs, and how often we look.
const LOG_INTERVAL_MS: u64 = 500;
/// A log not written for this long belongs to a game that has ended.
const STALE: Duration = Duration::from_secs(3);
/// Ended games' logs are removed after this.
const KEEP: Duration = Duration::from_secs(600);

/// The environment for a game to be measured: MangoHud on, invisible, logging into `dir`.
pub fn mangohud_env(dir: &Path) -> [(&'static str, String); 2] {
    [
        ("MANGOHUD", "1".into()),
        (
            "MANGOHUD_CONFIG",
            format!(
                "alpha=0,background_alpha=0,autostart_log=1,log_interval={LOG_INTERVAL_MS},output_folder={}",
                dir.display()
            ),
        ),
    ]
}

/// The folder for the logs: in the runtime directory, so in RAM.
pub fn default_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from).join("sysprog/mangohud")
}

/// Reads a growing MangoHud CSV: finds the fps and frametime columns from the header row, and keeps
/// the latest values. Pure.
#[derive(Debug, Default)]
pub struct Tail {
    columns: Option<(usize, usize)>,
    partial: String,
}

impl Tail {
    /// Feeds newly appended text; returns the latest (fps, frame time in ms) it contained, if any.
    pub fn feed(&mut self, chunk: &str) -> Option<(f32, f32)> {
        self.partial.push_str(chunk);
        let mut latest = None;
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            let fields: Vec<&str> = line.trim().split(',').collect();
            let fps = fields.iter().position(|f| *f == "fps");
            let frametime = fields.iter().position(|f| *f == "frametime");
            if let (Some(a), Some(b)) = (fps, frametime) {
                self.columns = Some((a, b));
            } else if let Some((a, b)) = self.columns {
                let num = |i: usize| fields.get(i).and_then(|f| f.parse::<f32>().ok());
                if let (Some(fps), Some(ms)) = (num(a), num(b)) {
                    latest = Some((fps, ms));
                }
            }
        }
        latest
    }
}

pub fn spawn(dir: PathBuf, sink: impl Sink) -> std::io::Result<JoinHandle<()>> {
    fs::create_dir_all(&dir)?;
    thread::Builder::new().name("frames".into()).spawn(move || {
        let mut current: Option<(PathBuf, u64, Tail)> = None;
        let mut sent = None;
        loop {
            thread::sleep(Duration::from_millis(LOG_INTERVAL_MS));
            let newest = newest_fresh_log(&dir);
            if newest.as_ref() != current.as_ref().map(|(p, _, _)| p) {
                current = newest.map(|p| (p, 0, Tail::default()));
            }
            let reading = match current.as_mut() {
                Some((path, offset, tail)) => read_new(path, offset).and_then(|text| tail.feed(&text)).or(sent),
                None => None,
            };
            if reading != sent {
                log::debug!("frames: {reading:?}");
                if !sink.send(Event::Frames(reading)) {
                    return;
                }
                sent = reading;
            }
        }
    })
}

/// The most recently written CSV in `dir`, if it is still being written; removes old ones.
fn newest_fresh_log(dir: &Path) -> Option<PathBuf> {
    let now = SystemTime::now();
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("csv") {
            continue;
        }
        let Some(modified) = entry.metadata().ok().and_then(|m| m.modified().ok()) else {
            continue;
        };
        let age = now.duration_since(modified).unwrap_or_default();
        if age > KEEP {
            let _ = fs::remove_file(&path);
        } else if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
            newest = Some((modified, path));
        }
    }
    newest.filter(|(t, _)| now.duration_since(*t).unwrap_or_default() <= STALE).map(|(_, p)| p)
}

/// What was appended to `path` since `offset` (advanced past it).
fn read_new(path: &Path, offset: &mut u64) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(*offset)).ok()?;
    let mut text = String::new();
    let n = file.read_to_string(&mut text).ok()?;
    *offset += n as u64;
    Some(text)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const HEAD: &str = "os,cpu,gpu\nCachyOS,AMD,AMD\nfps,frametime,cpu_load\n";

    #[test]
    fn reads_the_latest_row_by_column_name() {
        let mut t = Tail::default();
        assert_eq!(t.feed(HEAD), None);
        assert_eq!(t.feed("160.4,6.23,1.2\n159.6,6.26,1.4\n"), Some((159.6, 6.26)));
    }

    #[test]
    fn a_row_split_across_reads_is_joined() {
        let mut t = Tail::default();
        t.feed(HEAD);
        assert_eq!(t.feed("59.9,16."), None);
        assert_eq!(t.feed("7,3.0\n"), Some((59.9, 16.7)));
    }

    #[test]
    fn rows_before_the_header_are_ignored() {
        let mut t = Tail::default();
        assert_eq!(t.feed("1,2,3\n"), None);
    }

    #[test]
    fn the_environment_hides_the_overlay_and_logs_to_the_folder() {
        let [(k1, v1), (k2, v2)] = mangohud_env(Path::new("/run/user/1000/sysprog/mangohud"));
        assert_eq!((k1, v1.as_str()), ("MANGOHUD", "1"));
        assert_eq!(k2, "MANGOHUD_CONFIG");
        assert!(v2.contains("alpha=0") && v2.contains("output_folder=/run/user/1000/sysprog/mangohud"));
    }
}
