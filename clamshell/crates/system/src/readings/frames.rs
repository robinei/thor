//! A running game's frame rate and frame time, from MangoHud.
//!
//! MangoHud measures inside the game (Vulkan layer, or OpenGL through its wrapper). Games are started
//! with a MangoHud config file ([`super::mangohud::config`]) that makes its overlay fully transparent and
//! logs a CSV row every 500 ms to a folder in RAM; this thread follows the newest log and reports its
//! latest frame rate and frame time. (MangoHud's `no_display` would hide the overlay too, but stops the
//! measuring with it.) A new log starts whenever MangoHud reloads its config (an FPS cap change).
//!
//! Under gamescope there is also mangoapp's message queue, which gamescope itself feeds; our sway
//! setup doesn't run games inside gamescope, so this reads MangoHud's own log instead.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use clamshell_core::{Event, Reading};
use rustix::event::{PollFd, PollFlags};
use rustix::fs::inotify;

use crate::os::worker::{self, Changed};
use crate::readings::mangohud;
use crate::{Closed, Sink};

/// A log not written for this long belongs to a game that has ended (MangoHud logs every 500 ms,
/// tuning::mangohud_config): while one is followed, looked at this often for that.
const STALE: Duration = Duration::from_secs(3);
/// Ended games' logs are removed after this.
const KEEP: Duration = Duration::from_secs(600);

/// Follows the logs in `dir` (MangoHud's folder, [`crate::paths::Paths::mangohud`]).
pub fn spawn(dir: PathBuf, sink: impl Sink) -> io::Result<()> {
    fs::create_dir_all(&dir)?;
    worker::detached("frames", move || {
        let _ = follow(&dir, &sink);
    })
}

/// Sends the latest frame rate whenever it changes, until the UI is gone. Asleep until the folder
/// changes (a log written), and while a log is followed at most [`STALE`] (to see its game has ended).
fn follow(dir: &Path, sink: &impl Sink) -> Result<(), Closed> {
    let notify = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC).map_err(|e| log::warn!("frames: inotify: {e}")).ok();
    let flags = inotify::WatchFlags::CREATE | inotify::WatchFlags::MODIFY | inotify::WatchFlags::MOVED_TO;
    let notify = notify.filter(|fd| inotify::add_watch(fd, dir, flags).map_err(|e| log::warn!("frames: {}: {e}", dir.display())).is_ok());
    // The log followed: its path, how far it has been read, and its columns.
    let mut current: Option<(PathBuf, u64, mangohud::Tail)> = None;
    let mut last = None;
    // No frame rate to begin with: the state has none either.
    let mut sent = Changed::starting(None);
    loop {
        let timeout = (current.is_some() || last.is_some()).then_some(STALE);
        match notify.as_ref() {
            Some(fd) => wait(fd, timeout),
            // (Without inotify: looked at twice a second.)
            None => thread::sleep(Duration::from_millis(500)),
        }
        let newest = newest_fresh_log(dir);
        if newest.as_ref() != current.as_ref().map(|(p, _, _)| p) {
            current = newest.map(|p| (p, 0, mangohud::Tail::default()));
        }
        // Without a new row the last reading stands; without a log there is none.
        last = match current.as_mut() {
            Some((path, offset, tail)) => read_new(path, offset).and_then(|text| tail.feed(&text)).or(last),
            None => None,
        };
        if let Some(reading) = sent.offer(last) {
            log::debug!("frames: {reading:?}");
            sink.send(Event::Reading(Reading::Frames { fps: reading.map(|r| r.0), frame_ms: reading.map(|r| r.1) }))?;
        }
    }
}

/// Waits for the folder's inotify events (all read), up to `timeout` if given.
fn wait(fd: &OwnedFd, timeout: Option<Duration>) {
    let timeout = timeout.map(crate::os::sys::timespec);
    let mut fds = [PollFd::new(fd, PollFlags::IN)];
    let _ = rustix::event::poll(&mut fds, timeout.as_ref());
    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buf);
    while reader.next().is_ok() {}
}

/// The most recently written CSV in `dir`, if it is still being written; removes old ones.
fn newest_fresh_log(dir: &Path) -> Option<PathBuf> {
    let now = SystemTime::now();
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        // MangoHud also writes a `<game>_summary.csv` when a log ends.
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".csv") || name.ends_with("_summary.csv") {
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
