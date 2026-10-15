//! ClamShell: shelf, the second screen, the bar, input and power policy in one process with one state.

use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};

use clamshell_core::keyboard::{self, Key, KeyPos, Modifier};
use clamshell_core::touch::TouchLayout;
use clamshell_core::settings::{Setting, Settings, Values};
use clamshell_core::{Effect, Event, Id, Tab, Scope, State, Status};
use clamshell_policy::{controls, job, keyboard as virtual_keyboard, pad, sway, touchpad, tuning};
use clamshell_ui::{Config, Screen};

fn main() -> ExitCode {
    // zbus logs its handshake as info: only its warnings.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn")).init();

    // `--run FILE`: become a launched job's program (sway's exec starts us this way; see job.rs).
    if let [flag, file] = std::env::args().skip(1).collect::<Vec<_>>().as_slice()
        && flag == "--run"
    {
        log::error!("{file}: {}", job::run(std::path::Path::new(file)));
        return ExitCode::FAILURE;
    }

    // Commands that only print: before anything starts or touches the hardware.
    // `--launcher-example`: an example launcher.json, to start one from.
    if std::env::args().nth(1).as_deref() == Some("--launcher-example") {
        print!("{LAUNCHER_EXAMPLE}");
        return ExitCode::SUCCESS;
    }
    // `--mangohud-env`: print the environment that makes a game (started by hand) report its frame rate
    // here.
    if std::env::args().nth(1).as_deref() == Some("--mangohud-env") {
        let dir = clamshell_policy::frames::default_dir();
        return match tuning::mangohud_config(&dir, "default", 0, 0) {
            Ok(config) => {
                for (k, v) in tuning::mangohud_env(&config) {
                    println!("{k}={v}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                log::error!("{e}");
                ExitCode::FAILURE
            }
        };
    }

    let config = Config {
        bottom_output: std::env::var("CLAMSHELL_BOTTOM_OUTPUT").unwrap_or_else(|_| "DSI-1".into()),
        // The Thor's panels at half size: top 1920x1080 (landscape), bottom 1240x1080.
        top_size: (960, 540),
        bottom_size: (620, 540),
        bottom_panel: std::env::var("CLAMSHELL_BOTTOM_PANEL").as_deref() == Ok("1"),
        touch_layout: Arc::new(Mutex::new(TouchLayout::default())),
    };
    let touchpad = touchpad::Config {
        device: std::env::var("CLAMSHELL_TOUCHSCREEN").ok(),
        // The Thor's bottom panel: 1240 pixels across about 75 mm.
        px_per_mm: 16.5,
    };
    // The library: Pegasus game dirs (CLAMSHELL_PEGASUS_CONFIG: another config dir) and Steam; the demo
    // games if it is empty. Snapshots use the demo games unless CLAMSHELL_SNAPSHOT_LIBRARY=real.
    let snapshotting = std::env::args().nth(1).as_deref() == Some("--snapshot");
    let real_snapshot = std::env::var("CLAMSHELL_SNAPSHOT_LIBRARY").as_deref() == Ok("real");
    let mut sources = clamshell_library::Sources::default_for_user();
    // CLAMSHELL_STEAM=off: without Steam's games (tests).
    if std::env::var("CLAMSHELL_STEAM").as_deref() == Ok("off") {
        sources.steam = None;
    }
    if let Some(dir) = std::env::var_os("CLAMSHELL_PEGASUS_CONFIG") {
        let dir = std::path::PathBuf::from(dir);
        sources.state_dir = clamshell_library::Sources::state_dir_for(Some(&dir));
        sources.pegasus_config = Some(dir);
    }
    let state_dir = sources.state_dir.clone();
    let library = if snapshotting && !real_snapshot {
        clamshell_library::demo_library()
    } else {
        let library = clamshell_library::Library::load(&sources);
        if library.collections.is_empty() { clamshell_library::demo_library() } else { library }
    };
    let mut state = State::new(library);
    // Where shelf was last time (~/.local/state/clamshell/last).
    let resume_file = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))
        .map(|d| d.join("clamshell/last"));
    if !snapshotting && let Some(text) = resume_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
        state.restore(&text);
    }
    // The screens' brightness as it was left (the level and each screen's factor), next to it.
    let brightness_file = resume_file.as_ref().and_then(|f| f.parent()).map(|d| d.join("brightness"));
    if !snapshotting && let Some(text) = brightness_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
        state.brightness = clamshell_core::levels::Brightness::from_text(&text);
    }
    // The launcher's apps (~/.config/clamshell/launcher.json; without it, shelf, Steam and a terminal).
    if !snapshotting {
        let (apps, error) = load_apps();
        state.set_apps(apps, error);
    }
    // Each game's settings (power profile, FPS cap).
    if !snapshotting && let Some(text) = state_dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("game-settings.txt")).ok()) {
        state.settings = Settings::load(&text);
    }

    // `--library`: print the collections and games found (Pegasus game dirs and Steam).
    if std::env::args().nth(1).as_deref() == Some("--library") {
        for c in &state.library.collections {
            println!("{} ({}, {} games){}", c.name, c.short_name, c.games.len(), if c.tools { " tools" } else { "" });
            for g in c.games.iter().filter_map(|&i| state.library.games.get(i)) {
                let art = if g.box_front.is_some() { "art" } else { "no art" };
                println!("  {}  [{art}] {:?}", g.title, g.launch);
            }
        }
        return ExitCode::SUCCESS;
    }
    // `--snapshot DIR`: draw both screens at the Thor's size into PNG files, no Wayland needed.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, dir] = args.as_slice()
        && flag == "--snapshot"
    {
        return snapshot(&mut state, std::path::Path::new(dir));
    }

    // Development: start in a tab (CLAMSHELL_START_MODE=touchpad|keyboard|dock|...); what it watches
    // starts with the others' at Event::Start.
    let (touchpad_tx, touchpad_rx) = mpsc::channel();
    if let Some(mode) = std::env::var("CLAMSHELL_START_MODE").ok().and_then(|m| Tab::from_name(&m)) {
        state.start_in(mode);
    }
    let layout = config.touch_layout.clone();
    // The controller (CLAMSHELL_GAMEPAD: path or part of its name; default the first gamepad), grabbed for
    // good with a virtual Xbox pad for games; CLAMSHELL_PAD_MOUSE=1 starts with the pad mouse on.
    let (padmouse_tx, padmouse_rx) = mpsc::channel();
    if std::env::var("CLAMSHELL_PAD_MOUSE").as_deref() == Ok("1") {
        // (Where the pad goes is sent at Event::Start.)
        state.apply(Event::Activate(Id::PadMouse));
    }
    let gamepad = std::env::var("CLAMSHELL_GAMEPAD").ok();
    // The global settings, until a game brings its own.
    let tuning_tx = tuning::spawn().unwrap_or_else(|e| {
        log::error!("tuning thread: {e}");
        mpsc::channel().0
    });
    apply_settings(&state.settings.global, None, &clamshell_policy::frames::default_dir(), &padmouse_tx, &tuning_tx);
    // Games start from the launcher (each on a thread of its own, so several run side by side), with
    // MangoHud's environment; plays go to stats.db in the state dir.
    let (launch_tx, launch_rx) = mpsc::channel();
    // sway's windows, for the launcher (focus, close).
    let (sway_tx, sway_rx) = mpsc::channel();
    // Volume and brightness from the bar's sliders; Wi-Fi from its mode.
    let (controls_tx, controls_rx) = mpsc::channel();
    if let Err(e) = controls::spawn(controls_rx) {
        log::error!("controls thread: {e}");
    }
    let (wifi_tx, wifi_rx) = mpsc::channel();
    let (power_tx, power_rx) = mpsc::channel();
    let (mixer_tx, mixer_rx) = mpsc::channel();
    let (bluetooth_tx, bluetooth_rx) = mpsc::channel();
    let on_sway = std::env::var_os("SWAYSOCK").is_some();
    let launcher = clamshell_policy::launcher::Config {
        stats_db: state_dir.as_ref().map(|d| d.join("stats.db")),
        env: vec![],
        scope: clamshell_policy::launcher::Config::has_user_systemd(),
        sway: on_sway.then(|| sway_tx.clone()),
    };
    let mangohud_dir = clamshell_policy::frames::default_dir();
    if let Some(dir) = &state_dir
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        log::warn!("state dir {}: {e}", dir.display());
    }
    // The on-screen keyboard types through a virtual keyboard (US positions).
    let mut keyboard = virtual_keyboard::VirtualKeyboard::new().map_err(|e| log::error!("virtual keyboard: {e}")).ok();

    let scope = clamshell_policy::launcher::Config::has_user_systemd();
    let bottom_output = config.bottom_output.clone();
    let result = clamshell_ui::run(
        state,
        config,
        |sender| {
            let clock = sender.clone();
            if let Err(e) = clamshell_policy::spawn_clock(move |event| clock.send(event)) {
                log::error!("clock thread: {e}");
            }
            let status = sender.clone();
            if let Err(e) = clamshell_policy::spawn_status(move |event| status.send(event)) {
                log::error!("status thread: {e}");
            }
            let metrics = sender.clone();
            if let Err(e) = clamshell_policy::metrics::spawn(move |event| metrics.send(event)) {
                log::error!("metrics thread: {e}");
            }
            let frames = sender.clone();
            let dir = clamshell_policy::frames::default_dir();
            if let Err(e) = clamshell_policy::frames::spawn(dir, move |event| frames.send(event)) {
                log::error!("frames thread: {e}");
            }
            let touched = sender.clone();
            if let Err(e) = touchpad::spawn(touchpad, layout, touchpad_rx, move |event| touched.send(event)) {
                log::error!("touchpad thread: {e}");
            }
            // Tests drive the program through a pipe (CLAMSHELL_CONTROL), not input devices.
            if let Some(path) = std::env::var_os("CLAMSHELL_CONTROL") {
                let control = sender.clone();
                spawn_control(std::path::PathBuf::from(path), move |event| control.send(event));
            }
            let windows = sender.clone();
            if let Err(e) = sway::spawn(move |event| windows.send(event), sway_rx, bottom_output.clone()) {
                log::error!("sway thread: {e}");
            }
            let units = sender.clone();
            if let Err(e) = clamshell_policy::units::spawn(move |event| units.send(event)) {
                log::error!("units thread: {e}");
            }
            let launched = sender.clone();
            if let Err(e) = clamshell_policy::launcher::spawn(launcher, launch_rx, move |event| launched.send(event)) {
                log::error!("launcher thread: {e}");
            }
            let home = sender.clone();
            if let Err(e) = pad::spawn(gamepad, padmouse_rx, move |event| home.send(event)) {
                log::error!("pad thread: {e}");
            }
            let power = sender.clone();
            if let Err(e) = clamshell_policy::power::spawn(move |event| power.send(event), power_rx) {
                log::error!("power thread: {e}");
            }
            let mixer = sender.clone();
            if let Err(e) = clamshell_policy::mixer::spawn(move |event| mixer.send(event), mixer_rx) {
                log::error!("mixer thread: {e}");
            }
            let bluetooth = sender.clone();
            if let Err(e) = clamshell_policy::bluetooth::spawn(move |event| bluetooth.send(event), bluetooth_rx) {
                log::error!("bluetooth thread: {e}");
            }
            let wifi = sender.clone();
            if let Err(e) = clamshell_policy::wifi::spawn(move |event| wifi.send(event), wifi_rx) {
                log::error!("wifi thread: {e}");
            }
        },
        move |effect| match effect {
            Effect::Touchpad(on) => {
                log::info!("effect: {effect:?}");
                let command = if *on { touchpad::Command::On } else { touchpad::Command::Off };
                if touchpad_tx.send(command).is_err() {
                    log::warn!("touchpad thread is gone");
                }
            }
            Effect::PadRoute(route) => {
                log::info!("effect: {effect:?}");
                if padmouse_tx.send(pad::Command::Route(*route)).is_err() {
                    log::warn!("pad thread is gone");
                }
            }
            Effect::Key(stroke) => match keyboard.as_mut() {
                Some(kb) => {
                    if let Err(e) = kb.stroke(stroke) {
                        log::warn!("virtual keyboard: {e}");
                    }
                }
                None => log::info!("effect: {effect:?} (no virtual keyboard)"),
            },
            Effect::Launch { game, workspace, launch, file, settings, env } => {
                log::info!("launching {game}");
                let mut env = env.clone();
                if let Some(config) = apply_settings(settings, Some(game), &mangohud_dir, &padmouse_tx, &tuning_tx) {
                    // MangoHud's first, so a game's own variables can still override it.
                    env.splice(0..0, tuning::mangohud_env(&config));
                }
                // Its own workspace, shown when its first window appears (the state asks for that).
                let request = clamshell_policy::launcher::Request {
                    game: game.clone(),
                    workspace: workspace.clone(),
                    launch: launch.clone(),
                    file: file.clone(),
                    env,
                };
                if launch_tx.send(request).is_err() {
                    log::warn!("launcher thread is gone");
                }
            }
            Effect::Apply { game, settings } => {
                apply_settings(settings, game.as_deref(), &mangohud_dir, &padmouse_tx, &tuning_tx);
            }
            Effect::SaveSettings(text) => {
                if let Some(dir) = &state_dir
                    && let Err(e) = std::fs::write(dir.join("game-settings.txt"), text)
                {
                    log::warn!("game settings: {e}");
                }
            }
            Effect::SaveResume(text) => save(resume_file.as_deref(), text),
            Effect::SaveBrightness(text) => save(brightness_file.as_deref(), text),
            Effect::RecordPlay { game, start, duration } => {
                if let Some(dir) = &state_dir
                    && let Err(e) = clamshell_library::stats::record(&dir.join("stats.db"), game, *start, *duration)
                {
                    log::warn!("recording the play of {game}: {e}");
                }
            }
            Effect::SaveFavourites(ids) => {
                if let Some(dir) = &state_dir
                    && let Err(e) = clamshell_library::favorites::write(dir, ids.iter().map(String::as_str))
                {
                    log::warn!("favourites: {e}");
                }
            }
            Effect::RunApp { name, command } => {
                // A new instance: its scope; under sway started for its workspace (shown once its window
                // is there), elsewhere just started.
                log::info!("running {command:?} as {name}");
                let job = job::Job { unit: name.clone(), argv: command.clone(), env: vec![], workdir: None, scope };
                if on_sway {
                    match job::write(&job) {
                        Ok(path) => {
                            let _ = sway_tx.send(sway::Command::Launch { workspace: name.clone(), job: path });
                        }
                        Err(e) => log::warn!("{name}: {e}"),
                    }
                } else if let Some(mut cmd) = job::command(&job) {
                    use std::os::unix::process::CommandExt;
                    match cmd.stdin(std::process::Stdio::null()).process_group(0).spawn() {
                        // Reaped whenever it ends; systemd's list says whether it runs.
                        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
                        Err(e) => log::warn!("{name}: {e}"),
                    }
                }
            }
            Effect::ShowShelf => {
                let _ = sway_tx.send(sway::Command::Workspace(clamshell_core::windows::SHELF_WORKSPACE.into()));
            }
            Effect::FocusWorkspace(name) => {
                let _ = sway_tx.send(sway::Command::Workspace(name.clone()));
            }
            Effect::MoveWindows { windows, workspace } => {
                let _ = sway_tx.send(sway::Command::Move { windows: windows.clone(), workspace: workspace.clone() });
            }
            Effect::CloseWindows(ids) => {
                for id in ids {
                    let _ = sway_tx.send(sway::Command::Close(*id));
                }
            }
            Effect::SetVolume(v) => {
                let _ = controls_tx.send(controls::Command::Volume(*v));
            }
            Effect::SetMute(m) => {
                let _ = controls_tx.send(controls::Command::Mute(*m));
            }
            Effect::Brightness { level, factors } => {
                let _ = controls_tx.send(controls::Command::Brightness { level: *level, factors: factors.clone() });
            }
            Effect::Wifi(command) => {
                // (The password stays out of the log.)
                if let clamshell_core::wifi::Command::Connect { ssid, .. } = command {
                    log::info!("wifi: join {ssid}");
                }
                if wifi_tx.send(command.clone()).is_err() {
                    log::warn!("wifi thread is gone");
                }
            }
            Effect::Bluetooth(command) => {
                log::info!("effect: {effect:?}");
                if bluetooth_tx.send(command.clone()).is_err() {
                    log::warn!("bluetooth thread is gone");
                }
            }
            Effect::WatchPower(on) => {
                let _ = power_tx.send(*on);
            }
            Effect::WatchBacklights(on) => {
                let _ = mixer_tx.send(clamshell_policy::mixer::Command::Backlights(*on));
            }
            Effect::Audio(command) => {
                use clamshell_core::levels::Command as Audio;
                let _ = match command {
                    Audio::Watch(on) => mixer_tx.send(clamshell_policy::mixer::Command::Audio(*on)).is_ok(),
                    Audio::Output(id) => controls_tx.send(controls::Command::Output(*id)).is_ok(),
                    Audio::StreamVolume(id, v) => controls_tx.send(controls::Command::StreamVolume(*id, *v)).is_ok(),
                    Audio::StreamMute(id, m) => controls_tx.send(controls::Command::StreamMute(*id, *m)).is_ok(),
                };
            }
            Effect::QuitGame { windows, units } => {
                log::info!("quitting the game");
                for id in windows {
                    let _ = sway_tx.send(sway::Command::Close(*id));
                }
                end_units_later(units.clone());
            }
            // (The UI handles quitting.)
            Effect::Quit => {}
        },
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Writes one of ClamShell's state files (its folder made if needed).
fn save(file: Option<&std::path::Path>, text: &str) {
    let Some(file) = file else {
        return;
    };
    let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(file, text));
    if let Err(e) = written {
        log::warn!("{}: {e}", file.display());
    }
}

