//! The controller path. Every controller is grabbed for good, and games get a virtual Xbox 360 pad per
//! player that is there for as long as the count of players stands.
//!
//! Controllers are found as they come (an inotify watch of /dev/input rings the thread, [`Command::Look`])
//! and let go as they go, while the virtual devices stay. Each controller is one player's, as the state
//! says ([`Command::Players`]; one it hasn't said is player 1's). A player's controllers are merged into
//! one ([`merge`]): a button is down while it is down on any of them, and each stick, trigger and hat axis
//! is the controller's furthest from rest. With CLAMSHELL_GAMEPAD set, only that controller is used.
//!
//! Each player's controller goes to its router ([`route`]), and from there through the player's keymap to
//! its virtual pad. Player 1's router drives ClamShell, as the state chooses ([`PadRoute`]):
//!
//! - the game, through the keymap in force ([`remap`]). That means the virtual pad, keys and mouse
//!   buttons on the virtual mouse's keyboard, and the pointer. A stick's or trigger's tuning, such as its
//!   deadzone, is part of the keymap.
//! - the pad mouse ([`mouse`]);
//! - ClamShell's screens ([`navigator`]);
//! - typing into an app ([`typing`]);
//! - a keymap's edit screen, listening for an input ([`listen`]).
//!
//! The other players' routers are always the game's, each through its own keymap, with Home their
//! game's Guide.
//!
//! The gyro comes from a motion sensor next to the built-in controller (the device profile's
//! `builtin_driver`), or else next to the first controller found ([`motion`]), and goes to that
//! controller's player. Away from the game player 1's virtual pad shows a neutral controller, so games
//! never see it disappear.
//!
//! The home button (BTN_MODE) on a player 1 controller is ours and goes to the program as
//! [`Event::Home`]. Holding it and then pressing Back with a game in front gives the game's Guide (see
//! [`route`]). The Back button ([`codes::BACK_KEY`]) is a keymap input in the game and means back on
//! ClamShell's screens. The AYN button ([`codes::AYN_KEY`]) is on the Thor's GPIO keys and is read as the
//! built-in controller's ([`device::AuxiliaryKey`]). It is also a keymap input in the game, and Guide by
//! default, as on the pad.
//!
//! The virtual devices outlive the controllers. A player whose controllers are gone has its pad at
//! rest until one is back. Games' rumble on a player's virtual pad reaches the motors of that player's
//! controllers ([`rumble`]), for player 1 while the game has the pad, scaled by the Rumble setting.

mod calibrate;
mod calibrator;
mod carry;
mod codes;
mod device;
mod listen;
mod merge;
pub(crate) mod motion;
mod mouse;
mod navigator;
#[cfg(test)]
mod no_alloc;
mod remap;
mod route;
mod rumble;
mod sensor_hub;
mod typing;

use std::io;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clamshell_state::Event;
use clamshell_state::PadRoute;
use clamshell_state::device_view::SticksEvent;
use clamshell_state::device_view::controllers::{ControllerEvent, Player, Players};
use clamshell_state::keymap::Keymap;
use clamshell_state::settings::RumbleStrength;
use rustix::event::{PollFd, PollFlags};

use crate::os::evdev::Raw;
use crate::os::evdev::device::{DeviceSpec, Grabbed};
use crate::os::worker::{self, Bell, Ended, Next, Service, Worker, lost_on};
use crate::{Closed, Sink};
use device::{AuxiliaryKey, Capabilities, VirtualMouse, VirtualPad};
use merge::{Inputs, Merge, Merged, Normalise};
use motion::{EvdevMotion, MotionSource, Sample};
use route::{Out, PadRouter, PadSnapshot};
use rumble::{Buzz, Rumble};

/// How often the virtual devices are tried again when they can't be made, besides after any command.
const RETRY: Duration = Duration::from_secs(30);
/// How often controllers are looked for when /dev/input can't be watched.
const LOOK_UNWATCHED: Duration = Duration::from_secs(5);
/// The most controllers taken at once. One more is left alone, logged.
const MOST_CONTROLLERS: usize = 8;

#[derive(Debug, Clone)]
pub(crate) enum Command {
    Route(PadRoute),
    WatchActivity(bool),
    /// A player's keymap in force. Default has no bindings.
    Keymap(Player, Box<Keymap>),
    /// Calibrate the gyro from its next samples.
    CalibrateGyro,
    /// The Rumble setting in force: 0 off, 1 low, 2 mid, 3 full.
    Rumble(RumbleStrength),
    /// /dev/input changed, so controllers are looked for again.
    Look,
    /// The system is about to sleep. The gyro's stream is closed by dropping its source, and then the
    /// command is acknowledged ([`crate::power::sleep::spawn_hold`]).
    Sleep(Ack),
    /// The system is awake again, so the gyro is found again.
    Woke,
    /// Calibrate the sticks of the controller used last (see [`calibrate`]). The progress and the end go to
    /// the state.
    CalibrateSticks,
    /// The sticks' calibrations, by controller. They are sent at start, and a calibration made here is
    /// kept here too.
    Sticks(clamshell_state::device_view::sticks::Calibrations),
    /// How many players there are, and the controllers' players by key. They are sent at start and whenever
    /// they change. A controller not among them is player 1.
    Players(Players),
}

/// A command's acknowledgement, sent once the command is carried out.
#[derive(Debug, Clone)]
pub(crate) struct Ack(pub(crate) std::sync::mpsc::Sender<()>);

lost_on!(io::Error);

/// The grabbed controllers' USB ids (vendor, product), for what is launched. SDL programs are told to
/// ignore them with `SDL_GAMECONTROLLER_IGNORE_DEVICES`, as Steam does, so they take the virtual pads
/// rather than the controllers, which are grabbed and so silent to them. mpv, for one, opens only the
/// first controller it finds. Every controller grabbed since the start is kept, since one may come back
/// while the program runs. The list is also written to a file ([`crate::paths::Paths::grabbed`]), for
/// Steam's games, whose launch hook (`clamshell --launch-env`) runs outside ClamShell.
#[derive(Debug, Clone, Default)]
pub(crate) struct Taken {
    ids: Arc<Mutex<Vec<(u16, u16)>>>,
    file: Option<Arc<std::path::Path>>,
}

impl Taken {
    /// None grabbed yet, the list written to `file`; one left by an earlier run is removed.
    pub(crate) fn new(file: Option<std::path::PathBuf>) -> Taken {
        if let Some(file) = &file
            && let Err(e) = std::fs::remove_file(file)
            && e.kind() != io::ErrorKind::NotFound
        {
            log::warn!("pad: {}: {e}: Steam's games may be told to ignore controllers no longer grabbed", file.display());
        }
        Taken { ids: Arc::default(), file: file.map(Into::into) }
    }

    fn add(&self, ids: (u16, u16)) {
        let ignore = {
            let mut taken = crate::os::lock(&self.ids);
            if taken.contains(&ids) {
                return;
            }
            taken.push(ids);
            ignore_of(&taken)
        };
        if let (Some(file), Some(ignore)) = (&self.file, ignore)
            && let Err(e) = crate::os::fs::write_file(file, ignore.as_bytes())
        {
            log::warn!("pad: {}: {e}: Steam's games aren't told to ignore the grabbed controllers", file.display());
        }
    }

    /// The controllers to ignore, as `SDL_GAMECONTROLLER_IGNORE_DEVICES` takes them, if one was grabbed.
    pub(crate) fn ignore(&self) -> Option<String> {
        ignore_of(&crate::os::lock(&self.ids))
    }
}

/// The ids as `SDL_GAMECONTROLLER_IGNORE_DEVICES` takes them, if any. A controller with the virtual pad's
/// ids is left out, since ignoring those would hide the virtual pads too.
fn ignore_of(taken: &[(u16, u16)]) -> Option<String> {
    let ids: Vec<String> = taken.iter().filter(|&&ids| ids != device::VIRTUAL_IDS).map(|(v, p)| format!("0x{v:04x}/0x{p:04x}")).collect();
    (!ids.is_empty()).then(|| ids.join(","))
}

