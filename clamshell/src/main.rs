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
mod session;

use std::process::ExitCode;

use clamshell_core::settings::Hud;
use clamshell_core::{Init, State};
use clamshell_system::launch::job;
use clamshell_system::library::{self, Sources};
use clamshell_system::paths::Paths;
use clamshell_system::readings::mangohud;

use cli::Command;
use config::Env;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,tracing=warn")).init();
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

/// Carries out what the command line asked for. The error is a message saying what went wrong.
fn carry_out(command: Command) -> Result<(), String> {
    let env = Env::from_env();
    let paths = Paths::new(&env.base);
    let sources = Sources::new(&paths, env.base.home.as_deref(), env.steam);
    match command {
        Command::Run => session::run(&env, paths, &sources),
        // `job::run` returns only if the job's program couldn't take over.
        Command::RunJob(file) => Err(format!("{}: {}", file.display(), job::run(&file))),
        Command::Ctl(words) if words == ["help"] => {
            print!("{}", control::help());
            Ok(())
        }
        Command::Ctl(words) => control::send(env.control.as_ref().or(paths.control.as_ref()), &words.join(" ")).map_err(|e| format!("ctl: {e}")),
        Command::FollowSteam(appid) => {
            clamshell_system::launch::steam::follow(appid);
            Ok(())
        }
        Command::MangohudEnv => {
            let config = mangohud::config(&paths.mangohud, "default", 0, Hud::Off).map_err(|e| format!("MangoHud config: {e}"))?;
            for (k, v) in mangohud::env(&config) {
                println!("{k}={v}");
            }
            Ok(())
        }
        Command::Library => {
            commands::print_library(State::new(Init { library: library::load(&sources).library, ..Init::default() }).shelf().library());
            Ok(())
        }
        Command::WaylandFilter => {
            let runtime = paths.mangohud.parent().map_or_else(|| paths.mangohud.clone(), std::path::Path::to_path_buf);
            clamshell_system::waydroid::run(runtime, paths.mangohud.clone()).map_err(|e| format!("wayland filter: {e}"))
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
