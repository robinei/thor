//! The controller path: the real Xbox-style controller is grabbed for good, and games get a virtual Xbox
//! 360 pad that is always there. Where its events go is the state's choice ([`PadRoute`], see
//! [`route`]): the game, through the keymap in force ([`remap`]: the virtual pad, keys and mouse
//! buttons on the virtual mouse's keyboard, the pointer; a stick's or trigger's tuning, such as its
//! deadzone, is the keymap's); the pad mouse ([`mouse`]); ClamShell's screens ([`navigator`]); typing into an app
//! ([`typing`]); or a keymap's edit screen listening ([`listen`]). The gyro comes from a motion sensor
//! next to the controller ([`motion`]). Away from the game the virtual pad shows a neutral controller,
//! so games never see it disappear. The home button (BTN_MODE) is ours: it goes to the program as
//! [`Event::Home`]; held, then the Back button with a game in front, it is the game's Guide (see
//! [`route`]). The Back button ([`remap::BACK_KEY`]) is a keymap's input in the game, back on
//! ClamShell's screens; the AYN button ([`remap::AYN_KEY`], on the Thor's GPIO keys, read alongside the
//! controller: [`device::AynKey`]) a keymap's input in the game too, Guide as on the pad. The virtual
//! devices outlive the controller: unplugged (or failing), the pad stays, at rest, until one is back.
//! Games' rumble on the virtual pad reaches the real controller's motors while the game has the pad
//! ([`rumble`]), scaled by the Rumble setting.

mod device;
mod listen;
pub mod motion;
mod mouse;
mod navigator;
mod remap;
mod route;
mod rumble;
mod typing;

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clamshell_core::PadRoute;
use clamshell_core::keymap::Keymap;
use rustix::event::{PollFd, PollFlags, Timespec};

use crate::input::device::{DeviceSpec, Grabbed};
use crate::worker::{self, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink};
use device::{AynKey, Capabilities, VirtualDevices};
use motion::{EvdevMotion, MotionSource, Sample};
use route::{Out, PadRouter, Ranges};
use rumble::{Buzz, Effects, Rumble};

pub use route::{Mapping, PadSnapshot};

/// How often a missing controller is looked for.
const RETRY: Duration = Duration::from_secs(1);
/// The longest the thread waits for the controller before looking at its commands.
const COMMANDS_EVERY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Route(PadRoute),
    /// The keymap in force (Default: no bindings).
    Keymap(Box<Keymap>),
    /// Calibrate the gyro from its next samples.
    CalibrateGyro,
    /// The Rumble setting in force: 0 off, 1 low, 2 mid, 3 full.
    Rumble(u16),
}

lost_on!(io::Error);

/// The grabbed controller's USB ids (vendor, product), for what is launched: SDL programs are told to
/// ignore it (`SDL_GAMECONTROLLER_IGNORE_DEVICES`, as Steam does), so they take the virtual pad, not the
/// controller (grabbed: silent to them). mpv opens one controller only, the first it finds.
#[derive(Debug, Clone, Default)]
pub struct Taken(Arc<Mutex<Option<(u16, u16)>>>);

impl Taken {
    fn set(&self, ids: (u16, u16)) {
        if let Ok(mut taken) = self.0.lock() {
            *taken = Some(ids);
        }
    }

    /// A launch's variable for it, if a controller was grabbed (and it isn't the virtual pad's ids:
    /// ignoring those would hide the virtual pad too).
    pub fn env(&self) -> Option<(String, String)> {
        let (vendor, product) = (*self.0.lock().ok()?)?;
        ((vendor, product) != device::VIRTUAL_IDS).then(|| ("SDL_GAMECONTROLLER_IGNORE_DEVICES".to_owned(), format!("0x{vendor:04x}/0x{product:04x}")))
    }
}