/// Owns the controllers as they come, with their ids in `taken`. `spec` comes from CLAMSHELL_GAMEPAD.
pub(crate) fn spawn(
    spec: DeviceSpec,
    controls: clamshell_device::input::Controls,
    sensor_hub: Option<clamshell_device::gyro::SensorHub>,
    taken: Taken,
    sink: impl Sink,
) -> io::Result<Worker<Command>> {
    let watched = crate::input::watch::devices_watch();
    // Kept through a restart after a panic: games keep their virtual controllers, and the keymaps, route,
    // calibrations and players stay.
    let bell = worker::Bell::new()?;
    let mut pad = PadThread {
        bell: bell.clone(),
        spec,
        controls,
        sensor_hub,
        taken,
        sink,
        route: PadRoute::default(),
        watch_activity: false,
        keymaps: Default::default(),
        devices: None,
        rumble_setting: RumbleStrength::default(),
        sticks: Default::default(),
        players: Players::default(),
        told: Vec::new(),
        look_every: if watched.is_some() { None } else { Some(LOOK_UNWATCHED) },
    };
    let worker = Worker::spawn_belled(clamshell_state::Thread::Pad.name(), bell, move |commands| worker::supervise(&mut pad, commands, RETRY))?;
    if let Some(notify) = watched {
        let pad = worker.clone();
        worker::lasting(clamshell_state::Thread::PadDevices.name(), move || {
            let wait = || {
                let mut fds = [PollFd::new(&notify, PollFlags::IN)];
                rustix::event::poll(&mut fds, None)?;
                let mut buf = [std::mem::MaybeUninit::uninit(); 4096];
                let mut reader = rustix::fs::inotify::Reader::new(&notify, &mut buf);
                while reader.next().is_ok() {}
                Ok(())
            };
            watch_devices(wait, || pad.try_send(Command::Look).is_ok(), std::thread::sleep);
        })?;
    }
    Ok(worker)
}

/// Tells the pad thread to look for controllers each time /dev/input changes (`wait` returns, having
/// taken the watch's events), until the thread is gone (`look` is false). If waiting fails, the watch is
/// given up and the thread is told to look every [`LOOK_UNWATCHED`] instead, as when /dev/input can't be
/// watched, so controllers plugged in are still found.
fn watch_devices(mut wait: impl FnMut() -> rustix::io::Result<()>, mut look: impl FnMut() -> bool, mut pause: impl FnMut(Duration)) {
    loop {
        match wait() {
            Ok(()) | Err(rustix::io::Errno::INTR) => {}
            Err(e) => {
                log::warn!("pad devices: {e}: /dev/input no longer watched; controllers looked for every {} s", LOOK_UNWATCHED.as_secs());
                break;
            }
        }
        if !look() {
            return;
        }
    }
    loop {
        pause(LOOK_UNWATCHED);
        if !look() {
            return;
        }
    }
}

/// What the pad thread keeps across the controllers' comings and goings, and a restart.
struct PadThread<S> {
    /// Rung by the thread's commands, and waited on together with the controllers.
    bell: Bell,
    spec: DeviceSpec,
    controls: clamshell_device::input::Controls,
    sensor_hub: Option<clamshell_device::gyro::SensorHub>,
    taken: Taken,
    sink: S,
    route: PadRoute,
    watch_activity: bool,
    /// Each player's keymap in force.
    keymaps: [Keymap; Player::MOST],
    devices: Option<Devices>,
    rumble_setting: RumbleStrength,
    /// The sticks' calibrations, by controller.
    sticks: clamshell_state::device_view::sticks::Calibrations,
    /// How many players there are, and the controllers' players by key.
    players: Players,
    /// The keys of the controllers the state was told are connected, so that it is told they are gone
    /// when they go with a run.
    told: Vec<String>,
    /// How often controllers are looked for besides when /dev/input changes: when it isn't watched.
    look_every: Option<Duration>,
}

/// The virtual devices: the mouse, and a pad for each player.
struct Devices {
    mouse: VirtualMouse,
    pads: Vec<VirtualPad>,
}

impl Devices {
    /// Every pad at rest and every key of the mouse let go.
    fn rest(&mut self) -> io::Result<()> {
        for pad in &mut self.pads {
            pad.rest()?;
        }
        self.mouse.rest()
    }
}

/// A run: the controllers taken, and each player's controller and router.
struct Pads {
    controllers: Vec<Controller>,
    merge: Merge,
    merged: Merged,
    /// A router per player, player 1's first.
    players: Vec<PlayerRoute>,
    /// The gyro, and the slot of the controller it is next to, whose player it is.
    motion: Option<(Box<dyn MotionSource>, u32)>,
    /// The slot the next controller gets.
    next_slot: u32,
    /// The controller whose input came last, whose sticks a calibration is for.
    last_used: Option<u32>,
    raws: Vec<Raw>,
    samples: Vec<Sample>,
    /// When controllers are next looked for, when /dev/input isn't watched.
    next_look: Option<Instant>,
    /// Controllers are to be looked for: /dev/input changed, one failed, or it is time to. However many
    /// reasons gather, they are looked for once (see [`PadThread::route_events`]).
    look_due: bool,
    /// The system sleeps: the gyro stays closed until it wakes.
    asleep: bool,
    /// The extras' last search that left one missing: the controller they were for and the input devices
    /// there were. They aren't looked for again until either changes (see [`Extras`]).
    extras_missing: Option<Extras>,
}

/// What the gyro and the AYN button were looked for with: the slot of the controller they go with, and
/// the input devices (`/dev/input/event*`) there were. Looking opens every input device, which holds up
/// the controllers' reads (see [`PadThread::route_events`]), so a search that found nothing is made again
/// only for another controller or another set of devices.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Extras {
    slot: u32,
    devices: Vec<std::ffi::OsString>,
}

impl Extras {
    fn now(slot: u32) -> Extras {
        let mut devices: Vec<std::ffi::OsString> = std::fs::read_dir("/dev/input")
            .map(|d| d.flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().starts_with("event")).collect())
            .unwrap_or_default();
        devices.sort();
        Extras { slot, devices }
    }
}

/// A player's router, what it makes of the player's controller, and its keymap's buzz playing.
struct PlayerRoute {
    router: PadRouter,
    out: Out,
    buzz: Option<(Buzz, Instant)>,
}

/// A controller taken over.
struct Controller {
    slot: u32,
    key: String,
    name: String,
    phys: String,
    /// Its unique id, such as a Bluetooth controller's address, or empty.
    uniq: String,
    path: PathBuf,
    grabbed: Grabbed,
    normalise: Normalise,
    /// Its motors, if it has force feedback.
    rumble: Option<Rumble>,
    /// Whether its X and Y codes are by letter, as with xpad and rsinput, rather than by place. If so
    /// they are swapped as they come in.
    letters: bool,
    /// Whether it is the device's built-in controller, which the gyro and the AYN button go with.
    builtin: bool,
    /// The AYN button's device, with the built-in controller on the Thor.
    ayn: Option<AuxiliaryKey>,
    /// Its sticks: their calibration, and any calibration in progress.
    sticks: calibrate::StickState,
}

/// The gyro of the controller `controller`: its own motion sensor, or else the sensor hub, if it may have
/// it.
fn find_motion(controller: motion::Ident, sensor_hub: Option<clamshell_device::gyro::SensorHub>) -> Option<Box<dyn MotionSource>> {
    EvdevMotion::find(controller)
        .map(|m| Box::new(m) as Box<dyn MotionSource>)
        .or_else(|| sensor_hub::SensorHub::find(sensor_hub).map(|m| Box::new(m) as Box<dyn MotionSource>))
}

/// A router for each player of `count`, player 1's with the route and the others' always in the game,
/// each with its keymap.
fn routers(count: Player, keymaps: &[Keymap; Player::MOST], route: PadRoute, watch_activity: bool) -> Vec<PlayerRoute> {
    count.up_to().map(|p| player_route(p, keymaps, route, watch_activity)).collect()
}

