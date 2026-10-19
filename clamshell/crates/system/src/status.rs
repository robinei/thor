//! The bar's status: battery, Wi-Fi signal, volume and brightness, read every few seconds while the
//! screens are on and sent when they change; while they are off, only the battery, once a minute (for
//! its warnings), without starting `wpctl`.

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use clamshell_core::{Event, Reading, Status};

use crate::brightness::Backlights;
use crate::worker::{Changed, Worker};
use crate::{Closed, Sink, command, mixer, sys};

/// How often the status is read: battery and Wi-Fi change slowly; a volume changed elsewhere (not with
/// ClamShell's own controls, which show it at once) shows within this.
const EVERY: Duration = Duration::from_secs(5);
/// How often only the battery is read while the screens are off.
const UNSEEN_EVERY: Duration = Duration::from_secs(60);

/// The status thread; its commands: whether the screens are on (see [`clamshell_core::Watch::Status`]).
pub fn spawn(backlights: Backlights, sink: impl Sink) -> io::Result<Worker<bool>> {
    Worker::spawn("status", move |seen| {
        let _ = follow(&backlights, &sink, &seen);
    })
}

fn follow(backlights: &Backlights, sink: &impl Sink, commands: &Receiver<bool>) -> Result<(), Closed> {
    let mut last = Changed::default();
    let mut seen = true;
    // (Unseen: the rest as last read, the battery read anew.)
    let mut status = read(backlights);
    loop {
        if seen {
            status = read(backlights);
        } else {
            (status.battery, status.charging) = read_battery().unwrap_or((None, false));
        }
        if let Some(changed) = last.offer(status) {
            sink.send(Event::Reading(Reading::Status(changed)))?;
        }
        match commands.recv_timeout(if seen { EVERY } else { UNSEEN_EVERY }) {
            Ok(on) => seen = on,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn read(backlights: &Backlights) -> Status {
    let (battery, charging) = read_battery().unwrap_or((None, false));
    let wifi = sys::text("/proc/net/wireless").as_deref().and_then(parse_wireless);
    let (volume, muted) = command::output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .and_then(|out| String::from_utf8(out).ok())
        .as_deref()
        .and_then(mixer::parse_get_volume)
        .map_or((None, false), |(v, m)| (Some(v), m));
    Status { battery, charging, wifi, volume, muted, brightness: backlights.level() }
}

/// The system's battery: its capacity and whether it is charging (or full on power).
fn read_battery() -> Option<(Option<u8>, bool)> {
    let dir = sys::system_battery()?;
    let capacity = sys::read::<u8>(dir.join("capacity")).map(|c| c.min(100));
    let charging = matches!(sys::read::<String>(dir.join("status")).as_deref(), Some("Charging" | "Full"));
    Some((capacity, charging))
}

/// Link quality (0-100) of the first interface in `/proc/net/wireless`; `None` if none is listed (not
/// connected). Quality is out of 70 on most drivers.
fn parse_wireless(text: &str) -> Option<u8> {
    let line = text.lines().nth(2)?;
    let quality: f32 = line.split_whitespace().nth(2)?.trim_end_matches('.').parse().ok()?;
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
