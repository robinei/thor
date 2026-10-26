//! Calibrating the top screen's refresh rates with thor-panel-calibration.

use std::time::Duration;

use crate::os::command;

/// How long the top screen's calibration may take. It measures each rate for a few seconds.
const CALIBRATION_LIMIT: Duration = Duration::from_secs(600);

/// Runs thor-panel-calibration. Returns the lines in which it reports the rates it found, or why it
/// failed.
pub(crate) fn calibrate_refresh(program: Option<&str>) -> Result<String, String> {
    let program = program.ok_or_else(|| "refresh calibration unavailable for this panel".to_owned())?;
    let out = command::run::<&str>(program, &[], None, CALIBRATION_LIMIT)?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_owned();
    if out.status.success() {
        let rates: Vec<String> = text(&out.stdout).lines().filter(|l| l.contains(" Hz: ")).map(str::to_owned).collect();
        Ok(rates.join("; "))
    } else {
        Err(text(&out.stderr).lines().last().unwrap_or("failed").to_owned())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_profile_without_calibration_reports_unavailable_without_running_a_tool() {
        assert_eq!(super::calibrate_refresh(None), Err("refresh calibration unavailable for this panel".into()));
    }
}
