//! MangoHud's files: the config file a game is started with (its frame rate cap, its overlay, its log),
//! the CSV logs it writes (read by [`super::frames`]), and logs written the same way for what it can't
//! measure.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clamshell_core::settings::Hud;

/// The MangoHud config file for a game (in `dir`), written with its frame rate cap (0: none) and HUD
/// (0: invisible, 1: the frame rate only, 2: the full overlay), logging for the FPS tile. Rewritten in
/// place, which MangoHud picks up live.
pub fn config(dir: &Path, game: &str, fps_cap: u16, hud: Hud) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    // (Named by the entry's key, as its scopes, workspace and art are.)
    let path = dir.join(format!("{}.conf", clamshell_library::key_of(game)));
    let overlay = match hud {
        Hud::Off => "alpha=0\nbackground_alpha=0\n",
        Hud::Fps => "fps_only\n",
        Hud::Full => "",
    };
    let text = format!("{overlay}autostart_log=1\nlog_interval=500\noutput_folder={}\nfps_limit={fps_cap}\n", dir.display());
    fs::write(&path, text)?;
    Ok(path)
}

/// The environment that loads MangoHud with this config file.
pub fn env(config: &Path) -> Vec<(String, String)> {
    vec![("MANGOHUD".into(), "1".into()), ("MANGOHUD_CONFIGFILE".into(), config.display().to_string())]
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
                let num = |i: usize| fields.get(i).and_then(|f| f.parse::<f32>().inspect_err(|e| log::debug!("frames: {f:?}: {e}")).ok());
                if let (Some(fps), Some(ms)) = (num(a), num(b)) {
                    latest = Some((fps, ms));
                }
            }
        }
        latest
    }
}

/// Frame rate logs written MangoHud's way, for apps it can't measure (Waydroid's: see
/// [`crate::waydroid`]): a file per app, its header first.
pub struct FrameLog {
    dir: PathBuf,
    files: HashMap<String, File>,
}

impl FrameLog {
    pub fn new(dir: &Path) -> FrameLog {
        FrameLog { dir: dir.to_path_buf(), files: HashMap::new() }
    }

    /// A row: `fps` now for `app`.
    pub fn row(&mut self, app: &str, fps: f32) {
        let name = format!("android-{}.csv", app.replace(['/', ' '], "_"));
        if !self.files.contains_key(&name) {
            if let Err(e) = std::fs::create_dir_all(&self.dir) {
                log::debug!("frame log: {}: {e}", self.dir.display());
            }
            match OpenOptions::new().create(true).truncate(true).write(true).open(self.dir.join(&name)) {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(b"fps,frametime\n") {
                        log::debug!("frame log: {name}: {e}");
                    }
                    self.files.insert(name.clone(), f);
                }
                Err(e) => return log::debug!("frame log: {name}: {e}"),
            }
        }
        if let Some(f) = self.files.get_mut(&name) {
            let frame_ms = if fps > 0.0 { 1000.0 / fps } else { 0.0 };
            if let Err(e) = writeln!(f, "{fps:.1},{frame_ms:.2}") {
                log::debug!("frame log: {name}: {e}");
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const HEAD: &str = "os,cpu,gpu\nCachyOS,AMD,AMD\nfps,frametime,cpu_load\n";

    #[test]
    fn a_games_config_file_carries_its_cap_and_hud() {
        let dir = clamshell_core::kit::testdir::TempDir::new("tuning");
        let a = config(&dir, "steam:1", 40, Hud::Off).unwrap();
        let text = fs::read_to_string(&a).unwrap();
        assert!(text.contains("fps_limit=40\n") && text.contains("alpha=0\n"));
        let again = config(&dir, "steam:1", 0, Hud::Fps).unwrap();
        assert_eq!(a, again, "the same file, rewritten");
        let text = fs::read_to_string(&a).unwrap();
        assert!(text.contains("fps_limit=0\n") && text.contains("fps_only\n") && !text.contains("alpha"));
        assert_ne!(config(&dir, "steam:2", 0, Hud::Off).unwrap(), a);
    }

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
}
