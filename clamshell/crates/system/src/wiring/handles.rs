//! Carrying out the state's effects. Each effect goes to the thread or the device it is for.

use clamshell_state::{Effect, Instance, Watch, WindowCommand};

use super::Tuning;
use crate::audio::mixer;
use crate::display::brightness;
use crate::input::keyboard::VirtualKeyboard;
use crate::input::{pad, touchpad};
use crate::launch::job::Runner;
use crate::launch::launcher;
use crate::network::{bluetooth, wifi};
use crate::os::worker::{self, Shown, Worker};
use crate::paths::Paths;
use crate::store::{Write, Writer};
use crate::sway;

/// The threads, and the one device, that effects go to.
pub struct Handles {
    pub paths: Paths,
    /// Where saves go. `None` if there is no state dir or its thread couldn't start, and then nothing is
    /// kept.
    pub store: Option<Writer>,
    /// The entries waiting for their metadata.
    pub fetching: crate::library::meta::Queue,
    /// Reads the library again on a thread, and fetches what it wants. With `true` the metadata misses
    /// are forgotten first.
    pub rescan: Box<dyn Fn(bool)>,
    pub touchpad: Worker<touchpad::Command>,
    pub mixer: Worker<mixer::Command>,
    pub wifi: Worker<wifi::Request>,
    pub bluetooth: Worker<bluetooth::Request>,
    pub tray: Worker<clamshell_state::session::tray::TrayCommand>,
    pub metrics: Worker<Shown>,
    /// The bar's status, read often while the screens are on.
    pub status: Worker<Shown>,
    pub icons: Worker<(std::sync::Arc<str>, String)>,
    pub battery: Worker<Shown>,
    pub pad: Worker<pad::Command>,
    /// The controller the pad thread grabbed. Launched SDL programs are told to ignore it.
    pub pad_taken: pad::Taken,
    pub launcher: Worker<launcher::Command>,
    /// Windows and launches, under sway.
    pub sway: Option<Worker<sway::Command>>,
    pub jobs: Runner,
    pub sound: Worker<clamshell_state::Sound>,
    pub brightness: Worker<brightness::Command>,
    pub tuning: Tuning,
    /// Screenshots, sleep and the other device-wide commands (see [`crate::device`]).
    pub system: Worker<crate::device::Command>,
    /// Lists and watches the folder open in the browser.
    pub browse: Worker<clamshell_state::shelf::files::BrowseCommand>,
    pub files: Worker<clamshell_state::shelf::files::FileOp>,
    /// The on-screen keyboard types through it. `None` if it couldn't be made.
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
            Effect::Touchpad(contacts) => self.touchpad.send(touchpad::Command::Contacts(contacts.clone())),
            Effect::Key(stroke) => match self.keyboard.as_mut() {
                Some(kb) => {
                    if let Err(e) = kb.stroke(stroke) {
                        log::warn!("virtual keyboard: {e}");
                    }
                }
                None => log::info!("effect: {effect:?} (no virtual keyboard)"),
            },
            Effect::Launch { entry, instance, launch, settings, env } => {
                log::info!("launching {entry}");
                // MangoHud's first, so a game's own variables can still override it.
                let mut game_env = self.tuning.apply(settings, Some(entry)).map(|config| crate::readings::mangohud::env(&config)).unwrap_or_default();
                game_env.extend(env.iter().cloned());
                // The variables for the grabbed controller go first, so a game's own may override them.
                game_env.splice(0..0, self.pad_taken.env());
                // The game gets its own workspace, which is shown when its first window appears. The state
                // asks for that.
                self.launcher.send(launcher::Command::Launch(launcher::Request {
                    entry: entry.clone(),
                    instance: instance.clone(),
                    launch: launch.clone(),
                    env: game_env,
                }));
            }
            Effect::Apply { entry, settings } => {
                self.tuning.apply(settings, entry.as_deref());
            }
            Effect::Keymap(keymap) => self.tuning.keymap(keymap),
            Effect::CalibrateGyro => self.tuning.calibrate_gyro(),
            Effect::Save(save) => {
                if let Some(store) = &self.store {
                    store.send(Write::Save(save.clone()));
                }
            }
            Effect::FetchFirst(ids) => self.fetching.first(ids),
            Effect::Tool(tool) => (self.rescan)(*tool == clamshell_library::Tool::Refetch),
            Effect::ReadPosition { entry, file } => self.launcher.send(launcher::Command::ReadPosition { entry: entry.clone(), file: file.clone() }),
            Effect::RecordPlay { entry, start, duration } => {
                if let Some(store) = &self.store {
                    store.send(Write::Play { id: entry.clone(), start: *start, duration: *duration });
                }
            }
            Effect::Window(command) => self.window(command),
            Effect::Enqueue { key, lines } => {
                log::info!("effect: {effect:?}");
                let socket = self.paths.socket(key);
                let lines = lines.clone();
                // Off the main thread, so a player slow to read holds nothing up here.
                let started = worker::detached("enqueue", move || {
                    if let Err(e) = crate::launch::mpv::enqueue(&socket, &lines) {
                        log::warn!("enqueue to {}: {e}", socket.display());
                    }
                });
                if let Err(e) = started {
                    log::warn!("enqueue: {e}");
                }
            }
            Effect::Browse(command) => {
                log::info!("effect: {effect:?}");
                self.browse.send(command.clone());
            }
            Effect::Files(op) => self.files.send(op.clone()),
            Effect::Sound(sound) => self.sound.send(sound.clone()),
            Effect::Brightness { level, shares } => self.brightness.send(brightness::Command { level: *level, shares: shares.clone() }),
            Effect::Wifi(command) => {
                log::info!("effect: {effect:?}");
                self.wifi.send(wifi::Request::Do(command.clone()));
            }
            Effect::Bluetooth(command) => {
                log::info!("effect: {effect:?}");
                self.bluetooth.send(bluetooth::Request::Do(command.clone()));
            }
            Effect::EndRun { key, force } => launcher::end_run(key, *force),
            Effect::RunnerCommand(argv) => launcher::runner_command(argv),
            Effect::Adopt { instance, pid } => launcher::adopt(instance.clone(), *pid),
            Effect::FindIcon { entry, app_id } => self.icons.send((entry.clone(), app_id.clone())),
            Effect::EndProcesses { pids, force } => launcher::end_processes(pids, *force),
            Effect::Freeze { slice, frozen } => crate::launch::freezer::freeze(slice.clone(), *frozen),
            Effect::Tray(command) => self.tray.send(command.clone()),
            Effect::Screenshot => self.system.send(crate::device::Command::Screenshot),
            Effect::AskAwake(argv) => self.system.send(crate::device::Command::AskAwake(argv.clone())),
            Effect::Sleep => self.system.send(crate::device::Command::Sleep),
            Effect::Account(command) => self.system.send(crate::device::Command::Account(command.clone())),
            Effect::CalibrateRefresh => self.system.send(crate::device::Command::CalibrateRefresh),
            Effect::CalibrateSticks => self.tuning.pad.send(pad::Command::CalibrateSticks),
            Effect::SetTimeZone(zone) => self.system.send(crate::device::Command::SetTimeZone(zone.clone())),
            Effect::SetNtp(on) => self.system.send(crate::device::Command::SetNtp(*on)),
            Effect::StepClock(field, by) => self.system.send(crate::device::Command::StepClock(*field, *by)),
            Effect::Notify { summary, body, urgent } => {
                self.system.send(crate::device::Command::Notify { summary: summary.clone(), body: body.clone(), urgent: *urgent })
            }
            // The UI handles quitting and the screens' power, since they are its Wayland business.
            Effect::Quit | Effect::Screens { .. } => {}
        }
    }

    /// Starts or stops reading what a tab shows.
    fn watch(&self, watch: Watch, on: bool) {
        match watch {
            Watch::Audio => self.mixer.send(mixer::Command::Audio(on)),
            Watch::Backlights => self.mixer.send(mixer::Command::Backlights(on)),
            Watch::Wifi => self.wifi.send(wifi::Request::Watch(on)),
            Watch::Bluetooth => self.bluetooth.send(bluetooth::Request::Watch(on)),
            // The battery tab shows the metrics too.
            Watch::Battery => {
                self.battery.send(Shown(on));
                self.metrics.send(Shown(on));
            }
            Watch::Metrics => self.metrics.send(Shown(on)),
            Watch::Status => self.status.send(Shown(on)),
            // Read once as the tab opens, since it changes only through the tab.
            Watch::Time if on => self.system.send(crate::device::Command::ReadTime),
            Watch::Time => {}
            // The Device view shows the account and the time zone.
            Watch::Account if on => {
                self.system.send(crate::device::Command::Account(clamshell_state::device::account::AccountCommand::Read));
                self.system.send(crate::device::Command::ReadTime);
            }
            Watch::Account => {}
        }
    }

    fn window(&self, command: &WindowCommand) {
        let Some(sway) = &self.sway else {
            return;
        };
        match command {
            WindowCommand::ShowShelf => sway.send(sway::Command::Workspace(Instance::Shelf.name())),
            WindowCommand::Focus(name) => sway.send(sway::Command::Workspace(name.clone())),
            WindowCommand::Move { windows, to } => sway.send(sway::Command::Move { windows: windows.clone(), workspace: to.clone() }),
            WindowCommand::Close(ids) => ids.iter().for_each(|id| sway.send(sway::Command::Close(*id))),
            WindowCommand::Down(id) => sway.send(sway::Command::Down(*id)),
            WindowCommand::Up { window, to } => sway.send(sway::Command::Up { window: *window, workspace: to.as_ref().map(Instance::workspace) }),
            WindowCommand::ShowDown(id) => sway.send(sway::Command::ShowDown(*id)),
            WindowCommand::BottomPower(on) => sway.send(sway::Command::Power(*on)),
            WindowCommand::DownRules(rules) => sway.send(sway::Command::DownRules(rules.clone())),
            WindowCommand::LandRules(ids) => sway.send(sway::Command::LandRules(ids.clone())),
        }
    }
}