/// Owns the controller whenever one is connected (`spec`: CLAMSHELL_GAMEPAD), looking again every second
/// when there is none; its ids in `taken`.
pub fn spawn(spec: DeviceSpec, taken: Taken, sink: impl Sink) -> io::Result<Worker<Command>> {
    Worker::spawn("pad", move |commands| {
        let mut pad = PadThread {
            spec,
            taken,
            sink,
            route: PadRoute::default(),
            keymap: Keymap::default(),
            devices: None,
            effects: Effects::default(),
            rumble_setting: 3,
            buzz: None,
        };
        worker::supervise(&mut pad, &commands, RETRY);
    })
}

/// What the pad thread keeps across the controller's comings and goings.
struct PadThread<S> {
    spec: DeviceSpec,
    taken: Taken,
    sink: S,
    route: PadRoute,
    keymap: Keymap,
    devices: Option<VirtualDevices>,
    /// Games' rumble effects on the virtual pad, and the Rumble setting.
    effects: Effects,
    rumble_setting: u16,
    /// The keymap's buzz playing, until when.
    buzz: Option<(Buzz, Instant)>,
}

/// A controller taken over, with its gyro if one is found.
struct Controller {
    grabbed: Grabbed,
    ranges: Ranges,
    snapshot: PadSnapshot,
    motion: Option<Box<dyn MotionSource>>,
    /// The AYN button's device, on the Thor.
    ayn: Option<AynKey>,
    /// Its motors, if it has force feedback.
    rumble: Option<Rumble>,
    /// Its X and Y codes are by letter (xpad's), not by place: swapped as they come in.
    letters: bool,
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
            Command::Rumble(setting) => self.rumble_setting = setting,
            Command::CalibrateGyro => match router {
                Some((router, _)) => router.calibrate(),
                // (No controller: no gyro to calibrate.)
                None => log::info!("pad: no controller to calibrate the gyro of"),
            },
        }
    }

    /// Sends what the router made of the controller's events: to the virtual devices and the state.
    fn emit(&mut self, out: &mut Out) -> Result<(), Ended<io::Error>> {
        for event in out.events.drain(..) {
            self.sink.send(event)?;
        }
        let Some(devices) = self.devices.as_mut() else {
            return Err(Ended::Lost(io::Error::other("no virtual devices")));
        };
        if !out.pad.is_empty() {
            // (Games read it as an Xbox 360 pad: X and Y by letter.)
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

    /// Finds the controller, (re)makes the virtual devices if it differs from the one they were made
    /// for, and grabs it.
    fn connect(&mut self) -> io::Result<Controller> {
        // (Games wait for their rumble uploads to be answered, controller or not.)
        self.serve_effects(Instant::now());
        let mut real = device::find(&self.spec)?;
        let id = real.input_id();
        self.taken.set((id.vendor(), id.product()));
        let caps = Capabilities::of(&real)?;
        let (ranges, snapshot) = (caps.ranges(), caps.snapshot());
        if self.devices.as_ref().is_none_or(|v| v.made_for != caps) {
            self.devices = Some(VirtualDevices::new(caps)?);
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
        let motion = EvdevMotion::find(&name, &phys).map(|m| Box::new(m) as Box<dyn MotionSource>);
        let letters = device::letter_codes(&name, &phys);
        if letters {
            log::info!("pad: its X and Y codes are by letter (xpad): read by place");
        }
        Ok(Controller { grabbed, ranges, snapshot, motion, ayn: AynKey::find(), rumble, letters })
    }

    /// Routes the controller's events until it is gone (or the program is).
    fn run_while_up(&mut self, c: Controller, commands: &Receiver<Command>) -> Result<(), Ended<io::Error>> {
        let mut c = c;
        let mut out = Out::default();
        let mut router = PadRouter::new(c.ranges, std::mem::take(&mut c.snapshot), self.route, self.keymap.clone(), &mut out);
        let result = self.route_events(&mut c, &mut router, &mut out, commands);
        if let Some(r) = c.rumble.as_mut() {
            let _ = r.set(0, 0);
        }
        // Gone (or failing): what the route held on the keyboard and mouse is let go.
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

    /// The controller is gone: the virtual pad stays, at rest.
    fn lost(&mut self, _: &io::Error) -> Result<(), Closed> {
        if let Some(devices) = self.devices.as_mut()
            && let Err(e) = devices.rest()
        {
            log::warn!("pad: {e}");
        }
        Ok(())
    }

    /// No controller plugged in is a normal state.
    fn expected(e: &io::Error) -> bool {
        e.kind() == io::ErrorKind::NotFound
    }
}

/// Waits up to `timeout` for the controller's, the gyro's, the AYN button's or the virtual pad's (rumble)
/// events.
fn wait(pad: &Grabbed, motion: Option<&dyn MotionSource>, ayn: Option<&AynKey>, virtual_pad: Option<BorrowedFd>, timeout: Duration) -> io::Result<()> {
    let timeout = Timespec { tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX), tv_nsec: i64::from(timeout.subsec_nanos()) };
    let mut fds = vec![PollFd::new(pad.device(), PollFlags::IN)];
    if let Some(m) = motion {
        fds.push(PollFd::from_borrowed_fd(m.fd(), PollFlags::IN));
    }
    if let Some(a) = ayn {
        fds.push(PollFd::new(a.device(), PollFlags::IN));
    }
    if let Some(v) = virtual_pad {
        fds.push(PollFd::from_borrowed_fd(v, PollFlags::IN));
    }
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

impl<S: Sink> PadThread<S> {
    /// Routes the controller's (and the gyro's) events until it is gone (or the program is).
    fn route_events(&mut self, c: &mut Controller, router: &mut PadRouter, out: &mut Out, commands: &Receiver<Command>) -> Result<(), Ended<io::Error>> {
        let Controller { grabbed, motion, ayn, rumble, letters, .. } = c;
        let mut raws = Vec::new();
        let mut samples: Vec<Sample> = Vec::new();
        let mut wake = None;
        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => self.apply(command, Some((router, out))),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            self.emit(out)?;
            // Wake for events, when the mapping wants a tick, and now and then for commands.
            let wait_for = wake.map_or(COMMANDS_EVERY, |at: Instant| at.saturating_duration_since(Instant::now())).min(COMMANDS_EVERY);
            let virtual_pad = self.devices.as_ref().map(|d| d.pad.as_fd());
            wait(grabbed, motion.as_deref(), ayn.as_ref(), virtual_pad, wait_for)?;
            let from = raws.len();
            grabbed.fetch(&mut raws)?;
            if *letters {
                device::swap_xy(raws.get_mut(from..).unwrap_or_default());
            }
            if let Some(a) = ayn.as_mut()
                && let Err(e) = a.fetch(&mut raws)
            {
                // (Left out if it fails; the controller goes on.)
                log::warn!("pad: AYN button: {e}");
                *ayn = None;
            }
            if let Some(m) = motion.as_mut()
                && let Err(e) = m.fetch(&mut samples)
            {
                // (A gyro that fails is left out; the controller goes on.)
                log::warn!("pad: gyro: {e}");
                *motion = None;
            }
            let now = Instant::now();
            for raw in raws.drain(..) {
                router.event(raw, now, out);
            }
            for sample in samples.drain(..) {
                router.motion(sample, out);
            }
            wake = router.tick(now, out);
            // Games' rumble, on the controller while the game has the pad, with the keymap's buzzes.
            self.serve_effects(now);
            if let Some(buzz) = out.buzz.take() {
                self.buzz = Some((buzz, now + Buzz::LENGTH));
            }
            self.buzz = self.buzz.filter(|(_, until)| now < *until);
            let (mut strong, mut weak) = if self.route == PadRoute::Game { self.effects.level(now) } else { (0, 0) };
            if let Some((buzz, _)) = self.buzz {
                let (s, w) = buzz.level();
                (strong, weak) = (strong.max(s), weak.max(w));
            }
            let setting = self.rumble_setting;
            if let Some(r) = rumble.as_mut()
                && let Err(e) = r.set(rumble::scale(strong, setting), rumble::scale(weak, setting))
            {
                log::warn!("pad: rumble: {e}");
                *rumble = None;
            }
            wake = [wake, self.effects.next_end(), self.buzz.map(|(_, until)| until)].into_iter().flatten().min();
            self.emit(out)?;
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
