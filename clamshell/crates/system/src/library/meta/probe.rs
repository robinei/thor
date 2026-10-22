//! Videos: their length (ffprobe's) and a frame to show for them (ffmpeg's), both at the lowest priority
//! (a game started meanwhile comes first): of the keyframes from a point on, the most representative
//! (its `thumbnail` filter), so not a dark scene, a fade or a title card.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use clamshell_core::Meta;
use clamshell_library::Details;

use super::{Shared, Wanted};
use crate::os::command;

impl Shared {
    /// A video's length, and its most representative frame from a quarter in (see [`frame`])
    /// as its screenshot.
    pub(super) fn probe(&self, w: &Wanted) -> Meta {
        let Some(file) = w.file.as_deref() else {
            return Meta::default();
        };
        let duration = duration(file);
        let out = self.art.join(format!("{}-frame.jpg", w.key));
        let shot = duration.filter(|d| frame(file, d / 4, &out)).map(|_| out);
        Meta { duration, details: Details { shot, ..Details::default() }, ..Meta::default() }
    }
}

/// A video's length (seconds), from ffprobe.
pub fn duration(file: &Path) -> Option<u32> {
    let mut args: Vec<&OsStr> = ["-n", "19", "ffprobe", "-v", "quiet", "-show_entries", "format=duration", "-of", "csv=p=0"].map(OsStr::new).into();
    args.push(file.as_os_str());
    let out = command::run("nice", &args, None, PROBE_LIMIT).ok()?;
    let seconds: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    (seconds > 0.0).then_some(seconds.round() as u32)
}

/// How long ffprobe may take to read a video's length, and ffmpeg to pick a frame (it decodes keyframes
/// only; a network mount may be slow).
const PROBE_LIMIT: Duration = Duration::from_secs(30);
const FRAME_LIMIT: Duration = Duration::from_secs(120);

/// Keyframes `frame` chooses among (from its starting point: a few minutes of a video).
const FRAMES_LOOKED_AT: u32 = 60;

/// A frame of `file`, 640 pixels wide, into `out` (JPEG; written whole, then renamed into place): of the
/// [`FRAMES_LOOKED_AT`] keyframes from `at` seconds on, the most representative (ffmpeg's `thumbnail`
/// filter; keyframes only, so it is quick). Whether it worked.
pub fn frame(file: &Path, at: u32, out: &Path) -> bool {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let part = out.with_extension("part.jpg");
    let (choose, at) = (format!("scale=640:-2,thumbnail={FRAMES_LOOKED_AT}"), at.to_string());
    let mut args: Vec<&OsStr> = ["-n", "19", "ffmpeg", "-v", "quiet", "-y", "-skip_frame", "nokey", "-ss", &at, "-i"].map(OsStr::new).into();
    args.push(file.as_os_str());
    args.extend(["-vf", &choose, "-frames:v", "1", "-fps_mode", "vfr", "-q:v", "4"].map(OsStr::new));
    args.push(part.as_os_str());
    let ok = command::run("nice", &args, None, FRAME_LIMIT).is_ok_and(|ran| ran.status.success());
    ok && std::fs::rename(&part, out).is_ok()
}