/// Events to apply first, the screen, its size, the file name.
type Shot = (Vec<Event>, Screen, (u32, u32), &'static str);

/// A test's command as events: `tap app 1`, `long game`, `tap plus 1`, `tap launch`, `tap network 2`,
/// `long network 0`, `tap wifi-power`, `tap keep`, `tap close`, `tap game N`, `list N` (shelf's selection),
/// `tap device N`, `long device N`, `tap bluetooth-power`, `tab dock`, `home`, `type text` (on the on-screen keyboard),
/// `slide volume|brightness FROM TO [hold]` (a finger down at FROM on a 620-wide layout, moved to TO,
/// lifted unless held), `lift volume|brightness`, `level volume|brightness V`, `level backlight|stream N V`
/// (a level row in the volume or brightness tab), `pad up|down|left|right|a|b|y|lb|rb|switch` (the pad
/// while shelf is in front; `a` presses and releases).
fn control_events(line: &str) -> Option<Vec<Event>> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let index = |w: &str| w.parse::<u16>().ok();
    let control = |name: &str| match name {
        "volume" => Some(clamshell_core::slider::Control::Volume),
        "brightness" => Some(clamshell_core::slider::Control::Brightness),
        _ => None,
    };
    // (A slide never ends in a tap: that would open the tab.)
    if let ["lift", name] = words.as_slice() {
        return Some(vec![Event::SlideEnd, Event::Up(Id::Control(control(name)?))]);
    }
    if let ["level", rest @ ..] = words.as_slice() {
        use clamshell_core::levels::Level;
        let (level, value) = match rest {
            ["volume", v] => (Level::Volume, v),
            ["brightness", v] => (Level::Brightness, v),
            ["backlight", n, v] => (Level::Backlight(index(n)?), v),
            ["stream", n, v] => (Level::Stream(index(n)?), v),
            _ => return None,
        };
        return Some(vec![Event::SetLevel(level, value.parse().ok()?), Event::Up(Id::Level(level))]);
    }
    if let ["slide", name, from, to, rest @ ..] = words.as_slice() {
        let control = control(name)?;
        let hold = rest == ["hold"];
        let (from, to): (f32, f32) = (from.parse().ok()?, to.parse().ok()?);
        let id = Id::Control(control);
        let steps = (1..=10).map(|i| Event::SlideTo { x: from + (to - from) * i as f32 / 10.0 });
        return Some(
            [Event::Down(id.clone()), Event::SlideStart { control, x: from, width: 620.0 }]
                .into_iter()
                .chain(steps)
                .chain(if hold { vec![] } else { vec![Event::SlideEnd, Event::Up(id)] })
                .collect(),
        );
    }
    if let Some(text) = line.strip_prefix("type ") {
        return Some(text.chars().filter_map(type_char).flatten().collect());
    }
    let id = match words.as_slice() {
        ["home"] => return Some(vec![Event::Home]),
        ["list", step] => return Some(vec![Event::ListStep(step.parse().ok()?)]),
        ["pad", button] => {
            use clamshell_core::PadInput as P;
            return Some(match *button {
                "up" => vec![Event::Pad(P::Up)],
                "down" => vec![Event::Pad(P::Down)],
                "left" => vec![Event::Pad(P::Left)],
                "right" => vec![Event::Pad(P::Right)],
                "a" => vec![Event::Pad(P::A(true)), Event::Pad(P::A(false))],
                "b" => vec![Event::Pad(P::B)],
                "y" => vec![Event::Pad(P::Y)],
                "lb" => vec![Event::Pad(P::Shoulder(-1))],
                "rb" => vec![Event::Pad(P::Shoulder(1))],
                "switch" => vec![Event::Pad(P::Switch)],
                _ => return None,
            });
        }
        ["tab", name] => return Some(vec![Event::Activate(Id::Tab(Tab::from_name(name)?))]),
        [_, "app", n] => Id::App(index(n)?),
        [_, "plus", n] => Id::NewInstance(index(n)?),
        [_, "network", n] => Id::Network(index(n)?),
        [_, "wifi-power"] => Id::WifiPower,
        [_, "pad-mouse"] => Id::PadMouse,
        [_, "device", n] => Id::Device(index(n)?),
        [_, "bluetooth-power"] => Id::BluetoothPower,
        [_, "save"] => Id::EditorSave,
        [_, "game"] => Id::GameTile(0),
        [_, "game", n] => Id::GameTile(index(n)?),
        [_, "keep"] => Id::StartKeep,
        [_, "close"] => Id::StartClose,
        [_, "start-game"] => Id::StartGame,
        [_, "launch"] => Id::Launch,
        _ => return None,
    };
    match words.first() {
        Some(&"tap") => Some(vec![Event::Activate(id)]),
        Some(&"long") => Some(vec![Event::LongPress(id)]),
        _ => None,
    }
}

