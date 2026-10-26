//! What every system thread is made of:
//!
//! - A thread is spawned with its command channel ([`Worker::spawn`]). A panic in it is logged and the
//!   thread is started again, on the same channel, with what the body makes afresh ([`restarted`]).
//! - It waits for its next command or for its period to pass ([`next`]). A thread that waits on files,
//!   such as a device or inotify, waits on those and its [`Bell`] together, and its commands ring the
//!   bell.
//! - It sends a reading only when the reading changed ([`Changed`]).
//! - A thread that talks to a service, such as a bus, a socket or a device, connects and runs while the
//!   service is up. It is supervised: while the service is down, it is tried again every `retry`
//!   ([`supervise`]).
//!
//! Commands that arrive while a service is down follow one rule. Something a command sets, such as
//! watching or a route, is kept and applies once the service is back. Something a command asks to be
//! done is refused at once as "unavailable" and never queued. Either way the service is tried again right
//! after the command, so opening a tab is a fresh attempt.
//!
//! In a run of failures, the first is logged as a warning and the retries at debug (see [`Quiet`]). A
//! failure that is a normal state, such as no controller being plugged in, is always logged at debug.

use std::convert::Infallible;
use std::fmt::Display;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// The UI is gone. A thread whose events can't be delivered ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// A thread's command channel. Sending to a thread that has ended is logged once (`<name> is gone`).
pub(crate) struct Worker<C> {
    name: &'static str,
    tx: Option<Sender<C>>,
    gone: Arc<AtomicBool>,
    bell: Option<Bell>,
}

impl<C> Clone for Worker<C> {
    fn clone(&self) -> Self {
        Worker { name: self.name, tx: self.tx.clone(), gone: self.gone.clone(), bell: self.bell.clone() }
    }
}

/// A thread's bell: an eventfd that is rung as commands are sent to the thread. It is for a thread that
/// polls files and so can't wait on its channel too. The thread polls the bell's fd along with its own,
/// and when woken it clears the bell and takes its commands with try_recv. Without the bell it would have
/// to wake now and then to look.
#[derive(Clone)]
pub(crate) struct Bell(Arc<OwnedFd>);

impl Bell {
    pub(crate) fn new() -> io::Result<Bell> {
        let fd = rustix::event::eventfd(0, rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK)?;
        Ok(Bell(Arc::new(fd)))
    }

    /// The fd to wait on. It is readable once the bell has rung.
    pub(crate) fn fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }

    pub(crate) fn ring(&self) {
        if let Err(e) = rustix::io::write(&*self.0, &1u64.to_ne_bytes()) {
            log::warn!("ringing a thread's bell: {e}");
        }
    }

    /// Resets the bell, however often it rang. The bell carries no message. Whatever rang it is waiting
    /// in the command channel, or is for the ringer to say.
    pub(crate) fn clear(&self) {
        let mut count = [0u8; 8];
        if let Err(e) = rustix::io::read(&*self.0, &mut count)
            && e != rustix::io::Errno::AGAIN
        {
            log::warn!("clearing a thread's bell: {e}");
        }
    }
}

/// A command for a reader thread: whether what it reads is shown. While it is, the thread reads often.
/// While it isn't, the thread reads seldom or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shown(pub(crate) bool);

/// A command reached no thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gone;

impl<C: Send + 'static> Worker<C> {
    /// Starts a thread named `name` running `body` with the receiving end of its command channel. The
    /// thread ends when `body` returns, usually when every sender is dropped. After a panic `body` runs
    /// again ([`restarted`]), so what it makes at its start is made afresh, and what it captured is kept.
    pub(crate) fn spawn(name: &'static str, mut body: impl FnMut(&Receiver<C>) + Send + 'static) -> io::Result<Worker<C>> {
        let (tx, rx) = mpsc::channel();
        start(name, move || restarted(name, || body(&rx)))?;
        Ok(Worker { name, tx: Some(tx), gone: Arc::default(), bell: None })
    }

    /// Like [`Worker::spawn`], for a thread that waits on files: its commands ring `bell`, which `body`
    /// holds a clone of to wait on.
    pub(crate) fn spawn_belled(name: &'static str, bell: Bell, mut body: impl FnMut(&Receiver<C>) + Send + 'static) -> io::Result<Worker<C>> {
        let (tx, rx) = mpsc::channel();
        start(name, move || restarted(name, || body(&rx)))?;
        Ok(Worker { name, tx: Some(tx), gone: Arc::default(), bell: Some(bell) })
    }
}

