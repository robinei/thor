//! ClamShell's X11 window manager, inside the compositor: it finds Xwayland's windows, reads their
//! properties, pairs each with its Wayland surface, configures each to the place the state gives it, sets
//! the focus and closes them, and turns them into the state's window facts. It never waits on Xwayland:
//! requests are pipelined and each reply handled when it comes (the transport decides how long Xwayland
//! may take).

use super::{
    properties::{Atoms, NAMES, PROPERTIES, Properties},
    transport::Transport,
};
use crate::{Process, server::Server, surface::Surface, windows::ClientWindow};
use clamshell_state::{WindowEvent, presentation::WindowLayout};
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
};
use std::{borrow::Cow, io, time::Instant};
use x11rb_protocol::{
    id_allocator::IdAllocator,
    protocol::{composite, randr, res, xproto::*},
    x11_utils::{Request, Serialize, TryParse},
};

/// The most top-level windows Xwayland's clients may have: a bound on what a runaway client can make the
/// window manager hold. Programs keep many unmapped ones (Steam has dozens), so it is far above what any
/// shows; the client whose window goes over it is ended.
const WINDOWS_MAX: usize = 4096;

mod events;
mod settle;
mod setup;

struct Pending {
    sequence: u64,
    kind: Query,
}
enum Query {
    Atom(&'static str),
    Extension,
    CompositeExtension,
    CompositeVersion,
    Version,
    Property(i64, &'static str),
    Pid(i64),
    Attributes(i64),
    Geometry(i64),
    OwnershipBarrier,
    RandrExtension,
    RandrVersion,
    RandrModes,
}
/// The server time for a focus change. The X server ignores a focus request older than the last focus
/// change, which an active client may have made itself, so each change waits for a fresh time: an empty
/// append to a property of our own window, whose PropertyNotify carries the current time (ICCCM 2.1).
#[derive(Default)]
enum Stamp {
    #[default]
    None,
    Requested(Instant),
    Fresh(u32),
}
struct Window {
    xid: u32,
    id: i64,
    override_redirect: bool,
    position: Point<i32, Logical>,
    popup_parent: Option<i64>,
    properties: Properties,
    pid: Option<u32>,
    /// Its process, once identified (see [`crate::clients::Identities`]). Only a managed window's is asked
    /// for, once its other answers are in, and waited for as one more outstanding answer: a menu or
    /// tooltip is shown by its process id alone, and doesn't wait.
    process: Option<Process>,
    outstanding: usize,
    wants_map: bool,
    queried: bool,
    input_only: bool,
    layout: Option<WindowLayout>,
    opened: bool,
    /// The serial Xwayland gave its surface (see `super::shell`), and the surface once it is committed.
    serial: Option<u64>,
    surface: Option<WlSurface>,
    size: (u16, u16),
    /// Whether its size is being asked for (a window reparented to the root: ReparentNotify carries none),
    /// until the answer, or a size the client asks for, comes.
    measuring: bool,
    configured: Option<Rectangle<i32, smithay::utils::Logical>>,
    /// The parent its facts were last given with: it is looked up again at each settle, until it opens.
    parent: Option<i64>,
    dirty: bool,
    refresh: u16,
    /// How the state shows it, and the state's revision that was read at (see [`crate::windows::Shown`]):
    /// read again only when the state changed, as a Wayland window's is.
    shown: Option<(u64, crate::windows::Shown)>,
}
#[derive(Default)]
pub(super) struct Wm {
    atoms: Atoms,
    pending: Vec<Pending>,
    windows: Vec<Window>,
    started: bool,
    owner: u32,
    root: u32,
    res: Option<u8>,
    extension_done: bool,
    composite: Option<u8>,
    composite_done: bool,
    /// RandR's opcode, for the modes.
    randr: Option<u8>,
    pub ready: bool,
    focus: Option<u32>,
    stamp: Stamp,
    selection: super::selection::Bridge,
}
impl Wm {
    pub fn deadline(&self) -> Option<Instant> {
        let stamp = match self.stamp {
            Stamp::Requested(deadline) => Some(deadline),
            _ => None,
        };
        self.selection.deadline().into_iter().chain(stamp).min()
    }
    fn query(&mut self, wire: &mut Transport, request: impl Request, opcode: u8, kind: Query) -> io::Result<()> {
        if self.pending.len() >= 256 {
            return Err(io::Error::other("too many pending XWM replies"));
        }
        let sequence = wire.request(request, opcode, true)?;
        self.pending.push(Pending { sequence, kind });
        Ok(())
    }
    pub fn turn(&mut self, wire: &mut Transport, server: &mut Server, now: Instant) -> io::Result<()> {
        self.start_setup(wire)?;
        self.dispatch_replies(wire, server, now)?;
        self.finish_setup(wire)?;
        self.anchor_sequence(wire)?;
        self.dispatch_events(wire, server)?;
        self.query_window_properties(wire)?;
        self.sync_selection(wire, server, now)?;
        self.take_identified(server);
        self.settle(wire, server)?;
        // Order: the menus are placed by their windows, which what came from X11 may have moved. (The
        // server's turn, which follows, syncs the windows.)
        server.place();
        self.settle_menus(wire, server)?;
        Ok(())
    }
    /// The windows' processes identified since the last turn.
    fn take_identified(&mut self, server: &mut Server) {
        for (id, process) in server.x11.identified.drain(..) {
            if let Some(window) = self.windows.iter_mut().find(|w| w.id == id) {
                window.process = Some(process);
                window.outstanding = window.outstanding.saturating_sub(1);
            }
        }
    }
    fn dispatch_replies(&mut self, wire: &mut Transport, server: &mut Server, now: Instant) -> io::Result<()> {
        let mut index = 0;
        while index < self.pending.len() {
            let Some(pending) = self.pending.get(index) else { break };
            if let Some(bytes) = wire.reply(pending.sequence) {
                let pending = self.pending.remove(index);
                self.reply(pending.kind, &bytes, wire, server)?;
            } else {
                index += 1;
            }
        }
        if let Stamp::Requested(deadline) = self.stamp
            && now >= deadline
        {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "X11 server time for focus timed out"));
        }
        Ok(())
    }
    fn dispatch_events(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        for _ in 0..64 {
            let Some(bytes) = wire.event() else { break };
            self.event(&bytes, wire, server)?;
        }
        Ok(())
    }
    fn sync_selection(&mut self, wire: &mut Transport, server: &mut Server, now: Instant) -> io::Result<()> {
        if self.ready {
            self.selection.init_imports(wire, self.owner)?;
            self.selection.turn(wire, &self.atoms, self.owner, server, now)?;
        }
        Ok(())
    }
    fn query_window_properties(&mut self, wire: &mut Transport) -> io::Result<()> {
        for i in 0..self.windows.len() {
            self.query_initial_properties(i, wire)?;
            self.query_changed_properties(i, wire)?;
        }
        Ok(())
    }
    fn query_initial_properties(&mut self, i: usize, wire: &mut Transport) -> io::Result<()> {
        let Some(window) = self.windows.get(i) else { return Ok(()) };
        if window.wants_map && !window.queried && self.pending.len() + PROPERTIES.len() + 2 <= 256 {
            let (id, xid) = (window.id, window.xid);
            self.query(wire, GetWindowAttributesRequest { window: xid }, 0, Query::Attributes(id))?;
            for name in PROPERTIES {
                self.query(
                    wire,
                    GetPropertyRequest { delete: false, window: xid, property: self.atoms.get(name), type_: 0, long_offset: 0, long_length: 1024 },
                    0,
                    Query::Property(id, name),
                )?;
            }
            self.query(
                wire,
                res::QueryClientIdsRequest { specs: vec![res::ClientIdSpec { client: xid, mask: res::ClientIdMask::LOCAL_CLIENT_PID }].into() },
                self.res.unwrap_or_default(),
                Query::Pid(id),
            )?;
            if let Some(window) = self.windows.get_mut(i) {
                window.queried = true;
                window.refresh = 0;
            }
        }
        Ok(())
    }
    fn query_changed_properties(&mut self, i: usize, wire: &mut Transport) -> io::Result<()> {
        for (bit, name) in PROPERTIES.iter().enumerate() {
            let Some(window) = self.windows.get_mut(i) else { continue };
            if !window.queried
                || self.pending.len() >= 256
                || window.refresh & (1 << bit) == 0
                || self.pending.iter().any(|p| matches!(p.kind, Query::Property(id, n) if id == window.id && n == *name))
            {
                continue;
            }
            window.refresh &= !(1 << bit);
            let (id, xid) = (window.id, window.xid);
            self.query(
                wire,
                GetPropertyRequest { delete: false, window: xid, property: self.atoms.get(name), type_: 0, long_offset: 0, long_length: 1024 },
                0,
                Query::Property(id, name),
            )?;
        }
        Ok(())
    }
    pub fn sync_selection_sources(
        &self,
        handle: &smithay::reexports::calloop::LoopHandle<'static, Server>,
        sources: &mut Vec<super::selection::IoSource>,
    ) -> io::Result<()> {
        self.selection.sync_sources(handle, sources)
    }
    pub fn clear(&mut self, server: &mut Server) {
        self.selection.clear(server);
        while let Some(window) = self.windows.last() {
            let id = window.id;
            self.forget(id, server);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    #[test]
    fn outstanding_deadlines_stale_replies_and_menu_ownership_follow_window_lifetimes() {
        let (_display, mut server) = crate::tests::ServerBuilder::new().build();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut wire = Transport::new(socket, now).unwrap();
        let mut wm = Wm::default();
        assert_eq!(wm.deadline(), None);
        wm.query(&mut wire, GetWindowAttributesRequest { window: 10 }, 0, Query::Attributes(7)).unwrap();
        // How long its answer may take is the transport's to say (see `transport`).
        assert_eq!(wm.deadline(), None);
        wm.turn(&mut wire, &mut server, now + Duration::from_secs(3)).expect("a late reply is waited for");
        wm.turn(&mut wire, &mut server, now + crate::x11::HUNG).expect("the WM keeps no deadline of its own");
        wm.pending.clear();
        wm.discover(10, Rectangle::from_size((640, 360).into()), false, &mut wire, &mut server).unwrap();
        let old_id = wm.windows[0].id;
        wm.forget(old_id, &mut server);
        wm.discover(10, Rectangle::from_size((640, 360).into()), false, &mut wire, &mut server).unwrap();
        assert_ne!(wm.windows[0].id, old_id);
        wm.res = Some(140);
        wm.turn(&mut wire, &mut server, now).unwrap();
        assert!(wm.pending.is_empty(), "creation alone does not snapshot properties before the app sets them");
        let event = MapRequestEvent { response_type: MAP_REQUEST_EVENT, parent: 1, window: 10, ..MapRequestEvent::default() };
        let mut event = Serialize::serialize(&event).to_vec();
        event.resize(32, 0);
        wm.event(&event, &mut wire, &mut server).unwrap();
        wm.turn(&mut wire, &mut server, now).unwrap();
        assert_eq!(wm.pending.len(), PROPERTIES.len() + 2, "MapRequest starts the attributes, properties and PID snapshot");
        assert!(server.core.state().session().windows().is_empty(), "the initial facts precede placement");

        // An old window's reply cannot change another window, even if X11 reuses its resource id.
        let mut bytes = Serialize::serialize(&GetPropertyReply {
            format: 8,
            type_: AtomEnum::STRING.into(),
            value: b"stale".to_vec(),
            value_len: 5,
            length: 2,
            ..GetPropertyReply::default()
        });
        bytes.resize(40, 0);
        wm.reply(Query::Property(old_id, "WM_NAME"), &bytes, &mut wire, &mut server).unwrap();
        assert_eq!(wm.windows.len(), 1);
        assert!(wm.windows[0].properties.name.is_empty());
        assert_eq!(server.core.state().session().windows().len(), 0);

        // Override-redirect ownership follows live transient ancestors; cached ownership never
        // transfers a mapped menu to a different focused client.
        for (xid, redirect) in [(20, false), (30, true), (40, true)] {
            wm.discover(xid, Rectangle::from_size((100, 100).into()), redirect, &mut wire, &mut server).unwrap();
        }
        for window in &mut wm.windows {
            window.pid = Some(123);
            window.wants_map = true;
        }
        let parent = wm.windows[0].id;
        let other = wm.windows[1].id;
        assert_eq!(wm.popup_owner(&wm.windows[2], Some(parent), None), Some(parent));
        wm.windows[2].pid = Some(456);
        assert_eq!(wm.popup_owner(&wm.windows[2], Some(parent), None), None);
        wm.windows[2].pid = Some(123);
        wm.windows[2].properties.tooltip = true;
        assert_eq!(wm.popup_owner(&wm.windows[2], Some(parent), Some(other)), Some(other));
        wm.windows[2].properties.transient = Some(10);
        wm.windows[3].properties.transient = Some(30);
        wm.windows[3].popup_parent = Some(parent);
        assert_eq!(wm.popup_owner(&wm.windows[3], Some(other), None), Some(parent));
        wm.windows[2].wants_map = false;
        assert_eq!(wm.popup_owner(&wm.windows[3], Some(parent), None), None);
        wm.windows[2].wants_map = true;
        wm.windows[2].properties.transient = Some(40);
        assert_eq!(wm.popup_owner(&wm.windows[3], Some(parent), None), None, "transient cycles are bounded");
        wm.windows[2].properties.transient = Some(20);
        assert_eq!(wm.popup_owner(&wm.windows[3], Some(other), None), None, "a mapped menu cannot change owner");
    }
    /// A window created override-redirect that clears it before mapping asks us to map it, and is
    /// managed: mapped once its facts are in, rather than left waiting as a menu would.
    #[test]
    fn a_window_no_longer_override_redirect_is_managed() {
        let (_display, mut server) = crate::tests::ServerBuilder::new().build();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut wire = Transport::new(socket, Instant::now()).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        wm.discover(10, Rectangle::from_size((400, 300).into()), true, &mut wire, &mut server).unwrap();
        let event = MapRequestEvent { response_type: MAP_REQUEST_EVENT, parent: 1, window: 10, ..MapRequestEvent::default() };
        let mut event = Serialize::serialize(&event).to_vec();
        event.resize(32, 0);
        wm.event(&event, &mut wire, &mut server).unwrap();
        assert!(wm.windows[0].wants_map && !wm.windows[0].override_redirect);
    }
    /// An override-redirect window with no owner is managed as a splash when its program has no managed
    /// window, and left mapped, to be shown once its program has the focus, when it has; one whose owner
    /// lost the focus is dismissed.
    #[test]
    fn an_ownerless_override_redirect_window_is_a_splash_or_waits() {
        let (_display, mut server) = crate::tests::ServerBuilder::new().build();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut wire = Transport::new(socket, Instant::now()).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        for (xid, redirect) in [(10, true), (20, true), (30, false)] {
            wm.discover(xid, Rectangle::from_size((100, 100).into()), redirect, &mut wire, &mut server).unwrap();
        }
        for (w, pid) in wm.windows.iter_mut().zip([1, 2, 2]) {
            (w.pid, w.outstanding, w.wants_map) = (Some(pid), 0, true);
        }
        wm.settle_menus(&mut wire, &mut server).unwrap();
        let splash = &wm.windows[0];
        assert!(!splash.override_redirect && splash.properties.role == clamshell_state::presentation::WindowRole::Splash);
        let menu = &wm.windows[1];
        assert!(menu.override_redirect && menu.wants_map, "its program has a window: waiting for it to have the focus");
        wm.windows[1].popup_parent = Some(99);
        wm.settle_menus(&mut wire, &mut server).unwrap();
        assert!(!wm.windows[1].wants_map, "its owner gone: dismissed");
    }
    /// A window reparented into the root is found, and its size asked for (ReparentNotify carries none),
    /// unless the client asks for one first; one reparented out of it is forgotten.
    #[test]
    fn reparenting_finds_and_forgets_top_level_windows() {
        let (_display, mut server) = crate::tests::ServerBuilder::new().build();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut wire = Transport::new(socket, Instant::now()).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        let event = |event: &dyn Fn() -> Vec<u8>| {
            let mut bytes = event();
            bytes.resize(32, 0);
            bytes
        };
        let reparent = |parent| {
            event(&|| {
                Serialize::serialize(&ReparentNotifyEvent {
                    response_type: REPARENT_NOTIFY_EVENT,
                    event: 1,
                    window: 10,
                    parent,
                    ..ReparentNotifyEvent::default()
                })
                .to_vec()
            })
        };
        let geometry = |width, height| {
            let mut bytes = Serialize::serialize(&GetGeometryReply { width, height, ..GetGeometryReply::default() }).to_vec();
            bytes.resize(32, 0);
            bytes[0] = 1;
            bytes
        };
        wm.event(&reparent(1), &mut wire, &mut server).unwrap();
        assert_eq!(wm.windows.iter().map(|w| w.xid).collect::<Vec<_>>(), [10]);
        assert!(matches!(wm.pending.as_slice(), [Pending { kind: Query::Geometry(_), .. }]));
        assert_eq!(wm.windows[0].outstanding, PROPERTIES.len() + 3, "waits for its size too");
        let id = wm.windows[0].id;
        wm.reply(Query::Geometry(id), &geometry(640, 480), &mut wire, &mut server).unwrap();
        assert_eq!((wm.windows[0].size, wm.windows[0].outstanding), ((640, 480), PROPERTIES.len() + 2));
        wm.event(&reparent(50), &mut wire, &mut server).unwrap();
        assert!(wm.windows.is_empty());
        // A size the client asks for before the answer is newer than the answer.
        wm.event(&reparent(1), &mut wire, &mut server).unwrap();
        let id = wm.windows[0].id;
        let configure = ConfigureRequestEvent {
            response_type: CONFIGURE_REQUEST_EVENT,
            window: 10,
            width: 800,
            height: 600,
            value_mask: ConfigWindow::WIDTH | ConfigWindow::HEIGHT,
            ..ConfigureRequestEvent::default()
        };
        wm.event(&event(&|| Serialize::serialize(&configure).to_vec()), &mut wire, &mut server).unwrap();
        wm.reply(Query::Geometry(id), &geometry(640, 480), &mut wire, &mut server).unwrap();
        assert_eq!(wm.windows[0].size, (800, 600));
    }
    /// A client may put a window in front only while its program has the focus, and only its program's.
    #[test]
    fn only_the_focused_program_activates_a_window() {
        let (_display, mut server) = crate::tests::ServerBuilder::new().build();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut wire = Transport::new(socket, Instant::now()).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        for (xid, pid) in [(10, 1), (20, 1), (30, 2)] {
            wm.discover(xid, Rectangle::from_size((400, 300).into()), false, &mut wire, &mut server).unwrap();
            let w = wm.windows.last_mut().unwrap();
            w.pid = Some(pid);
            w.opened = true;
        }
        assert!(!wm.may_activate(&wm.windows[1]), "nothing focused");
        wm.focus = Some(10);
        assert!(wm.may_activate(&wm.windows[1]), "the focused program's own window");
        // Another program's, whatever the message names as the asker's active window: that is the
        // asker's to write, and the focused window's id is public.
        assert!(!wm.may_activate(&wm.windows[2]), "another program's");
    }
    /// A window opens only once its process is identified, off the compositor's thread, so its facts carry
    /// its unit and processes; a window gone before the answer is forgotten with it, and one whose client
    /// has no process id doesn't wait.
    #[test]
    fn a_window_opens_once_its_process_is_identified() {
        let identify = |pid| Process { unit: Some("app-x11.scope".into()), pids: vec![pid, 1000], game: None };
        let (_display, mut server) = crate::tests::ServerBuilder::new().identify(identify).build();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut wire = Transport::new(socket, now).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        let pid_reply = |pid: Option<u32>| {
            let ids = pid
                .map(|pid| res::ClientIdValue { spec: res::ClientIdSpec { client: 10, mask: res::ClientIdMask::LOCAL_CLIENT_PID }, value: vec![pid] })
                .into_iter()
                .collect();
            let length = if pid.is_some() { 4 } else { 0 };
            Serialize::serialize(&res::QueryClientIdsReply { sequence: 0, length, ids })
        };
        for xid in [10, 20, 30] {
            wm.discover(xid, Rectangle::from_size((400, 300).into()), false, &mut wire, &mut server).unwrap();
        }
        for window in &mut wm.windows {
            // Only the process id's answer outstanding.
            window.outstanding = 1;
            window.wants_map = true;
        }
        let [waits, gone, no_pid] = [0, 1, 2].map(|i| wm.windows[i].id);
        wm.reply(Query::Pid(waits), &pid_reply(Some(4321)), &mut wire, &mut server).unwrap();
        wm.reply(Query::Pid(gone), &pid_reply(Some(4322)), &mut wire, &mut server).unwrap();
        wm.reply(Query::Pid(no_pid), &pid_reply(None), &mut wire, &mut server).unwrap();
        wm.settle(&mut wire, &mut server).unwrap();
        let session = server.core.state().session();
        assert!(session.window(waits).is_none() && session.window(gone).is_none(), "waiting for their processes");
        assert_eq!(session.window(no_pid).map(|w| w.unit.clone()), Some(None), "nothing to wait for");
        wm.forget(gone, &mut server);
        let until = Instant::now() + Duration::from_secs(5);
        while server.x11.identified.len() < 2 && Instant::now() < until {
            server.take_identified();
            std::thread::sleep(Duration::from_millis(1));
        }
        wm.take_identified(&mut server);
        wm.settle(&mut wire, &mut server).unwrap();
        let window = server.core.state().session().window(waits).unwrap();
        assert_eq!((window.unit.as_deref(), window.pids.clone()), (Some("app-x11.scope"), vec![4321, 1000]));
        assert!(server.core.state().session().window(gone).is_none(), "forgotten with its answer");
        assert_eq!(wm.windows.len(), 2);
    }
    /// A menu's process is never asked for: it is shown by its process id alone, so a process whose read
    /// is stuck holds up none of its menus. One promoted to a managed window (a splash) asks then, and
    /// opens once its process is identified.
    #[test]
    fn a_menu_doesnt_wait_for_its_process_and_a_promoted_one_does() {
        use std::sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        };
        let (open, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        let identify = move |pid| {
            called.fetch_add(1, Ordering::SeqCst);
            let _ = gate.lock().unwrap().recv();
            Process { unit: Some("splash.scope".into()), pids: vec![pid], game: None }
        };
        let (_display, mut server) = crate::tests::ServerBuilder::new().identify(identify).build();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut wire = Transport::new(socket, now).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        // A menu of program 7, which has a managed window (already identified), and a window of program 9
        // alone, a splash.
        for (xid, redirect) in [(10, true), (20, true), (30, false)] {
            wm.discover(xid, Rectangle::from_size((100, 100).into()), redirect, &mut wire, &mut server).unwrap();
        }
        for w in &mut wm.windows {
            // Only the process id's answer outstanding.
            (w.outstanding, w.wants_map) = (1, true);
        }
        wm.windows[2].process = Some(Process::default());
        let [menu, splash, managed] = [0, 1, 2].map(|i| wm.windows[i].id);
        for (id, pid) in [(menu, 7), (splash, 9), (managed, 7)] {
            let ids = vec![res::ClientIdValue { spec: res::ClientIdSpec { client: 10, mask: res::ClientIdMask::LOCAL_CLIENT_PID }, value: vec![pid] }];
            let reply = Serialize::serialize(&res::QueryClientIdsReply { sequence: 0, length: 4, ids });
            wm.reply(Query::Pid(id), &reply, &mut wire, &mut server).unwrap();
        }
        wm.settle(&mut wire, &mut server).unwrap();
        wm.settle_menus(&mut wire, &mut server).unwrap();
        assert!(wm.windows[0].override_redirect && wm.windows[0].outstanding == 0, "the menu waits for nothing");
        assert!(!wm.windows[1].override_redirect, "promoted");
        wm.settle(&mut wire, &mut server).unwrap();
        assert_eq!(wm.windows[1].outstanding, 1, "it waits for its process");
        assert!(server.core.state().session().window(splash).is_none());
        open.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while server.x11.identified.is_empty() && Instant::now() < until {
            server.take_identified();
            std::thread::sleep(Duration::from_millis(1));
        }
        wm.take_identified(&mut server);
        wm.settle(&mut wire, &mut server).unwrap();
        let window = server.core.state().session().window(splash).unwrap();
        assert_eq!((window.unit.as_deref(), window.pids.clone()), (Some("splash.scope"), vec![9]));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "only the splash's process was read");
        assert!(server.core.state().session().window(menu).is_none(), "the menu is no window of the state's");
    }

    /// Xwayland gives a window's surface a 64-bit serial: through `xwayland_shell_v1` on the Wayland side,
    /// and in `WL_SURFACE_SERIAL` as two 32-bit halves, low first, on the X11 side. The window is paired
    /// with the surface committed with that serial, once both have come.
    #[test]
    fn a_window_is_paired_with_the_surface_committed_with_its_serial() {
        use smithay::reexports::wayland_protocols::xwayland::shell::v1::client::xwayland_shell_v1;
        use wayland_client::{Proxy, protocol::wl_compositor};
        let (mut display, mut server) = crate::tests::ServerBuilder::new().build();
        let xwayland = crate::clients::ClientState { xwayland: true, ..crate::clients::ClientState::default() };
        let client = crate::tests::spawn_client(&mut display, xwayland, |globals, mut queue, steps| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
            let shell: xwayland_shell_v1::XwaylandShellV1 = globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let role = shell.get_xwayland_surface(&surface, &qh, ());
            role.set_serial(7, 1);
            surface.commit();
            queue.roundtrip(&mut crate::tests::Client::default()).unwrap();
            steps.step(surface.id().protocol_id());
        });
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut wire = Transport::new(socket, Instant::now()).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        wm.discover(10, Rectangle::from_size((400, 300).into()), false, &mut wire, &mut server).unwrap();
        let peer = client.peer.clone();
        let checked = client.serve(
            &mut display,
            &mut server,
            |_| {},
            |_, server, id| {
                let surface: WlSurface = peer.object_from_protocol_id(&server.dh, id).unwrap();
                wm.pair_surfaces(server);
                assert!(wm.windows[0].surface.is_none(), "no serial for the window yet");
                let message = <[u8; 32]>::from(&ClientMessageEvent::new(32, 10, wm.atoms.get("WL_SURFACE_SERIAL"), [7, 1, 0, 0, 0]));
                // One another client sent (SendEvent's bit) is ignored.
                let mut forged = message;
                forged[0] |= 0x80;
                wm.event(&forged, &mut wire, server).unwrap();
                assert_eq!(wm.windows[0].serial, None, "a forged serial is ignored");
                wm.event(&message, &mut wire, server).unwrap();
                assert_eq!(wm.windows[0].serial, Some(0x1_0000_0007));
                wm.pair_surfaces(server);
                assert_eq!(wm.windows[0].surface.as_ref(), Some(&surface));
                assert!(super::super::shell::take(server, 0x1_0000_0007).is_none(), "taken once");
            },
        );
        assert_eq!(checked, 1);
    }

