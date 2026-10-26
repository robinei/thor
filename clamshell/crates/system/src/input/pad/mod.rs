//! The controller path. The real Xbox-style controller is grabbed for good, and games get a virtual Xbox
//! 360 pad that is always there.
//!
//! Where the controller's events go is the state's choice ([`PadRoute`], see [`route`]):
//!
//! - the game, through the keymap in force ([`remap`]). That means the virtual pad, keys and mouse
//!   buttons on the virtual mouse's keyboard, and the pointer. A stick's or trigger's tuning, such as its
//!   deadzone, is part of the keymap.
//! - the pad mouse ([`mouse`]);
//! - ClamShell's screens ([`navigator`]);
//! - typing into an app ([`typing`]);
//! - a keymap's edit screen, listening for an input ([`listen`]).
//!
//! The gyro comes from a motion sensor next to the controller ([`motion`]). Away from the game the
//! virtual pad shows a neutral controller, so games never see it disappear.
//!
//! The home button (BTN_MODE) is ours and goes to the program as [`Event::Home`]. Holding it and then
//! pressing Back with a game in front gives the game's Guide (see [`route`]). The Back button
//! ([`codes::BACK_KEY`]) is a keymap input in the game and means back on ClamShell's screens. The AYN
//! button ([`codes::AYN_KEY`]) is on the Thor's GPIO keys and is read alongside the controller
//! ([`device::AuxiliaryKey`]). It is also a keymap input in the game, and Guide by default, as on the pad.
//!
//! The virtual devices outlive the controller. If it is unplugged or fails, the pad stays, at rest,
//! until a controller is back. Games' rumble on the virtual pad reaches the real controller's motors
//! while the game has the pad ([`rumble`]), scaled by the Rumble setting.

mod calibrate;
mod calibrator;
mod codes;
mod device;
mod listen;
pub(crate) mod motion;
mod mouse;
mod navigator;
mod remap;
mod route;
mod rumble;
mod sensor_hub;
mod typing;

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clamshell_state::Event;
use clamshell_state::PadRoute;
use clamshell_state::device::SticksEvent;
use clamshell_state::keymap::Keymap;
use clamshell_state::settings::RumbleStrength;
use rustix::event::{PollFd, PollFlags};

use crate::os::evdev::Raw;
use crate::os::evdev::device::{DeviceSpec, Grabbed};
use crate::os::worker::{self, Bell, Ended, Next, Service, Worker, lost_on};
use crate::{Closed, Sink};
use device::{AuxiliaryKey, Capabilities, VirtualDevices};
use motion::{EvdevMotion, MotionSource, Sample};
use route::{Out, PadRouter, Ranges};
use rumble::{Buzz, Effects, Rumble};

pub(crate) use route::PadSnapshot;

/// How often a missing controller is looked for, besides whenever /dev/input changes because a device was
/// plugged in ([`Command::Look`]). If /dev/input can't be watched, it is every second.
const RETRY: Duration = Duration::from_secs(30);
const RETRY_UNWATCHED: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) enum Command {
    Route(PadRoute),
    WatchActivity(bool),
    /// The keymap in force. Default has no bindings.
    Keymap(Box<Keymap>),
    /// Calibrate the gyro from its next samples.
    CalibrateGyro,
    /// The Rumble setting in force: 0 off, 1 low, 2 mid, 3 full.
    Rumble(RumbleStrength),
    /// /dev/input changed, so a missing controller is looked for again. The supervisor tries again after
    /// any command.
    Look,
    /// The system is about to sleep. The gyro's stream is closed by dropping its source, and then the
    /// command is acknowledged ([`crate::power::sleep::spawn_hold`]).
    Sleep(Ack),
    /// The system is awake again, so the gyro is found again.
    Woke,
    /// Calibrate the controller's sticks (see [`calibrate`]). The progress and the end go to the state.
    CalibrateSticks,
    /// The sticks' calibrations, by controller. They are sent at start, and a calibration made here is
    /// kept here too.
    Sticks(clamshell_state::device::sticks::Calibrations),
}

/// A command's acknowledgement, sent once the command is carried out.
#[derive(Debug, Clone)]
pub(crate) struct Ack(pub(crate) std::sync::mpsc::Sender<()>);