/// A character typed on the on-screen keyboard: its key, after Shift (once) if it needs it.
fn type_char(c: char) -> Option<Vec<Event>> {
    let key = |k: Key| keyboard::find(k).map(|pos| Event::Activate(Id::Key(pos)));
    match (' '..='~').find(|&b| b != c && keyboard::shifted(b) == c) {
        Some(base) => Some(vec![key(Key::Mod(Modifier::Shift))?, key(Key::Char(base))?]),
        None => Some(vec![key(Key::Char(c))?]),
    }
}

/// Reads test commands from a pipe, one per line, for as long as the program runs.
fn spawn_control(path: std::path::PathBuf, send: impl Fn(Event) -> bool + Send + 'static) {
    let spawned = std::thread::Builder::new().name("control".into()).spawn(move || {
        use std::io::BufRead;
        // Each writer closes the pipe after its line: open it again for the next.
        while let Ok(file) = std::fs::File::open(&path) {
            for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                match control_events(&line) {
                    Some(events) => {
                        for event in events {
                            if !send(event) {
                                return;
                            }
                        }
                    }
                    None => log::warn!("control: {line:?}?"),
                }
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("control: {e}");
    }
}

/// An example launcher.json: the defaults and a few more (an app without an icon shows its initial).
const LAUNCHER_EXAMPLE: &str = r#"{
  "apps": [
    { "desktop": "steam" },
    { "name": "Terminal", "icon": "terminal", "command": ["foot"], "multiple": true },
    { "desktop": "firefox" },
    { "name": "Files", "icon": "files", "command": ["thunar"], "multiple": true },
    { "name": "Music", "icon": "music", "command": ["foot", "ncmpcpp"] },
    { "name": "Kodi", "command": ["kodi"] }
  ]
}
"#;

/// The launcher's apps from their file (desktop files read for those that name one), or the defaults;
/// with what is wrong, if anything (apps whose desktop file is missing are left out).
fn load_apps() -> (Vec<clamshell_core::apps::App>, Option<String>) {
    let file = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|d| d.join("clamshell/launcher.json"));
    let Some(text) = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return (clamshell_core::apps::defaults(), None);
    };
    match clamshell_core::apps::parse(&text) {
        Ok(apps) => resolve_apps(apps),
        Err(e) => {
            log::warn!("launcher.json: {e}");
            (clamshell_core::apps::defaults(), Some(format!("launcher.json: {e}")))
        }
    }
}

