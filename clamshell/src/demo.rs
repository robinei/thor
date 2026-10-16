//! Made-up readings for the snapshots: a status with every bar icon, Wi-Fi networks, Bluetooth devices,
//! audio, a day of battery and a minute of metrics, running instances and their windows.

use clamshell_core::battery::{Battery, Charge, Group, Spender};
use clamshell_core::bluetooth::{self, Bluetooth, Kind};
use clamshell_core::levels::{Audio, Output, Stream};
use clamshell_core::metrics::Metrics;
use clamshell_core::wifi::{Network, Wifi};
use clamshell_core::windows::Window;
use clamshell_core::{Event, GameEvent, Id, Instance, PointerEvent, Reading, Status, Tab};

/// Every bar icon shows.
pub const STATUS: Status = Status { battery: Some(72), charging: true, wifi: Some(80), volume: Some(60), muted: false, brightness: Some(70) };

/// Low battery, no Wi-Fi, muted.
pub const STATUS_LOW: Status = Status { battery: Some(9), charging: false, wifi: None, volume: Some(40), muted: true, brightness: None };

/// The game running, two terminals and a browser, and their windows (the second terminal's in front).
pub fn running() -> Vec<Event> {
    let window = |id: i64, app_id: &str, unit: &str, focused: bool| Window {
        id,
        app_id: app_id.into(),
        title: String::new(),
        focused,
        unit: Some(unit.into()),
        workspace: unit.trim_end_matches(".scope").replace("clamshell-game-1-0", &Instance::Landing.name()),
    };
    vec![
        Event::Game(GameEvent::Started("zelda-lttp".into())),
        Event::Units(
            ["clamshell-game-1-0", "clamshell-app-terminal-1-0", "clamshell-app-terminal-1-1", "clamshell-app-firefox-1-2"]
                .into_iter()
                .filter_map(Instance::parse)
                .collect(),
        ),
        Event::Windows(vec![
            window(5, "retroarch", "clamshell-game-1-0.scope", false),
            window(7, "foot", "clamshell-app-terminal-1-0.scope", false),
            window(8, "foot", "clamshell-app-terminal-1-1.scope", true),
            window(9, "firefox", "clamshell-app-firefox-1-2.scope", false),
        ]),
    ]
}

/// Four networks in range, one connected.
pub fn wifi() -> Wifi {
    Wifi {
        device: true,
        enabled: true,
        networks: [
            ("home", 82, true, true, true),
            ("cafe-guest", 64, false, false, false),
            ("neighbours", 41, true, false, false),
            ("office", 30, true, true, false),
        ]
        .map(|(ssid, strength, secure, known, active)| Network { ssid: ssid.into(), strength, secure, known, active, enterprise: false })
        .to_vec(),
        connecting: None,
        error: None,
    }
}

/// Many networks (a city street): more than the list shows.
pub fn wifi_crowded() -> Wifi {
    let mut wifi = wifi();
    let more = [
        "FRITZ!Box 7590",
        "TP-Link_5G",
        "eduroam",
        "Vodafone Hotspot",
        "DIRECT-printer",
        "Bakery",
        "Library Guest",
        "AndroidAP",
        "Linksys",
        "Hotel Lobby",
        "upstairs",
        "garden",
    ];
    wifi.networks.extend(more.iter().enumerate().map(|(i, ssid)| Network {
        ssid: (*ssid).into(),
        strength: 60 - 4 * i as u8,
        secure: i % 3 != 1,
        known: false,
        active: false,
        enterprise: false,
    }));
    wifi
}

/// Four devices: paired and connected, paired, and new ones.
pub fn bluetooth() -> Bluetooth {
    Bluetooth {
        adapter: true,
        powered: true,
        devices: [
            ("WH-1000XM4", Kind::Audio, true, true),
            ("Xbox Wireless Controller", Kind::Gamepad, true, false),
            ("MX Keys", Kind::Keyboard, false, false),
            ("Pixel 8", Kind::Phone, false, false),
        ]
        .map(|(name, kind, paired, connected)| bluetooth::Device {
            address: format!("00:11:22:33:44:{:02X}", name.len()),
            name: name.into(),
            kind,
            paired,
            connected,
        })
        .to_vec(),
        busy: None,
        error: None,
    }
}

/// Two outputs and three apps playing, one muted.
pub fn audio() -> Audio {
    Audio {
        outputs: vec![Output { id: 50, name: "Speakers".into(), default: true }, Output { id: 51, name: "WH-1000XM4".into(), default: false }],
        streams: [("Firefox", 80, false), ("Steam", 100, false), ("Spotify", 35, true)]
            .map(|(app, volume, muted)| Stream { id: 90, app: app.into(), title: String::new(), volume, muted })
            .to_vec(),
    }
}

/// A made-up day of charge (down while playing, charging overnight) and a busy moment, then the battery
/// tab.
pub fn battery(now: u64) -> Vec<Event> {
    let mut level = 64.0f32;
    let history = (0..96u64)
        .map(|i| {
            let charging = (40..58).contains(&i);
            level = if charging { (level + 3.2).min(100.0) } else { (level - if (70..84).contains(&i) { 2.4 } else { 0.5 }).max(5.0) };
            Charge { at: now - 24 * 3600 + i * 900, level: level as u8, charging }
        })
        .collect();
    let spender = |group: Group, cpu: f32, gpu: f32| Spender { group, cpu, gpu };
    let spenders = vec![
        spender(Group::Instance(Instance::Game { secs: 1_760_000_000, n: 0 }), 0.31, 0.62),
        spender(Group::Process("firefox".into()), 0.08, 0.03),
        spender(Group::Instance(Instance::App { key: "terminal".into(), secs: 1_760_000_000, n: 1 }), 0.02, 0.0),
        spender(Group::System, 0.03, 0.0),
        spender(Group::Process("clamshell".into()), 0.01, 0.0),
    ];
    vec![
        Event::Game(GameEvent::Started("zelda-lttp".into())),
        Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Battery))),
        Event::Reading(Reading::Battery(Battery { history, spenders, has_history: true })),
        Event::Reading(Reading::Status(Status { battery: Some(41), charging: false, ..STATUS })),
    ]
}

/// A minute of made-up readings, then the sysinfo tab.
pub fn metrics() -> Vec<Event> {
    let mut events: Vec<Event> = (0..60u32)
        .map(|i| {
            let t = i as f32 / 60.0;
            let wave = |phase: f32| (0.5 + 0.4 * ((t * 9.0 + phase).sin())).clamp(0.0, 1.0);
            let mut cores = [0.0; 16];
            for (k, c) in cores.iter_mut().take(8).enumerate() {
                *c = wave(k as f32);
            }
            Event::Reading(Reading::Metrics(Metrics {
                cpu: Some(wave(0.0) * 0.6),
                cores,
                core_count: 8,
                cpu_mhz: Some(2900),
                gpu: Some(wave(1.5)),
                gpu_mhz: Some(680),
                cpu_temp: Some(58.0),
                gpu_temp: Some(52.0),
                mem_used: 6_400_000_000,
                mem_total: 16_000_000_000,
                swap_used: 0,
                power_w: Some(6.0 + 3.0 * wave(3.0)),
                minutes_left: Some(190),
                fps: Some(55.0 + 5.0 * wave(2.0)),
                frame_ms: Some(16.9),
            }))
        })
        .collect();
    events.push(Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Sysinfo))));
    events
}