lost_on!(io::Error);

/// The grabbed controller's USB ids (vendor, product), for what is launched. SDL programs are told to
/// ignore it with `SDL_GAMECONTROLLER_IGNORE_DEVICES`, as Steam does, so they take the virtual pad rather
/// than the controller, which is grabbed and so silent to them. mpv, for one, opens only the first
/// controller it finds.
#[derive(Debug, Clone, Default)]
pub(crate) struct Taken(Arc<Mutex<Option<(u16, u16)>>>);

impl Taken {
    fn set(&self, ids: (u16, u16)) {
        *crate::os::lock(&self.0) = Some(ids);
    }

    /// The variable for a launch, if a controller was grabbed. It is left out if the controller has the
    /// virtual pad's ids, since ignoring those would hide the virtual pad too.
    pub(crate) fn env(&self) -> Option<(String, String)> {
        let (vendor, product) = (*crate::os::lock(&self.0))?;
        ((vendor, product) != device::VIRTUAL_IDS).then(|| ("SDL_GAMECONTROLLER_IGNORE_DEVICES".to_owned(), format!("0x{vendor:04x}/0x{product:04x}")))
    }
}

/// Owns the controller whenever one is connected, with its ids in `taken`. `spec` comes from
/// CLAMSHELL_GAMEPAD. Without a controller it looks again when /dev/input changes, and also every
/// [`RETRY`].
pub(crate) fn spawn(
    spec: DeviceSpec,
    controls: clamshell_device::input::Controls,
    sensor_hub: Option<clamshell_device::gyro::SensorHub>,
    taken: Taken,
    sink: impl Sink,
) -> io::Result<Worker<Command>> {
    let watched = crate::input::watch::devices_watch();
    let retry = if watched.is_some() { RETRY } else { RETRY_UNWATCHED };
    // Kept through a restart after a panic, as through a reconnection: a game keeps its virtual
    // controller, and the keymap, route and calibrations stay.
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
        keymap: Keymap::default(),
        devices: None,
        effects: Effects::default(),
        rumble_setting: RumbleStrength::default(),
        buzz: None,
        sticks: Default::default(),
    };
    let worker = Worker::spawn_belled(clamshell_state::Thread::Pad.name(), bell, move |commands| worker::supervise(&mut pad, commands, retry))?;
    if let Some(notify) = watched {
        let pad = worker.clone();
        worker::lasting(clamshell_state::Thread::PadDevices.name(), move || {
            let mut fds = [PollFd::new(&notify, PollFlags::IN)];
            loop {
                match rustix::event::poll(&mut fds, None) {
                    Ok(_) | Err(rustix::io::Errno::INTR) => {}
                    Err(e) => return log::warn!("pad devices: {e}"),
                }
                let mut buf = [std::mem::MaybeUninit::uninit(); 4096];
                let mut reader = rustix::fs::inotify::Reader::new(&notify, &mut buf);
                while reader.next().is_ok() {}
                if pad.try_send(Command::Look).is_err() {
                    return;
                }
            }
        })?;
    }
    Ok(worker)
}

/// What the pad thread keeps across the controller's comings and goings.
struct PadThread<S> {
    /// Rung by the thread's commands, and waited on together with the controller.
    bell: Bell,
    spec: DeviceSpec,
    controls: clamshell_device::input::Controls,
    sensor_hub: Option<clamshell_device::gyro::SensorHub>,
    taken: Taken,
    sink: S,
    route: PadRoute,
    watch_activity: bool,
    keymap: Keymap,
    devices: Option<VirtualDevices>,
    /// Games' rumble effects on the virtual pad, and the Rumble setting.
    effects: Effects,
    rumble_setting: RumbleStrength,
    /// The keymap's buzz that is playing, and when it ends.
    buzz: Option<(Buzz, Instant)>,
    /// The sticks' calibrations, by controller.
    sticks: clamshell_state::device::sticks::Calibrations,
}

