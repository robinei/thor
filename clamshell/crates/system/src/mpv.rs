//! Where mpv left a video, and how long a video is. With `save-position-on-quit`, mpv keeps the place of a
//! video quit before its end in a file of its watch_later folder, named by the MD5 (in capitals) of the
//! video's path, or of its file name alone with `ignore-path-in-watch-later-config`; the line
//! `start=<seconds>` is the place. A video watched to its end has none. Both names are looked for (the path's
//! first), in the folder mpv.conf names (`watch-later-dir`) and in mpv's own (~/.local/state/mpv/watch_later,
//! older ~/.config/mpv/watch_later). The length is ffprobe's; a frame to show for it, ffmpeg's (both at the
//! lowest priority: a game started meanwhile comes first): of the keyframes from a point on, the most
//! representative (its `thumbnail` filter), so not a dark scene, a fade or a title card.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::command;

/// The folders mpv keeps places in, the one mpv.conf names first.
pub fn watch_later_dirs(config_home: Option<&Path>, state_home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let conf = config_home.map(|c| c.join("mpv/mpv.conf"));
    if let Some(text) = conf.and_then(|c| std::fs::read_to_string(c).ok()) {
        for line in text.lines().map(str::trim) {
            if let Some(dir) = line.strip_prefix("watch-later-dir=").or_else(|| line.strip_prefix("watch-later-directory=")) {
                let dir = dir.trim().trim_matches('"');
                // (mpv's `~/`, and `~~state/`, `~~/` for its own folders.)
                let dir = match (dir.strip_prefix("~~state/"), dir.strip_prefix("~~/"), dir.strip_prefix("~/")) {
                    (Some(rest), _, _) => state_home.map(|s| s.join("mpv").join(rest)),
                    (_, Some(rest), _) => config_home.map(|c| c.join("mpv").join(rest)),
                    (_, _, Some(rest)) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)),
                    _ => Some(PathBuf::from(dir)),
                };
                dirs.extend(dir);
            }
        }
    }
    dirs.extend(state_home.map(|s| s.join("mpv/watch_later")));
    dirs.extend(config_home.map(|c| c.join("mpv/watch_later")));
    dirs
}

/// Where mpv left `file` (seconds), if it kept a place, and when it did (its file's time, seconds since
/// the epoch).
pub fn position(dirs: &[PathBuf], file: &Path) -> Option<(u32, u64)> {
    let path = file.to_string_lossy();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let names = [md5_hex(path.as_bytes()), md5_hex(name.as_bytes())];
    let kept = names.iter().flat_map(|n| dirs.iter().map(move |d| d.join(n))).find(|f| f.is_file())?;
    let text = std::fs::read_to_string(&kept).ok()?;
    let start = text.lines().find_map(|l| l.trim().strip_prefix("start="))?;
    let at = std::fs::metadata(&kept).and_then(|m| m.modified()).ok().map_or(0, clamshell_core::unix_secs_at);
    start.trim().parse::<f64>().ok().filter(|s| *s >= 0.0).map(|s| (s as u32, at))
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

/// MD5 of `data` as 32 capital hex digits (as mpv names its watch_later files).
fn md5_hex(data: &[u8]) -> String {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4,
        11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i: i32| (f64::from(i + 1).sin().abs() * 4_294_967_296.0) as u32).collect();
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_le_bytes());
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    for chunk in message.chunks_exact(64) {
        let m: Vec<u32> = chunk.chunks_exact(4).filter_map(|w| <[u8; 4]>::try_from(w).ok()).map(u32::from_le_bytes).collect();
        let [mut a, mut b, mut c, mut d] = state;
        for (i, (&k, &shift)) in k.iter().zip(SHIFTS.iter()).enumerate() {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let word = m.get(g).copied().unwrap_or(0);
            let rotated = a.wrapping_add(f).wrapping_add(k).wrapping_add(word).rotate_left(shift);
            (a, d, c) = (d, c, b);
            b = b.wrapping_add(rotated);
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    state.iter().flat_map(|s| s.to_le_bytes()).map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn md5_as_mpv_names_its_files() {
        assert_eq!(md5_hex(b""), "D41D8CD98F00B204E9800998ECF8427E");
        assert_eq!(md5_hex(b"The quick brown fox jumps over the lazy dog"), "9E107D9D372BB6826BD81D3542A419D6");
        assert_eq!(md5_hex(b"Silo S02E01.mp4"), "BCD88637A55DC78D245B2F1BCFF455D4");
    }

    #[test]
    fn a_place_by_the_path_or_the_name_in_the_folder_mpv_conf_names() {
        let root = clamshell_core::testdir::TempDir::new("mpv");
        let (config, state) = (root.join("config"), root.join("state"));
        std::fs::create_dir_all(config.join("mpv")).unwrap();
        std::fs::write(config.join("mpv/mpv.conf"), "save-position-on-quit=yes\nwatch-later-dir=~~state/places\n").unwrap();
        let dirs = watch_later_dirs(Some(&config), Some(&state));
        assert_eq!(dirs, [state.join("mpv/places"), state.join("mpv/watch_later"), config.join("mpv/watch_later")]);
        let film = Path::new("/videos/Film.mkv");
        assert_eq!(position(&dirs, film), None, "none kept");
        std::fs::create_dir_all(&dirs[1]).unwrap();
        std::fs::write(dirs[1].join(md5_hex(b"Film.mkv")), "# Film.mkv\nstart=1403.402000\nsid=1\n").unwrap();
        assert_eq!(position(&dirs, film).map(|(s, _)| s), Some(1403), "by its name");
        assert!(position(&dirs, film).is_some_and(|(_, at)| at > 1_700_000_000), "and when");
        std::fs::create_dir_all(&dirs[0]).unwrap();
        std::fs::write(dirs[0].join(md5_hex(b"/videos/Film.mkv")), "start=60.5\n").unwrap();
        assert_eq!(position(&dirs, film).map(|(s, _)| s), Some(60), "by its path first");
    }
}
