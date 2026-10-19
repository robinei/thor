//! Carrying out the state's effects: each goes to the thread, the device or the database it is for.

use clamshell_core::{Effect, Instance, Save, Watch, WindowCommand};

use crate::job::Runner;
use crate::keyboard::VirtualKeyboard;
use crate::paths::Paths;
use crate::store::Store;
use crate::tuning::Tuning;
use crate::worker::Worker;
use crate::{battery, bluetooth, brightness, launcher, mixer, pad, sway, touchpad, wifi};

/// The threads (and the one device and the database) that effects go to.
pub struct Handles {
    pub paths: Paths,
    /// Where saves go; `None` if it couldn't be opened (then nothing is kept).
    pub store: Option<Store>,
    /// What waits for its metadata.
    pub fetching: crate::meta::Queue,
    /// Reads the library again (and fetches what it wants), on a thread; `true`: the metadata misses
    /// forgotten first.
    pub rescan: Box<dyn Fn(bool)>,
    /// Apps and games run in systemd scopes (there is a user session).
    pub scope: bool,
    pub touchpad: Worker<touchpad::Command>,
    pub mixer: Worker<mixer::Command>,
    pub wifi: Worker<wifi::Request>,
    pub bluetooth: Worker<bluetooth::Request>,
    pub tray: Worker<clamshell_core::tray::TrayCommand>,
    pub battery: Worker<battery::Command>,
    pub pad: Worker<pad::Command>,
    /// The controller the pad thread grabbed: SDL programs launched are told to ignore it.
    pub pad_taken: pad::Taken,
    pub launcher: Worker<launcher::Request>,
    /// Under sway: windows and launches.
    pub sway: Option<Worker<sway::Command>>,
    pub jobs: Runner,
    pub sound: Worker<clamshell_core::Sound>,
    pub brightness: Worker<brightness::Command>,
    pub tuning: Tuning,
    /// Screenshots and sleep.
    pub system: Worker<crate::device::Command>,
    /// Lists (and watches) the browsed folder open; does the file operations.
    pub browse: Worker<Option<clamshell_core::files::Browse>>,
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
            Effect::Launch { game, instance, launch, settings, env, watch } => {
                log::info!("launching {game}");
                // MangoHud's first, so a game's own variables can still override it.
                let mut game_env = self.tuning.apply(settings, Some(game)).map(|config| crate::tuning::mangohud_env(&config)).unwrap_or_default();
                game_env.extend(env.iter().cloned());
                // (First: a game's own may override it.)
                game_env.splice(0..0, self.pad_taken.env());
                // Its own workspace, shown when its first window appears (the state asks for that).
                self.launcher.send(launcher::Request {
                    game: game.clone(),
                    instance: instance.clone(),
                    launch: launch.clone(),
                    watch: watch.clone(),
                    env: game_env,
                });
            }
            Effect::Apply { game, settings } => {
                self.tuning.apply(settings, game.as_deref());
            }
            Effect::Keymap(keymap) => self.tuning.keymap(keymap),
            Effect::CalibrateGyro => self.tuning.calibrate_gyro(),
            Effect::Save(save) => self.save(save),
            Effect::FetchFirst(ids) => self.fetching.first(ids),
            Effect::Tool(tool) => match tool.as_str() {
                "rescan" => (self.rescan)(false),
                "refetch" => (self.rescan)(true),
                other => log::warn!("no tool {other}"),
            },
            Effect::RecordPlay { game, start, duration } => {
                if let Some(store) = &self.store
                    && let Err(e) = store.add_play(game, *start, *duration)
                {
                    log::warn!("recording the play of {game}: {e}");
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
                self.browse.send(browse.clone());
            }
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
            Effect::EndSteam { appid } => launcher::end_steam(*appid),
            Effect::Stop(argv) => launcher::stop(argv),
            Effect::Tray(command) => self.tray.send(command.clone()),
            Effect::Screenshot => self.system.send(crate::device::Command::Screenshot),
            Effect::Sleep => self.system.send(crate::device::Command::Sleep),
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
            Watch::Battery => self.battery.send(if on { battery::Command::Start } else { battery::Command::Stop }),
            // (Read once as the tab opens: it changes only by the tab.)
            Watch::Time if on => self.system.send(crate::device::Command::ReadTime),
            Watch::Time => {}
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

    /// Saves what the state keeps, replacing what was saved of it.
    fn save(&mut self, save: &Save) {
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let saved = match save {
            Save::Settings(s) => store.save_settings(s),
            Save::Keymaps(k) => store.save_keymaps(k),
            Save::Favourites(ids) => store.save_favourites(ids),
            Save::Resume(r) => store.save_resume(r),
            Save::Brightness(b) => store.save_brightness(b),
            Save::TwelveHour(on) => store.save_twelve_hour(*on),
            Save::WindowRules(r) => store.save_window_rules(r),
            Save::Dock(ids) => store.save_dock(ids),
        };
        if let Err(e) = saved {
            log::warn!("store: {e}");
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