impl<C> Worker<C> {
    /// Stands in for a thread that couldn't be started. Commands go nowhere, which is logged once.
    pub(crate) fn gone(name: &'static str) -> Worker<C> {
        Worker { name, tx: None, gone: Arc::default(), bell: None }
    }

    /// Sends a command. If the thread has ended, this is logged once.
    pub(crate) fn send(&self, command: C) {
        let _ = self.try_send(command);
    }

    /// Sends a command, or returns `Gone` if the thread has ended. That is also logged once.
    pub(crate) fn try_send(&self, command: C) -> Result<(), Gone> {
        let sent = self.tx.as_ref().is_some_and(|tx| tx.send(command).is_ok());
        if sent {
            if let Some(bell) = &self.bell {
                bell.ring();
            }
            return Ok(());
        }
        if !self.gone.swap(true, Ordering::Relaxed) {
            log::warn!("{} is gone", self.name);
        }
        Err(Gone)
    }
}

/// Starts a thread that takes no commands, for one piece of work: it isn't started again after a panic.
/// A long-lived one is [`lasting`].
pub fn detached(name: &str, body: impl FnOnce() + Send + 'static) -> io::Result<()> {
    start(name, body).map(drop)
}

/// Starts a long-lived thread that takes no commands, started again after a panic ([`restarted`]). It
/// ends when `body` returns, usually when the UI is gone.
pub(crate) fn lasting(name: &'static str, mut body: impl FnMut() + Send + 'static) -> io::Result<()> {
    start(name, move || restarted(name, &mut body)).map(drop)
}

/// What `work` gives, or what `fallback` gives if it panics (the panic hook has logged it): for a
/// short-lived thread whose answer the UI waits for, which then gets one.
pub(crate) fn or_on_panic<T>(work: impl FnOnce() -> T, fallback: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| fallback())
}

/// After a run of panics, the waits before starting again grow from none to this, doubling.
const RESTART_FIRST: Duration = Duration::from_millis(250);
const RESTART_MOST: Duration = Duration::from_secs(5);
/// A thread that ran this long before panicking is started again at once.
const RESTART_CALM: Duration = Duration::from_secs(30);

/// Told the name of each thread started again after a panic (see [`on_restart`]).
type Restarts = Box<dyn Fn(&'static str) + Send>;
static RESTARTS: std::sync::Mutex<Option<Restarts>> = std::sync::Mutex::new(None);

/// Sets what is told when a thread is started again after a panic, so that the UI sends it again what
/// it had been sent and lost.
pub(crate) fn on_restart(told: impl Fn(&'static str) + Send + 'static) {
    *crate::os::lock(&RESTARTS) = Some(Box::new(told));
}

/// Runs `body` until it returns. A panic in it (the panic hook has logged it) is logged and `body` runs
/// again: at once, then after waits that double to [`RESTART_MOST`] while it keeps panicking soon after
/// starting. Before it runs again, [`on_restart`]'s function is told.
pub(crate) fn restarted(name: &'static str, mut body: impl FnMut()) {
    let mut delay = Duration::ZERO;
    loop {
        let started = Instant::now();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut body)).is_ok() {
            return;
        }
        let wait = if started.elapsed() >= RESTART_CALM { Duration::ZERO } else { delay };
        delay = (wait * 2).clamp(RESTART_FIRST, RESTART_MOST);
        log::error!("{name}: panicked; starting it again in {} ms", wait.as_millis());
        thread::sleep(wait);
        if let Some(told) = crate::os::lock(&RESTARTS).as_ref() {
            told(name);
        }
    }
}

/// Starts a thread named `name` running `body`, at the ordinary priority whichever thread started it:
/// the compositor's thread runs raised (see [`crate::session::priority`]), and a new thread would
/// inherit that, ahead of the game.
pub(crate) fn start<T: Send + 'static>(name: &str, body: impl FnOnce() -> T + Send + 'static) -> io::Result<thread::JoinHandle<T>> {
    thread::Builder::new().name(name.into()).spawn(move || {
        ordinary_priority();
        body()
    })
}

/// The calling thread back at nice level 0 if it is below (raising the level needs no privilege).
fn ordinary_priority() {
    let thread = rustix::thread::gettid();
    match rustix::process::getpriority_process(Some(thread)) {
        Ok(level) if level < 0 => {
            if let Err(e) = rustix::process::setpriority_process(Some(thread), 0) {
                log::warn!("{}: nice level {level} kept: {e}", thread::current().name().unwrap_or("a thread"));
            }
        }
        Ok(_) => {}
        Err(e) => log::warn!("{}: nice level unknown: {e}", thread::current().name().unwrap_or("a thread")),
    }
}

