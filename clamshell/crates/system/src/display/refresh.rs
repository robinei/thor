//! Calibrating the top screen's refresh rates with thor-panel-calibration.

use std::time::Duration;

use crate::os::command;

/// How long the top screen's calibration may take. It measures each rate for a few seconds.
const CALIBRATION_LIMIT: Duration = Duration::from_secs(600);

/// Runs thor-panel-calibration. Returns the lines in which it reports the rates it found, or why it
/// failed.
pub fn calibrate_refresh() -> Result<String, String> {
    let out = command::run::<&str>("thor-panel-calibration", &[], None, CALIBRATION_LIMIT)?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_owned();
    if out.status.success() {
        let rates: Vec<String> = text(&out.stdout).lines().filter(|l| l.contains(" Hz: ")).map(str::to_owned).collect();
        Ok(rates.join("; "))
    } else {
        Err(text(&out.stderr).lines().last().unwrap_or("failed").to_owned())
    }
}