fn resolve_apps(apps: Vec<clamshell_core::apps::App>) -> (Vec<clamshell_core::apps::App>, Option<String>) {
    let dirs = clamshell_policy::desktop::data_dirs();
    let mut error = None;
    let apps = apps
        .into_iter()
        .filter_map(|mut app| match clamshell_policy::desktop::resolve(&mut app, &dirs) {
            Ok(()) => Some(app),
            Err(e) => {
                log::warn!("launcher.json: {e}");
                error.get_or_insert(format!("launcher.json: {e}"));
                None
            }
        })
        .collect();
    (apps, error)
}

/// After a polite close: kills what is left of these scopes (the game's) if they are still there in a
/// few seconds.
fn end_units_later(units: Vec<String>) {
    if units.is_empty() {
        return;
    }
    let ended = std::thread::Builder::new().name("quit".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        for unit in units {
            if clamshell_policy::command::succeeds("systemctl", &["--user", "is-active", "--quiet", &unit]) {
                log::info!("{unit} still runs: killing it");
                clamshell_policy::command::run("systemctl", &["--user", "kill", "--signal=SIGKILL", &unit]);
            }
        }
    });
    if let Err(e) = ended {
        log::warn!("quit: {e}");
    }
}

/// Applies the settings in force: hardware (power, cores, charge limit), the pad's shaping, and for a
/// game its MangoHud config (written, and reloaded live if it runs). Returns that config's path.
/// The settings in force to the hardware (its own thread), the virtual pad's shaping, and the game's
/// MangoHud config (returned, for its launch).
fn apply_settings(
    v: &Values,
    game: Option<&str>,
    mangohud_dir: &std::path::Path,
    pad_tx: &mpsc::Sender<pad::Command>,
    tuning_tx: &mpsc::Sender<Values>,
) -> Option<std::path::PathBuf> {
    if tuning_tx.send(*v).is_err() {
        log::warn!("tuning thread is gone");
    }
    let shaping = pad::Shaping { nintendo: v.get(Setting::Buttons) == 1, deadzone: v.get(Setting::Deadzone) };
    let _ = pad_tx.send(pad::Command::Shape(shaping));
    let game = game?;
    match tuning::mangohud_config(mangohud_dir, game, v.get(Setting::FpsCap), v.get(Setting::Hud)) {
        Ok(config) => Some(config),
        Err(e) => {
            log::warn!("MangoHud config: {e}");
            None
        }
    }
}