/// What woke a thread waiting for its next command.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Next<C> {
    Command(C),
    /// The period passed without one.
    Elapsed,
    /// Every sender is gone, so the thread ends.
    Disconnected,
}

/// Waits for the next command, for at most `every`. With `None` it waits until one comes.
pub(crate) fn next<C>(commands: &Receiver<C>, every: Option<Duration>) -> Next<C> {
    let received = match every {
        Some(every) => commands.recv_timeout(every),
        None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
    };
    match received {
        Ok(command) => Next::Command(command),
        Err(RecvTimeoutError::Timeout) => Next::Elapsed,
        Err(RecvTimeoutError::Disconnected) => Next::Disconnected,
    }
}

/// A reading that is sent only when it differs from the last one sent.
#[derive(Debug)]
pub(crate) struct Changed<T> {
    last: Option<T>,
}

impl<T> Default for Changed<T> {
    fn default() -> Self {
        Changed { last: None }
    }
}

impl<T: PartialEq + Clone> Changed<T> {
    /// Returns `value` if it differs from the last one offered, and makes it the last. Otherwise `None`.
    pub(crate) fn offer(&mut self, value: T) -> Option<T> {
        if self.last.as_ref() == Some(&value) {
            return None;
        }
        self.last = Some(value.clone());
        Some(value)
    }

    /// Lets the next offer through whatever it is. This is used after a pause, so that what changed in
    /// the meantime is shown.
    pub(crate) fn forget(&mut self) {
        self.last = None;
    }
}

/// How a run with the service up ended, if not for good.
#[derive(Debug)]
pub(crate) enum Ended<E> {
    /// The UI is gone, so the thread ends.
    Closed,
    /// The service failed or went away.
    Lost(E),
}

impl<E> From<Closed> for Ended<E> {
    fn from(_: Closed) -> Self {
        Ended::Closed
    }
}

/// Lets `?` in a run end it as [`Ended::Lost`]. This works on the domain's error and on the other errors
/// listed, which are converted to it. For example `lost_on!(Error: dbus::Error, io::Error);` for
/// `Ended<Error>`, or `lost_on!(io::Error);`.
macro_rules! lost_on {
    ($domain:ty $(: $($from:ty),*)?) => {
        impl From<$domain> for $crate::os::worker::Ended<$domain> {
            fn from(e: $domain) -> Self {
                $crate::os::worker::Ended::Lost(e)
            }
        }
        $($(impl From<$from> for $crate::os::worker::Ended<$domain> {
            fn from(e: $from) -> Self {
                $crate::os::worker::Ended::Lost(e.into())
            }
        })*)?
    };
}
pub(crate) use lost_on;

/// A thread that talks to a service. It connects, runs while the service is up, and says what to do while
/// it is down. It is driven by [`supervise`].
pub(crate) trait Service {
    type Command;
    /// What a connection gives, such as the bus and its proxies or a grabbed device.
    type Session;
    type Error: Display;
    /// For the log.
    const NAME: &'static str;

    fn connect(&mut self) -> Result<Self::Session, Self::Error>;

    /// Runs with the service up until the service fails (`Err(Ended::Lost)`), the UI is gone
    /// (`Err(Ended::Closed)`) or every command sender is gone (`Ok`).
    fn run_while_up(&mut self, session: Self::Session, commands: &Receiver<Self::Command>) -> Result<(), Ended<Self::Error>>;

    /// Handles a command while the service is down, following the rule in the module docs.
    fn command_while_down(&mut self, command: Self::Command) -> Result<(), Closed>;

    /// Called when the service couldn't be reached, to tell the UI if it shows that.
    fn unavailable(&mut self, _error: &Self::Error) -> Result<(), Closed> {
        Ok(())
    }

    /// Called when the service was up and went away, to tell the UI if it shows that.
    fn lost(&mut self, error: &Self::Error) -> Result<(), Closed> {
        self.unavailable(error)
    }

    /// Whether this failure to connect is a normal state, such as there being nothing to connect to. Such
    /// failures are logged at debug only.
    fn expected(_error: &Self::Error) -> bool {
        false
    }

    /// Waits for the next command while the service is down, for at most `retry`. A service that has
    /// other things to answer in the meantime waits on those too.
    fn wait_while_down(&mut self, commands: &Receiver<Self::Command>, retry: Duration) -> Next<Self::Command> {
        next(commands, Some(retry))
    }
}