/// A controller taken over, with its gyro if one is found.
struct Controller {
    grabbed: Grabbed,
    controls: clamshell_device::input::Controls,
    ranges: Ranges,
    snapshot: PadSnapshot,
    motion: Option<Box<dyn MotionSource>>,
    /// The AYN button's device, on the Thor.
    ayn: Option<AuxiliaryKey>,
    /// Its motors, if it has force feedback.
    rumble: Option<Rumble>,
    /// Whether its X and Y codes are by letter, as with xpad and rsinput, rather than by place. If so
    /// they are swapped as they come in.
    letters: bool,
    /// Its name and physical path, to find its gyro again after sleep.
    name: String,
    phys: String,
    /// Its sticks: their calibration, and any calibration in progress.
    sticks: calibrate::StickState,
}

/// The gyro of the controller named `name` at `phys`: its own motion sensor, or else the Thor's sensor
/// hub.
fn find_motion(name: &str, phys: &str, sensor_hub: Option<clamshell_device::gyro::SensorHub>) -> Option<Box<dyn MotionSource>> {
    EvdevMotion::find(name, phys)
        .map(|m| Box::new(m) as Box<dyn MotionSource>)
        .or_else(|| sensor_hub::SensorHub::find(sensor_hub).map(|m| Box::new(m) as Box<dyn MotionSource>))
}

impl<S: Sink> PadThread<S> {
    fn apply(&mut self, command: Command, router: Option<(&mut PadRouter, &mut Out)>) {
        match command {
            Command::Keymap(keymap) => {
                log::info!("pad: keymap {} ({})", keymap.id, if keymap.id == 0 { "Default" } else { keymap.name.as_str() });
                if let Some((router, out)) = router {
                    router.set_keymap((*keymap).clone(), out);
                }
                self.keymap = *keymap;
            }
            Command::Route(route) => {
                self.route = route;
                if let Some((router, out)) = router {
                    router.set_route(route, out);
                }
            }
            Command::WatchActivity(watch) => {
                self.watch_activity = watch;
                if let Some((router, _)) = router {
                    router.watch_activity(watch);
                }
            }
            Command::Rumble(setting) => self.rumble_setting = setting,
            Command::CalibrateGyro => match router {
                Some((router, _)) => router.calibrate(),
                // Without a controller there is no gyro to calibrate.
                None => log::info!("pad: no controller to calibrate the gyro of"),
            },
            // The supervisor does the looking, after the command. With a controller there is nothing to do.
            Command::Look => {}
            // Without a controller there is no gyro to close or find. With a controller, these are handled
            // in route_events.
            Command::Sleep(Ack(done)) => {
                if done.send(()).is_err() {
                    log::debug!("pad: sleep no longer waits for the pad");
                }
            }
            Command::Woke => {}
            // Without a controller there is nothing to calibrate.
            Command::CalibrateSticks => {
                log::info!("pad: no controller to calibrate the sticks of");
                if self.sink.send(Event::Sticks(SticksEvent::Done(String::new(), None))).is_err() {
                    log::debug!("pad: the UI is gone");
                }
            }
            Command::Sticks(all) => self.sticks = all,
        }
    }

    /// Sends what the router made of the controller's events to the virtual devices and the state.
    fn emit(&mut self, out: &mut Out) -> Result<(), Ended<io::Error>> {
        for event in out.events.drain(..) {
            self.sink.send(event)?;
        }
        let Some(devices) = self.devices.as_mut() else {
            return Err(Ended::Lost(io::Error::other("no virtual devices")));
        };
        if !out.pad.is_empty() {
            // Games read it as an Xbox 360 pad, with X and Y by letter.
            device::swap_xy(&mut out.pad);
            device::emit(&mut devices.pad, &out.pad)?;
            out.pad.clear();
        }
        if !out.mouse.is_empty() {
            device::emit(&mut devices.mouse, &out.mouse)?;
            out.mouse.clear();
        }
        Ok(())
    }
}

impl<S: Sink> Service for PadThread<S> {
    type Command = Command;
    type Session = Controller;
    type Error = io::Error;
    const NAME: &'static str = "pad";

