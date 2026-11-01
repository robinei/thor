//! Carrying out the state's effects. Each effect goes to the thread or the device it is for.

use clamshell_state::{Effect, Watch};

use super::Tuning;
use crate::audio::mixer;
use crate::display::brightness;
use crate::input::{pad, touchpad};
use crate::launch::launcher;
use crate::network::{bluetooth, wifi};
use crate::os::worker::{Shown, Worker};
use crate::paths::Paths;
use crate::session::notifications::Command as Notifications;
use crate::session::portal::Command as Portal;
use crate::store::{Write, Writer};

/// The threads, and the one device, that effects go to.
pub struct Handles {
    pub(crate) paths: Paths,
    /// Where saves go. `None` if there is no state dir or its thread couldn't start, and then nothing is
    /// kept.
    pub(crate) store: Option<Writer>,
    /// The entries waiting for their metadata.
    pub(crate) fetching: crate::library::meta::Queue,
    /// Reads the library again on a thread, and fetches what it wants (see [`super::Rescan`]).
    pub(crate) rescan: Worker<super::Rescan>,
    pub(crate) touchpad: Worker<touchpad::Command>,
    pub(crate) mixer: Worker<mixer::Command>,
    pub(crate) wifi: Worker<wifi::Request>,
    pub(crate) bluetooth: Worker<bluetooth::Request>,
    pub(crate) tray: Worker<clamshell_state::session::tray::TrayCommand>,
    /// The notification server, which emits the signals for programs' notifications closed or tapped.
    pub(crate) notifications: Worker<Notifications>,
    /// The file chooser served to apps, which answers their requests.
    pub(crate) portal: Worker<Portal>,
    pub(crate) metrics: Worker<Shown>,
    /// The bar's status, read often while the screens are on.
    pub(crate) status: Worker<Shown>,
    pub(crate) icons: Worker<(std::sync::Arc<str>, String)>,
    pub(crate) battery: Worker<Shown>,
    pub(crate) pad: Worker<pad::Command>,
    /// The controller the pad thread grabbed. Launched SDL programs are told to ignore it.
    pub(crate) pad_taken: pad::Taken,
    pub(crate) launcher: Worker<launcher::Command>,
    /// Ends, adopts and pauses running programs, in order (see [`launcher::spawn_processes`]).
    pub(crate) processes: Worker<launcher::ToProcesses>,
    /// Freezes hidden games' slices and thaws them.
    pub(crate) freezer: Worker<crate::launch::freezer::Request>,
    pub(crate) sound: Worker<(clamshell_state::Sound, u64)>,
    pub(crate) brightness: Worker<brightness::Command>,
    /// Writes the stick lights, and turns them off when dropped, as ClamShell ends.
    pub(crate) lights: crate::lights::Lights,
    pub(crate) tuning: Tuning,
    /// Screenshots, sleep and the other one-off requests (see [`crate::errands`]).
    pub(crate) system: Worker<crate::errands::Command>,
    /// Lists and watches the folder open in the browser.
    pub(crate) browse: Worker<clamshell_state::library_view::files::BrowseCommand>,
    pub(crate) files: Worker<clamshell_state::library_view::files::FileOp>,
}

