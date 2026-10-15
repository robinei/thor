//! What every policy thread is made of. A thread is spawned with its command channel made inside
//! ([`Worker::spawn`]); it waits for the next command or for its period ([`next`]); it sends a reading
//! only when it changed ([`Changed`]); a thread that talks to a service (a bus, a socket, a device)
//! connects, runs while the service is up, and is supervised: tried again every `retry` while the
//! service is down ([`supervise`]).
//!
//! The rule for commands while a service is down: what a command sets (watching, a route) is kept and
//! applies once the service is back; what it asks to be done is refused at once ("unavailable"), never
//! queued. Either way the service is tried again right after the command, so opening a tab is a fresh
//! attempt.
//!
//! Logging failures: the first failure of a run of them is a warning, the retries are debug (see
//! [`Quiet`]); a failure that is a normal state (no controller plugged in) is always debug.

use std::convert::Infallible;
use std::fmt::Display;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::Closed;

/// A thread's command channel. Sending to a thread that has ended is logged once ("<name> is gone").
pub struct Worker<C> {
    name: &'static str,
    tx: Option<Sender<C>>,
    gone: Arc<AtomicBool>,
}

impl<C> Clone for Worker<C> {
    fn clone(&self) -> Self {
        Worker { name: self.name, tx: self.tx.clone(), gone: self.gone.clone() }
    }
}

/// A command reached no thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

impl<C: Send + 'static> Worker<C> {
    /// A thread named `name` running `body` with the receiving end of its command channel. The thread
    /// ends when `body` returns (usually when every sender is dropped).
    pub fn spawn(name: &'static str, body: impl FnOnce(Receiver<C>) + Send + 'static) -> io::Result<Worker<C>> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new().name(name.into()).spawn(move || body(rx))?;
        Ok(Worker { name, tx: Some(tx), gone: Arc::default() })
    }
}

impl<C> Worker<C> {
    /// Stands in for a thread that couldn't be started: commands go nowhere (logged once).
    pub fn gone(name: &'static str) -> Worker<C> {
        Worker { name, tx: None, gone: Arc::default() }
    }

    /// Sends a command; if the thread has ended, says so in the log (once).
    pub fn send(&self, command: C) {
        let _ = self.try_send(command);
    }

    /// Sends a command, or says the thread has ended (also in the log, once).
    pub fn try_send(&self, command: C) -> Result<(), Gone> {
        let sent = self.tx.as_ref().is_some_and(|tx| tx.send(command).is_ok());
        if sent {
            return Ok(());
        }
        if !self.gone.swap(true, Ordering::Relaxed) {
            log::warn!("{} is gone", self.name);
        }
        Err(Gone)
    }
}

/// Starts a thread without commands (it ends when the UI is gone).
pub fn detached(name: &str, body: impl FnOnce() + Send + 'static) -> io::Result<()> {
    thread::Builder::new().name(name.into()).spawn(body).map(drop)
}

/// What woke a thread waiting for its next command.
#[derive(Debug, PartialEq, Eq)]
pub enum Next<C> {
    Command(C),
    /// The period passed without one.
    Elapsed,
    /// Every sender is gone: the thread ends.
    Disconnected,
}

