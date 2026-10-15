//! The controller path: the real Xbox-style controller is grabbed for good, and games get a virtual pad
//! that is always there: an Xbox 360 controller (uinput), or a DualSense (uhid, [`dualsense`]) with
//! motion sensors and rumble, as the settings in force say ([`PadKind`]). Where its events go is the
//! state's choice ([`PadRoute`], see [`route`]): the game's virtual pad (copied straight through,
//! [`Shaping`] applied); the pad mouse ([`mouse`]); ClamShell's screens ([`navigator`]); or typing into
//! an app ([`typing`]). Away from the game the virtual pad shows a neutral controller, so games never
//! see it disappear. The home button (BTN_MODE) is ours: games never see it, it goes to the program as
//! [`Event::Home`]. The virtual devices outlive the controller: unplugged (or failing), the pad stays,
//! at rest, until one is back. Button layers (remaps) go in this same path later.
//!
//! Rumble reaches the real controller from the DualSense's output reports ([`rumble`]); the Xbox pad
//! has no force feedback yet (that needs uinput's force-feedback uploads).

mod device;
pub mod dualsense;
mod mouse;
mod navigator;
mod route;
pub mod rumble;
mod shaping;
mod typing;

use std::io;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clamshell_core::PadRoute;

use crate::input::device::{DeviceSpec, Grabbed};
use crate::input::Raw;
use crate::worker::{self, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink};
use device::{Capabilities, VirtualDevices};
use route::{Out, PadRouter};
use rumble::{Rumble, RumbleSlot};

pub use route::{Mapping, PadSnapshot, Ranges};
pub use shaping::{PadKind, Shaping};

/// How often a missing controller is looked for.
const RETRY: Duration = Duration::from_secs(1);
/// The longest the thread waits for the controller before looking at its commands.
const COMMANDS_EVERY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Route(PadRoute),
    Shape(Shaping),
}

lost_on!(io::Error);

/// Owns the controller whenever one is connected (`spec`: CLAMSHELL_GAMEPAD), looking again every second
/// when there is none.
pub fn spawn(spec: DeviceSpec, sink: impl Sink) -> io::Result<Worker<Command>> {
    Worker::spawn("pad", move |commands| {
        let rumble = Arc::new(Mutex::new(None));
        let mut pad = PadThread { spec, sink, route: PadRoute::default(), shaping: Shaping::default(), devices: None, rumble };
        worker::supervise(&mut pad, &commands, RETRY);
    })
}

/// What the pad thread keeps across the controller's comings and goings.
struct PadThread<S> {
    spec: DeviceSpec,
    sink: S,
    route: PadRoute,
    shaping: Shaping,
    devices: Option<VirtualDevices>,
    /// The real controller's rumble, for the DualSense's thread.
    rumble: RumbleSlot,
}

/// A controller taken over.
struct Controller {
    grabbed: Grabbed,
    ranges: Ranges,
    snapshot: PadSnapshot,
}

impl<S: Sink> PadThread<S> {
    fn apply(&mut self, command: Command, router: Option<(&mut PadRouter, &mut Out)>) {
        match command {
            Command::Shape(shaping) => {
                self.shaping = shaping;
                let Some(devices) = self.devices.as_mut() else {
                    return;
                };
                match devices.set_kind(shaping.kind) {
                    Ok(true) => {
                        if let Some((router, out)) = router {
                            router.show(out);
                        }
                    }
                    Ok(false) => {}
                    Err(e) => log::warn!("pad: {e}"),
                }
            }
            Command::Route(route) => {
                self.route = route;
                if let Some((router, out)) = router {
                    router.set_route(route, out);
                }
            }
        }
    }

    /// Sends what the router made of the controller's events: to the virtual devices and the state.
    fn emit(&mut self, ranges: Ranges, out: &mut Out) -> Result<(), Ended<io::Error>> {
        for event in out.events.drain(..) {
            self.sink.send(event)?;
        }
        let Some(devices) = self.devices.as_mut() else {
            return Err(Ended::Lost(io::Error::other("no virtual devices")));
        };
        if !out.pad.is_empty() {
            let shaped: Vec<Raw> = out.pad.drain(..).map(|r| self.shaping.apply(ranges.stick, r)).collect();
            devices.pad.emit(&shaped)?;
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
        let mut real = device::find(&self.spec)?;
        let caps = Capabilities::of(&real)?;
        let (ranges, snapshot) = (caps.ranges(), caps.snapshot());
        if self.devices.as_ref().is_none_or(|v| v.made_for != caps) {
            self.devices = None;
            self.devices = Some(VirtualDevices::new(caps, self.shaping.kind, self.rumble.clone())?);
        }
        let rumble = Rumble::of(&mut real).unwrap_or_else(|e| {
            log::debug!("pad: no rumble: {e}");
            None
        });
        let grabbed = Grabbed::new(real)?;
        rumble::set(&self.rumble, rumble);
        log::info!("pad: {} grabbed; games see the virtual pad", grabbed.device().name().unwrap_or("controller"));
        Ok(Controller { grabbed, ranges, snapshot })
    }

    /// Routes the controller's events until it is gone (or the program is).
    fn run_while_up(&mut self, c: Controller, commands: &Receiver<Command>) -> Result<(), Ended<io::Error>> {
        let Controller { mut grabbed, ranges, snapshot } = c;
        let mut out = Out::default();
        let mut router = PadRouter::new(ranges, snapshot, self.route, &mut out);
        let mut raws = Vec::new();
        let mut wake = None;
        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => self.apply(command, Some((&mut router, &mut out))),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            self.emit(ranges, &mut out)?;
            // Wake for events, when the mapping wants a tick, and now and then for commands.
            let wait = wake.map_or(COMMANDS_EVERY, |at: Instant| at.saturating_duration_since(Instant::now())).min(COMMANDS_EVERY);
            grabbed.wait(wait)?;
            grabbed.fetch(&mut raws)?;
            let now = Instant::now();
            for raw in raws.drain(..) {
                router.event(raw, now, &mut out);
            }
            wake = router.tick(now, &mut out);
            self.emit(ranges, &mut out)?;
        }
    }

    fn command_while_down(&mut self, command: Command) -> Result<(), Closed> {
        self.apply(command, None);
        Ok(())
    }

    /// The controller is gone: the virtual pad stays, at rest.
    fn lost(&mut self, _: &io::Error) -> Result<(), Closed> {
        rumble::set(&self.rumble, None);
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
