//! Carrying out the state's effects: each goes to the thread or the device it is for.

use clamshell_core::{Effect, Instance, Watch, WindowCommand};

use crate::job::Runner;
use crate::keyboard::VirtualKeyboard;
use crate::paths::Paths;
use crate::store::{Write, Writer};
use crate::tuning::Tuning;
use crate::worker::Worker;
use crate::{battery, bluetooth, brightness, launcher, mixer, pad, sway, touchpad, wifi};

/// The threads (and the one device) that effects go to.
pub struct Handles {
    pub paths: Paths,
    /// Where saves go; `None` without a state dir, or its thread (then nothing is kept).
    pub store: Option<Writer>,
    /// What waits for its metadata.
    pub fetching: crate::meta::Queue,
    /// Reads the library again (and fetches what it wants), on a thread; `true`: the metadata misses
    /// forgotten first.
    pub rescan: Box<dyn Fn(bool)>,
    pub touchpad: Worker<touchpad::Command>,
    pub mixer: Worker<mixer::Command>,
    pub wifi: Worker<wifi::Request>,
    pub bluetooth: Worker<bluetooth::Request>,
    pub tray: Worker<clamshell_core::tray::TrayCommand>,
    pub metrics: Worker<bool>,
    /// The bar's status, read often while the screens are on.
    pub status: Worker<bool>,
    pub icons: Worker<(std::sync::Arc<str>, String)>,
    pub battery: Worker<battery::Command>,
    pub pad: Worker<pad::Command>,
    /// The controller the pad thread grabbed: SDL programs launched are told to ignore it.
    pub pad_taken: pad::Taken,
    pub launcher: Worker<launcher::Command>,
    /// Under sway: windows and launches.
    pub sway: Option<Worker<sway::Command>>,
    pub jobs: Runner,
    pub sound: Worker<clamshell_core::Sound>,
    pub brightness: Worker<brightness::Command>,
    pub tuning: Tuning,
    /// Screenshots and sleep.
    pub system: Worker<crate::device::Command>,
    /// Lists (and watches) the browsed folder open; does the file operations.
    pub browse: Worker<clamshell_core::files::BrowseCommand>,
    pub files: Worker<clamshell_core::files::FileOp>,
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
            Effect::Touchpad(contacts) => self.touchpad.send(touchpad::Command::Contacts(contacts.clone())),
            Effect::Key(stroke) => match self.keyboard.as_mut() {
                Some(kb) => {
                    if let Err(e) = kb.stroke(stroke) {
                        log::warn!("virtual keyboard: {e}");
                    }
                }
                None => log::info!("effect: {effect:?} (no virtual keyboard)"),
            },
            Effect::Launch { game, instance, launch, settings, env } => {
                log::info!("launching {game}");
                // MangoHud's first, so a game's own variables can still override it.
                let mut game_env = self.tuning.apply(settings, Some(game)).map(|config| crate::tuning::mangohud_env(&config)).unwrap_or_default();
                game_env.extend(env.iter().cloned());
                // (First: a game's own may override it.)
                game_env.splice(0..0, self.pad_taken.env());
                // Its own workspace, shown when its first window appears (the state asks for that).
                self.launcher.send(launcher::Command::Launch(launcher::Request {
                    game: game.clone(),
                    instance: instance.clone(),
                    launch: launch.clone(),
                    env: game_env,
                }));
            }
            Effect::Apply { game, settings } => {
                self.tuning.apply(settings, game.as_deref());
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
            Effect::ReadPosition { game, file } => self.launcher.send(launcher::Command::ReadPosition { game: game.clone(), file: file.clone() }),
            Effect::RecordPlay { game, start, duration } => {
                if let Some(store) = &self.store {
                    store.send(Write::Play { id: game.clone(), start: *start, duration: *duration });
                }
            }
            Effect::Window(command) => self.window(command),
            Effect::Enqueue { key, lines } => {
                log::info!("effect: {effect:?}");
                let socket = self.paths.socket(key);
                let lines = lines.clone();
                // (Off the main thread: a player slow to read waits nothing here.)
                std::thread::spawn(move || {
                    if let Err(e) = enqueue(&socket, &lines) {
                        log::warn!("enqueue to {}: {e}", socket.display());
                    }
                });
            }
            Effect::Browse(browse) => {
                log::info!("effect: {effect:?}");
                self.browse.send(clamshell_core::files::BrowseCommand::Open(browse.clone()));
            }
            Effect::FilesInUse(files) => self.browse.send(clamshell_core::files::BrowseCommand::InUse(files.clone())),
            Effect::Files(op) => self.files.send(op.clone()),
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
            Effect::EndRun { key, force } => launcher::end_run(key, *force),
            Effect::RunnerCommand(argv) => launcher::runner_command(argv),
            Effect::Adopt { instance, pid } => launcher::adopt(instance.clone(), *pid),
            Effect::FindIcon { entry, app_id } => self.icons.send((entry.clone(), app_id.clone())),
            Effect::EndProcesses { pids, force } => launcher::end_processes(pids, *force),
            Effect::Freeze { slice, frozen } => crate::units::freeze(slice.clone(), *frozen),
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
            // (The UI handles quitting and the screens' power: they are its Wayland business.)
            Effect::Quit | Effect::Screens(_) => {}
        }
    }

    /// Starts or stops reading what a tab shows.
    fn watch(&self, watch: Watch, on: bool) {
        match watch {
            Watch::Audio => self.mixer.send(mixer::Command::Audio(on)),
            Watch::Backlights => self.mixer.send(mixer::Command::Backlights(on)),
            Watch::Wifi => self.wifi.send(wifi::Request::Watch(on)),
            Watch::Bluetooth => self.bluetooth.send(bluetooth::Request::Watch(on)),
            // (The battery tab shows the metrics too.)
            Watch::Battery => {
                self.battery.send(if on { battery::Command::Start } else { battery::Command::Stop });
                self.metrics.send(on);
            }
            Watch::Metrics => self.metrics.send(on),
            Watch::Status => self.status.send(on),
            // (Read once as the tab opens: it changes only by the tab.)
            Watch::Time if on => self.system.send(crate::device::Command::ReadTime),
            Watch::Time => {}
            // (The Device view: the account, and the time zone it shows.)
            Watch::Account if on => {
                self.system.send(crate::device::Command::Account(clamshell_core::account::AccountCommand::Read));
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

/// `lines` written to the control socket at `socket`, each its own line, then as many lines read back
/// (or none, for a while), so the player reads them all before the connection closes.
fn enqueue(socket: &std::path::Path, lines: &[String]) -> std::io::Result<()> {
    use std::io::{BufRead, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    for line in lines {
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
    }
    // (mpv answers each command, and drops those it hasn't read when the client goes.)
    let mut replies = std::io::BufReader::new(stream).lines();
    for _ in lines {
        if !matches!(replies.next(), Some(Ok(_))) {
            break;
        }
    }
    Ok(())
}
