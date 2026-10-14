//! The system program: shelf, the second screen, the bar, input and power policy in one process with one
//! state (docs/plans.md in the GPD repo, "One system program").

use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};

use sysprog_core::keyboard::{self, Key, KeyPos, Modifier};
use sysprog_core::touch::TouchLayout;
use sysprog_core::settings::{Setting, Settings, Values};
use sysprog_core::{Effect, Event, Id, Mode, Scope, State, Status};
use sysprog_policy::{job, keyboard as virtual_keyboard, pad, sway, touchpad, tuning};
use sysprog_ui::{Config, Screen};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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
        let dir = sysprog_policy::frames::default_dir();
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
        bottom_output: std::env::var("SYSPROG_BOTTOM_OUTPUT").unwrap_or_else(|_| "DSI-1".into()),
        // The Thor's panels at half size: top 1920x1080 (landscape), bottom 1240x1080.
        top_size: (960, 540),
        bottom_size: (620, 540),
        bottom_panel: std::env::var("SYSPROG_BOTTOM_PANEL").as_deref() == Ok("1"),
        touch_layout: Arc::new(Mutex::new(TouchLayout::default())),
    };
    let touchpad = touchpad::Config {
        device: std::env::var("SYSPROG_TOUCHSCREEN").ok(),
        // The Thor's bottom panel: 1240 pixels across about 75 mm.
        px_per_mm: 16.5,
    };
    // The library: Pegasus game dirs (SYSPROG_PEGASUS_CONFIG: another config dir) and Steam; the demo
    // games if it is empty. Snapshots use the demo games unless SYSPROG_SNAPSHOT_LIBRARY=real.
    let snapshotting = std::env::args().nth(1).as_deref() == Some("--snapshot");
    let real_snapshot = std::env::var("SYSPROG_SNAPSHOT_LIBRARY").as_deref() == Ok("real");
    let mut sources = sysprog_library::Sources::default_for_user();
    // SYSPROG_STEAM=off: without Steam's games (tests).
    if std::env::var("SYSPROG_STEAM").as_deref() == Ok("off") {
        sources.steam = None;
    }
    if let Some(dir) = std::env::var_os("SYSPROG_PEGASUS_CONFIG") {
        let dir = std::path::PathBuf::from(dir);
        sources.state_dir = sysprog_library::Sources::state_dir_for(Some(&dir));
        sources.pegasus_config = Some(dir);
    }
    let state_dir = sources.state_dir.clone();
    let library = if snapshotting && !real_snapshot {
        sysprog_library::demo_library()
    } else {
        let library = sysprog_library::Library::load(&sources);
        if library.collections.is_empty() { sysprog_library::demo_library() } else { library }
    };
    let mut state = State::new(library);
    // Where shelf was last time (~/.local/state/sysprog/last).
    let resume_file = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))
        .map(|d| d.join("sysprog/last"));
    if !snapshotting && let Some(text) = resume_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
        state.restore(&text);
    }
    // The launcher's apps (~/.config/sysprog/launcher.json; without it, shelf, Steam and a terminal).
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

    // Development: start in a mode (SYSPROG_START_MODE=touchpad|keyboard|launcher|sysinfo).
    let (touchpad_tx, touchpad_rx) = mpsc::channel();
    let start_mode = std::env::var("SYSPROG_START_MODE").ok().and_then(|m| Mode::from_name(&m));
    if let Some(mode) = start_mode {
        for effect in state.apply(Event::Activate(Id::Mode(mode))).1 {
            if effect == Effect::Touchpad(true) {
                let _ = touchpad_tx.send(touchpad::Command::On);
            }
        }
    }
    let layout = config.touch_layout.clone();
    // The controller (SYSPROG_GAMEPAD: path or part of its name; default the first gamepad), grabbed for
    // good with a virtual Xbox pad for games; SYSPROG_PAD_MOUSE=1 starts with the pad mouse on.
    let (padmouse_tx, padmouse_rx) = mpsc::channel();
    if std::env::var("SYSPROG_PAD_MOUSE").as_deref() == Ok("1") {
        for effect in state.apply(Event::Activate(Id::PadMouse)).1 {
            if effect == Effect::PadMouse(true) {
                let _ = padmouse_tx.send(pad::Command::MouseOn);
            }
        }
    }
    let gamepad = std::env::var("SYSPROG_GAMEPAD").ok();
    // The global settings, until a game brings its own.
    apply_settings(&state.settings.global, None, &sysprog_policy::frames::default_dir(), &padmouse_tx);
    // Games start from the launcher thread, one at a time, with MangoHud's environment; plays go to
    // stats.db in the state dir.
    let (launch_tx, launch_rx) = mpsc::channel();
    // sway's windows, for the launcher (focus, close).
    let (sway_tx, sway_rx) = mpsc::channel();
    let on_sway = std::env::var_os("SWAYSOCK").is_some();
    let launcher = sysprog_policy::launcher::Config {
        stats_db: state_dir.as_ref().map(|d| d.join("stats.db")),
        env: vec![],
        scope: sysprog_policy::launcher::Config::has_user_systemd(),
        sway: on_sway.then(|| sway_tx.clone()),
    };
    let mangohud_dir = sysprog_policy::frames::default_dir();
    if let Some(dir) = &state_dir
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        log::warn!("state dir {}: {e}", dir.display());
    }
    // The on-screen keyboard types through a virtual keyboard (US positions).
    let mut keyboard = virtual_keyboard::VirtualKeyboard::new().map_err(|e| log::error!("virtual keyboard: {e}")).ok();

    let scope = sysprog_policy::launcher::Config::has_user_systemd();
    let bottom_output = config.bottom_output.clone();
    let result = sysprog_ui::run(
        state,
        config,
        |sender| {
            let clock = sender.clone();
            if let Err(e) = sysprog_policy::spawn_clock(move |event| clock.send(event)) {
                log::error!("clock thread: {e}");
            }
            let status = sender.clone();
            if let Err(e) = sysprog_policy::spawn_status(move |event| status.send(event)) {
                log::error!("status thread: {e}");
            }
            let metrics = sender.clone();
            if let Err(e) = sysprog_policy::metrics::spawn(move |event| metrics.send(event)) {
                log::error!("metrics thread: {e}");
            }
            let frames = sender.clone();
            let dir = sysprog_policy::frames::default_dir();
            if let Err(e) = sysprog_policy::frames::spawn(dir, move |event| frames.send(event)) {
                log::error!("frames thread: {e}");
            }
            let touched = sender.clone();
            if let Err(e) = touchpad::spawn(touchpad, layout, touchpad_rx, move |event| touched.send(event)) {
                log::error!("touchpad thread: {e}");
            }
            // Tests drive the program through a pipe (SYSPROG_CONTROL), not input devices.
            if let Some(path) = std::env::var_os("SYSPROG_CONTROL") {
                let control = sender.clone();
                spawn_control(std::path::PathBuf::from(path), move |event| control.send(event));
            }
            let windows = sender.clone();
            if let Err(e) = sway::spawn(move |event| windows.send(event), sway_rx, bottom_output.clone()) {
                log::error!("sway thread: {e}");
            }
            let units = sender.clone();
            if let Err(e) = sysprog_policy::units::spawn(move |event| units.send(event)) {
                log::error!("units thread: {e}");
            }
            if let Err(e) = sysprog_policy::launcher::spawn(launcher, launch_rx, move |event| sender.send(event)) {
                log::error!("launcher thread: {e}");
            }
            if let Err(e) = pad::spawn(gamepad, padmouse_rx) {
                log::error!("pad thread: {e}");
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
            Effect::PadMouse(on) => {
                log::info!("effect: {effect:?}");
                let command = if *on { pad::Command::MouseOn } else { pad::Command::MouseOff };
                if padmouse_tx.send(command).is_err() {
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
            Effect::Launch { game, launch, file, settings, env } => {
                log::info!("launching {game}");
                let mut env = env.clone();
                if let Some(config) = apply_settings(settings, Some(game), &mangohud_dir, &padmouse_tx) {
                    // MangoHud's first, so a game's own variables can still override it.
                    env.splice(0..0, tuning::mangohud_env(&config));
                }
                // The game's workspace first, so it opens in front.
                let _ = sway_tx.send(sway::Command::Workspace(sysprog_core::windows::GAME_WORKSPACE.into()));
                let request = sysprog_policy::launcher::Request { game: game.clone(), launch: launch.clone(), file: file.clone(), env };
                if launch_tx.send(request).is_err() {
                    log::warn!("launcher thread is gone");
                }
            }
            Effect::Apply { game, settings } => {
                apply_settings(settings, game.as_deref(), &mangohud_dir, &padmouse_tx);
            }
            Effect::SaveSettings(text) => {
                if let Some(dir) = &state_dir
                    && let Err(e) = std::fs::write(dir.join("game-settings.txt"), text)
                {
                    log::warn!("game settings: {e}");
                }
            }
            Effect::SaveResume(text) => {
                if let Some(file) = &resume_file {
                    let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(file, text));
                    if let Err(e) = written {
                        log::warn!("{}: {e}", file.display());
                    }
                }
            }
            Effect::SaveFavourites(ids) => {
                if let Some(dir) = &state_dir
                    && let Err(e) = sysprog_library::favorites::write(dir, ids.iter().map(String::as_str))
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
                let _ = sway_tx.send(sway::Command::Workspace(sysprog_core::windows::SHELF_WORKSPACE.into()));
            }
            Effect::FocusWorkspace(name) => {
                let _ = sway_tx.send(sway::Command::Workspace(name.clone()));
            }
            Effect::CloseWindows(ids) => {
                for id in ids {
                    let _ = sway_tx.send(sway::Command::Close(*id));
                }
            }
            Effect::QuitGame { windows, units } => {
                log::info!("quitting the game");
                for id in windows {
                    let _ = sway_tx.send(sway::Command::Close(*id));
                }
                end_units_later(units.clone());
            }
            other => log::info!("effect: {other:?}"),
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

/// Events to apply first, the screen, its size, the file name.
type Shot = (Vec<Event>, Screen, (u32, u32), &'static str);

/// A test's command (`tap app 1`, `long game`, `tap plus 1`, `tap launch`, `mode launcher`) as an event.
fn control_event(line: &str) -> Option<Event> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let index = |w: &str| w.parse::<u16>().ok();
    let id = match words.as_slice() {
        [_, "app", n] => Id::App(index(n)?),
        [_, "plus", n] => Id::NewInstance(index(n)?),
        [_, "game"] => Id::GameTile,
        [_, "launch"] => Id::Launch,
        ["mode", name] => return Some(Event::Activate(Id::Mode(Mode::from_name(name)?))),
        _ => return None,
    };
    match words.first() {
        Some(&"tap") => Some(Event::Activate(id)),
        Some(&"long") => Some(Event::LongPress(id)),
        _ => None,
    }
}

/// Reads test commands from a pipe, one per line, for as long as the program runs.
fn spawn_control(path: std::path::PathBuf, send: impl Fn(Event) -> bool + Send + 'static) {
    let spawned = std::thread::Builder::new().name("control".into()).spawn(move || {
        use std::io::BufRead;
        // Each writer closes the pipe after its line: open it again for the next.
        while let Ok(file) = std::fs::File::open(&path) {
            for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                match control_event(&line) {
                    Some(event) => {
                        if !send(event) {
                            return;
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
    { "name": "Shelf", "icon": "shelf", "builtin": "shelf" },
    { "desktop": "steam", "command": ["steam", "-gamepadui"] },
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
fn load_apps() -> (Vec<sysprog_core::apps::App>, Option<String>) {
    let file = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|d| d.join("sysprog/launcher.json"));
    let Some(text) = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return (sysprog_core::apps::defaults(), None);
    };
    match sysprog_core::apps::parse(&text) {
        Ok(apps) => resolve_apps(apps),
        Err(e) => {
            log::warn!("launcher.json: {e}");
            (sysprog_core::apps::defaults(), Some(format!("launcher.json: {e}")))
        }
    }
}

fn resolve_apps(apps: Vec<sysprog_core::apps::App>) -> (Vec<sysprog_core::apps::App>, Option<String>) {
    let dirs = sysprog_policy::desktop::data_dirs();
    let mut error = None;
    let apps = apps
        .into_iter()
        .filter_map(|mut app| match sysprog_policy::desktop::resolve(&mut app, &dirs) {
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
            let active = std::process::Command::new("systemctl").args(["--user", "is-active", "--quiet", &unit]).status();
            if active.is_ok_and(|s| s.success()) {
                log::info!("{unit} still runs: killing it");
                let _ = std::process::Command::new("systemctl").args(["--user", "kill", "--signal=SIGKILL", &unit]).status();
            }
        }
    });
    if let Err(e) = ended {
        log::warn!("quit: {e}");
    }
}

/// Applies the settings in force: hardware (power, cores, charge limit), the pad's shaping, and for a
/// game its MangoHud config (written, and reloaded live if it runs). Returns that config's path.
fn apply_settings(
    v: &Values,
    game: Option<&str>,
    mangohud_dir: &std::path::Path,
    pad_tx: &mpsc::Sender<pad::Command>,
) -> Option<std::path::PathBuf> {
    tuning::apply_hardware(v);
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
    // A fixed time unless SYSPROG_SNAPSHOT_NOW is set, so snapshots compare byte for byte across runs.
    let now = match std::env::var_os("SYSPROG_SNAPSHOT_NOW") {
        Some(_) => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
        None => 1_760_000_000,
    };
    state.apply(Event::Tick { unix_secs: now });
    // Demo status, so every bar icon shows.
    let status = Status { battery: Some(72), charging: true, wifi: Some(80), volume: Some(60), muted: false };
    state.apply(Event::Status(status));
    // Each shot after the events that lead to it: shelf, then the keyboard plain, with Fn, and with
    // Shift (once) and Ctrl (locked).
    let key = |k: Key, long: bool| {
        let id = Id::Key(keyboard::find(k).unwrap_or(KeyPos { row: 0, col: 0 }));
        if long { Event::LongPress(id) } else { Event::Activate(id) }
    };
    let bottom = (1240, 1080);
    if let Ok(apps) = sysprog_core::apps::parse(LAUNCHER_EXAMPLE) {
        let (apps, error) = resolve_apps(apps);
        state.set_apps(apps, error);
    }
    let window = |id: i64, app_id: &str, unit: Option<&str>, focused: bool| sysprog_core::windows::Window {
        id,
        app_id: app_id.into(),
        title: String::new(),
        focused,
        unit: unit.map(Into::into),
        workspace: unit.map_or("1", |u| u.trim_end_matches(".scope")).replace("sysprog-game-1-0", sysprog_core::windows::GAME_WORKSPACE),
    };
    let taps = |setting: Setting, n: usize| (0..n).map(move |_| Event::Activate(Id::Tile(setting)));
    let shots: Vec<Shot> = vec![
        (vec![], Screen::Top, (1920, 1080), "top.png"),
        (vec![Event::TabStep(1)], Screen::Top, (1920, 1080), "top-tab2.png"),
        (vec![Event::ListStep(5)], Screen::Top, (1920, 1080), "top-tab2-down.png"),
        (vec![Event::TabStep(-1)], Screen::Top, (1920, 1080), "top-back.png"),
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
        (vec![], Screen::Bottom, bottom, "bottom.png"),
        (
            [Event::Activate(Id::Mode(Mode::Settings))]
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
            vec![Event::Activate(Id::EnvSave), Event::Status(Status { battery: Some(9), charging: false, wifi: None, volume: Some(40), muted: true })],
            Screen::Bottom,
            bottom,
            "status-low.png",
        ),
        (vec![Event::Status(status)], Screen::Bottom, bottom, "bottom.png"),
        (
            vec![Event::Activate(Id::Mode(Mode::Launcher)), Event::HideFocus],
            Screen::Bottom,
            bottom,
            "launcher.png",
        ),
        (
            vec![
                Event::Started("zelda-lttp".into()),
                Event::Units(
                    ["sysprog-game-1-0", "sysprog-app-terminal-1-0", "sysprog-app-terminal-1-1", "sysprog-app-firefox-1-2"]
                        .map(String::from)
                        .to_vec(),
                ),
                Event::Windows(vec![
                    window(5, "retroarch", Some("sysprog-game-1-0.scope"), false),
                    window(7, "foot", Some("sysprog-app-terminal-1-0.scope"), false),
                    window(8, "foot", Some("sysprog-app-terminal-1-1.scope"), true),
                    window(9, "firefox", Some("sysprog-app-firefox-1-2.scope"), false),
                ]),
            ],
            Screen::Bottom,
            bottom,
            "launcher-running.png",
        ),
        (vec![Event::Activate(Id::Mode(Mode::Keyboard))], Screen::Bottom, bottom, "keyboard.png"),
        (vec![key(Key::Mod(Modifier::Fn), false)], Screen::Bottom, bottom, "keyboard-fn.png"),
        (
            vec![key(Key::Mod(Modifier::Fn), false), key(Key::Mod(Modifier::Shift), false), key(Key::Mod(Modifier::Ctrl), true)],
            Screen::Bottom,
            bottom,
            "keyboard-mods.png",
        ),
        (vec![key(Key::Mod(Modifier::Shift), true)], Screen::Bottom, bottom, "keyboard-caps.png"),
        (vec![Event::Activate(Id::Mode(Mode::Touchpad)), Event::Activate(Id::PadMouse)], Screen::Bottom, bottom, "touchpad.png"),
        (demo_metrics(), Screen::Bottom, bottom, "sysinfo.png"),
        (vec![Event::Activate(Id::Mode(Mode::Keyboard))], Screen::Bottom, bottom, "keyboard-back.png"),
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
        match sysprog_ui::snapshot(state, screen, size, 1, &path) {
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

/// A minute of made-up readings, then sysinfo mode, for the snapshot.
fn demo_metrics() -> Vec<Event> {
    use sysprog_core::metrics::Metrics;
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
    events.push(Event::Activate(Id::Mode(Mode::Sysinfo)));
    events
}
