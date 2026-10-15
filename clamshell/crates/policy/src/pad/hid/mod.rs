//! Virtual HID pads made through /dev/uhid: a USB DualSense ([`dualsense`], bound by the kernel's
//! hid-playstation) and a Bluetooth Switch Pro Controller ([`procon`], bound by hid-nintendo). The
//! kernel's drivers bind them as they would the real ones, so games and Steam Input get their gamepad,
//! their motion sensors (from a [`MotionSource`]) and their hidraw node. The pad thread feeds one
//! controller events ([`VirtualHid::emit`]); a thread of its own answers the kernel (the reports and
//! commands the driver sends when it binds, and later) and streams reports as often as the pad and
//! the motion source want; rumble asked for in output reports goes to the real controller
//! ([`RumbleSlot`]). Touchpads are never touched, the PS and home buttons never pressed.

pub mod dualsense;
pub mod motion;
pub mod procon;
pub mod state;
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
use motion::{Motion, MotionSource};
use state::PadState;
use uhid::KernelEvent;

/// How long the kernel's thread waits for events before looking at whether it should end.
const POLL: Duration = Duration::from_millis(100);
/// Input reports sent while a driver binds may be dropped: for this long after it starts, the pad's
/// state is sent again every [`SETTLE_EVERY`].
const SETTLE: Duration = Duration::from_secs(1);
const SETTLE_EVERY: Duration = Duration::from_millis(50);
/// The errno for a report the pad doesn't have.
const EIO: u16 = 5;

/// The bus a pad says it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Usb,
    Bluetooth,
}

/// What a pad is made as.
pub struct Identity {
    pub name: &'static str,
    pub bus: Bus,
    pub vendor: u32,
    pub product: u32,
    pub descriptor: &'static [u8],
}

/// The motors' strengths (0-255) a game asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Motors {
    /// The left, heavy motor.
    pub strong: u8,
    /// The right, light motor.
    pub weak: u8,
}

/// What an output report asks for: rumble, and input reports to send in answer.
#[derive(Debug, Default, PartialEq)]
pub struct Answer {
    pub motors: Option<Motors>,
    pub reports: Vec<Vec<u8>>,
}

/// A pad's reports: what the kernel's driver (and hidraw readers) read and write. Pure: the device
/// does the I/O.
pub trait Protocol: Send {
    fn identity(&self) -> Identity;

    /// The input report for the pad and motion as they are (`at`: time since the device was made).
    fn input(&mut self, pad: &PadState, motion: &Motion, at: Duration) -> Vec<u8>;

    /// The answer to GET_REPORT for a feature report (`None`: it has none).
    fn feature(&mut self, id: u8) -> Option<Vec<u8>>;

    /// An output report (rumble, LEDs, commands).
    fn output(&mut self, report: &[u8], pad: &PadState, at: Duration) -> Answer;

    /// How often the pad reports while nothing changes (`None`: only on changes).
    fn every(&self) -> Option<Duration>;
}

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
    motors: Mutex<Option<Motors>>,
    stop: AtomicBool,
    rumble: RumbleSlot,
    /// How often to report while nothing changes: the pad's or the motion source's choice.
    every: Option<Duration>,
    born: Instant,
}

struct Pad {
    state: PadState,
    protocol: Box<dyn Protocol>,
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
        let motion = pad.motion.read();
        let report = pad.protocol.input(&pad.state, &motion, now.duration_since(self.born));
        pad.sent = now;
        (&self.uhid).write_all(&uhid::input2(&report))
    }

    fn write(&self, event: &[u8]) {
        if let Err(e) = (&self.uhid).write_all(event) {
            log::warn!("hid pad: {e}");
        }
    }

    /// Answers one kernel event.
    fn handle(&self, event: KernelEvent, no_rumble_noted: &mut bool) {
        let lock = || self.pad.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            KernelEvent::Start => self.started.store(true, Ordering::Release),
            KernelEvent::Stop => self.started.store(false, Ordering::Release),
            KernelEvent::GetReport { id, report } => {
                let reply = lock().protocol.feature(report);
                if reply.is_none() {
                    log::debug!("hid pad: feature report {report:#04x} asked for; it has none");
                }
                self.write(&uhid::get_report_reply(id, reply.as_deref().ok_or(EIO)));
            }
            // Nothing set through SET_REPORT is used: accepted.
            KernelEvent::SetReport { id, .. } => self.write(&uhid::set_report_reply(id, 0)),
            KernelEvent::Output(data) => {
                let answer = {
                    let mut pad = lock();
                    let at = self.born.elapsed();
                    let Pad { state, protocol, .. } = &mut *pad;
                    let answer = protocol.output(&data, state, at);
                    for report in &answer.reports {
                        self.write(&uhid::input2(report));
                    }
                    answer
                };
                if let Some(motors) = answer.motors {
                    self.rumble(motors, no_rumble_noted);
                }
            }
            KernelEvent::Open | KernelEvent::Close | KernelEvent::Other(_) => {}
        }
    }

    /// Plays the motors' strengths on the real controller.
    fn rumble(&self, motors: Motors, no_rumble_noted: &mut bool) {
        let last = self.motors.lock().unwrap_or_else(PoisonError::into_inner).replace(motors);
        if last == Some(motors) {
            return;
        }
        let on = motors != Motors::default();
        let had = rumble::with(&self.rumble, |r| {
            if let Err(e) = r.set(motors.strong, motors.weak) {
                log::debug!("hid pad: rumble: {e}");
            }
        });
        if had {
            self.rumbling.store(on, Ordering::Release);
        } else if on && !*no_rumble_noted {
            *no_rumble_noted = true;
            log::debug!("hid pad: a game asks for rumble; the controller has no force feedback");
        }
    }
}

