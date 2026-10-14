//! The system program: shelf, the second screen, the bar, input and power policy in one process with one
//! state (docs/plans.md in the GPD repo, "One system program").

use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};

use sysprog_core::keyboard::{self, Key, KeyPos, Modifier};
use sysprog_core::touch::TouchLayout;
use sysprog_core::{Effect, Event, Id, Mode, State, Status};
use sysprog_policy::{pad, touchpad};
use sysprog_ui::{Config, Screen};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Config {
        bottom_output: std::env::var("SYSPROG_BOTTOM_OUTPUT").unwrap_or_else(|_| "DSI-1".into()),
        // The Thor's panels at half size: top 1920x1080 (landscape), bottom 1240x1080.
        top_size: (960, 540),
        bottom_size: (620, 540),
        touch_layout: Arc::new(Mutex::new(TouchLayout::default())),
    };
    let touchpad = touchpad::Config {
        device: std::env::var("SYSPROG_TOUCHSCREEN").ok(),
        // The Thor's bottom panel: 1240 pixels across about 75 mm.
        px_per_mm: 16.5,
    };
    let mut state = State::new(sysprog_library::demo_games());

    // `--snapshot DIR`: draw both screens at the Thor's size into PNG files, no Wayland needed.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, dir] = args.as_slice()
        && flag == "--snapshot"
    {
        return snapshot(&mut state, std::path::Path::new(dir));
    }

    // Development: start in a mode (SYSPROG_START_MODE=touchpad|keyboard|launcher|sysinfo).
    let (touchpad_tx, touchpad_rx) = mpsc::channel();
    let start_mode = match std::env::var("SYSPROG_START_MODE").as_deref() {
        Ok("touchpad") => Some(Mode::Touchpad),
        Ok("keyboard") => Some(Mode::Keyboard),
        Ok("launcher") => Some(Mode::Launcher),
        Ok("sysinfo") => Some(Mode::Sysinfo),
        _ => None,
    };
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
            if let Err(e) = touchpad::spawn(touchpad, layout, touchpad_rx, move |event| sender.send(event)) {
                log::error!("touchpad thread: {e}");
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
    let shots: Vec<Shot> = vec![
        (vec![], Screen::Top, (1920, 1080), "top.png"),
        (vec![], Screen::Bottom, bottom, "bottom.png"),
        (
            vec![Event::Status(Status { battery: Some(9), charging: false, wifi: None, volume: Some(40), muted: true })],
            Screen::Bottom,
            bottom,
            "status-low.png",
        ),
        (vec![Event::Status(status)], Screen::Bottom, bottom, "bottom.png"),
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
        if let Err(e) = sysprog_ui::snapshot(state, screen, size, 1, &path) {
            log::error!("{}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        log::info!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}