/// Runs `service` until the UI or every command sender is gone. It connects and runs while the service
/// is up. While the service is down, it waits `retry` or for a command before trying again. A panic while
/// connecting or running (the panic hook has logged it) is taken as the service lost: it is connected
/// again, so a thread doesn't end for good on one bad reply.
pub(crate) fn supervise<S: Service>(service: &mut S, commands: &Receiver<S::Command>, retry: Duration) {
    let mut quiet = Quiet::default();
    loop {
        let turn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_once(service, commands, &mut quiet)));
        let told = match turn {
            Ok(Some(told)) => told,
            Ok(None) => return,
            Err(_) => {
                log::error!("{}: panicked; connecting again", S::NAME);
                Ok(())
            }
        };
        if told.is_err() {
            return;
        }
        match service.wait_while_down(commands, retry) {
            Next::Command(command) => {
                if service.command_while_down(command).is_err() {
                    return;
                }
            }
            Next::Elapsed => {}
            Next::Disconnected => return,
        }
    }
}

/// Connects `service` and runs it while it is up. `None` when the thread is to end; else what telling the
/// UI the service is down gave.
fn run_once<S: Service>(service: &mut S, commands: &Receiver<S::Command>, quiet: &mut Quiet) -> Option<Result<(), Closed>> {
    Some(match service.connect() {
        Ok(session) => {
            quiet.ok();
            match service.run_while_up(session, commands) {
                Ok(()) | Err(Ended::Closed) => return None,
                Err(Ended::Lost(e)) => {
                    quiet.failed(S::NAME, &e);
                    service.lost(&e)
                }
            }
        }
        Err(e) => {
            if S::expected(&e) {
                log::debug!("{}: {e}", S::NAME);
            } else {
                quiet.failed(S::NAME, &e);
            }
            service.unavailable(&e)
        }
    })
}

/// Supervises a service that takes no commands and follows only the service's own signals or events.
pub(crate) fn supervise_alone<S: Service<Command = Infallible>>(service: &mut S, retry: Duration) {
    // Keep a sender for as long as this runs, so that waiting for a command means waiting `retry`.
    let (_keep, commands) = mpsc::channel();
    supervise(service, &commands, retry);
}

/// Warns about the first failure in a run and logs the rest at debug, so a service that stays away, or a
/// file that stays read-only, is mentioned once and not at every retry.
#[derive(Debug, Default)]
pub(crate) struct Quiet {
    warned: bool,
}

impl Quiet {
    pub(crate) fn failed(&mut self, context: &str, error: &dyn Display) {
        if self.warned {
            log::debug!("{context}: {error}");
        } else {
            log::warn!("{context}: {error}");
            self.warned = true;
        }
    }

    /// Records a success, so the next failure is worth a warning again.
    pub(crate) fn ok(&mut self) {
        self.warned = false;
    }
}

/// The first command and every command waiting behind it, keeping only the latest of each kind, in the
/// order the kinds first came. While a finger slides, requests come faster than they can be carried out.
pub(crate) fn coalesce<C, K: PartialEq>(first: C, commands: &Receiver<C>, kind: impl Fn(&C) -> K) -> Vec<C> {
    let mut kept: Vec<C> = Vec::new();
    for c in std::iter::once(first).chain(commands.try_iter()) {
        match kept.iter_mut().find(|k| kind(k) == kind(&c)) {
            Some(k) => *k = c,
            None => kept.push(c),
        }
    }
    kept
}

