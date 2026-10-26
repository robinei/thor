//! A client is admitted once its process is identified off the compositor's thread (see
//! [`crate::clients::Identities`]): until then its requests wait in its socket, so the outputs it is shown
//! and the modes it is told are its game's from its first request.

use super::*;
use clamshell_state::settings::{Resolution, Setting};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use wayland_client::protocol::wl_output;

/// The modes an output was told, in order.
#[derive(Default)]
struct Modes(Mutex<Vec<(i32, i32)>>);

impl Dispatch<wl_output::WlOutput, Arc<Modes>> for Client {
    fn event(_: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, modes: &Arc<Modes>, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Mode { width, height, .. } = event {
            modes.0.lock().unwrap().push((width, height));
        }
    }
}

/// The server with the demo game `zelda-lttp` running at half the screen's size, each process
/// identified by `identify` with the game's unit.
fn running(identify: impl Fn(u32) -> Process + Send + Sync + 'static) -> (Display<Server>, Server) {
    let mut settings = clamshell_state::settings::Settings::default();
    settings.set(None, Setting::Resolution, Some(3));
    let state = State::new(Init { library: Some(clamshell_library::demo::library()), settings, ..Init::default() });
    let unit = clamshell_state::LaunchUnit { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 1 };
    let service = unit.service();
    let (display, mut server) = ServerBuilder::new().state(state).identify(move |pid| Process { unit: Some(service.clone()), ..identify(pid) }).started();
    server.core.handle(Event::Units(vec![unit]));
    (display, server)
}

fn told(server: &Server) -> Option<Resolution> {
    server.core.state().session().running_entries().iter().find(|r| &*r.id == "zelda-lttp").and_then(|r| r.told)
}

/// Turns the server as its event loop would until `done`, for at most `limit`.
fn turn_until(display: &mut Display<Server>, server: &mut Server, limit: Duration, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + limit;
    while !done() && Instant::now() < until {
        display.dispatch_clients(server).unwrap();
        server.frame();
        display.flush_clients().unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A game's client that asks for the globals and binds the outputs before its process is identified is
/// answered only after: it sees the top screen alone, told its game's mode after the screen's, and the
/// state is told the resolution it was given.
#[test]
fn a_games_client_is_answered_as_the_games_once_identified() {
    let (open, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let (mut display, mut server) = running(move |pid| {
        let _ = gate.lock().unwrap().recv();
        Process { pids: vec![pid], ..Process::default() }
    });
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    server.admit(server_socket);
    let (report, reports) = mpsc::channel();
    let client = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
        let qh = queue.handle();
        let modes: Vec<Arc<Modes>> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| {
                    let modes = Arc::new(Modes::default());
                    globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, 2, &qh, modes.clone());
                    modes
                })
                .collect()
        });
        queue.roundtrip(&mut Client::default()).unwrap();
        report.send(modes.iter().map(|m| m.0.lock().unwrap().clone()).collect::<Vec<_>>()).unwrap();
    });
    turn_until(&mut display, &mut server, Duration::from_millis(100), || false);
    assert_eq!(told(&server), None, "not identified yet");
    assert!(reports.try_recv().is_err(), "its requests wait");
    open.send(()).unwrap();
    let mut outputs = None;
    turn_until(&mut display, &mut server, Duration::from_secs(5), || {
        outputs = outputs.take().or_else(|| reports.try_recv().ok());
        outputs.is_some()
    });
    client.join().unwrap();
    assert_eq!(outputs, Some(vec![vec![(960, 540), (480, 270)]]), "the top screen alone, then its game's mode");
    assert_eq!(told(&server), Some(Resolution::Half));
}

/// A client gone before its process is identified is taken in and ends at once, and a client whose
/// process can't be read yet holds up no other.
#[test]
fn a_client_gone_or_stuck_while_identified_holds_up_nothing() {
    // The first process read is stuck until the test ends.
    let (_open, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let (mut display, mut server) = running(move |pid| {
        if called.fetch_add(1, Ordering::SeqCst) == 0 {
            let _ = gate.lock().unwrap().recv();
        }
        Process { pids: vec![pid], ..Process::default() }
    });
    let (stuck, _stuck_peer) = UnixStream::pair().unwrap();
    server.admit(stuck);
    let until = Instant::now() + Duration::from_secs(5);
    while calls.load(Ordering::SeqCst) == 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(1));
    }
    let (gone, gone_peer) = UnixStream::pair().unwrap();
    server.admit(gone);
    drop(gone_peer);
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    server.admit(server_socket);
    let client = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        registry_queue_init::<Client>(&connection).unwrap();
        connection
    });
    turn_until(&mut display, &mut server, Duration::from_secs(5), || client.is_finished());
    let _connection = client.join().unwrap();
    // The gone client's end is seen at the next dispatch.
    display.dispatch_clients(&mut server).unwrap();
    let mut clients = 0;
    server.dh.backend_handle().with_all_clients(|_| clients += 1);
    assert_eq!(clients, 1, "the client gone ended, and the stuck one isn't taken in yet");
    assert_eq!(told(&server), Some(Resolution::Half));
}
