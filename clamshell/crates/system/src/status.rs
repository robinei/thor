//! The bar's status: battery, Wi-Fi signal, volume and brightness, read every few seconds and sent when
//! they change.

use std::io;
use std::thread;
use std::time::Duration;

use clamshell_core::{Event, Reading, Status};

use crate::brightness::Backlights;
use crate::worker::{self, Changed};
use crate::{Closed, Sink, command, mixer, sys};

/// How often the status is read: battery and Wi-Fi change slowly; a volume changed elsewhere (not with
/// ClamShell's own controls, which show it at once) shows within this.
const EVERY: Duration = Duration::from_secs(5);

pub fn spawn(backlights: Backlights, sink: impl Sink) -> io::Result<()> {
    worker::detached("status", move || {
        let _ = follow(&backlights, &sink);
    })
}

fn follow(backlights: &Backlights, sink: &impl Sink) -> Result<(), Closed> {
    let mut last = Changed::default();
    loop {
        if let Some(status) = last.offer(read(backlights)) {
            sink.send(Event::Reading(Reading::Status(status)))?;
        }
        thread::sleep(EVERY);
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