/// A thread that carries out commands as they come, coalescing them ([`coalesce`]) when several wait.
pub(crate) fn spawn_coalescing<C: Send + 'static, K: PartialEq + 'static>(
    name: &'static str,
    kind: fn(&C) -> K,
    mut carry_out: impl FnMut(C) + Send + 'static,
) -> io::Result<Worker<C>> {
    Worker::spawn(name, move |commands| {
        while let Ok(first) = commands.recv() {
            for command in coalesce(first, commands, kind) {
                carry_out(command);
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// A service whose first connection panics, then connects and ends.
    struct Panicky {
        connects: u32,
    }

    impl Service for Panicky {
        type Command = ();
        type Session = ();
        type Error = String;
        const NAME: &'static str = "panicky";

        fn connect(&mut self) -> Result<(), String> {
            self.connects += 1;
            if self.connects == 1 {
                panic!("a bad reply");
            }
            Ok(())
        }

        fn run_while_up(&mut self, _: (), _: &Receiver<()>) -> Result<(), Ended<String>> {
            Ok(())
        }

        fn command_while_down(&mut self, _: ()) -> Result<(), Closed> {
            Ok(())
        }
    }

    #[test]
    fn a_service_that_panics_is_connected_again() {
        let (_keep, commands) = mpsc::channel();
        let mut panicky = Panicky { connects: 0 };
        supervise(&mut panicky, &commands, Duration::from_millis(10));
        assert_eq!(panicky.connects, 2, "connected again after the panic, then ended as it was told");
    }

    #[test]
    fn a_reading_goes_out_when_it_changes() {
        let mut c = Changed::default();
        assert_eq!(c.offer(1), Some(1));
        assert_eq!(c.offer(1), None);
        assert_eq!(c.offer(2), Some(2));
        c.forget();
        assert_eq!(c.offer(2), Some(2));
    }

    #[test]
    fn waiting_for_commands() {
        let (tx, rx) = mpsc::channel();
        tx.send(5).unwrap();
        assert_eq!(next(&rx, None), Next::Command(5));
        assert_eq!(next(&rx, Some(Duration::from_millis(1))), Next::Elapsed);
        drop(tx);
        assert_eq!(next(&rx, None), Next::Disconnected);
    }

    #[test]
    fn only_the_latest_of_each_kind() {
        let (tx, rx) = mpsc::channel();
        for c in [(1, 'b'), (0, 'c'), (1, 'd'), (2, 'e')] {
            tx.send(c).unwrap();
        }
        assert_eq!(coalesce((0, 'a'), &rx, |c| c.0), vec![(0, 'c'), (1, 'd'), (2, 'e')]);
    }

    #[test]
    fn a_gone_worker_takes_no_commands() {
        let w: Worker<u8> = Worker::gone("test");
        assert_eq!(w.try_send(1), Err(Gone));
        let w = Worker::spawn("test", |rx: &Receiver<u8>| {
            let _ = rx.recv();
        })
        .unwrap();
        assert_eq!(w.try_send(1), Ok(()));
    }

    /// A service that is down twice, then up until its commands stop.
    struct Flaky {
        connects: u32,
        log: Vec<String>,
    }

    impl Service for Flaky {
        type Command = &'static str;
        type Session = ();
        type Error = String;
        const NAME: &'static str = "flaky";

        fn connect(&mut self) -> Result<(), String> {
            self.connects += 1;
            if self.connects <= 2 { Err(format!("down {}", self.connects)) } else { Ok(()) }
        }

        fn run_while_up(&mut self, (): (), commands: &Receiver<&'static str>) -> Result<(), Ended<String>> {
            while let Ok(c) = commands.recv() {
                self.log.push(format!("up: {c}"));
            }
            Ok(())
        }

        fn command_while_down(&mut self, command: &'static str) -> Result<(), Closed> {
            self.log.push(format!("down: {command}"));
            Ok(())
        }

        fn unavailable(&mut self, error: &String) -> Result<(), Closed> {
            self.log.push(error.clone());
            Ok(())
        }
    }

    #[test]
    fn a_service_is_tried_again_and_takes_commands_meanwhile() {
        let (tx, rx) = mpsc::channel();
        tx.send("watch").unwrap();
        let worker = thread::spawn(move || {
            let mut flaky = Flaky { connects: 0, log: Vec::new() };
            supervise(&mut flaky, &rx, Duration::from_millis(10));
            flaky.log
        });
        thread::sleep(Duration::from_millis(100));
        tx.send("join").unwrap();
        drop(tx);
        // The command sent while the service is down is taken and the service is tried again at once.
        // After the second failure the retry interval passes, and then the service is up.
        assert_eq!(worker.join().unwrap(), vec!["down 1", "down: watch", "down 2", "up: join"]);
    }

    #[test]
    fn a_panicking_thread_runs_again_at_once_then_after_growing_waits() {
        let started = std::time::Instant::now();
        let mut runs = Vec::new();
        restarted("test", || {
            runs.push(started.elapsed());
            if runs.len() < 3 {
                panic!("run {}", runs.len());
            }
        });
        let [_, second, third] = runs[..] else { panic!("{runs:?}") };
        assert!(second < RESTART_FIRST, "at once: {runs:?}");
        assert!(third.saturating_sub(second) >= RESTART_FIRST, "then after a wait: {runs:?}");
    }

    #[test]
    fn a_worker_keeps_its_commands_through_a_panic() {
        let (seen, saw) = mpsc::channel();
        let w = Worker::spawn("test", move |commands: &Receiver<u8>| {
            for command in commands {
                assert_ne!(command, 1, "the first panics");
                seen.send(command).unwrap();
            }
        })
        .unwrap();
        for command in [1, 2, 3] {
            w.send(command);
        }
        assert_eq!(saw.recv_timeout(Duration::from_secs(5)), Ok(2));
        assert_eq!(saw.recv_timeout(Duration::from_secs(5)), Ok(3));
    }
}
