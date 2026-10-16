//! Carrying out the state's effects: each goes to the thread or the file it is for.

use clamshell_core::{Effect, Instance, Save, Watch, WindowCommand};

use crate::job::Runner;
use crate::keyboard::VirtualKeyboard;
use crate::library::{favorites, stats};
use crate::paths::{self, Paths};
use crate::tuning::Tuning;
use crate::worker::Worker;
use crate::{battery, bluetooth, brightness, launcher, mixer, pad, sway, touchpad, wifi};

/// The threads (and the one device and the files) that effects go to.
pub struct Handles {
    pub paths: Paths,
    /// Apps and games run in systemd scopes (there is a user session).
    pub scope: bool,
    pub touchpad: Worker<touchpad::Command>,
    pub mixer: Worker<mixer::Command>,
    pub wifi: Worker<wifi::Request>,
    pub bluetooth: Worker<bluetooth::Request>,
    pub battery: Worker<battery::Command>,
    pub pad: Worker<pad::Command>,
    pub launcher: Worker<launcher::Request>,
    /// Under sway: windows and launches.
    pub sway: Option<Worker<sway::Command>>,
    pub jobs: Runner,
    pub sound: Worker<clamshell_core::Sound>,
    pub brightness: Worker<brightness::Command>,
    pub tuning: Tuning,
    /// Screenshots and sleep.
    pub system: Worker<crate::system::Command>,
    /// The on-screen keyboard types through it; `None` if it couldn't be made.
    pub keyboard: Option<VirtualKeyboard>,
}

impl Handles {
    pub fn dispatch(&mut self, effect: &Effect) {
        match effect {
            Effect::Watch(watch, on) => {
                log::info!("effect: {effect:?}");
                self.watch(*watch, *on);
            }
            Effect::PadRoute(route) => {
                log::info!("effect: {effect:?}");
                self.pad.send(pad::Command::Route(*route));
            }
            Effect::Key(stroke) => match self.keyboard.as_mut() {
                Some(kb) => {
                    if let Err(e) = kb.stroke(stroke) {
                        log::warn!("virtual keyboard: {e}");
                    }
                }
                None => log::info!("effect: {effect:?} (no virtual keyboard)"),
            },
            Effect::Launch { game, instance, launch, file, settings, env } => {
                log::info!("launching {game}");
                // MangoHud's first, so a game's own variables can still override it.
                let mut game_env = self.tuning.apply(settings, Some(game)).map(|config| crate::tuning::mangohud_env(&config)).unwrap_or_default();
                game_env.extend(env.iter().cloned());
                // Its own workspace, shown when its first window appears (the state asks for that).
                self.launcher.send(launcher::Request {
                    game: game.clone(),
                    instance: instance.clone(),
                    launch: launch.clone(),
                    file: file.clone(),
                    env: game_env,
                });
            }
            Effect::Apply { game, settings } => {
                self.tuning.apply(settings, game.as_deref());
            }
            Effect::Keymap(keymap) => self.tuning.keymap(keymap),
            Effect::CalibrateGyro => self.tuning.calibrate_gyro(),
            Effect::Save(save) => self.save(save),
            Effect::RecordPlay { game, start, duration } => {
                if let Some(db) = self.paths.stats_db()
                    && let Err(e) = stats::record(&db, game, *start, *duration)
                {
                    log::warn!("recording the play of {game}: {e}");
                }
            }
            Effect::RunApp { instance, command } => launcher::run_app(&self.jobs, instance, command, self.scope),
            Effect::Window(command) => self.window(command),
            Effect::Sound(sound) => self.sound.send(sound.clone()),
            Effect::Brightness { level, shares } => self.brightness.send(brightness::Command { level: *level, shares: shares.clone() }),
            Effect::Wifi(command) => {
                // (The password stays out of the log.)
                if let clamshell_core::wifi::Command::Connect { ssid, .. } = command {
                    log::info!("wifi: join {ssid}");
                }
                self.wifi.send(wifi::Request::Do(command.clone()));
            }
            Effect::Bluetooth(command) => {
                log::info!("effect: {effect:?}");
                self.bluetooth.send(bluetooth::Request::Do(command.clone()));
            }
            Effect::QuitGame { windows, units } => {
                log::info!("quitting the game: windows {windows:?}, scopes {units:?}");
                if let Some(sway) = &self.sway {
                    windows.iter().for_each(|id| sway.send(sway::Command::Close(*id)));
                }
                launcher::end_later(units.iter().filter_map(Instance::unit).collect());
            }
            Effect::Screenshot => self.system.send(crate::system::Command::Screenshot),
            Effect::Sleep => self.system.send(crate::system::Command::Sleep),
            // (The UI handles quitting and the screens' power: they are its Wayland business.)
            Effect::Quit | Effect::Screens(_) => {}
        }
    }

    /// Starts or stops reading what a tab shows.
    fn watch(&self, watch: Watch, on: bool) {
        match watch {
            Watch::Touchpad => self.touchpad.send(if on { touchpad::Command::On } else { touchpad::Command::Off }),
            Watch::Audio => self.mixer.send(mixer::Command::Audio(on)),
            Watch::Backlights => self.mixer.send(mixer::Command::Backlights(on)),
            Watch::Wifi => self.wifi.send(wifi::Request::Watch(on)),
            Watch::Bluetooth => self.bluetooth.send(bluetooth::Request::Watch(on)),
            Watch::Battery => self.battery.send(if on { battery::Command::Start } else { battery::Command::Stop }),
        }
    }

    fn window(&self, command: &WindowCommand) {
        let Some(sway) = &self.sway else {
            return;
        };
        match command {
            WindowCommand::ShowShelf => sway.send(sway::Command::Workspace(Instance::Shelf.name())),
            WindowCommand::Focus(name) => sway.send(sway::Command::Workspace(name.clone())),
            WindowCommand::Move { windows, to } => sway.send(sway::Command::Move { windows: windows.clone(), workspace: to.name() }),
            WindowCommand::Close(ids) => ids.iter().for_each(|id| sway.send(sway::Command::Close(*id))),
        }
    }

    /// Writes one of ClamShell's files.
    fn save(&self, save: &Save) {
        let (file, text) = match save {
            Save::Settings(text) => (self.paths.game_settings(), text),
            Save::Resume(text) => (self.paths.resume.clone(), text),
            Save::Brightness(text) => (self.paths.brightness.clone(), text),
            Save::Keymaps(text) => (self.paths.keymaps(), text),
            Save::Favourites(ids) => {
                if let Some(dir) = &self.paths.state_dir
                    && let Err(e) = favorites::write(dir, ids.iter().map(String::as_str))
                {
                    log::warn!("favourites: {e}");
                }
                return;
            }
        };
        if let Some(file) = file
            && let Err(e) = paths::write(&file, text)
        {
            log::warn!("{}: {e}", file.display());
        }
    }
}