impl Handles {
    /// Save an in-process compositor capture and report the result through the system sink.
    pub fn save_screenshot(&self, pixels: Result<clamshell_state::ScreenshotPixels, String>) {
        self.system.send(crate::errands::Command::SaveScreenshot(pixels));
    }

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
            Effect::PadActivity(watch) => self.pad.send(pad::Command::WatchActivity(*watch)),
            Effect::Touchpad(contacts) => self.touchpad.send(touchpad::Command::Contacts(contacts.clone())),
            Effect::SizeTouchpad(layout) => self.touchpad.send(touchpad::Command::Layout(*layout)),
            Effect::Launch { entry, unit, launch, settings, tile_env } => {
                log::info!("launching {entry}");
                let ignore = self.pad_taken.ignore();
                self.launcher.send(launcher::Command::Launch(launcher::Request {
                    entry: entry.clone(),
                    unit: unit.clone(),
                    launch: launch.clone(),
                    env: tile_env.clone(),
                    settings: *settings,
                    ignore,
                }));
            }
            Effect::Apply { settings, .. } => self.tuning.apply(settings),
            Effect::Keymap(player, keymap) => self.tuning.keymap(*player, keymap),
            Effect::CalibrateGyro => self.tuning.calibrate_gyro(),
            Effect::Save(save) => {
                if let Some(store) = &self.store {
                    store.send(Write::Save(save.clone()));
                }
            }
            Effect::FetchFirst(ids) => self.fetching.first(ids),
            Effect::Tool(tool) if *tool == clamshell_library::Tool::Refetch => self.rescan.send(super::Rescan::Refetch),
            Effect::Tool(_) => self.rescan.send(super::Rescan::Read),
            Effect::WriteLibrary(config) => {
                log::info!("effect: library.json written");
                self.rescan.send(super::Rescan::Write(config.clone()));
            }
            Effect::ReadPosition { entry, file } => self.launcher.send(launcher::Command::ReadPosition { entry: entry.clone(), file: file.clone() }),
            Effect::RecordPlay { entry, start, duration } => {
                if let Some(store) = &self.store {
                    store.send(Write::Play { id: entry.clone(), start: *start, duration: *duration });
                }
            }
            Effect::Enqueue { key, lines } => {
                log::info!("effect: {effect:?}");
                self.processes.send(launcher::ToProcesses::Enqueue { socket: self.paths.socket(key), lines: lines.clone() });
            }
            Effect::Browse(command) => {
                log::info!("effect: {effect:?}");
                self.browse.send(command.clone());
            }
            Effect::Files(op) => self.files.send(op.clone()),
            Effect::Sound { sound, number } => self.sound.send((sound.clone(), *number)),
            Effect::Brightness { level, shares } => self.brightness.send(brightness::Command { level: *level, shares: shares.clone() }),
            Effect::Wifi(command) => {
                log::info!("effect: {effect:?}");
                self.wifi.send(wifi::Request::Do(command.clone()));
            }
            Effect::Bluetooth(command) => {
                log::info!("effect: {effect:?}");
                self.bluetooth.send(bluetooth::Request::Do(command.clone()));
            }
            Effect::EndRun { key, force } => self.processes.send(launcher::ToProcesses::EndRun { key: key.clone(), force: *force }),
            Effect::RunnerCommand(argv) => self.processes.send(launcher::ToProcesses::Runner(argv.clone())),
            Effect::Adopt { unit, pid } => self.processes.send(launcher::ToProcesses::Adopt(unit.clone(), *pid)),
            Effect::FindIcon { entry, app_id } => self.icons.send((entry.clone(), app_id.clone())),
            Effect::EndProcesses { pids, force } => self.processes.send(launcher::ToProcesses::End { pids: pids.clone(), force: *force }),
            Effect::Freeze { slice, frozen } => self.freezer.send((slice.clone(), *frozen)),
            Effect::Tray(command) => self.tray.send(command.clone()),
            Effect::AskAwake(argv) => self.system.send(crate::errands::Command::AskAwake(argv.clone())),
            Effect::Sleep => self.system.send(crate::errands::Command::Sleep),
            Effect::Account(command) => self.system.send(crate::errands::Command::Account(command.clone())),
            Effect::ReadInstalled => self.system.send(crate::errands::Command::ReadInstalled),
            Effect::CalibrateRefresh => self.system.send(crate::errands::Command::CalibrateRefresh),
            Effect::CalibrateSticks => self.tuning.pad.send(pad::Command::CalibrateSticks),
            Effect::Players(players) => self.tuning.pad.send(pad::Command::Players(players.clone())),
            Effect::SetTimeZone(zone) => self.system.send(crate::errands::Command::SetTimeZone(zone.clone())),
            Effect::SetNtp(on) => self.system.send(crate::errands::Command::SetNtp(*on)),
            Effect::StepClock(field, by) => self.system.send(crate::errands::Command::StepClock(*field, *by)),
            Effect::NotificationClosed { id, reason } => self.notifications.send(Notifications::Closed { id: *id, reason: reason.code() }),
            Effect::ActionInvoked { id, key } => self.notifications.send(Notifications::Invoked { id: *id, key: key.clone() }),
            Effect::FileChosen { request, paths } => self.portal.send(Portal::Answer { id: *request, paths: paths.clone() }),
            Effect::Lights(lit) => self.lights.send(*lit),
            // The compositor carries out its own.
            Effect::Compositor(_) => {}
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
            Watch::Time if on => self.system.send(crate::errands::Command::ReadTime),
            Watch::Time => {}
            // The Device view shows the account and the time zone.
            Watch::Account if on => {
                self.system.send(crate::errands::Command::Account(clamshell_state::setup::account::AccountCommand::Read));
                self.system.send(crate::errands::Command::ReadTime);
            }
            Watch::Account => {}
        }
    }
}