    /// An X11 window's colour is its surface's, as a Wayland window's is: told with its next facts once its
    /// surface is bound.
    #[test]
    fn an_x11_window_reports_its_surfaces_colour() {
        use smithay::reexports::wayland_protocols::xwayland::shell::v1::client::xwayland_shell_v1;
        use wayland_client::{Proxy, protocol::wl_compositor};
        let (mut display, mut server) = crate::tests::ServerBuilder::new().build();
        let xwayland = crate::clients::ClientState { xwayland: true, ..crate::clients::ClientState::default() };
        let client = crate::tests::spawn_client(&mut display, xwayland, |globals, mut queue, steps| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
            let shell: xwayland_shell_v1::XwaylandShellV1 = globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            shell.get_xwayland_surface(&surface, &qh, ()).set_serial(7, 0);
            surface.commit();
            queue.roundtrip(&mut crate::tests::Client::default()).unwrap();
            steps.step(surface.id().protocol_id());
        });
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut wire = Transport::new(socket, Instant::now()).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        wm.discover(10, Rectangle::from_size((400, 300).into()), false, &mut wire, &mut server).unwrap();
        (wm.windows[0].outstanding, wm.windows[0].wants_map, wm.windows[0].serial) = (0, true, Some(7));
        let id = wm.windows[0].id;
        let peer = client.peer.clone();
        let colour = clamshell_state::colour::ColourDescription { transfer: clamshell_state::colour::TransferFunction::Pq, ..Default::default() };
        let checked = client.serve(
            &mut display,
            &mut server,
            |_| {},
            |_, server, surface| {
                let surface: WlSurface = peer.object_from_protocol_id(&server.dh, surface).unwrap();
                wm.settle(&mut wire, server).unwrap();
                assert_eq!(server.core.state().session().window(id).map(|w| w.colour.clone()), Some(None), "opened before its surface");
                server.colour.surfaces.insert(smithay::backend::renderer::element::Id::from_wayland_resource(&surface), std::sync::Arc::new(colour));
                wm.windows[0].dirty = true;
                wm.settle(&mut wire, server).unwrap();
                server.frame();
                assert_eq!(server.core.state().session().window(id).and_then(|w| w.colour.as_deref().copied()), Some(colour));
            },
        );
        assert_eq!(checked, 1);
    }

    /// The loop runs the window manager's turn after its dispatch and then the server's (see `Base::run`): a
    /// window the window manager opens is configured on both sides in that turn, and a turn after with
    /// nothing new from Xwayland places and configures nothing.
    #[test]
    fn a_window_opened_by_the_wm_is_configured_in_the_same_turn() {
        let (_display, mut server) = crate::tests::ServerBuilder::new().started();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut wire = Transport::new(socket, now).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        wm.discover(10, Rectangle::from_size((400, 300).into()), false, &mut wire, &mut server).unwrap();
        (wm.windows[0].outstanding, wm.windows[0].wants_map) = (0, true);
        let id = wm.windows[0].id;
        let configures = server.configures;
        wm.turn(&mut wire, &mut server, now).unwrap();
        server.turn();
        assert!(server.core.state().session().window(id).is_some(), "opened");
        assert!(wm.windows[0].configured.is_some(), "configured on the X11 side");
        assert!(server.configures > configures, "and on the server's");
        let counts = (server.scene.placements, server.configures);
        for _ in 0..3 {
            wm.turn(&mut wire, &mut server, now).unwrap();
            server.turn();
        }
        assert_eq!((server.scene.placements, server.configures), counts, "an idle turn");
    }

    /// A window manager with one X11 window (see [`placed`]): the window's id, Xwayland's end of the
    /// connection, and the requests sent so far.
    struct Placed {
        _display: smithay::reexports::wayland_server::Display<Server>,
        server: Server,
        wm: Wm,
        wire: Transport,
        peer: UnixStream,
        id: i64,
        sent: Vec<u8>,
    }

    /// One X11 window (10, of the class `fixture`), discovered and settled: placed by the state and mapped.
    fn placed() -> Placed {
        use std::io::Read;
        let (display, mut server) = crate::tests::ServerBuilder::new().build();
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut wire = Transport::new(socket, now).unwrap();
        wire.turn(now).unwrap();
        peer.read_exact(&mut [0; 12]).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        wm.discover(10, Rectangle::from_size((400, 300).into()), false, &mut wire, &mut server).unwrap();
        wm.windows[0].outstanding = 0;
        wm.windows[0].wants_map = true;
        wm.windows[0].properties.class = "fixture".into();
        let id = wm.windows[0].id;
        wm.settle(&mut wire, &mut server).unwrap();
        wire.turn(now).unwrap();
        let mut bytes = [0; 4096];
        let length = peer.read(&mut bytes).unwrap();
        Placed { _display: display, server, wm, wire, peer, id, sent: bytes[..length].to_vec() }
    }

    /// The server's time, `time`, as the PropertyNotify of the window manager's own window brings it.
    fn notify(wm: &mut Wm, server: &mut Server, wire: &mut Transport, time: u32) {
        let event = PropertyNotifyEvent {
            response_type: PROPERTY_NOTIFY_EVENT,
            sequence: 0,
            window: 2,
            atom: wm.atoms.get("_THOR_TIMESTAMP"),
            time,
            state: Property::NEW_VALUE,
        };
        wm.event(&<[u8; 32]>::from(&event), wire, server).unwrap();
    }

    #[test]
    fn placement_is_configured_before_mapping_and_focus_waits_for_the_servers_time() {
        let Placed { server, id, sent, .. } = placed();
        assert!(server.core.state().session().window(id).is_some());
        assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(id));
        let mut packets = sent.as_slice();
        let mut requests = Vec::new();
        while !packets.is_empty() {
            requests.push(packets[0]);
            let size = usize::from(u16::from_ne_bytes([packets[2], packets[3]])) * 4;
            packets = &packets[size..];
        }
        // The X server must receive final geometry before it can map or focus the new window.
        assert_eq!(
            requests,
            vec![
                CHANGE_WINDOW_ATTRIBUTES_REQUEST,
                CONFIGURE_WINDOW_REQUEST,
                CHANGE_PROPERTY_REQUEST,
                CHANGE_PROPERTY_REQUEST,
                MAP_WINDOW_REQUEST,
                CHANGE_PROPERTY_REQUEST
            ]
        );
        // Focus waits for the server's time: the last request is the empty append on our own window.
        let stamp = &sent[sent.len() - 24..];
        assert_eq!((stamp[0], stamp[1]), (CHANGE_PROPERTY_REQUEST, u8::from(PropMode::APPEND)));
        assert_eq!(u32::from_ne_bytes(stamp[4..8].try_into().unwrap()), 2);
        assert_eq!(u32::from_ne_bytes(stamp[20..24].try_into().unwrap()), 0);
    }

    #[test]
    fn focus_is_set_at_a_fresh_server_time_and_offered_as_the_window_asks() {
        use std::io::Read;
        let Placed { mut server, mut wm, mut wire, mut peer, .. } = placed();
        let now = Instant::now();
        let mut bytes = [0; 4096];
        for (n, (input, take)) in [(true, false), (true, true), (false, true), (false, false), (true, false)].into_iter().enumerate() {
            wm.focus = None;
            wm.windows[0].properties.input = Some(input);
            wm.windows[0].properties.take_focus = take;
            wm.settle_focus(&mut wire, &server).unwrap();
            if n > 0 {
                // Each change asks for a new time; the last one is never reused.
                wire.turn(now).unwrap();
                let count = peer.read(&mut bytes).unwrap();
                assert_eq!((count, bytes[0], bytes[1]), (24, CHANGE_PROPERTY_REQUEST, u8::from(PropMode::APPEND)));
            }
            let time = 1234 + n as u32;
            notify(&mut wm, &mut server, &mut wire, time);
            wm.settle_focus(&mut wire, &server).unwrap();
            wire.turn(now).unwrap();
            let count = peer.read(&mut bytes).unwrap();
            let focus = &bytes[..12];
            assert_eq!(focus[0], SET_INPUT_FOCUS_REQUEST);
            assert_eq!(u32::from_ne_bytes(focus[4..8].try_into().unwrap()), if input { 10 } else { 0 });
            assert_eq!(u32::from_ne_bytes(focus[8..12].try_into().unwrap()), time);
            let rest = &bytes[12..count];
            if take {
                // The client sees the same nonzero time as the preceding SetInputFocus request.
                assert_eq!(rest[0], SEND_EVENT_REQUEST);
                let message = ClientMessageEvent::try_parse(&rest[12..]).unwrap().0;
                assert_eq!(message.data.as_data32()[0], wm.atoms.get("WM_TAKE_FOCUS"));
                assert_eq!(message.data.as_data32()[1], time);
                assert_eq!(rest[44], CHANGE_PROPERTY_REQUEST);
            } else {
                assert_eq!(rest[0], CHANGE_PROPERTY_REQUEST);
            }
        }
    }

    #[test]
    fn an_unchanged_window_sends_no_x11_requests() {
        use std::io::Read;
        let Placed { mut server, mut wm, mut wire, mut peer, .. } = placed();
        // Focused at the server's time.
        notify(&mut wm, &mut server, &mut wire, 1234);
        wm.settle_focus(&mut wire, &server).unwrap();
        wire.turn(Instant::now()).unwrap();
        let mut bytes = [0; 4096];
        assert!(peer.read(&mut bytes).unwrap() > 0);
        assert_eq!(bytes[0], SET_INPUT_FOCUS_REQUEST);
        wm.settle(&mut wire, &mut server).unwrap();
        wire.turn(Instant::now()).unwrap();
        peer.set_nonblocking(true).unwrap();
        assert_eq!(peer.read(&mut bytes).unwrap_err().kind(), io::ErrorKind::WouldBlock, "unchanged state sends no X11 commands");
    }

    #[test]
    fn x11_title_changes_reach_the_state_once_per_frame() {
        let Placed { mut server, mut wm, mut wire, id, .. } = placed();
        wm.windows[0].properties.name = "first title".into();
        wm.windows[0].dirty = true;
        wm.settle(&mut wire, &mut server).unwrap();
        wm.windows[0].properties.name = "last title".into();
        wm.windows[0].dirty = true;
        wm.settle(&mut wire, &mut server).unwrap();
        assert_eq!(server.changed_facts.len(), 1, "X11 title changes coalesce before recording a frame");
        assert_eq!(server.core.state().session().window(id).unwrap().title, "");
        server.frame();
        assert_eq!(server.core.state().session().window(id).unwrap().title, "last title");
        assert!(server.changed_facts.is_empty());
    }

    #[test]
    fn x11_facts_deferred_to_the_frame_settle_with_their_title() {
        let Placed { mut server, mut wm, mut wire, id, .. } = placed();
        wm.windows[0].properties.name = "dialog title".into();
        wm.windows[0].properties.role = clamshell_state::presentation::WindowRole::Dialog;
        wm.windows[0].dirty = true;
        wm.settle(&mut wire, &mut server).unwrap();
        server.frame();
        let window = server.core.state().session().window(id).unwrap();
        assert_eq!(window.title, "dialog title");
        assert_eq!(window.role, clamshell_state::presentation::WindowRole::Dialog);
        let recording = server.trace.borrow().snapshot(server.core.state());
        // Identity and title settle atomically: the UI must never record half of this update.
        assert!(
            matches!(recording.records.back().map(|r| &r.input), Some(crate::trace::Input::Batch(events)) if matches!(events.as_slice(), [WindowEvent::Changed(_), WindowEvent::Title { window, title }] if *window == id && title == "dialog title"))
        );
    }
}

mod menus;