    /// Finds the controller and grabs it. The virtual devices are made again if the controller differs
    /// from the one they were made for.
    fn connect(&mut self) -> io::Result<Controller> {
        // Games wait for their rumble uploads to be answered, whether there is a controller or not.
        self.serve_effects(Instant::now());
        let mut real = device::find(&self.spec)?;
        let id = real.input_id();
        self.taken.set((id.vendor(), id.product()));
        let caps = Capabilities::of(&real)?;
        let (ranges, mut snapshot) = (caps.ranges(), caps.snapshot());
        if self.devices.as_ref().is_none_or(|v| v.made_for != caps) {
            self.devices = Some(VirtualDevices::new(caps, self.controls)?);
        }
        let (name, phys) = (real.name().unwrap_or("").to_owned(), real.physical_path().unwrap_or("").to_owned());
        let rumble = Rumble::of(&mut real).unwrap_or_else(|e| {
            log::warn!("pad: rumble: {e}");
            None
        });
        if rumble.is_none() {
            log::info!("pad: the controller has no rumble");
        }
        let grabbed = Grabbed::new(real)?;
        log::info!("pad: {} grabbed; games see the virtual pad", grabbed.device().name().unwrap_or("controller"));
        let motion = find_motion(&name, &phys, self.sensor_hub);
        let letters = device::letter_codes(self.controls, &name, &phys);
        if letters {
            log::info!("pad: its driver reports X and Y by letter: read by place");
        }
        let sticks = calibrate::StickState::new(ranges.stick, self.sticks.get(&name).copied(), &mut snapshot);
        if sticks.sticks.is_some() {
            log::info!("pad: its sticks calibrated");
        }
        Ok(Controller {
            grabbed,
            controls: self.controls,
            ranges,
            snapshot,
            motion,
            ayn: AuxiliaryKey::find(self.controls.auxiliary),
            rumble,
            letters,
            name,
            phys,
            sticks,
        })
    }

    /// Routes the controller's events until it, or the program, is gone.
    fn run_while_up(&mut self, c: Controller, commands: &Receiver<Command>) -> Result<(), Ended<io::Error>> {
        let mut c = c;
        let mut out = Out::default();
        let mut router = PadRouter::new(c.ranges, std::mem::take(&mut c.snapshot), self.route, self.keymap.clone(), &mut out);
        // The state's watch for game input stands across reconnections, as the route and keymap do.
        router.watch_activity(self.watch_activity);
        let result = self.route_events(&mut c, &mut router, &mut out, commands);
        // A calibration in progress when the controller went is given up, and the state is told so it
        // doesn't wait for it.
        if c.sticks.calibrating() && self.sink.send(Event::Sticks(SticksEvent::Done(c.name.clone(), None))).is_err() {
            log::debug!("pad: the UI is gone");
        }
        if let Some(r) = c.rumble.as_mut()
            && let Err(e) = r.set(0, 0)
        {
            log::debug!("pad: stopping the rumble of {}: {e}", c.name);
        }
        // The controller is gone or failing, so whatever the route held on the keyboard and mouse is let
        // go.
        router.lose(&mut out);
        if let Err(e) = self.emit(&mut out) {
            log::warn!("pad: letting go: {e:?}");
        }
        result
    }

    fn command_while_down(&mut self, command: Command) -> Result<(), Closed> {
        self.apply(command, None);
        Ok(())
    }

    /// The controller is gone. The virtual pad stays, at rest.
    fn lost(&mut self, _: &io::Error) -> Result<(), Closed> {
        if let Some(devices) = self.devices.as_mut()
            && let Err(e) = devices.rest()
        {
            log::warn!("pad: {e}");
        }
        Ok(())
    }

    /// A panic ended the run: what its route held is let go (the router went with the unwinding), and Home
    /// counts as up.
    fn panicked(&mut self) {
        if let Some(devices) = self.devices.as_mut()
            && let Err(e) = devices.rest()
        {
            log::warn!("pad: letting go after a panic: {e}");
        }
        if self.sink.send(Event::HomeHeld(false)).is_err() {
            log::debug!("pad: the UI is gone");
        }
    }

    /// No controller plugged in is a normal state.
    fn expected(e: &io::Error) -> bool {
        e.kind() == io::ErrorKind::NotFound
    }

