//! Made-up readings for the snapshots: a status with every bar icon, Wi-Fi networks, Bluetooth devices,
//! audio, a day of battery, a minute of metrics, and running instances with their windows.

use clamshell_state::levels::{Audio, AudioOutput, Stream};
use clamshell_state::radios::bluetooth::{self, Bluetooth, Kind};
use clamshell_state::radios::wifi::{Network, Wifi};
use clamshell_state::session::windows::WindowFacts;
use clamshell_state::status::battery::{Battery, Charge, Group, Spender};
use clamshell_state::status::metrics::Metrics;
use clamshell_state::{Event, Id, LaunchUnit, PointerEvent, Reading, Status, Tab, WindowEvent};

/// A status that shows every bar icon.
pub const STATUS: Status =
    Status { battery: Some(72), charging: true, discharging: false, wifi: Some(80), volume: Some(60), muted: false, brightness: Some(70) };

/// Low battery, no Wi-Fi, muted.
pub const STATUS_LOW: Status = Status { battery: Some(9), charging: false, discharging: true, wifi: None, volume: Some(40), muted: true, brightness: None };

/// A launch of entry `id`, the `n`th thing started.
pub fn run(id: &str, n: u64) -> LaunchUnit {
    LaunchUnit { key: clamshell_library::key_of(id), secs: 1, n }
}

/// A window of a launch.
fn window_of(id: i64, app_id: &str, launch: &LaunchUnit) -> WindowFacts {
    WindowFacts { id, app_id: app_id.into(), unit: Some(launch.service()), ..WindowFacts::default() }
}

/// These windows opening, reported by the compositor in one batch.
pub fn opened(windows: Vec<WindowFacts>) -> Event {
    Event::WindowBatch(windows.into_iter().map(WindowEvent::Opened).collect())
}

/// A client asks to show window `id`, as a program does once it is up.
fn activate(id: i64) -> Event {
    WindowEvent::Activate(id).into()
}

/// A game, two terminals and a browser running, with their windows. The second terminal is in front.
pub fn running() -> Vec<Event> {
    let (game, t1, t2, web) = (run("zelda-lttp", 0), run("desktop:foot", 1), run("desktop:foot", 2), run("desktop:firefox", 3));
    // Their units as systemd lists them, which marks them as running.
    vec![
        Event::Units(vec![game.clone(), t1.clone(), t2.clone(), web.clone()]),
        opened(vec![window_of(5, "retroarch", &game), window_of(7, "foot", &t1), window_of(8, "foot", &t2), window_of(9, "firefox", &web)]),
        activate(8),
    ]
}

/// The terminal in front from [`running`], with a second window of it moved to the bottom screen by the
/// snapshots' window rule (see [`BELOW`]), and the dock's tab shown again over it.
pub fn window_below() -> Vec<Event> {
    let t2 = run("desktop:foot", 2);
    let below = WindowFacts { title: BELOW.into(), ..window_of(10, "foot", &t2) };
    vec![
        Event::WindowBatch(vec![WindowEvent::Closed(5), WindowEvent::Closed(7), WindowEvent::Closed(9), WindowEvent::Opened(below)]),
        Event::HardwareKey(clamshell_state::HardwareKey::BottomView),
    ]
}

/// The title of the terminal's window that the snapshots' window rule sends to the bottom screen.
pub const BELOW: &str = "below";

/// The screens as the compositor describes them with the VM's HDR profile: the Thor's top panel with
/// SDR white at the reference, 650 nits at its peak; its second panel, SDR.
pub fn screens() -> clamshell_state::presentation::Screens {
    let profile = clamshell_state::DeviceProfile::vm_hdr();
    clamshell_state::presentation::Screens { colour: [profile.top.colour.into(), profile.bottom.colour.into()], top_size: (1920, 1080) }
}

/// `entry` running, with its window on the top screen sending HDR: PQ mastered to 1000 nits if `declared`,
/// else scRGB with no peak given.
/// An SDR game's window for `entry`, and a histogram of its picture as a game's might be: mostly dark
/// and mid tones, some highlights, and a spike of clipped white.
pub fn sdr_game_measured(entry: &str) -> Vec<Event> {
    use clamshell_state::colour::{HISTOGRAM_BINS, Histogram};
    let launch = run(entry, 0);
    let window = window_of(30, "game", &launch);
    let mut bins = [0u16; HISTOGRAM_BINS];
    for (i, bin) in bins.iter_mut().enumerate() {
        let x = (i as f64 + 0.5) / HISTOGRAM_BINS as f64;
        let hump = |at: f64, wide: f64, high: f64| high * (-((x - at) / wide).powi(2)).exp();
        *bin = (hump(0.12, 0.07, 260.0) + hump(0.42, 0.14, 150.0) + hump(0.82, 0.06, 45.0) + 6.0) as u16;
    }
    if let Some(last) = bins.last_mut() {
        *last += 140;
    }
    let histogram = Histogram { window: 30, hdr: false, bins };
    vec![Event::Units(vec![launch]), opened(vec![window]), activate(30), Event::Reading(clamshell_state::Reading::Histogram(Box::new(histogram)))]
}