fn snapshot(state: &mut State, dir: &std::path::Path) -> ExitCode {
    // A fixed time unless CLAMSHELL_SNAPSHOT_NOW is set, so snapshots compare byte for byte across runs.
    let now = match std::env::var_os("CLAMSHELL_SNAPSHOT_NOW") {
        Some(_) => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
        None => 1_760_000_000,
    };
    state.apply(Event::Tick { unix_secs: now });
    // Demo status, so every bar icon shows.
    let status = Status { battery: Some(72), charging: true, wifi: Some(80), volume: Some(60), muted: false, brightness: Some(70) };
    state.apply(Event::Status(status));
    // Each shot after the events that lead to it: shelf, then the keyboard plain, with Fn, and with
    // Shift (once) and Ctrl (locked).
    let key = |k: Key, long: bool| {
        let id = Id::Key(keyboard::find(k).unwrap_or(KeyPos { row: 0, col: 0 }));
        if long { Event::LongPress(id) } else { Event::Activate(id) }
    };
    let bottom = (1240, 1080);
    if let Ok(apps) = clamshell_core::apps::parse(LAUNCHER_EXAMPLE) {
        let (apps, error) = resolve_apps(apps);
        state.set_apps(apps, error);
    }
    let window = |id: i64, app_id: &str, unit: Option<&str>, focused: bool| clamshell_core::windows::Window {
        id,
        app_id: app_id.into(),
        title: String::new(),
        focused,
        unit: unit.map(Into::into),
        workspace: unit.map_or("1", |u| u.trim_end_matches(".scope")).replace("clamshell-game-1-0", clamshell_core::windows::GAME_WORKSPACE),
    };
    let taps = |setting: Setting, n: usize| (0..n).map(move |_| Event::Activate(Id::Tile(setting)));
    let shots: Vec<Shot> = vec![
        (vec![], Screen::Top, (1920, 1080), "top.png"),
        (vec![Event::CollectionStep(1)], Screen::Top, (1920, 1080), "top-tab2.png"),
        (vec![Event::ListStep(5)], Screen::Top, (1920, 1080), "top-tab2-down.png"),
        (vec![Event::CollectionStep(-1)], Screen::Top, (1920, 1080), "top-back.png"),
        (
            vec![
                Event::Played { game: "zelda-lttp".into(), start: 1_759_900_000, duration: 4 * 3600 + 1260 },
                Event::ToggleFavourite,
                Event::Started("zelda-lttp".into()),
            ],
            Screen::Top,
            (1920, 1080),
            "top-played.png",
        ),
        (
            vec![Event::Activate(Id::Game("ff6".into())), Event::Activate(Id::Game("ff6".into()))],
            Screen::Top,
            (1920, 1080),
            "top-switch.png",
        ),
        (vec![], Screen::Bottom, bottom, "bottom.png"),
        (
            [Event::Activate(Id::Tab(Tab::Settings))]
                .into_iter()
                .chain(taps(Setting::FpsCap, 4))
                .chain(taps(Setting::Buttons, 1))
                .chain(taps(Setting::Hud, 1))
                .collect(),
            Screen::Bottom,
            bottom,
            "settings-running.png",
        ),
        (
            [Event::Activate(Id::Scope(Scope::Global))]
                .into_iter()
                .chain(taps(Setting::Power, 1))
                .chain(taps(Setting::Deadzone, 2))
                .chain(taps(Setting::ChargeLimit, 2))
                .collect(),
            Screen::Bottom,
            bottom,
            "settings-global.png",
        ),
        (
            vec![Event::Played { game: "zelda-lttp".into(), start: now, duration: 60 }, Event::Activate(Id::Scope(Scope::Game))],
            Screen::Bottom,
            bottom,
            "settings.png",
        ),
        (
            [Event::Activate(Id::Env)]
                .into_iter()
                .chain("dxvk_hud=fps mesa_vk_wsi_present_mode=fifo".chars().flat_map(|c| {
                    // Characters that need Shift: Shift (once), then their key.
                    match (' '..='~').find(|&b| b != c && keyboard::shifted(b) == c) {
                        Some(b) => vec![key(Key::Mod(Modifier::Shift), false), key(Key::Char(b), false)],
                        None => vec![key(Key::Char(c), false)],
                    }
                }))
                .collect(),
            Screen::Bottom,
            bottom,
            "settings-env.png",
        ),
        (
            vec![Event::Activate(Id::EditorSave), Event::Status(Status { battery: Some(9), charging: false, wifi: None, volume: Some(40), muted: true, brightness: None })],
            Screen::Bottom,
            bottom,
            "status-low.png",
        ),
        (vec![Event::Status(status)], Screen::Bottom, bottom, "bottom.png"),
        (
            vec![Event::Activate(Id::Tab(Tab::Dock)), Event::HideFocus],
            Screen::Bottom,
            bottom,
            "launcher.png",
        ),
        (
            vec![
                Event::Started("zelda-lttp".into()),
                Event::Units(
                    ["clamshell-game-1-0", "clamshell-app-terminal-1-0", "clamshell-app-terminal-1-1", "clamshell-app-firefox-1-2"]
                        .map(String::from)
                        .to_vec(),
                ),
                Event::Windows(vec![
                    window(5, "retroarch", Some("clamshell-game-1-0.scope"), false),
                    window(7, "foot", Some("clamshell-app-terminal-1-0.scope"), false),
                    window(8, "foot", Some("clamshell-app-terminal-1-1.scope"), true),
                    window(9, "firefox", Some("clamshell-app-firefox-1-2.scope"), false),
                ]),
            ],
            Screen::Bottom,
            bottom,
            "launcher-running.png",
        ),
        (
            vec![
                Event::Activate(Id::Tab(Tab::Wifi)),
                Event::Wifi(clamshell_core::wifi::Wifi {
                    device: true,
                    enabled: true,
                    networks: [("home", 82, true, true, true), ("cafe-guest", 64, false, false, false), ("neighbours", 41, true, false, false), ("office", 30, true, true, false)]
                        .map(|(ssid, strength, secure, known, active)| clamshell_core::wifi::Network {
                            ssid: ssid.into(),
                            strength,
                            secure,
                            known,
                            active,
                            enterprise: false,
                        })
                        .to_vec(),
                    connecting: None,
                    error: None,
                }),
            ],
            Screen::Bottom,
            bottom,
            "wifi.png",
        ),
        (battery_demo(now), Screen::Bottom, bottom, "battery.png"),
        (
            vec![
                Event::Activate(Id::Tab(Tab::Bluetooth)),
                Event::Bluetooth(clamshell_core::bluetooth::Bluetooth {
                    adapter: true,
                    powered: true,
                    devices: [
                        ("WH-1000XM4", clamshell_core::bluetooth::Kind::Audio, true, true),
                        ("Xbox Wireless Controller", clamshell_core::bluetooth::Kind::Gamepad, true, false),
                        ("MX Keys", clamshell_core::bluetooth::Kind::Keyboard, false, false),
                        ("Pixel 8", clamshell_core::bluetooth::Kind::Phone, false, false),
                    ]
                    .map(|(name, kind, paired, connected)| clamshell_core::bluetooth::Device {
                        address: format!("00:11:22:33:44:{:02X}", name.len()),
                        name: name.into(),
                        kind,
                        paired,
                        connected,
                    })
                    .to_vec(),
                    busy: None,
                    error: None,
                }),
            ],
            Screen::Bottom,
            bottom,
            "bluetooth.png",
        ),
        (
            vec![
                Event::Activate(Id::Tab(Tab::Volume)),
                Event::Status(clamshell_core::Status { volume: Some(64), brightness: Some(70), battery: Some(76), wifi: Some(80), ..clamshell_core::Status::default() }),
                Event::Audio(clamshell_core::levels::Audio {
                    outputs: vec![
                        clamshell_core::levels::Output { id: 50, name: "Speakers".into(), default: true },
                        clamshell_core::levels::Output { id: 51, name: "WH-1000XM4".into(), default: false },
                    ],
                    streams: [("Firefox", 80, false), ("Steam", 100, false), ("Spotify", 35, true)]
                        .map(|(app, volume, muted)| clamshell_core::levels::Stream { id: 90, app: app.into(), title: String::new(), volume, muted })
                        .to_vec(),
                }),
            ],
            Screen::Bottom,
            bottom,
            "volume.png",
        ),
        (
            vec![
                Event::Activate(Id::Tab(Tab::Brightness)),
                Event::Backlights(vec!["dsi0-backlight".into(), "dsi1-backlight".into()]),
                Event::SetLevel(clamshell_core::levels::Level::Backlight(1), 65),
                Event::Up(Id::Level(clamshell_core::levels::Level::Backlight(1))),
            ],
            Screen::Bottom,
            bottom,
            "brightness.png",
        ),
        (
            vec![
                Event::Activate(Id::Tab(Tab::Dock)),
                Event::Down(Id::Control(clamshell_core::slider::Control::Volume)),
                Event::SlideStart { control: clamshell_core::slider::Control::Volume, x: 560.0, width: 1232.0 },
                Event::SlideTo { x: 700.0 },
            ],
            Screen::Bottom,
            bottom,
            "slider.png",
        ),
        (
            vec![Event::Up(Id::Control(clamshell_core::slider::Control::Volume)), Event::Activate(Id::Tab(Tab::Keyboard))],
            Screen::Bottom,
            bottom,
            "keyboard.png",
        ),
        (vec![key(Key::Mod(Modifier::Fn), false)], Screen::Bottom, bottom, "keyboard-fn.png"),
        (
            vec![
                key(Key::Mod(Modifier::Fn), false),
                key(Key::Mod(Modifier::Shift), false),
                Event::Down(Id::Key(keyboard::find(Key::Mod(Modifier::Ctrl)).unwrap_or(KeyPos { row: 0, col: 0 }))),
            ],
            Screen::Bottom,
            bottom,
            "keyboard-mods.png",
        ),
        (
            vec![
                Event::Up(Id::Key(keyboard::find(Key::Mod(Modifier::Ctrl)).unwrap_or(KeyPos { row: 0, col: 0 }))),
                key(Key::Mod(Modifier::Shift), false),
                key(Key::Caps, false),
            ],
            Screen::Bottom,
            bottom,
            "keyboard-caps.png",
        ),
        (vec![Event::Activate(Id::Tab(Tab::Touchpad)), Event::Activate(Id::PadMouse)], Screen::Bottom, bottom, "touchpad.png"),
        (demo_metrics(), Screen::Bottom, bottom, "sysinfo.png"),
        (vec![Event::Activate(Id::Tab(Tab::Keyboard))], Screen::Bottom, bottom, "keyboard-back.png"),
        (
            vec![
                key(Key::Mod(Modifier::Shift), false),
                Event::Down(Id::Key(keyboard::find(Key::Char('g')).unwrap_or(KeyPos { row: 0, col: 0 }))),
            ],
            Screen::Bottom,
            bottom,
            "keyboard-pressed.png",
        ),
    ];
    for (events, screen, size, name) in shots {
        for event in events {
            state.apply(event);
        }
        let path = dir.join(name);
        match clamshell_ui::snapshot(state, screen, size, 1, &path) {
            Ok(Some(rows)) => {
                state.apply(Event::Rows(rows));
            }
            Ok(None) => {}
            Err(e) => {
                log::error!("{}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
        log::info!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}

/// A made-up day of charge (down while playing, charging overnight) and a busy moment, then battery mode,
/// for the snapshot.
fn battery_demo(now: u64) -> Vec<Event> {
    use clamshell_core::battery::{Charge, Group, Battery, Spender};
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
        spender(Group::Unit("clamshell-game-1760000000-0".into()), 0.31, 0.62),
        spender(Group::Process("firefox".into()), 0.08, 0.03),
        spender(Group::Unit("clamshell-app-terminal-1760000000-1".into()), 0.02, 0.0),
        spender(Group::System, 0.03, 0.0),
        spender(Group::Process("clamshell".into()), 0.01, 0.0),
    ];
    vec![
        Event::Started("zelda-lttp".into()),
        Event::Activate(Id::Tab(Tab::Battery)),
        Event::Battery(Battery { history, spenders, has_history: true }),
        Event::Status(Status { battery: Some(41), charging: false, wifi: Some(80), volume: Some(60), muted: false, brightness: Some(70) }),
    ]
}

/// A minute of made-up readings, then sysinfo mode, for the snapshot.
fn demo_metrics() -> Vec<Event> {
    use clamshell_core::metrics::Metrics;
    let mut events: Vec<Event> = (0..60u32)
        .map(|i| {
            let t = i as f32 / 60.0;
            let wave = |phase: f32| (0.5 + 0.4 * ((t * 9.0 + phase).sin())).clamp(0.0, 1.0);
            let mut cores = [0.0; 16];
            for (k, c) in cores.iter_mut().take(8).enumerate() {
                *c = wave(k as f32);
            }
            Event::Metrics(Metrics {
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
            })
        })
        .collect();
    events.push(Event::Activate(Id::Tab(Tab::Sysinfo)));
    events
}
