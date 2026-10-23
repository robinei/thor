//! The bar's status: battery, Wi-Fi signal, volume and brightness. They are read every few seconds while
//! the screens are on, and sent when they change. While the screens are off, only the battery is read,
//! once a minute for its warnings, so `pw-dump` isn't started.

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use clamshell_state::{Event, Reading, Status};

use crate::audio::mixer;
use crate::display::brightness::Backlights;
use crate::os::sys;
use crate::os::worker::{Changed, Shown, Worker};
use crate::{Closed, Sink};

/// How often the status is read. Battery and Wi-Fi change slowly, and a volume changed outside ClamShell
/// shows within this time. ClamShell's own controls show their changes at once.
const EVERY: Duration = Duration::from_secs(5);
/// How often only the battery is read while the screens are off.
const UNSEEN_EVERY: Duration = Duration::from_secs(60);

/// Starts the status thread. Its commands say whether the screens are on (see
/// [`clamshell_state::Watch::Status`]).
pub fn spawn(backlights: Backlights, sink: impl Sink) -> io::Result<Worker<Shown>> {
    Worker::spawn("status", move |seen| {
        let _ = follow(&backlights, &sink, &seen);
    })
}

fn follow(backlights: &Backlights, sink: &impl Sink, commands: &Receiver<Shown>) -> Result<(), Closed> {
    let mut last = Changed::default();
    let mut seen = true;
    // While unseen, the battery is read again and the rest stays as last read.
    let mut status = read(backlights);
    loop {
        if seen {
            status = read(backlights);
        } else {
            (status.battery, status.charging, status.discharging) = read_battery().unwrap_or((None, false, false));
        }
        if let Some(changed) = last.offer(status) {
            sink.send(Event::Reading(Reading::Status(changed)))?;
        }
        match commands.recv_timeout(if seen { EVERY } else { UNSEEN_EVERY }) {
            Ok(Shown(on)) => seen = on,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn read(backlights: &Backlights) -> Status {
    let (battery, charging, discharging) = read_battery().unwrap_or((None, false, false));
    let wifi = sys::text("/proc/net/wireless").as_deref().and_then(parse_wireless);
    let (volume, muted) = mixer::read_volume().map_or((None, false), |(v, m)| (Some(v), m));
    Status { battery, charging, discharging, wifi, volume, muted, brightness: backlights.level() }
}

/// The system's battery: its capacity, whether it is charging (or full on power), and whether it is
/// discharging. Discharging means running on battery, not held at a charge limit on the charger, which
/// reads "Not charging".
fn read_battery() -> Option<(Option<u8>, bool, bool)> {
    let dir = sys::system_battery()?;
    let capacity = sys::read::<u8>(dir.join("capacity")).map(|c| c.min(100));
    let status = sys::read::<String>(dir.join("status"));
    let charging = matches!(status.as_deref(), Some("Charging" | "Full"));
    Some((capacity, charging, status.as_deref() == Some("Discharging")))
}

/// Link quality (0-100) of the first interface in `/proc/net/wireless`, or `None` if none is listed
/// because it isn't connected. Quality is out of 70 on most drivers.
fn parse_wireless(text: &str) -> Option<u8> {
    let line = text.lines().nth(2)?;
    let field = line.split_whitespace().nth(2)?;
    let quality: f32 = field.trim_end_matches('.').parse().inspect_err(|e| log::debug!("status: /proc/net/wireless: {field:?}: {e}")).ok()?;
    Some((quality / 70.0 * 100.0).clamp(0.0, 100.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wireless_quality_as_percent() {
        let text = "Inter-| sta-|   Quality        |   Discarded packets\n face | tus | link level noise |  nwid\n wlan0: 0000   56.  -54.  -256        0      0      0\n";
        assert_eq!(parse_wireless(text), Some(80));
        assert_eq!(parse_wireless("Inter-|\n face |\n"), None);
    }
}
