//! The system program: shelf, the second screen, the bar, input and power policy in one process with one
//! state (docs/plans.md in the GPD repo, "One system program").

use std::process::ExitCode;

use sysprog_core::{Event, Id, Key, Mode, State};
use sysprog_ui::{Config, Screen};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Config {
        bottom_output: std::env::var("SYSPROG_BOTTOM_OUTPUT").unwrap_or_else(|_| "DSI-1".into()),
        // The Thor's panels at half size: top 1920x1080 (landscape), bottom 1240x1080.
        top_size: (960, 540),
        bottom_size: (620, 540),
    };
    let mut state = State::new(sysprog_library::demo_games());

    // `--snapshot DIR`: draw both screens at the Thor's size into PNG files, no Wayland needed.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, dir] = args.as_slice()
        && flag == "--snapshot"
    {
        return snapshot(&mut state, std::path::Path::new(dir));
    }

    let result = sysprog_ui::run(
        state,
        config,
        |sender| {
            if let Err(e) = sysprog_policy::spawn_clock(move |event| sender.send(event)) {
                log::error!("clock thread: {e}");
            }
        },
        |effect| log::info!("effect: {effect:?}"),
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
type Shot = (&'static [Event], Screen, (u32, u32), &'static str);

fn snapshot(state: &mut State, dir: &std::path::Path) -> ExitCode {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    state.apply(Event::Tick { unix_secs: now.as_secs() });
    // Each shot after the events that lead to it: shelf, then the keyboard's two layers.
    let shots: [Shot; 4] = [
        (&[], Screen::Top, (1920, 1080), "top.png"),
        (&[], Screen::Bottom, (1240, 1080), "bottom.png"),
        (&[Event::Activate(Id::Mode(Mode::Keyboard))], Screen::Bottom, (1240, 1080), "keyboard.png"),
        (&[Event::Activate(Id::Key(Key::Layer))], Screen::Bottom, (1240, 1080), "symbols.png"),
    ];
    for (events, screen, size, name) in shots {
        for event in events {
            state.apply(event.clone());
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