fn player_route(p: Player, keymaps: &[Keymap; Player::MOST], route: PadRoute, watch_activity: bool) -> PlayerRoute {
    let mut out = Out::default();
    let keymap = keymaps.get(p.index()).cloned().unwrap_or_default();
    let snapshot = PadSnapshot::at_rest();
    let mut router = if p == Player::ONE {
        PadRouter::new(merge::RANGES, snapshot, route, keymap, &mut out)
    } else {
        PadRouter::other_player(merge::RANGES, snapshot, keymap, &mut out)
    };
    if p == Player::ONE {
        // The state's watch for game input stands across reconnections, as the route and keymap do.
        router.watch_activity(watch_activity);
    }
    PlayerRoute { router, out, buzz: None }
}

impl<S: Sink> PadThread<S> {
    /// Commands that need no controller.
    fn apply(&mut self, command: Command, pads: Option<&mut Pads>) {
        match command {
            Command::Keymap(player, keymap) => {
                log::info!("pad: player {}'s keymap {} ({})", player.number(), keymap.id, if keymap.id == 0 { "Default" } else { keymap.name.as_str() });
                if let Some(p) = pads.and_then(|pads| pads.players.get_mut(player.index())) {
                    p.router.set_keymap((*keymap).clone(), &mut p.out);
                }
                if let Some(slot) = self.keymaps.get_mut(player.index()) {
                    *slot = *keymap;
                }
            }
            Command::Route(route) => {
                self.route = route;
                if let Some(p) = pads.and_then(|pads| pads.players.first_mut()) {
                    p.router.set_route(route, &mut p.out);
                }
            }
            Command::WatchActivity(watch) => {
                self.watch_activity = watch;
                if let Some(p) = pads.and_then(|pads| pads.players.first_mut()) {
                    p.router.watch_activity(watch);
                }
            }
            Command::Rumble(setting) => self.rumble_setting = setting,
            Command::CalibrateGyro => match pads.and_then(|pads| {
                let player = pads.gyro_player()?;
                pads.players.get_mut(player.index())
            }) {
                Some(p) => p.router.calibrate(Instant::now()),
                // Answered, so the state doesn't wait for it.
                None => {
                    log::info!("pad: no gyro to calibrate");
                    if self.sink.send(Event::GyroCalibrated(false)).is_err() {
                        log::debug!("pad: the UI is gone");
                    }
                }
            },
            // Looking is the run's, and the supervisor tries again after any command while down.
            Command::Look => {}
            // Without a run there is no gyro to close or find; with one, these are handled in take_commands.
            Command::Sleep(Ack(done)) => {
                if done.send(()).is_err() {
                    log::debug!("pad: sleep no longer waits for the pad");
                }
            }
            Command::Woke => {}
            // Without a run there is nothing to calibrate.
            Command::CalibrateSticks => self.no_sticks_to_calibrate(),
            Command::Sticks(all) => self.sticks = all,
            Command::Players(players) => self.players = players,
        }
    }

    fn no_sticks_to_calibrate(&mut self) {
        log::info!("pad: no controller to calibrate the sticks of");
        if self.sink.send(Event::Sticks(SticksEvent::Done(String::new(), None))).is_err() {
            log::debug!("pad: the UI is gone");
        }
    }

    /// Sends what the routers made of the controllers' events to the virtual devices and the state. A
    /// player whose pad couldn't be made has its pad's events dropped.
    fn emit(&mut self, pads: &mut Pads) -> Result<(), Ended<io::Error>> {
        let Some(devices) = self.devices.as_mut() else {
            return Err(Ended::Lost(io::Error::other("no virtual devices")));
        };
        for (i, p) in pads.players.iter_mut().enumerate() {
            for event in p.out.events.drain(..) {
                self.sink.send(event)?;
            }
            if !p.out.pad.is_empty() {
                if let Some(pad) = devices.pads.get_mut(i) {
                    // Games read it as an Xbox 360 pad, with X and Y by letter.
                    device::swap_xy(&mut p.out.pad);
                    pad.emit(&p.out.pad)?;
                }
                p.out.pad.clear();
            }
            if !p.out.mouse.is_empty() {
                devices.mouse.emit(&p.out.mouse)?;
                p.out.mouse.clear();
            }
        }
        Ok(())
    }

    /// The player of the controller with `key`, within the count.
    fn player_of(&self, key: &str) -> Player {
        self.players.of(key)
    }

    /// The virtual devices for `count` players: made if there are none, a pad added for each player
    /// added and dropped for each player gone. A pad that can't be made is tried again at the next look
    /// (see [`grow`]); the pads there are stay, so a game keeps its pad.
    fn make_devices(&mut self) -> io::Result<()> {
        let count = usize::from(self.players.count.number());
        let devices = match self.devices.as_mut() {
            Some(d) => d,
            None => self.devices.insert(Devices { mouse: VirtualMouse::new()?, pads: Vec::new() }),
        };
        grow(&mut devices.pads, count, VirtualPad::new)?;
        if devices.pads.len() > count {
            devices.pads.truncate(count);
            log::info!("pad: virtual pads for {count} players");
        }
        Ok(())
    }

    /// Looks for controllers not yet taken, and takes them. With CLAMSHELL_GAMEPAD set, only that one.
    fn look(&mut self, pads: &mut Pads) {
        let spec_only = self.spec != DeviceSpec::Auto;
        if spec_only && !pads.controllers.is_empty() {
            return;
        }
        for (path, real) in device::candidates(&self.spec) {
            if pads.controllers.iter().any(|c| c.path == path) {
                continue;
            }
            if pads.controllers.len() >= MOST_CONTROLLERS {
                log::warn!("pad: {} left alone: {MOST_CONTROLLERS} controllers are taken already", real.name().unwrap_or("a controller"));
                continue;
            }
            if let Err(e) = self.take(pads, path.clone(), real) {
                log::warn!("pad: {}: not taken: {e}", path.display());
            }
        }
        if spec_only && pads.controllers.is_empty() {
            log::debug!("pad: no controller ({})", self.spec);
        }
        self.place_extras(pads);
        // A player's pad that couldn't be made is tried again.
        if let Err(e) = self.make_devices() {
            log::warn!("pad: the virtual devices: {e}");
        }
    }

    /// Takes a controller: grabs it and puts it in its player's merge.
    fn take(&mut self, pads: &mut Pads, path: PathBuf, mut real: evdev::raw_stream::RawDevice) -> io::Result<()> {
        let id = real.input_id();
        let (name, phys) = (real.name().unwrap_or("").to_owned(), real.physical_path().unwrap_or("").to_owned());
        let key = device::key(id.vendor(), id.product(), real.unique_name(), &name);
        let uniq = real.unique_name().unwrap_or("").to_owned();
        let caps = Capabilities::of(&real)?;
        let rumble = Rumble::of(&mut real).unwrap_or_else(|e| {
            log::warn!("pad: {name}: rumble: {e}");
            None
        });
        let grabbed = Grabbed::new(real)?;
        self.taken.add((id.vendor(), id.product()));
        let driver = device::driver(&name, &phys);
        let letters = device::letter_codes(self.controls, driver.as_deref());
        let builtin = driver.is_some() && driver.as_deref() == self.controls.builtin_driver;
        let mut snapshot = caps.snapshot();
        let sticks = calibrate::StickState::new(caps.stick_range(), self.sticks.get(&name).copied(), &mut snapshot);
        let normalise = caps.normalise();
        let axes: Vec<_> = merge::PAD_AXES.iter().filter_map(|&code| snapshot.value(code).map(|v| (code, v))).collect();
        let inputs = Inputs::of(&normalise, &[], &axes);
        let player = self.player_of(&key);
        let slot = pads.next_slot;
        pads.next_slot += 1;
        log::info!(
            "pad: {name} grabbed as player {}{}{}{}{}",
            player.number(),
            if builtin { ", the built-in controller" } else { "" },
            if letters { "; X and Y by letter: read by place" } else { "" },
            if rumble.is_none() { "; no rumble" } else { "" },
            if sticks.sticks.is_some() { "; its sticks calibrated" } else { "" },
        );
        pads.merge.add(slot, player, inputs, &mut pads.merged);
        pads.controllers.push(Controller {
            slot,
            key: key.clone(),
            name: name.clone(),
            phys,
            uniq,
            path,
            grabbed,
            normalise,
            rumble,
            letters,
            builtin,
            ayn: None,
            sticks,
        });
        if self.sink.send(Event::Controller(ControllerEvent::Connected { key: key.clone(), name })).is_err() {
            log::debug!("pad: the UI is gone");
        }
        self.told.push(key);
        Ok(())
    }

