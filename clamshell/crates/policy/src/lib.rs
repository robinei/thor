//! Threads that watch the system and carry out what the state asks for: here the clock (with waking from
//! sleep) and the bar's status; in the modules the pad and touchpad paths (evdev, uinput), sway, systemd,
//! NetworkManager, BlueZ, PipeWire, UPower, logind and sysfs. Nothing here waits for the UI.

pub mod bluetooth;
pub mod command;
pub mod controls;
pub mod desktop;
pub mod frames;
pub mod job;
pub mod keyboard;
pub mod launcher;
pub mod library;
pub mod metrics;
pub mod mixer;
pub mod pad;
pub mod paths;
pub mod power;
pub mod sleep;
pub mod sway;
pub mod touchpad;
pub mod tuning;
pub mod units;
pub mod wifi;

use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clamshell_core::{Event, Reading, Status};

/// Where policy threads deliver events. Returns false once the receiver is gone, and the thread stops.
pub trait Sink: Send + 'static {
    fn send(&self, event: Event) -> bool;
}

impl<F: Fn(Event) -> bool + Send + 'static> Sink for F {
    fn send(&self, event: Event) -> bool {
        self(event)
    }
}

/// Sends a `Tick` at the start of every second, and `Resumed` after the system slept.
pub fn spawn_clock(sink: impl Sink) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name("clock".into()).spawn(move || {
        let mut asleep = sleep::Watch::new(sleep::slept());
        loop {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            if asleep.woke(sleep::slept()) {
                log::info!("woke from sleep");
                if !sink.send(Event::Resumed) {
                    return;
                }
            }
            if !sink.send(Event::Tick { unix_secs: now.as_secs() }) {
                return;
            }
            let into_second = Duration::from_nanos(u64::from(now.subsec_nanos()));
            thread::sleep(Duration::from_secs(1).saturating_sub(into_second));
        }
    })
}

/// How often the status is read: battery and Wi-Fi change slowly; a volume changed elsewhere (not with
/// ClamShell's own controls, which show it at once) shows within this.
const STATUS_EVERY: Duration = Duration::from_secs(5);

/// Reads battery, Wi-Fi and volume every few seconds and sends them when they change.
pub fn spawn_status(sink: impl Sink) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name("status".into()).spawn(move || {
        let mut last = None;
        loop {
            let status = read_status();
            if last != Some(status) {
                if !sink.send(Event::Reading(Reading::Status(status))) {
                    return;
                }
                last = Some(status);
            }
            thread::sleep(STATUS_EVERY);
        }
    })
}

fn read_status() -> Status {
    let (battery, charging) = read_battery().unwrap_or((None, false));
    let wifi = std::fs::read_to_string("/proc/net/wireless").ok().as_deref().and_then(parse_wireless);
    let (volume, muted) = command::output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .and_then(|out| String::from_utf8(out).ok())
        .as_deref()
        .and_then(mixer::parse_get_volume)
        .map_or((None, false), |(v, m)| (Some(v), m));
    Status { battery, charging, wifi, volume, muted, brightness: controls::brightness() }
}

/// The first power supply of type Battery that powers the system: its capacity and whether it is charging (or full on power).
fn read_battery() -> Option<(Option<u8>, bool)> {
    for entry in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let dir = entry.path();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).map(|s| s.trim().to_owned()).ok();
        // The system's battery, not a mouse's or a headset's (those have the scope Device).
        if read("type").as_deref() != Some("Battery") || read("scope").as_deref() == Some("Device") {
            continue;
        }
        let capacity = read("capacity").and_then(|c| c.parse::<u8>().ok()).map(|c| c.min(100));
        let charging = matches!(read("status").as_deref(), Some("Charging" | "Full"));
        return Some((capacity, charging));
    }
    None
}

/// Link quality (0-100) of the first interface in `/proc/net/wireless`; `None` if none is listed (not
/// connected). Quality is out of 70 on most drivers.
fn parse_wireless(text: &str) -> Option<u8> {
    let line = text.lines().nth(2)?;
    let quality: f32 = line.split_whitespace().nth(2)?.trim_end_matches('.').parse().ok()?;
    Some((quality / 70.0 * 100.0).clamp(0.0, 100.0) as u8)
}


#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn wireless_quality_as_percent() {
        let text = "Inter-| sta-|   Quality        |   Discarded packets\n face | tus | link level noise |  nwid\n wlan0: 0000   56.  -54.  -256        0      0      0\n";
        assert_eq!(parse_wireless(text), Some(80));
        assert_eq!(parse_wireless("Inter-|\n face |\n"), None);
    }

    #[test]
    fn wpctl_volume_and_mute() {
        assert_eq!(mixer::parse_get_volume("Volume: 0.60\n"), Some((60, false)));
        assert_eq!(mixer::parse_get_volume("Volume: 0.35 [MUTED]\n"), Some((35, true)));
        assert_eq!(mixer::parse_get_volume("garbage"), None);
    }
}