pub fn hdr_game(entry: &str, declared: bool) -> Vec<Event> {
    use clamshell_state::colour::{ColourDescription, ColourPrimaries, ColourTarget, TransferFunction};
    let launch = run(entry, 0);
    let colour = if declared {
        ColourDescription {
            primaries: ColourPrimaries::Bt2020,
            transfer: TransferFunction::Pq,
            luminance: None,
            target: Some(ColourTarget { luminance: Some((0, 1000)), max_cll: Some(1000), ..ColourTarget::default() }),
        }
    } else {
        ColourDescription { primaries: ColourPrimaries::Srgb, transfer: TransferFunction::Linear, luminance: None, target: None }
    };
    let window = WindowFacts { colour: Some(std::sync::Arc::new(colour)), ..window_of(30, "game", &launch) };
    vec![Event::Units(vec![launch]), opened(vec![window]), activate(30)]
}

/// Four networks in range, one connected.
pub fn wifi() -> Wifi {
    Wifi {
        present: true,
        on: true,
        networks: [
            ("home", 82, true, true, true),
            ("cafe-guest", 64, false, false, false),
            ("neighbours", 41, true, false, false),
            ("office", 30, true, true, false),
        ]
        .map(|(ssid, strength, secure, saved, connected)| Network { ssid: ssid.into(), strength, secure, saved, connected, enterprise: false })
        .to_vec(),
        busy: None,
        error: None,
    }
}

/// Many networks, as on a city street: more than the list shows.
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
        saved: false,
        connected: false,
        enterprise: false,
    }));
    wifi
}

/// Four devices: paired and connected, paired, and new ones.
pub fn bluetooth() -> Bluetooth {
    Bluetooth {
        present: true,
        on: true,
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
        outputs: vec![AudioOutput { id: 50, name: "Speakers".into(), default: true }, AudioOutput { id: 51, name: "WH-1000XM4".into(), default: false }],
        streams: [("Firefox", 80, false), ("Steam", 100, false), ("Spotify", 35, true)]
            .map(|(app, volume, muted)| Stream { id: 90, app: app.into(), title: String::new(), volume, muted })
            .to_vec(),
    }
}

/// A made-up day of charge, falling while playing and charging overnight, and a busy moment's spenders.
/// The battery tab is opened.
pub fn battery(now: u64) -> Vec<Event> {
    let mut level = 64.0f32;
    let history = (0..96u64)
        .map(|i| {
            let charging = (40..58).contains(&i);
            level = if charging { (level + 3.2).min(100.0) } else { (level - if (70..84).contains(&i) { 2.4 } else { 0.5 }).max(5.0) };
            Charge { at: now - 24 * 3600 + i * 900, level: level as u8, charging }
        })
        .collect();
    let spender = |group: Group, cpu: f32, gpu: f32, mb: u64| Spender { endable: group != Group::System, group, cpu, gpu, memory: mb << 20, pids: vec![] };
    let spenders = vec![
        spender(Group::Launch(run("zelda-lttp", 0)), 0.31, 0.62, 480),
        spender(Group::Process("firefox".into()), 0.08, 0.03, 1350),
        spender(Group::Launch(run("desktop:foot", 1)), 0.02, 0.0, 42),
        spender(Group::System, 0.03, 0.0, 610),
        spender(Group::Process("clamshell".into()), 0.01, 0.0, 85),
        spender(Group::Process("steam".into()), 0.0, 0.0, 720),
    ];
    vec![
        Event::Units(vec![run("zelda-lttp", 0)]),
        Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Battery))),
        Event::Reading(Reading::Battery(Battery { history, spenders, has_history: true })),
        Event::Reading(Reading::Status(Status { battery: Some(41), charging: false, ..STATUS })),
    ]
}

/// A stick calibration halfway through. The centres are taken, the left stick has been turned up and
/// right so far, and the right stick is resting.
pub fn sticks_progress() -> clamshell_state::setup::sticks::Progress {
    use clamshell_state::setup::sticks::{Phase, Progress, StickAxis};
    let axis = |low, high| StickAxis { low, centre: 0, high };
    Progress { phase: Phase::Turn, now: [620, -540, 10, -5], seen: [axis(-80, 760), axis(-700, 60), axis(-10, 15), axis(-8, 6)], min: -1024, max: 1024 }
}

/// A sample of the time zones as timedated lists them, with Europe/Oslo set.
pub fn time_zones() -> clamshell_state::TimeZones {
    let zones = [
        "Africa/Cairo",
        "Africa/Lagos",
        "America/Argentina/Buenos_Aires",
        "America/Chicago",
        "America/New_York",
        "America/Sao_Paulo",
        "Asia/Tokyo",
        "Australia/Sydney",
        "Europe/Amsterdam",
        "Europe/Berlin",
        "Europe/Copenhagen",
        "Europe/Helsinki",
        "Europe/Lisbon",
        "Europe/London",
        "Europe/Madrid",
        "Europe/Oslo",
        "Europe/Paris",
        "Europe/Rome",
        "Europe/Stockholm",
        "Europe/Vienna",
        "Europe/Zurich",
        "Pacific/Auckland",
        "UTC",
    ];
    clamshell_state::TimeZones { current: Some("Europe/Oslo".into()), zones: zones.map(String::from).to_vec(), ntp: Some(true), synced: true, error: None }
}

/// A minute of made-up readings, with the sysinfo tab opened.
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
                fan_rpm: Some(3100),
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
