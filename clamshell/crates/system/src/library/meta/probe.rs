//! Videos: their length from ffprobe and a frame to show for them from ffmpeg. Both run at the lowest
//! priority, so a game started meanwhile comes first. The frame is the most representative of the
//! keyframes from a given point on, picked by ffmpeg's `thumbnail` filter, so it isn't a dark scene, a
//! fade or a title card.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use clamshell_core::Meta;
use clamshell_library::Details;

use super::{Shared, Wanted};
use crate::os::command;

impl Shared {
    /// A video's length, and its most representative frame from a quarter of the way in (see [`frame`])
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

/// A video's length in seconds, from ffprobe.
pub fn duration(file: &Path) -> Option<u32> {
    let mut args: Vec<&OsStr> = ["-n", "19", "ffprobe", "-v", "quiet", "-show_entries", "format=duration", "-of", "csv=p=0"].map(OsStr::new).into();
    args.push(file.as_os_str());
    let out = command::run("nice", &args, None, PROBE_LIMIT).inspect_err(|e| log::debug!("probe: {}: {e}", file.display())).ok()?;
    let seconds: f64 =
        String::from_utf8_lossy(&out.stdout).trim().parse().inspect_err(|e| log::debug!("probe: {}: no length: {e} ({})", file.display(), out.status)).ok()?;
    (seconds > 0.0).then_some(seconds.round() as u32)
}

/// How long ffprobe may take to read a video's length, and ffmpeg to pick a frame. ffmpeg decodes only
/// keyframes, but a network mount may be slow.
const PROBE_LIMIT: Duration = Duration::from_secs(30);
const FRAME_LIMIT: Duration = Duration::from_secs(120);

/// How many keyframes [`frame`] chooses among from its starting point, which covers a few minutes of a
/// video.
const FRAMES_LOOKED_AT: u32 = 60;

/// Writes a frame of `file`, 640 pixels wide, to `out` as a JPEG. It is written whole, then renamed into
/// place. The frame is the most representative of the [`FRAMES_LOOKED_AT`] keyframes from `at` seconds
/// on, picked by ffmpeg's `thumbnail` filter; using only keyframes keeps it quick. Returns whether it
/// worked.
pub fn frame(file: &Path, at: u32, out: &Path) -> bool {
    if let Some(dir) = out.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        log::debug!("probe: {}: {e}", dir.display());
    }
    let part = out.with_extension("part.jpg");
    let (choose, at) = (format!("scale=640:-2,thumbnail={FRAMES_LOOKED_AT}"), at.to_string());
    let mut args: Vec<&OsStr> = ["-n", "19", "ffmpeg", "-v", "quiet", "-y", "-skip_frame", "nokey", "-ss", &at, "-i"].map(OsStr::new).into();
    args.push(file.as_os_str());
    args.extend(["-vf", &choose, "-frames:v", "1", "-fps_mode", "vfr", "-q:v", "4"].map(OsStr::new));
    args.push(part.as_os_str());
    let ok = match command::run("nice", &args, None, FRAME_LIMIT) {
        Ok(ran) if ran.status.success() => true,
        Ok(ran) => {
            log::debug!("probe: a frame of {}: ffmpeg {}", file.display(), ran.status);
            false
        }
        Err(e) => {
            log::debug!("probe: a frame of {}: {e}", file.display());
            false
        }
    };
    ok && std::fs::rename(&part, out).inspect_err(|e| log::warn!("probe: {}: {e}", out.display())).is_ok()
}