/// The next command, waiting at most `every` (`None`: until one comes).
pub fn next<C>(commands: &Receiver<C>, every: Option<Duration>) -> Next<C> {
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
pub struct Changed<T> {
    last: Option<T>,
}

impl<T> Default for Changed<T> {
    fn default() -> Self {
        Changed { last: None }
    }
}

impl<T: PartialEq + Clone> Changed<T> {
    /// As if `value` had been sent already.
    pub fn starting(value: T) -> Self {
        Changed { last: Some(value) }
    }

    /// `value` if it differs from the last one offered (it becomes the last), else `None`.
    pub fn offer(&mut self, value: T) -> Option<T> {
        if self.last.as_ref() == Some(&value) {
            return None;
        }
        self.last = Some(value.clone());
        Some(value)
    }

    /// The next offer goes through whatever it is (after a pause, so what changed meanwhile shows).
    pub fn forget(&mut self) {
        self.last = None;
    }
}

/// How a run with the service up ended, if not for good.
#[derive(Debug)]
pub enum Ended<E> {
    /// The UI is gone: the thread ends.
    Closed,
    /// The service failed or went away.
    Lost(E),
}

impl<E> From<Closed> for Ended<E> {
    fn from(_: Closed) -> Self {
        Ended::Closed
    }
}

/// Lets `?` in a run end it as [`Ended::Lost`] on the domain's error and on these others, converted to
/// it: `lost_on!(Error: zbus::Error, io::Error);` for `Ended<Error>`, or `lost_on!(io::Error);`.
macro_rules! lost_on {
    ($domain:ty $(: $($from:ty),*)?) => {
        impl From<$domain> for $crate::worker::Ended<$domain> {
            fn from(e: $domain) -> Self {
                $crate::worker::Ended::Lost(e)
            }
        }
        $($(impl From<$from> for $crate::worker::Ended<$domain> {
            fn from(e: $from) -> Self {
                $crate::worker::Ended::Lost(e.into())
            }
        })*)?
    };
}
pub(crate) use lost_on;

/// A thread that talks to a service: connects, runs while it is up, and says what to do while it is
/// down. Driven by [`supervise`].
pub trait Service {
    type Command;
    /// What a connection gives (the bus and its proxies, a grabbed device).
    type Session;
    type Error: Display;
    /// For the log.
    const NAME: &'static str;

    fn connect(&mut self) -> Result<Self::Session, Self::Error>;

    /// Runs with the service up until it fails (`Err(Ended::Lost)`), the UI is gone
    /// (`Err(Ended::Closed)`) or every command sender is (`Ok`).
    fn run_while_up(&mut self, session: Self::Session, commands: &Receiver<Self::Command>) -> Result<(), Ended<Self::Error>>;

    /// A command while the service is down (see the module's rule).
    fn command_while_down(&mut self, command: Self::Command) -> Result<(), Closed>;

    /// The service couldn't be reached: tell the UI, if it shows that.
    fn unavailable(&mut self, _error: &Self::Error) -> Result<(), Closed> {
        Ok(())
    }

    /// The service was up and went away: tell the UI, if it shows that.
    fn lost(&mut self, error: &Self::Error) -> Result<(), Closed> {
        self.unavailable(error)
    }

    /// Whether this failure to connect is a normal state (nothing to connect to), logged at debug only.
    fn expected(_error: &Self::Error) -> bool {
        false
    }
}

/// Runs `service` until the UI or every command sender is gone: connects, runs while it is up, and
/// while it is down waits `retry` (or for a command) before trying again.
pub fn supervise<S: Service>(service: &mut S, commands: &Receiver<S::Command>, retry: Duration) {
    let mut quiet = Quiet::default();
    loop {
        let told = match service.connect() {
            Ok(session) => {
                quiet.ok();
                match service.run_while_up(session, commands) {
                    Ok(()) | Err(Ended::Closed) => return,
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
        };
        if told.is_err() {
            return;
        }
        match next(commands, Some(retry)) {
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

/// Supervises a service that takes no commands (it follows the service's own signals or events).
pub fn supervise_alone<S: Service<Command = Infallible>>(service: &mut S, retry: Duration) {
    // A sender kept for as long as this runs: waiting for a command is then waiting `retry`.
    let (_keep, commands) = mpsc::channel();
    supervise(service, &commands, retry);
}

/// Warns about the first failure of a run of them and logs the rest at debug, so a service that stays
/// away (or a file that stays read-only) is mentioned once, not at every retry.
#[derive(Debug, Default)]
pub struct Quiet {
    warned: bool,
}

impl Quiet {
    pub fn failed(&mut self, context: &str, error: &dyn Display) {
        if self.warned {
            log::debug!("{context}: {error}");
        } else {
            log::warn!("{context}: {error}");
            self.warned = true;
        }
    }

    /// It worked: the next failure is worth a warning again.
    pub fn ok(&mut self) {
        self.warned = false;
    }
}

/// The first command and every one waiting behind it, keeping only the latest of each kind (in the order
/// the kinds first came): while a finger slides, requests come faster than they can be carried out.
pub fn coalesce<C, K: PartialEq>(first: C, commands: &Receiver<C>, kind: impl Fn(&C) -> K) -> Vec<C> {
    let mut kept: Vec<C> = Vec::new();
    for c in std::iter::once(first).chain(commands.try_iter()) {
        match kept.iter_mut().find(|k| kind(k) == kind(&c)) {
            Some(k) => *k = c,
            None => kept.push(c),
        }
    }
    kept
}

/// A thread that carries out commands as they come, coalesced ([`coalesce`]) when several wait.
pub fn spawn_coalescing<C: Send + 'static, K: PartialEq + 'static>(
    name: &'static str,
    kind: fn(&C) -> K,
    mut carry_out: impl FnMut(C) + Send + 'static,
) -> io::Result<Worker<C>> {
    Worker::spawn(name, move |commands| {
        while let Ok(first) = commands.recv() {
            for command in coalesce(first, &commands, kind) {
                carry_out(command);
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

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
        let w = Worker::spawn("test", |rx: Receiver<u8>| {
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
        // The command while down is taken and the service tried again at once; after the second
        // failure the retry interval passes; then it is up.
        assert_eq!(worker.join().unwrap(), vec!["down 1", "down: watch", "down 2", "up: join"]);
    }
}
