//! Screenshots of both screens: grim, through the compositor's screen capture.

use std::path::PathBuf;

use crate::os::command;

/// Both screens (as the compositor lays them out) into one PNG named by the time; its path, or why not.
pub fn screenshot(dir: Option<&PathBuf>) -> Result<String, String> {
    let dir = dir.ok_or("no folder for screenshots (no HOME)")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let secs = crate::time::clock::unix_secs();
    let file = dir.join(format!("clamshell-{}.png", timestamp(secs)));
    let path = file.to_string_lossy().into_owned();
    if command::output("grim", &[&path]).is_some() {
        log::info!("screenshot: {path}");
        Ok(path)
    } else {
        Err("grim failed (is it installed, and does the compositor allow screen capture?)".into())
    }
}

/// A UTC timestamp for file names: 20261015-103205.
fn timestamp(secs: u64) -> String {
    let at = i64::try_from(secs).ok().and_then(|s| jiff::Timestamp::from_second(s).ok()).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    at.strftime("%Y%m%d-%H%M%S").to_string()
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