    /// Lets go of controller `i`, which failed or is gone: what only it held on its player's controller is
    /// let go, and the state is told.
    fn drop_controller(&mut self, pads: &mut Pads, i: usize, why: &io::Error) {
        if i >= pads.controllers.len() {
            return;
        }
        let mut c = pads.controllers.remove(i);
        log::info!("pad: {} let go: {why}", c.name);
        if let Some(r) = c.rumble.as_mut()
            && let Err(e) = r.set(0, 0, Instant::now())
        {
            log::debug!("pad: stopping the rumble of {}: {e}", c.name);
        }
        if let Some(player) = pads.merge.remove(c.slot, &mut pads.merged) {
            if let Some(p) = pads.players.get_mut(player.index()) {
                p.router.controller_gone();
            }
            if player == Player::ONE && !pads.controllers.iter().any(|o| pads.merge.player(o.slot) == Some(Player::ONE)) {
                log::info!("pad: player 1 has no controller: ClamShell's screens by touch");
            }
        }
        // A calibration in progress is given up, and the state is told so it doesn't wait for it.
        if c.sticks.calibrating() && self.sink.send(Event::Sticks(SticksEvent::Done(c.name.clone(), None))).is_err() {
            log::debug!("pad: the UI is gone");
        }
        if pads.motion.as_ref().is_some_and(|(_, slot)| *slot == c.slot) {
            pads.motion = None;
            log::info!("pad: the gyro went with {}", c.name);
        }
        if let Some(k) = self.told.iter().position(|k| *k == c.key) {
            self.told.remove(k);
            // Another controller with the same key may still be there.
            if !pads.controllers.iter().any(|o| o.key == c.key) && self.sink.send(Event::Controller(ControllerEvent::Disconnected(c.key))).is_err() {
                log::debug!("pad: the UI is gone");
            }
        }
        if pads.last_used == Some(c.slot) {
            pads.last_used = None;
        }
        self.place_extras(pads);
    }

    /// Puts the gyro and the AYN button with the built-in controller, or else with the first controller:
    /// found if they aren't there, and moved if they are with another. A gyro that failed is looked for
    /// again only here, as controllers come and go and after waking.
    fn place_extras(&mut self, pads: &mut Pads) {
        let Some(host) = pads.controllers.iter().position(|c| c.builtin).or(if pads.controllers.is_empty() { None } else { Some(0) }) else {
            pads.motion = None;
            return;
        };
        let Some(slot) = pads.controllers.get(host).map(|c| c.slot) else { return };
        let looked = Extras::now(slot);
        let search = pads.extras_missing.as_ref() != Some(&looked);
        // The AYN button, from whichever controller had it.
        if self.controls.auxiliary.is_some() && pads.controllers.get(host).is_some_and(|c| c.ayn.is_none()) {
            let had = pads.controllers.iter_mut().find_map(|c| c.ayn.take());
            let ayn = had.or_else(|| search.then(|| AuxiliaryKey::find(self.controls.auxiliary)).flatten());
            if let Some(c) = pads.controllers.get_mut(host) {
                c.ayn = ayn;
            }
        }
        if pads.asleep || pads.motion.as_ref().is_some_and(|(_, s)| *s == slot) {
            pads.extras_missing = self.ayn_missing(pads, host).then_some(looked);
            return;
        }
        if !search {
            return;
        }
        // Closed before it is found again, for the sensor hub's stream.
        pads.motion = None;
        let Some(c) = pads.controllers.get(host) else { return };
        // The sensor hub is the built-in controller's, or any controller's where the device names none.
        let hub = if c.builtin || self.controls.builtin_driver.is_none() { self.sensor_hub } else { None };
        pads.motion = find_motion(motion::Ident { name: &c.name, phys: &c.phys, uniq: &c.uniq }, hub).map(|m| (m, c.slot));
        pads.extras_missing = (pads.motion.is_none() || self.ayn_missing(pads, host)).then_some(looked);
    }

    /// Whether the device has an AYN button the controller at `host` hasn't got.
    fn ayn_missing(&self, pads: &Pads, host: usize) -> bool {
        self.controls.auxiliary.is_some() && pads.controllers.get(host).is_some_and(|c| c.ayn.is_none())
    }

    /// The state's players changed: each controller goes to its player now, the count's pads and routers
    /// are made or dropped.
    fn players_changed(&mut self, pads: &mut Pads) -> Result<(), Ended<io::Error>> {
        let want = usize::from(self.players.count.number());
        for c in &pads.controllers {
            let to = self.players.of(&c.key);
            if pads.merge.player(c.slot) != Some(to) {
                log::info!("pad: {} is player {}", c.name, to.number());
            }
        }
        let moves = pads.controllers.iter().map(|c| (c.slot, self.players.of(&c.key)));
        let (keymaps, route, watch) = (&self.keymaps, self.route, self.watch_activity);
        regroup(&mut pads.merge, &mut pads.merged, &mut pads.players, want, moves, |p| player_route(p, keymaps, route, watch), Instant::now());
        // The controllers of players gone are player 1's now: those players' routers go.
        if pads.players.len() > want {
            for p in pads.players.iter_mut().skip(want) {
                p.router.lose(&mut p.out);
            }
            self.emit(pads)?;
            pads.players.truncate(want);
        }
        self.make_devices()?;
        Ok(())
    }

    /// Hands each player's merged events to its router.
    fn route_merged(&mut self, pads: &mut Pads, now: Instant) {
        route_merged(&mut pads.merged, &mut pads.players, now);
    }
}

/// Adds pads made by `make` until there are `count`. One that can't be made stops it there, logged, and
/// the pads there are stay: that is an error only when there is no pad at all.
fn grow<T>(pads: &mut Vec<T>, count: usize, mut make: impl FnMut() -> io::Result<T>) -> io::Result<()> {
    while pads.len() < count {
        match make() {
            Ok(pad) => {
                pads.push(pad);
                log::info!("pad: player {}'s virtual pad made", pads.len());
            }
            Err(e) if pads.is_empty() => return Err(e),
            Err(e) => {
                log::warn!("pad: player {}'s virtual pad: {e}: tried again at the next look", pads.len() + 1);
                break;
            }
        }
    }
    Ok(())
}

/// Each controller to its player (`moves`, its slot and player), with a router for each of `want` players
/// at least, made by `router`, and the controllers' events routed. Order: the routers of players added
/// are made before the controllers move, so what a moved controller holds reaches its new player's router
/// as it moves; made after, at rest, they missed it until it changed. Those of players gone go after this
/// (see [`PadThread::players_changed`]), once their controllers have moved to player 1.
fn regroup(
    merge: &mut Merge,
    merged: &mut Merged,
    players: &mut Vec<PlayerRoute>,
    want: usize,
    moves: impl Iterator<Item = (u32, Player)>,
    mut router: impl FnMut(Player) -> PlayerRoute,
    now: Instant,
) {
    while players.len() < want {
        let p = Player::ALL.get(players.len()).copied().unwrap_or(Player::ONE);
        players.push(router(p));
    }
    for (slot, to) in moves {
        if merge.player(slot) != Some(to) {
            merge.set_player(slot, to, merged);
        }
    }
    route_merged(merged, players, now);
}

/// Hands each player's merged events to its router, `players` being player 1's first.
fn route_merged(merged: &mut Merged, players: &mut [PlayerRoute], now: Instant) {
    for (events, p) in merged.events.iter_mut().zip(players.iter_mut()) {
        for raw in events.drain(..) {
            p.router.event(raw, now, &mut p.out);
        }
    }
    // A player past the count has no router: its events are dropped.
    for events in merged.events.iter_mut().skip(players.len()) {
        events.clear();
    }
}