/// The kernel's thread: answers its events, sends the pad again while the driver binds, and as often
/// as the pad or the motion source wants.
fn serve(shared: &Shared, reader: File) {
    let mut buf = vec![0u8; uhid::EVENT_SIZE];
    let mut started_at: Option<Instant> = None;
    let mut no_rumble_noted = false;
    let mut sent = Instant::now();
    while !shared.stop.load(Ordering::Acquire) {
        let settling = started_at.is_some_and(|at| at.elapsed() < SETTLE);
        let streaming = [shared.every, settling.then_some(SETTLE_EVERY)].into_iter().flatten().min();
        let wait = streaming.map_or(POLL, |every| (sent + every).saturating_duration_since(Instant::now()).min(POLL));
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
                    log::warn!("hid pad: reading /dev/uhid: {e}");
                    return;
                }
            },
            Err(e) => {
                log::warn!("hid pad: {e}");
                return;
            }
        }
        let mut pad = shared.pad.lock().unwrap_or_else(PoisonError::into_inner);
        if streaming.is_some_and(|every| pad.sent.elapsed() >= every)
            && let Err(e) = shared.send(&mut pad)
        {
            log::warn!("hid pad: {e}");
        }
        sent = pad.sent;
    }
}

/// A virtual HID pad; removed when dropped.
pub struct VirtualHid {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    name: &'static str,
}

impl VirtualHid {
    /// Makes the device: `protocol` says what it is, controller values are read in `ranges`, motion
    /// from `motion`, and rumble goes to whatever controller is in `rumble`.
    pub fn new(protocol: Box<dyn Protocol>, ranges: Ranges, motion: Box<dyn MotionSource>, rumble: RumbleSlot) -> io::Result<VirtualHid> {
        let uhid = OpenOptions::new().read(true).write(true).open("/dev/uhid")?;
        let reader = uhid.try_clone()?;
        let identity = protocol.identity();
        let every = [protocol.every(), motion.every()].into_iter().flatten().min();
        let now = Instant::now();
        let pad = Mutex::new(Pad { state: PadState::default(), protocol, motion, sent: now });
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
        // No serial: the drivers set it from the MAC address the pad reports.
        let create = uhid::create2(identity.name, "", identity.bus, identity.vendor, identity.product, identity.descriptor);
        (&shared.uhid).write_all(&create)?;
        let theirs = shared.clone();
        let thread = std::thread::Builder::new().name("hid pad".into()).spawn(move || serve(&theirs, reader))?;
        log::info!("pad: {} made", identity.name);
        Ok(VirtualHid { shared, thread: Some(thread), name: identity.name })
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
    pub fn motors(&self) -> Option<Motors> {
        *self.shared.motors.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends the pad as it is now (for a test: reports while the driver binds may be dropped).
    pub fn resend(&self) -> io::Result<()> {
        let mut pad = self.shared.pad.lock().unwrap_or_else(PoisonError::into_inner);
        self.shared.send(&mut pad)
    }
}

impl Drop for VirtualHid {
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
        log::info!("pad: {} removed", self.name);
    }
}

/// Puts `bytes` at `at` in `report` (nothing past its end).
fn put(report: &mut [u8], at: usize, bytes: &[u8]) {
    if let Some(slot) = report.get_mut(at..at + bytes.len()) {
        slot.copy_from_slice(bytes);
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
pub(crate) mod descriptor {
    /// Walks a report descriptor's short items, adding up the bytes of each report, by (kind, report
    /// ID): kind 0x80 input, 0x90 output, 0xb0 feature. Checks that items are whole and collections
    /// closed, and that every report is whole bytes.
    pub fn report_bytes(d: &[u8]) -> Vec<((u8, u8), u32)> {
        let (mut i, mut id, mut size, mut count) = (0, 0u8, 0u32, 0u32);
        let mut out: Vec<((u8, u8), u32)> = Vec::new();
        let mut depth = 0i32;
        while i < d.len() {
            let prefix = d[i];
            let len = match prefix & 0b11 {
                3 => 4,
                n => usize::from(n),
            };
            let data = d[i + 1..i + 1 + len].iter().rev().fold(0u32, |v, &b| (v << 8) | u32::from(b));
            match prefix & 0xfc {
                0x84 => id = data as u8,
                0x74 => size = data,
                0x94 => count = data,
                0xa0 => depth += 1,
                0xc0 => depth -= 1,
                kind @ (0x80 | 0x90 | 0xb0) => match out.iter_mut().find(|(k, _)| *k == (kind, id)) {
                    Some((_, bits)) => *bits += size * count,
                    None => out.push(((kind, id), size * count)),
                },
                _ => {}
            }
            i += 1 + len;
        }
        assert_eq!((i, depth), (d.len(), 0), "whole items, collections closed");
        assert!(out.iter().all(|(_, b)| b % 8 == 0), "whole bytes");
        out.into_iter().map(|(k, bits)| (k, bits / 8)).collect()
    }
}