    /// Waits for a command, answering games' rumble uploads to the virtual pad in the meantime, since a
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
            let Some(devices) = self.devices.as_ref() else {
                return worker::next(commands, Some(left));
            };
            let mut fds = [PollFd::from_borrowed_fd(devices.pad.as_fd(), PollFlags::IN), PollFd::from_borrowed_fd(self.bell.fd(), PollFlags::IN)];
            match rustix::event::poll(&mut fds, Some(&crate::os::sys::timespec(left))) {
                Ok(_) | Err(rustix::io::Errno::INTR) => self.bell.clear(),
                Err(e) => {
                    log::warn!("pad: {e}");
                    return worker::next(commands, Some(left));
                }
            }
            self.serve_effects(Instant::now());
        }
    }
}

/// Reads the controller's events and gyro samples since the last read into `raws` and `samples`. If the
/// kernel dropped events, they are replaced by the controller's current state. The letters are swapped
/// where the controller has them the other way, the sticks go through their calibration, and the AYN
/// button's events are added. A gyro or AYN button that fails is left out, and the controller goes on.
fn read_controller(c: &mut Controller, router: &mut PadRouter, raws: &mut Vec<Raw>, samples: &mut Vec<Sample>) -> io::Result<()> {
    let from = raws.len();
    if c.grabbed.fetch(raws)? {
        raws.truncate(from);
        // The axes are raw. sticks.take below corrects what resync gives, as it does any event.
        let (mut down, axes) = c.grabbed.state()?;
        if c.letters {
            down.iter_mut().for_each(|k| *k = device::swap_key(*k));
        }
        down.iter_mut().for_each(|key| key.0 = c.controls.normalise_button(key.0));
        router.resync(&down, &axes, raws);
        log::info!("pad: the controller's events overran; caught up with it as it is");
    } else {
        let physical = raws.get_mut(from..).unwrap_or_default();
        if c.letters {
            device::swap_xy(physical);
        }
        device::normalise_controls(c.controls, physical);
    }
    // The sticks go through their calibration. While a calibration is in progress, they go only to it.
    c.sticks.take(raws);
    if let Some(a) = c.ayn.as_mut()
        && let Err(e) = a.fetch(raws)
    {
        log::warn!("pad: AYN button: {e}");
        c.ayn = None;
    }
    if let Some(m) = c.motion.as_mut()
        && let Err(e) = m.fetch(samples)
    {
        log::warn!("pad: gyro: {e}");
        c.motion = None;
    }
    Ok(())
}