/// Each player's motors' strengths, from each player's games' effects and keymap's buzz, player 1's
/// first: player 1's games' rumble only while the game has the pad (`route`), the other players' always,
/// each at the Rumble setting's strength.
fn levels(players: impl Iterator<Item = ((u16, u16), Option<(u16, u16)>)>, route: PadRoute, setting: RumbleStrength) -> [(u16, u16); Player::MOST] {
    let mut levels = [(0, 0); Player::MOST];
    for ((i, (games, buzz)), level) in players.enumerate().zip(levels.iter_mut()) {
        *level = rumble::mix(games, i > 0 || route == PadRoute::Game, buzz, setting);
    }
    levels
}

impl Pads {
    fn new(count: Player, keymaps: &[Keymap; Player::MOST], route: PadRoute, watch_activity: bool) -> Pads {
        Pads {
            controllers: Vec::new(),
            merge: Merge::default(),
            merged: Merged::default(),
            players: routers(count, keymaps, route, watch_activity),
            motion: None,
            next_slot: 0,
            last_used: None,
            raws: Vec::new(),
            samples: Vec::new(),
            next_look: None,
            look_due: false,
            asleep: false,
            extras_missing: None,
        }
    }

    /// The player the gyro is, if there is one: its controller's.
    fn gyro_player(&self) -> Option<Player> {
        self.motion.as_ref().and_then(|(_, slot)| self.merge.player(*slot))
    }
}

impl<S: Sink> Service for PadThread<S> {
    type Command = Command;
    type Session = Pads;
    type Error = io::Error;
    const NAME: &'static str = "pad";

    /// Makes the virtual devices if they aren't there, and takes the controllers there are.
    fn connect(&mut self) -> io::Result<Pads> {
        // Games wait for their rumble uploads to be answered, whether there is a controller or not.
        self.serve_effects(Instant::now());
        self.make_devices()?;
        let mut pads = Pads::new(self.players.count, &self.keymaps, self.route, self.watch_activity);
        self.look(&mut pads);
        if pads.controllers.is_empty() {
            log::info!("pad: no controller yet: ClamShell's screens by touch");
        }
        Ok(pads)
    }

    /// Routes the controllers' events until the virtual devices fail or the program is gone.
    fn run_while_up(&mut self, pads: Pads, commands: &Receiver<Command>) -> Result<(), Ended<io::Error>> {
        let mut pads = pads;
        let result = self.route_events(&mut pads, commands);
        for c in &mut pads.controllers {
            if let Some(r) = c.rumble.as_mut()
                && let Err(e) = r.set(0, 0, Instant::now())
            {
                log::debug!("pad: stopping the rumble of {}: {e}", c.name);
            }
        }
        // The run is over, so whatever the routes held on the keyboard and mouse is let go.
        for p in &mut pads.players {
            p.router.lose(&mut p.out);
        }
        if let Err(e) = self.emit(&mut pads) {
            log::warn!("pad: letting go: {e:?}");
        }
        result
    }

    fn command_while_down(&mut self, command: Command) -> Result<(), Closed> {
        self.apply(command, None);
        Ok(())
    }

    /// The virtual devices failed. They are made again on the next try, and the controllers, which went
    /// with the run, are said to be gone.
    fn lost(&mut self, e: &io::Error) -> Result<(), Closed> {
        log::warn!("pad: the virtual devices failed: {e}; made again");
        self.devices = None;
        self.forget_controllers();
        Ok(())
    }

    /// A panic ended the run: what its routes held is let go (the routers went with the unwinding), Home
    /// counts as up, and the controllers, which went with it, are said to be gone. They are found again.
    fn panicked(&mut self) {
        if let Some(devices) = self.devices.as_mut()
            && let Err(e) = devices.rest()
        {
            log::warn!("pad: letting go after a panic: {e}");
        }
        if self.sink.send(Event::HomeHeld(false)).is_err() {
            log::debug!("pad: the UI is gone");
        }
        self.forget_controllers();
    }

    /// Waits for a command, answering games' rumble uploads to the virtual pads in the meantime, since a
    /// game waits for each.
    fn wait_while_down(&mut self, commands: &Receiver<Command>, retry: Duration) -> Next<Command> {
        let until = Instant::now() + retry;
        loop {
            match commands.try_recv() {
                Ok(command) => return Next::Command(command),
                Err(TryRecvError::Disconnected) => return Next::Disconnected,
                Err(TryRecvError::Empty) => {}
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Next::Elapsed;
            }
            // Every player's pad, as while up: a game of any player waits for its uploads.
            let virtual_pads = self.devices.as_ref().map_or(&[][..], |d| d.pads.as_slice());
            if virtual_pads.is_empty() {
                return worker::next(commands, Some(left));
            }
            if let Err(e) = wait(None, virtual_pads, &self.bell, Some(left)) {
                log::warn!("pad: {e}");
                return worker::next(commands, Some(left));
            }
            self.serve_effects(Instant::now());
        }
    }
}

impl<S> PadThread<S>
where
    S: Sink,
{
    /// The controllers went with a run: the state is told they are gone.
    fn forget_controllers(&mut self) {
        for key in self.told.drain(..) {
            if self.sink.send(Event::Controller(ControllerEvent::Disconnected(key))).is_err() {
                log::debug!("pad: the UI is gone");
            }
        }
    }
}

/// Reads a controller's events since the last read into `raws`, in the virtual pad's terms. If the
/// kernel dropped events, returns the controller as it is instead. The letters are swapped where the
/// controller has them the other way, the sticks go through their calibration, and the AYN button's
/// events are added. An AYN button that fails is left out, and the controller goes on.
fn read_controller(c: &mut Controller, controls: clamshell_device::input::Controls, raws: &mut Vec<Raw>) -> io::Result<Option<Inputs>> {
    raws.clear();
    if c.grabbed.fetch(raws)? {
        raws.clear();
        let (mut down, axes) = c.grabbed.state()?;
        if c.letters {
            down.iter_mut().for_each(|k| *k = device::swap_key(*k));
        }
        down.iter_mut().for_each(|key| key.0 = controls.normalise_button(key.0));
        // The axes go through the sticks' calibration as any event does.
        raws.extend(axes.iter().map(|&(code, value)| Raw::abs(code, value)));
        c.sticks.take(raws);
        let axes: Vec<_> = raws.drain(..).filter_map(Raw::as_abs).collect();
        log::info!("pad: {}'s events overran; caught up with it as it is", c.name);
        return Ok(Some(Inputs::of(&c.normalise, &down, &axes)));
    }
    if c.letters {
        device::swap_xy(raws);
    }
    device::normalise_controls(controls, raws);
    // The sticks go through their calibration. While a calibration is in progress, they go only to it.
    c.sticks.take(raws);
    if let Some(a) = c.ayn.as_mut()
        && let Err(e) = a.fetch(raws)
    {
        log::warn!("pad: AYN button: {e}");
        c.ayn = None;
    }
    let normalise = c.normalise;
    raws.retain_mut(|r| match normalise.raw(*r) {
        Some(n) => {
            *r = n;
            true
        }
        None => false,
    });
    Ok(None)
}

/// The most file descriptors waited on: the bell, the controllers, the gyro, the AYN button and the
/// players' virtual pads.
const MOST_FDS: usize = 1 + MOST_CONTROLLERS + 2 + Player::MOST;

