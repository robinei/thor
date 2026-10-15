//! A virtual USB DualSense, made through /dev/uhid: the kernel's hid-playstation driver binds it as it
//! would the real one, so games and Steam Input get its gamepad, its motion sensors (from a
//! [`MotionSource`]) and its hidraw node. The pad thread feeds it controller events ([`emit`]); a
//! thread of its own answers the kernel: the feature reports the driver asks for when it binds, and
//! output reports, whose rumble goes to the real controller ([`RumbleSlot`]). The touchpad is never
//! touched, the PS button never pressed.
//!
//! [`emit`]: VirtualDualSense::emit

pub mod motion;
pub mod report;
mod uhid;

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};

use super::route::Ranges;
use super::rumble::{self, RumbleSlot};
use crate::input::Raw;
use motion::MotionSource;
use report::PadState;
use uhid::KernelEvent;

/// The gamepad's name (hid-playstation adds " Motion Sensors" and " Touchpad" for the other two).
pub const NAME: &str = "clamshell pad (DualSense)";
pub const MOTION_NAME: &str = "clamshell pad (DualSense) Motion Sensors";
pub const TOUCHPAD_NAME: &str = "clamshell pad (DualSense) Touchpad";

/// How long the kernel's thread waits for events before looking at whether it should end.
const POLL: Duration = Duration::from_millis(100);
/// Input reports sent while the driver binds are dropped: for this long after it starts, the pad's
/// state is sent again every [`SETTLE_EVERY`].
const SETTLE: Duration = Duration::from_secs(1);
const SETTLE_EVERY: Duration = Duration::from_millis(50);
/// The errno for a feature report the pad doesn't have.
const EIO: u16 = 5;

/// What the pad thread and the kernel's thread share.
struct Shared {
    /// /dev/uhid, written by both threads (each write is one event).
    uhid: File,
    ranges: Ranges,
    pad: Mutex<Pad>,
    /// The driver has started the device: input reports are read.
    started: AtomicBool,
    /// Rumble was forwarded and not yet stopped.
    rumbling: AtomicBool,
    /// The motors as games last set them.
    motors: Mutex<Option<report::Motors>>,
    stop: AtomicBool,
    rumble: RumbleSlot,
    /// How often to report while nothing changes (the motion source's choice).
    every: Option<Duration>,
    born: Instant,
}

struct Pad {
    state: PadState,
    sequence: u8,
    motion: Box<dyn MotionSource>,
    sent: Instant,
}

impl Shared {
    /// Sends the pad as it is, with a fresh motion reading (nothing before the driver starts it).
    fn send(&self, pad: &mut Pad) -> io::Result<()> {
        if !self.started.load(Ordering::Acquire) {
            return Ok(());
        }
        let now = Instant::now();
        // The sensor clock counts thirds of a microsecond and wraps.
        let ticks = now.duration_since(self.born).as_micros().wrapping_mul(3) as u32;
        let report = report::input_report(&pad.state, &pad.motion.read(), pad.sequence, ticks);
        pad.sequence = pad.sequence.wrapping_add(1);
        pad.sent = now;
        (&self.uhid).write_all(&uhid::input2(&report))
    }

    fn send_now(&self) -> io::Result<()> {
        let mut pad = self.pad.lock().unwrap_or_else(PoisonError::into_inner);
        self.send(&mut pad)
    }

    fn write(&self, event: &[u8]) {
        if let Err(e) = (&self.uhid).write_all(event) {
            log::warn!("dualsense: {e}");
        }
    }

    /// Answers one kernel event.
    fn handle(&self, event: KernelEvent, no_rumble_noted: &mut bool) {
        match event {
            KernelEvent::Start => self.started.store(true, Ordering::Release),
            KernelEvent::Stop => self.started.store(false, Ordering::Release),
            KernelEvent::GetReport { id, report } => {
                let reply = report::feature_report(report);
                if reply.is_none() {
                    log::debug!("dualsense: feature report {report:#04x} asked for; it has none");
                }
                self.write(&uhid::get_report_reply(id, reply.as_deref().ok_or(EIO)));
            }
            // Nothing set through SET_REPORT is used (calibration, audio, test commands): accepted.
            KernelEvent::SetReport { id, .. } => self.write(&uhid::set_report_reply(id, 0)),
            KernelEvent::Output(data) => {
                if let Some(motors) = report::rumble(&data) {
                    self.rumble(motors, no_rumble_noted);
                }
            }
            KernelEvent::Open | KernelEvent::Close | KernelEvent::Other(_) => {}
        }
    }

    /// Plays the motors' strengths on the real controller.
    fn rumble(&self, motors: report::Motors, no_rumble_noted: &mut bool) {
        *self.motors.lock().unwrap_or_else(PoisonError::into_inner) = Some(motors);
        let on = motors != report::Motors::default();
        let had = rumble::with(&self.rumble, |r| {
            if let Err(e) = r.set(motors.strong, motors.weak) {
                log::debug!("dualsense: rumble: {e}");
            }
        });
        if had {
            self.rumbling.store(on, Ordering::Release);
        } else if on && !*no_rumble_noted {
            *no_rumble_noted = true;
            log::debug!("dualsense: a game asks for rumble; the controller has no force feedback");
        }
    }
}