/// Waits, up to `timeout` if given, for events from the controller, the gyro, the AYN button or the
/// virtual pad's rumble, or for a command through the bell.
fn wait(
    pad: &Grabbed,
    motion: Option<&dyn MotionSource>,
    ayn: Option<&AuxiliaryKey>,
    virtual_pad: Option<BorrowedFd>,
    bell: &Bell,
    timeout: Option<Duration>,
) -> io::Result<()> {
    let timeout = timeout.map(crate::os::sys::timespec);
    let mut fds = vec![PollFd::new(pad.device(), PollFlags::IN), PollFd::from_borrowed_fd(bell.fd(), PollFlags::IN)];
    if let Some(m) = motion {
        fds.push(PollFd::from_borrowed_fd(m.fd(), PollFlags::IN));
    }
    if let Some(a) = ayn {
        fds.push(PollFd::new(a.device(), PollFlags::IN));
    }
    if let Some(v) = virtual_pad {
        fds.push(PollFd::from_borrowed_fd(v, PollFlags::IN));
    }
    match rustix::event::poll(&mut fds, timeout.as_ref()) {
        Ok(_) | Err(rustix::io::Errno::INTR) => {
            bell.clear();
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

impl<S: Sink> PadThread<S> {
    /// Routes the controller's and the gyro's events until the controller, or the program, is gone.
    fn route_events(&mut self, c: &mut Controller, router: &mut PadRouter, out: &mut Out, commands: &Receiver<Command>) -> Result<(), Ended<io::Error>> {
        let mut raws = Vec::new();
        let mut samples: Vec<Sample> = Vec::new();
        let mut wake = None;
        loop {
            if !self.take_commands(c, router, out, commands) {
                return Ok(());
            }
            self.emit(out)?;
            // Wake for events, commands, and when the mapping wants a tick.
            let wait_for = wake.map(|at: Instant| at.saturating_duration_since(Instant::now()));
            let virtual_pad = self.devices.as_ref().map(|d| d.pad.as_fd());
            wait(&c.grabbed, c.motion.as_deref(), c.ayn.as_ref(), virtual_pad, &self.bell, wait_for)?;
            read_controller(c, router, &mut raws, &mut samples)?;
            let now = Instant::now();
            for raw in raws.drain(..) {
                router.event(raw, now, out);
            }
            for sample in samples.drain(..) {
                router.motion(sample, out);
            }
            let ticked = router.tick(now, out);
            // A calibration in progress is fed, its progress and end are sent, and it is looked at again
            // soon.
            let (said, again) = c.sticks.step(now);
            self.calibration_said(said, &c.name)?;
            // Games' rumble goes to the controller while the game has the pad, along with the keymap's
            // buzzes.
            self.rumble(&mut c.rumble, out, now);
            wake = [ticked, again, self.effects.next_end(), self.buzz.map(|(_, until)| until)].into_iter().flatten().min();
            self.emit(out)?;
        }
    }

    /// Carries out the waiting commands. Those about the controller are handled here and the rest by
    /// [`PadThread::apply`]. Returns false once the commands' sender is gone.
    fn take_commands(&mut self, c: &mut Controller, router: &mut PadRouter, out: &mut Out, commands: &Receiver<Command>) -> bool {
        loop {
            match commands.try_recv() {
                // The gyro, and its stream, is closed before sleep and found again after.
                Ok(Command::Sleep(Ack(done))) => {
                    if c.motion.take().is_some() {
                        log::info!("pad: the gyro closed for sleep");
                    }
                    if done.send(()).is_err() {
                        log::debug!("pad: sleep no longer waits for the pad");
                    }
                }
                Ok(Command::Woke) => {
                    if c.motion.is_none() {
                        c.motion = find_motion(&c.name, &c.phys, self.sensor_hub);
                    }
                }
                Ok(Command::CalibrateSticks) => {
                    log::info!("pad: calibrating the sticks");
                    c.sticks.calibrate(Instant::now());
                }
                Ok(Command::Sticks(all)) => {
                    c.sticks.sticks = all.get(c.name.as_str()).copied();
                    self.sticks = all;
                }
                Ok(command) => self.apply(command, Some((router, out))),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    /// Sends on what a stick calibration said. A finished calibration is kept for the controller.
    fn calibration_said(&mut self, said: Option<calibrate::Said>, name: &str) -> Result<(), Closed> {
        match said {
            Some(calibrate::Said::Progress(p)) => self.sink.send(Event::Sticks(SticksEvent::Progress(p))),
            Some(calibrate::Said::Done(result)) => {
                log::info!("pad: the sticks {}", if result.is_some() { "calibrated" } else { "not calibrated (given up)" });
                if let Some(s) = result {
                    self.sticks.insert(name.to_owned(), s);
                }
                self.sink.send(Event::Sticks(SticksEvent::Done(name.to_owned(), result)))
            }
            None => Ok(()),
        }
    }

    /// Drives the controller's motors with games' rumble while the game has the pad, and with the
    /// keymap's buzzes, at the Rumble setting's strength. A controller whose rumble fails is left without.
    fn rumble(&mut self, rumble: &mut Option<Rumble>, out: &mut Out, now: Instant) {
        self.serve_effects(now);
        if let Some(buzz) = out.buzz.take() {
            self.buzz = Some((buzz, now + Buzz::LENGTH));
        }
        self.buzz = self.buzz.filter(|(_, until)| now < *until);
        // Effects that ended are let go whatever the route, so the next end is never in the past.
        let games = self.effects.level(now);
        let (strong, weak) = rumble::mix(games, self.route == PadRoute::Game, self.buzz.map(|(b, _)| b.level()), self.rumble_setting);
        if let Some(r) = rumble.as_mut()
            && let Err(e) = r.set(strong, weak)
        {
            log::warn!("pad: rumble: {e}");
            *rumble = None;
        }
    }

    /// Answers the virtual pad's rumble uploads and follows its play events.
    fn serve_effects(&mut self, now: Instant) {
        if let Some(devices) = self.devices.as_mut()
            && let Err(e) = self.effects.serve(&mut devices.pad, now)
        {
            log::warn!("pad: rumble requests: {e}");
        }
    }
}