/// Waits, up to `timeout` if given, for events from the controllers, the gyro and the AYN button of a
/// run, if there is one, or the virtual pads' rumble, or for a command through the bell. Nothing is
/// allocated.
fn wait(pads: Option<&Pads>, virtual_pads: &[VirtualPad], bell: &Bell, timeout: Option<Duration>) -> io::Result<()> {
    let timeout = timeout.map(crate::os::sys::timespec);
    let mut fds: [PollFd; MOST_FDS] = std::array::from_fn(|_| PollFd::from_borrowed_fd(bell.fd(), PollFlags::IN));
    let controllers = pads.iter().flat_map(|p| &p.controllers);
    let ayn = controllers.clone().filter_map(|c| c.ayn.as_ref()).map(|a| a.device().as_fd());
    let motion = pads.and_then(|p| p.motion.as_ref()).map(|(m, _)| m.fd());
    let controllers = controllers.map(|c| c.grabbed.device().as_fd());
    let virtual_pads = virtual_pads.iter().map(|p| p.device.as_fd());
    let mut n = 1;
    let all = controllers.take(MOST_CONTROLLERS).chain(ayn.take(1)).chain(motion).chain(virtual_pads);
    for (slot, fd) in fds.iter_mut().skip(1).zip(all) {
        *slot = PollFd::from_borrowed_fd(fd, PollFlags::IN);
        n += 1;
    }
    match rustix::event::poll(fds.get_mut(..n).unwrap_or_default(), timeout.as_ref()) {
        Ok(_) | Err(rustix::io::Errno::INTR) => {
            bell.clear();
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

impl<S: Sink> PadThread<S> {
    /// Routes the controllers' and the gyro's events until the virtual devices fail, or the program is
    /// gone.
    fn route_events(&mut self, pads: &mut Pads, commands: &Receiver<Command>) -> Result<(), Ended<io::Error>> {
        pads.next_look = self.look_every.map(|every| Instant::now() + every);
        loop {
            if !self.take_commands(pads, commands)? {
                return Ok(());
            }
            // Once for every reason gathered since the last look. A device plugged in or out brings a run
            // of changes to /dev/input (each node made, then given its owner, mode and ACL), each a Look;
            // a look opens every input device, and closing one waits for the kernel (an RCU grace period),
            // so a look per Look kept the controllers unread for seconds, a controller gone among them.
            if std::mem::take(&mut pads.look_due) {
                self.look(pads);
            }
            // Order: the turn comes after the commands as well as after input, so what a command changes
            // (the route, the Rumble setting, a keymap, a calibration asked for) moves the motors and the
            // wake time at once rather than at the next input.
            let wake = self.turn(pads, Instant::now())?;
            self.emit(pads)?;
            // Wake for events, commands, when a mapping, a calibration or a rumble wants it, and to look
            // for controllers.
            let wait_for = [wake, pads.next_look].into_iter().flatten().min().map(|at: Instant| at.saturating_duration_since(Instant::now()));
            let virtual_pads = self.devices.as_ref().map_or(&[][..], |d| d.pads.as_slice());
            wait(Some(pads), virtual_pads, &self.bell, wait_for)?;
            let now = Instant::now();
            self.read(pads, now);
            if pads.next_look.is_some_and(|at| now >= at) {
                pads.look_due = true;
                pads.next_look = self.look_every.map(|every| now + every);
            }
        }
    }

    /// One turn of the run, after the commands and the input read since the last: the controllers' events
    /// and the gyro's samples go to the routers, the routers are ticked, the stick calibrations fed and the
    /// motors set. Returns when the next turn is due without input, if one is.
    fn turn(&mut self, pads: &mut Pads, now: Instant) -> Result<Option<Instant>, Closed> {
        self.route_merged(pads, now);
        let gyro = pads.gyro_player();
        match gyro.and_then(|p| pads.players.get_mut(p.index())) {
            Some(p) => {
                for sample in pads.samples.drain(..) {
                    p.router.motion(sample, &mut p.out);
                }
            }
            None => pads.samples.clear(),
        }
        let mut next = None;
        for p in &mut pads.players {
            next = [next, p.router.tick(now, &mut p.out)].into_iter().flatten().min();
        }
        // A calibration in progress is fed, its progress and end are sent, and it is looked at again
        // soon.
        for i in 0..pads.controllers.len() {
            let Some(c) = pads.controllers.get_mut(i) else { continue };
            let (said, again) = c.sticks.step(now);
            if said.is_some() {
                let name = c.name.clone();
                self.calibration_said(said, &name)?;
            }
            next = [next, again].into_iter().flatten().min();
        }
        // Games' rumble goes to each player's controllers, along with the keymaps' buzzes.
        let ends = self.rumble(pads, now);
        Ok([next, ends].into_iter().flatten().min())
    }

    /// Reads every controller and the gyro. A controller that fails is let go, and controllers are to be
    /// looked for again, since one gone may be back at once, even on the same node.
    fn read(&mut self, pads: &mut Pads, _now: Instant) {
        let mut i = 0;
        while i < pads.controllers.len() {
            let Some(c) = pads.controllers.get_mut(i) else { break };
            match read_controller(c, self.controls, &mut pads.raws) {
                Ok(Some(mut inputs)) => {
                    // The AYN button isn't the controller's own: what it holds stands.
                    let ayn = codes::AYN_KEY.0;
                    if c.ayn.is_some() && pads.merge.held(c.slot, ayn) {
                        inputs.press(ayn);
                    }
                    pads.merge.resync(c.slot, inputs, &mut pads.merged);
                    pads.last_used = Some(c.slot);
                }
                Ok(None) => {
                    if !pads.raws.is_empty() {
                        pads.last_used = Some(c.slot);
                    }
                    for raw in pads.raws.drain(..) {
                        pads.merge.event(c.slot, raw, &mut pads.merged);
                    }
                }
                Err(e) => {
                    self.drop_controller(pads, i, &e);
                    pads.look_due = true;
                    continue;
                }
            }
            i += 1;
        }
        if let Some((m, _)) = pads.motion.as_mut()
            && let Err(e) = m.fetch(&mut pads.samples)
        {
            log::warn!("pad: gyro: {e}");
            pads.motion = None;
        }
    }

    /// Carries out the waiting commands. Those about the run are handled here and the rest by
    /// [`PadThread::apply`]; a Look is noted, for the run to look once however many came. Returns false
    /// once the commands' sender is gone.
    fn take_commands(&mut self, pads: &mut Pads, commands: &Receiver<Command>) -> Result<bool, Ended<io::Error>> {
        loop {
            match commands.try_recv() {
                Ok(Command::Look) => pads.look_due = true,
                // The gyro, and its stream, is closed before sleep and found again after.
                Ok(Command::Sleep(Ack(done))) => {
                    pads.asleep = true;
                    if pads.motion.take().is_some() {
                        log::info!("pad: the gyro closed for sleep");
                    }
                    if done.send(()).is_err() {
                        log::debug!("pad: sleep no longer waits for the pad");
                    }
                }
                Ok(Command::Woke) => {
                    pads.asleep = false;
                    // The gyro was closed for sleep, not missing: it is looked for again.
                    pads.extras_missing = None;
                    self.place_extras(pads);
                }
                Ok(Command::CalibrateSticks) => {
                    let slot = pads.last_used.or_else(|| pads.controllers.first().map(|c| c.slot));
                    match pads.controllers.iter_mut().find(|c| Some(c.slot) == slot) {
                        Some(c) => {
                            log::info!("pad: calibrating the sticks of {}", c.name);
                            c.sticks.calibrate(Instant::now());
                        }
                        None => self.no_sticks_to_calibrate(),
                    }
                }
                Ok(Command::Sticks(all)) => {
                    for c in &mut pads.controllers {
                        c.sticks.sticks = all.get(c.name.as_str()).copied();
                    }
                    self.sticks = all;
                }
                Ok(Command::Players(players)) => {
                    self.players = players;
                    self.players_changed(pads)?;
                }
                Ok(command) => self.apply(command, Some(pads)),
                Err(TryRecvError::Empty) => return Ok(true),
                Err(TryRecvError::Disconnected) => return Ok(false),
            }
        }
    }

    /// Sends on what a stick calibration said. A finished calibration is kept for the controller.
    fn calibration_said(&mut self, said: Option<calibrate::Said>, name: &str) -> Result<(), Closed> {
        match said {
            Some(calibrate::Said::Progress(p)) => self.sink.send(Event::Sticks(SticksEvent::Progress(p))),
            Some(calibrate::Said::Done(result)) => {
                log::info!("pad: the sticks of {name} {}", if result.is_some() { "calibrated" } else { "not calibrated (given up)" });
                if let Some(s) = result {
                    self.sticks.insert(name.to_owned(), s);
                }
                self.sink.send(Event::Sticks(SticksEvent::Done(name.to_owned(), result)))
            }
            None => Ok(()),
        }
    }

    /// Drives each player's controllers' motors with games' rumble on its virtual pad (player 1's while
    /// the game has the pad) and with its keymap's buzzes, at the Rumble setting's strength. A controller
    /// whose rumble fails is tried again, and left without after several failures in a row. Returns when
    /// the next effect or buzz ends, or a failed rumble is tried again.
    fn rumble(&mut self, pads: &mut Pads, now: Instant) -> Option<Instant> {
        self.serve_effects(now);
        let devices = self.devices.as_mut()?;
        let mut next = None;
        for (p, pad) in pads.players.iter_mut().zip(devices.pads.iter()) {
            if let Some(buzz) = p.out.buzz.take() {
                p.buzz = Some((buzz, now + Buzz::LENGTH));
            }
            p.buzz = p.buzz.filter(|(_, until)| now < *until);
            next = [next, pad.effects.next_end(), p.buzz.map(|(_, until)| until)].into_iter().flatten().min();
        }
        // Effects that ended are let go whatever the route, so the next end is never in the past.
        let each = pads.players.iter().zip(devices.pads.iter_mut()).map(|(p, pad)| (pad.effects.level(now), p.buzz.map(|(b, _)| b.level())));
        let levels = levels(each, self.route, self.rumble_setting);
        for c in &mut pads.controllers {
            let level = pads.merge.player(c.slot).and_then(|p| levels.get(p.index()).copied()).unwrap_or((0, 0));
            if let Some(r) = c.rumble.as_mut()
                && let Err(e) = r.set(level.0, level.1, now)
            {
                if r.given_up() {
                    log::warn!("pad: {}: rumble: {e}: given up until it is taken again", c.name);
                    c.rumble = None;
                } else {
                    log::warn!("pad: {}: rumble: {e}: tried again", c.name);
                }
            }
            next = [next, c.rumble.as_ref().and_then(Rumble::retry_at)].into_iter().flatten().min();
        }
        next
    }

    /// Answers the virtual pads' rumble uploads and follows their play events.
    fn serve_effects(&mut self, now: Instant) {
        for pad in self.devices.iter_mut().flat_map(|d| d.pads.iter_mut()) {
            if let Err(e) = pad.effects.serve(&mut pad.device, now) {
                log::warn!("pad: rumble requests: {e}");
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A search for the gyro and the AYN button that found nothing is made again only for another
    /// controller or another set of input devices: a look with the same ones skips it.
    #[test]
    fn a_search_for_the_extras_is_made_again_only_when_something_changed() {
        let missing = Extras { slot: 1, devices: vec!["event0".into(), "event3".into()] };
        let pads = Pads { extras_missing: Some(missing.clone()), ..Pads::new(Player::ONE, &std::array::from_fn(|_| Keymap::default()), PadRoute::Ui, false) };
        let search = |now: &Extras| pads.extras_missing.as_ref() != Some(now);
        assert!(!search(&missing), "the same controller and devices");
        assert!(search(&Extras { devices: vec!["event0".into(), "event3".into(), "event4".into()], ..missing.clone() }), "a device added");
        assert!(search(&Extras { slot: 2, ..missing }), "another controller");
    }
    use crate::os::evdev::Press;
    use clamshell_state::keymap::{Binding, Button, Input, Output};
    use evdev::KeyCode;

    fn p(n: u8) -> Player {
        Player::new(n).unwrap()
    }

    fn key(code: KeyCode, down: bool) -> Raw {
        Raw::key(code, if down { Press::Down } else { Press::Up })
    }

    /// The keys down on a virtual pad after these events.
    fn held(events: &[Raw]) -> Vec<KeyCode> {
        let mut down = Vec::new();
        for (k, press) in events.iter().filter_map(|e| e.as_key()) {
            down.retain(|&d| d != k);
            if press.held() {
                down.push(k);
            }
        }
        down.sort_by_key(|k| k.0);
        down
    }

    /// Two players, player 2 with a keymap that makes A into B; a controller on each (slots 1 and 2),
    /// player 1 on ClamShell's screens.
    fn two_players() -> (Merge, Merged, Vec<PlayerRoute>) {
        let mut keymaps: [Keymap; Player::MOST] = Default::default();
        let mut swap = Keymap::new(1, "swap");
        assert!(swap.set(Input::Button(Button::A), Some(Binding::to(Output::Pad(clamshell_state::keymap::PadOut::Button(Button::B))))));
        keymaps[1] = swap;
        let mut players = routers(p(2), &keymaps, PadRoute::Ui, false);
        let (mut merge, mut merged) = (Merge::default(), Merged::default());
        merge.add(1, Player::ONE, Inputs::default(), &mut merged);
        merge.add(2, p(2), Inputs::default(), &mut merged);
        route_merged(&mut merged, &mut players, Instant::now());
        for pl in &mut players {
            pl.out = Out::default();
        }
        (merge, merged, players)
    }

    /// Player 2's controller goes to player 2's pad through player 2's keymap, and nothing of it reaches
    /// player 1's pad or ClamShell; player 1's goes to ClamShell's screens as its route says.
    #[test]
    fn a_player_2_controller_drives_its_own_pad_through_its_own_keymap() {
        let (mut merge, mut merged, mut players) = two_players();
        let now = Instant::now();
        for raw in [key(KeyCode::BTN_SOUTH, true), Raw::REPORT, key(KeyCode::BTN_MODE, true), Raw::REPORT] {
            merge.event(2, raw, &mut merged);
        }
        route_merged(&mut merged, &mut players, now);
        assert_eq!(held(&players[1].out.pad), [KeyCode::BTN_EAST, KeyCode::BTN_MODE], "A as B, and Home as the game's Guide");
        assert!(players[1].out.events.is_empty(), "nothing for ClamShell: {:?}", players[1].out.events);
        assert!(players[0].out.pad.is_empty() && players[0].out.events.is_empty(), "nothing on player 1's");
        // Player 1's A, on ClamShell's screens, is ClamShell's.
        merge.event(1, key(KeyCode::BTN_SOUTH, true), &mut merged);
        merge.event(1, Raw::REPORT, &mut merged);
        route_merged(&mut merged, &mut players, now);
        assert!(players[0].out.events.iter().any(|e| matches!(e, Event::Pad(_))), "{:?}", players[0].out.events);
        assert!(players[0].out.pad.is_empty(), "the game's pad untouched away from the game");
    }

    /// A controller unplugged lets go of what it held on its player's pad; the players' routers, and with
    /// them their pads, stay for the next controller.
    #[test]
    fn a_controller_unplugged_lets_go_on_its_pad_and_the_players_stay() {
        let (mut merge, mut merged, mut players) = two_players();
        let now = Instant::now();
        merge.event(2, key(KeyCode::BTN_NORTH, true), &mut merged);
        merge.event(2, Raw::REPORT, &mut merged);
        route_merged(&mut merged, &mut players, now);
        assert_eq!(merge.remove(2, &mut merged), Some(p(2)));
        players[1].router.controller_gone();
        route_merged(&mut merged, &mut players, now);
        assert_eq!(held(&players[1].out.pad), [], "let go");
        assert_eq!(players.len(), 2, "both players stay");
        merge.add(3, p(2), Inputs::default(), &mut merged);
        merge.event(3, key(KeyCode::BTN_WEST, true), &mut merged);
        merge.event(3, Raw::REPORT, &mut merged);
        route_merged(&mut merged, &mut players, now);
        assert_eq!(held(&players[1].out.pad), [KeyCode::BTN_WEST], "another controller drives the same pad");
    }

    /// Games' rumble on player 1's pad plays while the game has the pad, and on another player's always;
    /// player 1's keymap's buzz is player 1's alone.
    #[test]
    fn each_players_rumble_is_its_own() {
        let games = (60_000, 30_000);
        let buzz = Some((9_000, 3_000));
        let each = || [(games, None), ((0, 0), None), (games, None)].into_iter();
        let l = levels(each(), PadRoute::Game, RumbleStrength::Full);
        assert_eq!(&l[..3], [games, (0, 0), games]);
        assert_eq!(l[3], (0, 0), "no such player");
        let l = levels(each(), PadRoute::Ui, RumbleStrength::Full);
        assert_eq!(&l[..3], [(0, 0), (0, 0), games], "player 1 away from the game; player 3 still plays");
        let l = levels([((0, 0), buzz), ((0, 0), None)].into_iter(), PadRoute::Ui, RumbleStrength::Full);
        assert_eq!(&l[..2], [(9_000, 3_000), (0, 0)]);
        let l = levels([(games, None)].into_iter(), PadRoute::Game, RumbleStrength::Off);
        assert_eq!(l[0], (0, 0), "the setting off");
    }

    /// A pad thread with no virtual devices and no controllers, the pad on `route`.
    fn thread(route: PadRoute) -> (PadThread<std::sync::mpsc::Sender<Event>>, std::sync::mpsc::Receiver<Event>) {
        let (sink, events) = std::sync::mpsc::channel();
        let thread = PadThread {
            bell: Bell::new().unwrap(),
            spec: DeviceSpec::Path("/nonexistent/event0".into()),
            controls: clamshell_device::input::Controls::generic(),
            sensor_hub: None,
            taken: Taken::default(),
            sink,
            route,
            watch_activity: false,
            keymaps: Default::default(),
            devices: None,
            rumble_setting: RumbleStrength::default(),
            sticks: Default::default(),
            players: Players::default(),
            told: Vec::new(),
            look_every: None,
        };
        (thread, events)
    }

    /// Looks that come together are looked for once: each Look is only noted, the other commands are
    /// carried out meanwhile, and the run looks once for all of them. (A look per Look, each opening every
    /// input device, kept the controllers unread for seconds after a device was plugged in or out.)
    #[test]
    fn looks_that_come_together_are_one_look() {
        let (mut thread, _events) = thread(PadRoute::Ui);
        let mut pads = Pads::new(Player::ONE, &thread.keymaps, PadRoute::Ui, false);
        let (commands, received) = std::sync::mpsc::channel();
        for command in [Command::Look, Command::Route(PadRoute::Game), Command::Look, Command::Look] {
            commands.send(command).unwrap();
        }
        assert!(thread.take_commands(&mut pads, &received).unwrap());
        assert!(pads.look_due, "noted, to look once");
        assert_eq!(thread.route, PadRoute::Game, "the other commands carried out meanwhile");
        assert!(received.try_recv().is_err(), "every command taken");
    }

    /// A command's effect on time is seen in the turn that follows it, not only after the next input: a
    /// keymap with turbo on a button held wants its beat at once. (The wake time and the motors were
    /// worked out only after input, so an endless rumble kept playing after Home until the pad was
    /// touched.)
    #[test]
    fn the_turn_after_a_command_works_out_the_wake_time() {
        let (mut thread, _events) = thread(PadRoute::Game);
        let mut pads = Pads::new(Player::ONE, &thread.keymaps, PadRoute::Game, false);
        pads.merge.add(1, Player::ONE, Inputs::default(), &mut pads.merged);
        for raw in [key(KeyCode::BTN_SOUTH, true), Raw::REPORT] {
            pads.merge.event(1, raw, &mut pads.merged);
        }
        let now = Instant::now();
        assert_eq!(thread.turn(&mut pads, now).unwrap(), None, "A held: nothing waits for time");
        let mut turbo = Keymap::new(1, "turbo");
        let mut b = Binding::to(Output::Pad(clamshell_state::keymap::PadOut::Button(Button::A)));
        b.turbo = 10;
        assert!(turbo.set(Input::Button(Button::A), Some(b)));
        let (commands, received) = std::sync::mpsc::channel();
        commands.send(Command::Keymap(Player::ONE, Box::new(turbo))).unwrap();
        assert!(thread.take_commands(&mut pads, &received).unwrap());
        let wake = thread.turn(&mut pads, now).unwrap();
        assert!(wake.is_some_and(|at| at > now && at <= Instant::now() + Duration::from_millis(50)), "the turbo's next beat: {wake:?}");
    }

    /// A controller moved to a player added with the count holds what it held on its new player's pad.
    /// (The router was made after the move, at rest, and the held button was dropped.)
    #[test]
    fn a_controller_moved_to_a_new_player_keeps_what_it_holds() {
        let keymaps: [Keymap; Player::MOST] = Default::default();
        let mut players = routers(Player::ONE, &keymaps, PadRoute::Game, false);
        let (mut merge, mut merged) = (Merge::default(), Merged::default());
        merge.add(1, Player::ONE, Inputs::default(), &mut merged);
        for raw in [key(KeyCode::BTN_SOUTH, true), Raw::REPORT] {
            merge.event(1, raw, &mut merged);
        }
        route_merged(&mut merged, &mut players, Instant::now());
        for pl in &mut players {
            pl.out = Out::default();
        }
        regroup(&mut merge, &mut merged, &mut players, 2, [(1, p(2))].into_iter(), |p| player_route(p, &keymaps, PadRoute::Game, false), Instant::now());
        assert_eq!(players.len(), 2);
        assert_eq!(held(&players[1].out.pad), [KeyCode::BTN_SOUTH], "A held on player 2's pad");
        assert!(players[0].out.pad.iter().any(|e| *e == key(KeyCode::BTN_SOUTH, false)), "and let go on player 1's: {:?}", players[0].out.pad);
    }

    /// The watch of /dev/input failing falls back to timed looks, until the pad thread is gone. (It ended
    /// for good, and controllers plugged in weren't found until ClamShell started again.)
    #[test]
    fn a_failed_watch_falls_back_to_timed_looks() {
        let mut waits = [Ok(()), Err(rustix::io::Errno::INTR), Err(rustix::io::Errno::NOMEM)].into_iter();
        let (mut looks, mut pauses) = (0, Vec::new());
        watch_devices(
            || waits.next().unwrap_or(Ok(())),
            || {
                looks += 1;
                looks < 5
            },
            |d| pauses.push(d),
        );
        assert_eq!(looks, 5, "two for the watch, then three timed, the last finding the thread gone");
        assert_eq!(pauses, [LOOK_UNWATCHED; 3]);
    }

    /// Calibrating the gyro with no gyro answers at once that it wasn't calibrated.
    #[test]
    fn a_gyro_calibration_without_a_gyro_is_answered() {
        let (mut thread, events) = thread(PadRoute::Ui);
        let mut pads = Pads::new(Player::ONE, &thread.keymaps, PadRoute::Ui, false);
        let (commands, received) = std::sync::mpsc::channel();
        commands.send(Command::CalibrateGyro).unwrap();
        assert!(thread.take_commands(&mut pads, &received).unwrap());
        assert_eq!(events.try_recv().ok(), Some(Event::GyroCalibrated(false)));
        thread.command_while_down(Command::CalibrateGyro).unwrap();
        assert_eq!(events.try_recv().ok(), Some(Event::GyroCalibrated(false)), "nor while the thread is down");
    }

    /// A pad that can't be made leaves the pads there are, and the count made so far stands; only with no
    /// pad at all is it an error, and the virtual devices are made again. (One failure dropped every
    /// player's pad, player 1's from under its game.)
    #[test]
    fn a_pad_that_cant_be_made_leaves_the_others() {
        let mut made = 0;
        let mut make = || {
            made += 1;
            if made == 2 { Err(io::Error::other("refused")) } else { Ok(made) }
        };
        let mut pads = vec![0];
        grow(&mut pads, 3, &mut make).unwrap();
        assert_eq!(pads, [0, 1], "player 1's kept, player 2's made, player 3's refused");
        grow(&mut pads, 3, &mut make).unwrap();
        assert_eq!(pads, [0, 1, 3], "tried again later");
        let mut none: Vec<u8> = Vec::new();
        assert!(grow(&mut none, 2, || Err(io::Error::other("refused"))).is_err(), "no pad at all");
    }

    /// Launched programs are told to ignore every controller grabbed, but not ones with the virtual pad's
    /// ids. The list is written for Steam's launch hook, and one left by an earlier run is removed.
    #[test]
    fn launches_ignore_every_controller_grabbed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grabbed");
        std::fs::write(&file, "0x1234/0x5678").unwrap();
        let taken = Taken::new(Some(file.clone()));
        assert_eq!(taken.ignore(), None);
        assert!(!file.exists(), "an earlier run's list is gone");
        taken.add((0x045e, 0x0b13));
        taken.add((0x2dc8, 0x6003));
        taken.add((0x045e, 0x0b13));
        taken.add(device::VIRTUAL_IDS);
        assert_eq!(taken.ignore().as_deref(), Some("0x045e/0x0b13,0x2dc8/0x6003"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "0x045e/0x0b13,0x2dc8/0x6003");
    }
}