/// The kernel's thread: answers its events, sends the pad again while the driver binds, and as often
/// as the motion source wants.
fn serve(shared: &Shared, reader: File) {
    let mut buf = vec![0u8; uhid::EVENT_SIZE];
    let mut started_at: Option<Instant> = None;
    let mut no_rumble_noted = false;
    while !shared.stop.load(Ordering::Acquire) {
        let settling = started_at.is_some_and(|at| at.elapsed() < SETTLE);
        let wait = [Some(POLL), shared.every, settling.then_some(SETTLE_EVERY)].into_iter().flatten().min().unwrap_or(POLL);
        let timeout = Timespec { tv_sec: 0, tv_nsec: i64::from(wait.subsec_nanos()) };
        let mut fds = [PollFd::new(&reader, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(rustix::io::Errno::INTR) => {}
            Ok(_) => match (&reader).read(&mut buf) {
                Ok(n) => {
                    if let Some(event) = uhid::parse(buf.get(..n).unwrap_or_default()) {
                        if event == KernelEvent::Start {
                            started_at = Some(Instant::now());
                        }
                        shared.handle(event, &mut no_rumble_noted);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    log::warn!("dualsense: reading /dev/uhid: {e}");
                    return;
                }
            },
            Err(e) => {
                log::warn!("dualsense: {e}");
                return;
            }
        }
        let mut pad = shared.pad.lock().unwrap_or_else(PoisonError::into_inner);
        let due = shared.every.is_some_and(|every| pad.sent.elapsed() >= every) || (settling && pad.sent.elapsed() >= SETTLE_EVERY);
        if due && let Err(e) = shared.send(&mut pad) {
            log::warn!("dualsense: {e}");
        }
    }
}

/// The virtual DualSense; removed when dropped.
pub struct VirtualDualSense {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl VirtualDualSense {
    /// Makes the device: controller values are read in `ranges`, motion from `motion`, and rumble goes
    /// to whatever controller is in `rumble`.
    pub fn new(ranges: Ranges, motion: Box<dyn MotionSource>, rumble: RumbleSlot) -> io::Result<VirtualDualSense> {
        let uhid = OpenOptions::new().read(true).write(true).open("/dev/uhid")?;
        let reader = uhid.try_clone()?;
        let every = motion.every();
        let now = Instant::now();
        let pad = Mutex::new(Pad { state: PadState::default(), sequence: 0, motion, sent: now });
        let shared = Arc::new(Shared {
            uhid,
            ranges,
            pad,
            started: AtomicBool::new(false),
            rumbling: AtomicBool::new(false),
            motors: Mutex::new(None),
            stop: AtomicBool::new(false),
            rumble,
            every,
            born: now,
        });
        // No serial: hid-playstation sets it from the pairing report's MAC address.
        let create = uhid::create2(NAME, "", report::VENDOR, report::PRODUCT, report::DESCRIPTOR);
        (&shared.uhid).write_all(&create)?;
        let theirs = shared.clone();
        let thread = std::thread::Builder::new().name("dualsense".into()).spawn(move || serve(&theirs, reader))?;
        log::info!("pad: virtual DualSense made");
        Ok(VirtualDualSense { shared, thread: Some(thread) })
    }

    /// Controller events: each report (SYN_REPORT) sends the pad as it then is.
    pub fn emit(&mut self, events: &[Raw]) -> io::Result<()> {
        let shared = &self.shared;
        let mut pad = shared.pad.lock().unwrap_or_else(PoisonError::into_inner);
        for &raw in events {
            if raw.is_report() {
                shared.send(&mut pad)?;
            } else {
                pad.state.apply(raw, shared.ranges);
            }
        }
        Ok(())
    }

    /// Whether the driver has started it (input reports are read).
    pub fn started(&self) -> bool {
        self.shared.started.load(Ordering::Acquire)
    }

    /// The motors as games last set them (`None`: never).
    pub fn motors(&self) -> Option<report::Motors> {
        *self.shared.motors.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends the pad as it is now (for a test: reports while the driver binds are dropped).
    pub fn resend(&self) -> io::Result<()> {
        self.shared.send_now()
    }
}

impl Drop for VirtualDualSense {
    /// Stops rumble it started, ends its thread and removes the device.
    fn drop(&mut self) {
        let shared = &self.shared;
        if shared.rumbling.swap(false, Ordering::AcqRel) {
            rumble::with(&shared.rumble, |r| {
                let _ = r.set(0, 0);
            });
        }
        shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        shared.write(&uhid::destroy());
        log::info!("pad: virtual DualSense removed");
    }
}
