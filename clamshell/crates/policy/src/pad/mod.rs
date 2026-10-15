//! The controller path: the real Xbox-style controller is grabbed for good, and games get a virtual Xbox
//! 360 pad that is always there. Where its events go is the state's choice ([`PadRoute`], see
//! [`route`]): the game, through the keymap in force ([`remap`]: the virtual pad, keys and mouse
//! buttons on the virtual mouse's keyboard, the pointer; a stick's or trigger's tuning, such as its
//! deadzone, is the keymap's); the pad mouse ([`mouse`]); ClamShell's screens ([`navigator`]); typing into an app
//! ([`typing`]); or a keymap's edit screen listening ([`listen`]). The gyro comes from a motion sensor
//! next to the controller ([`motion`]). Away from the game the virtual pad shows a neutral controller,
//! so games never see it disappear. The home button (BTN_MODE) is ours: it goes to the program as
//! [`Event::Home`]; held, then the Back button with a game in front, it is the game's Guide (see
//! [`route`]). The Back button (the layer key, [`remap::LAYER_KEYS`]) is ours too. The virtual
//! devices outlive the controller: unplugged (or failing), the pad stays, at rest, until one is back.
//!
//! Not yet: forwarding rumble from the virtual pad to the real one (uinput force-feedback uploads).

mod device;
mod listen;
pub mod motion;
mod mouse;
mod navigator;
mod remap;
mod route;
mod typing;

use std::io;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use clamshell_core::PadRoute;
use clamshell_core::keymap::Keymap;
use rustix::event::{PollFd, PollFlags, Timespec};

use crate::input::device::{DeviceSpec, Grabbed};
use crate::worker::{self, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink};
use device::{Capabilities, VirtualDevices};
use motion::{EvdevMotion, MotionSource, Sample};
use route::{Out, PadRouter, Ranges};

pub use route::{Mapping, PadSnapshot};

/// How often a missing controller is looked for.
const RETRY: Duration = Duration::from_secs(1);
/// The longest the thread waits for the controller before looking at its commands.
const COMMANDS_EVERY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Route(PadRoute),
    /// The keymap in force (Default: no bindings).
    Keymap(Keymap),
    /// Calibrate the gyro from its next samples.
    CalibrateGyro,
}

lost_on!(io::Error);

/// Owns the controller whenever one is connected (`spec`: CLAMSHELL_GAMEPAD), looking again every second
/// when there is none.
pub fn spawn(spec: DeviceSpec, sink: impl Sink) -> io::Result<Worker<Command>> {
    Worker::spawn("pad", move |commands| {
        let mut pad = PadThread { spec, sink, route: PadRoute::default(), keymap: Keymap::default(), devices: None };
        worker::supervise(&mut pad, &commands, RETRY);
    })
}

/// What the pad thread keeps across the controller's comings and goings.
struct PadThread<S> {
    spec: DeviceSpec,
    sink: S,
    route: PadRoute,
    keymap: Keymap,
    devices: Option<VirtualDevices>,
}

/// A controller taken over, with its gyro if one is found.
struct Controller {
    grabbed: Grabbed,
    ranges: Ranges,
    snapshot: PadSnapshot,
    motion: Option<Box<dyn MotionSource>>,
}

impl<S: Sink> PadThread<S> {
    fn apply(&mut self, command: Command, router: Option<(&mut PadRouter, &mut Out)>) {
        match command {
            Command::Keymap(keymap) => {
                log::info!("pad: keymap {} ({})", keymap.id, if keymap.id == 0 { "Default" } else { keymap.name.as_str() });
                if let Some((router, out)) = router {
                    router.set_keymap(keymap.clone(), out);
                }
                self.keymap = keymap;
            }
            Command::Route(route) => {
                self.route = route;
                if let Some((router, out)) = router {
                    router.set_route(route, out);
                }
            }
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
        let real = device::find(&self.spec)?;
        let caps = Capabilities::of(&real)?;
        let (ranges, snapshot) = (caps.ranges(), caps.snapshot());
        if self.devices.as_ref().is_none_or(|v| v.made_for != caps) {
            self.devices = Some(VirtualDevices::new(caps)?);
        }
        let (name, phys) = (real.name().unwrap_or("").to_owned(), real.physical_path().unwrap_or("").to_owned());
        let grabbed = Grabbed::new(real)?;
        log::info!("pad: {} grabbed; games see the virtual pad", grabbed.device().name().unwrap_or("controller"));
        let motion = EvdevMotion::find(&name, &phys).map(|m| Box::new(m) as Box<dyn MotionSource>);
        Ok(Controller { grabbed, ranges, snapshot, motion })
    }

    /// Routes the controller's events until it is gone (or the program is).
    fn run_while_up(&mut self, c: Controller, commands: &Receiver<Command>) -> Result<(), Ended<io::Error>> {
        let Controller { mut grabbed, ranges, snapshot, mut motion } = c;
        let mut out = Out::default();
        let mut router = PadRouter::new(ranges, snapshot, self.route, self.keymap.clone(), &mut out);
        let result = self.route_events(&mut grabbed, &mut motion, &mut router, &mut out, commands);
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

/// Waits up to `timeout` for the controller's or the gyro's events.
fn wait(pad: &Grabbed, motion: Option<&dyn MotionSource>, timeout: Duration) -> io::Result<()> {
    let timeout = Timespec { tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX), tv_nsec: i64::from(timeout.subsec_nanos()) };
    let mut fds = vec![PollFd::new(pad.device(), PollFlags::IN)];
    if let Some(m) = motion {
        fds.push(PollFd::from_borrowed_fd(m.fd(), PollFlags::IN));
    }
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

impl<S: Sink> PadThread<S> {
    /// Routes the controller's (and the gyro's) events until it is gone (or the program is).
    fn route_events(
        &mut self,
        grabbed: &mut Grabbed,
        motion: &mut Option<Box<dyn MotionSource>>,
        router: &mut PadRouter,
        out: &mut Out,
        commands: &Receiver<Command>,
    ) -> Result<(), Ended<io::Error>> {
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
            wait(grabbed, motion.as_deref(), wait_for)?;
            grabbed.fetch(&mut raws)?;
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
            self.emit(out)?;
        }
    }
}
