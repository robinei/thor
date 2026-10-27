//! ClamShell runs the shelf, the second screen, the bar, and input and power policy in one process with
//! one state. This crate is the program around them: the command line ([`cli`]), the environment
//! ([`config`]), the session ([`session`]), the one-shot commands ([`commands`]) and the control pipe
//! ([`control`]). With the `dev` feature it also has the snapshots and their demo state (`dev`).

mod cli;
mod commands;
mod config;
mod control;
#[cfg(any(test, feature = "dev"))]
mod dev;
mod logging;
mod profile;
mod session;

use std::process::ExitCode;

use clamshell_state::{Init, State};
use clamshell_system::library::{self, Sources};
use clamshell_system::paths::Paths;

use cli::Command;
use config::Env;

fn main() -> ExitCode {
    logging::init(to_journal());
    // A panic is logged at error (the journal's priority), with its thread and place: a system thread's
    // panic doesn't end the program, so it would go unseen at stderr's info.
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::capture();
        let backtrace = if backtrace.status() == std::backtrace::BacktraceStatus::Captured { format!("\n{backtrace}") } else { String::new() };
        log::error!("panic in {}: {info}{backtrace}", thread.name().unwrap_or("a thread"));
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = cli::parse(&args) else {
        eprintln!("{}", cli::USAGE);
        return ExitCode::FAILURE;
    };
    match carry_out(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Whether stderr is the journal stream systemd connected (`JOURNAL_STREAM`, its device and inode), not
/// just inherited from a service, as by a terminal's shell.
fn to_journal() -> bool {
    let Some(stream) = std::env::var("JOURNAL_STREAM").ok() else { return false };
    rustix::fs::fstat(std::io::stderr()).is_ok_and(|stat| stream == format!("{}:{}", stat.st_dev, stat.st_ino))
}

/// Carries out what the command line asked for. The error is a message saying what went wrong.
fn carry_out(command: Command) -> Result<(), String> {
    let env = Env::from_env()?;
    let paths = Paths::new(&env.base);
    let sources = Sources::new(&paths, env.base.home.as_deref(), env.steam);
    match command {
        Command::DeviceConfirm => match clamshell_system::input::confirm::volume_up(std::time::Duration::from_secs(20)) {
            Ok(true) => Ok(()),
            Ok(false) => Err("Volume Up wasn't pressed".into()),
            Err(e) => Err(format!("confirm: {e}")),
        },
        Command::DevicePanel => {
            println!("{}", profile::panel(&env.profile));
            Ok(())
        }
        Command::Nested => session::run(&env, paths, &sources, session::Host::Nested),
        Command::Drm => session::run(&env, paths, &sources, session::Host::Drm),
        Command::Ctl(words) if words == ["help"] => {
            print!("{}", control::help());
            Ok(())
        }
        Command::Ctl(words) => control::send(env.control.as_ref().or(paths.control.as_ref()), &words.join(" ")).map_err(|e| format!("ctl: {e}")),
        Command::FollowSteam(appid) => {
            clamshell_system::launch::steam::follow(appid);
            Ok(())
        }
        Command::Library => {
            commands::print_library(State::new(Init { library: Some(library::load(&sources).library), ..Init::default() }).shelf().library());
            Ok(())
        }
        Command::WaylandFilter => {
            // ClamShell's files in RAM, where the Android script writes the filter's settings.
            let settings = paths.runtime.clone().unwrap_or_else(std::env::temp_dir).join("clamshell");
            clamshell_system::waydroid::run(settings).map_err(|e| format!("wayland filter: {e}"))
        }
        Command::ClipboardGet => {
            let text = clamshell_system::session::clipboard::get().map_err(|e| format!("clipboard: {e}"))?;
            print!("{}", text.unwrap_or_default());
            Ok(())
        }
        Command::ClipboardSet => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|e| format!("clipboard: {e}"))?;
            clamshell_system::session::clipboard::set(text).map_err(|e| format!("clipboard: {e}"))
        }
        Command::Tray => commands::print_tray(),
        Command::FetchMetadata => commands::fetch_metadata(&paths, &sources),
        #[cfg(any(test, feature = "dev"))]
        Command::Snapshot(dir) => crate::dev::snapshot::command(&sources, &dir),
    }
}
