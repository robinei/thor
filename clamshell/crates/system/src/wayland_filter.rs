//! A Wayland proxy for Waydroid (`clamshell --wayland-filter`): it listens on a socket of its own beside
//! the compositor's (`<display>-android`), and passes each client's connection through to the
//! compositor, messages and file descriptors alike, but for what it changes for Android's windows:
//!
//! - A toplevel configure of the size the window already has is dropped. Waydroid answers every one
//!   with a display hotplug, which clears its windows; a window is drawn again only with its app's next
//!   frame, and an app at rest draws none. Sway configures a window as it is hidden, shown, focused or
//!   unfocused (`suspended`, `activated`), its size unchanged: an Android app's window was lost whenever
//!   it went out of sight or lost focus. A resize still reaches Waydroid.
//! - Android's caption (its back, minimise, maximise and close buttons, drawn into the window along its
//!   top) is put out of sight: Android is told a window that much taller, and the compositor a window
//!   that starts below it (its window geometry), so the app's own area fills the window, unscaled. Its
//!   height, in pixels at Android's density, is in `android-caption` in ClamShell's runtime dir (the
//!   Android script writes it; none: shown).
//! - The app's frame rate is measured (its window's commits) and written as MangoHud logs its own (a
//!   CSV row every half second, fps and frame time, in ClamShell's MangoHud folder, as
//!   `android-<app>.csv`): ClamShell reads it as a game's.
//!
//! No frame rate cap: Waydroid paces Android by a vsync of its own, its period fixed as its session starts
//! (from the outputs' refresh), not by the compositor's frame callbacks.
//!
//! The proxy reads only what it must: the registry (`get_registry`), `xdg_wm_base` (bound), its
//! `xdg_surface`s and their `wl_surface`s, their toplevels (app id, size, activated); ids are forgotten as
//! the compositor deletes them (`wl_display.delete_id`).

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, IoSlice, IoSliceMut, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, mpsc};
use std::time::{Duration, Instant};

use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags};

use crate::worker;

/// The display object's id, and the opcodes read.
const DISPLAY: u32 = 1;
/// wl_display: `get_registry` (request), `delete_id` (event).
const GET_REGISTRY: u16 = 1;
const DELETE_ID: u16 = 1;
/// wl_registry: `bind`.
const BIND: u16 = 0;
/// xdg_wm_base: `get_xdg_surface`.
const GET_XDG_SURFACE: u16 = 2;
/// xdg_surface: `get_toplevel`, `set_window_geometry`.
const GET_TOPLEVEL: u16 = 1;
const SET_WINDOW_GEOMETRY: u16 = 3;
/// xdg_toplevel: `set_app_id` (request), `configure` (event; its `activated` state).
const SET_APP_ID: u16 = 3;
const CONFIGURE: u16 = 0;
const ACTIVATED: u32 = 4;
/// wl_surface: `commit`.
const COMMIT: u16 = 6;

/// File descriptors a message may carry at most (libwayland's).
const MAX_FDS: usize = 28;
/// How often a frame rate row is written, and how often the settings' files are read again.
const ROW_EVERY: Duration = Duration::from_millis(500);
const SETTINGS_EVERY: Duration = Duration::from_secs(1);

/// What to do with a message.
#[derive(Debug, PartialEq, Eq)]
enum Pass {
    Keep,
    Drop,
    /// Passed on changed (whole: header and arguments).
    Replace(Vec<u8>),
}

/// The settings ClamShell gives the proxy (see the module's doc), read again at most every second.
#[derive(Debug, Default)]
pub struct Settings {
    dir: Option<PathBuf>,
    read: Option<Instant>,
    /// The caption's height (pixels; 0: shown).
    caption: i32,
}

impl Settings {
    fn refresh(&mut self, now: Instant) {
        let Some(dir) = &self.dir else {
            return;
        };
        if self.read.is_some_and(|t| now.duration_since(t) < SETTINGS_EVERY) {
            return;
        }
        self.read = Some(now);
        self.caption = std::fs::read_to_string(dir.join("android-caption")).unwrap_or_default().trim().parse().unwrap_or(0);
    }
}

/// A toplevel the proxy follows.
#[derive(Debug)]
struct Toplevel {
    xdg_surface: u32,
    surface: u32,
    app_id: String,
    /// The size of the last configure passed on (none yet: the first goes), as the compositor gave it.
    size: Option<(i32, i32)>,
    active: bool,
    /// Commits since `since`.
    commits: u32,
    since: Option<Instant>,
}

/// What the proxy knows of a connection's objects.
#[derive(Debug, Default)]
struct Objects {
    registries: HashSet<u32>,
    wm_bases: HashSet<u32>,
    /// Each xdg_surface's wl_surface.
    xdg_surfaces: HashMap<u32, u32>,
    toplevels: HashMap<u32, Toplevel>,
    settings: Settings,
}

impl Objects {
    pub fn new(settings_dir: Option<PathBuf>) -> Objects {
        Objects { settings: Settings { dir: settings_dir, ..Settings::default() }, ..Objects::default() }
    }

    fn toplevel_of_xdg_surface(&self, xdg_surface: u32) -> Option<u32> {
        self.toplevels.iter().find(|(_, t)| t.xdg_surface == xdg_surface).map(|(id, _)| *id)
    }

    fn toplevel_of_surface(&self, surface: u32) -> Option<u32> {
        self.toplevels.iter().find(|(_, t)| t.surface == surface).map(|(id, _)| *id)
    }

    /// A request from the client: the objects it makes that the proxy follows; a window geometry moved
    /// below the caption.
    fn request(&mut self, object: u32, opcode: u16, args: &[u8], now: Instant) -> Pass {
        let word = |i: usize| args.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
        if self.registries.contains(&object) && opcode == BIND {
            // name, interface (a string: its length, then its bytes padded to 4), version, id.
            let len = word(1).unwrap_or(0) as usize;
            let after = 8 + len.div_ceil(4) * 4;
            let id = args.get(after + 4..after + 8).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
            if args.get(8..8 + len.saturating_sub(1)) == Some(b"xdg_wm_base") {
                self.wm_bases.extend(id);
            }
            return Pass::Keep;
        }
        match opcode {
            GET_REGISTRY if object == DISPLAY => self.registries.extend(word(0)),
            GET_XDG_SURFACE if self.wm_bases.contains(&object) => {
                if let (Some(id), Some(surface)) = (word(0), word(1)) {
                    self.xdg_surfaces.insert(id, surface);
                }
            }
            GET_TOPLEVEL if self.xdg_surfaces.contains_key(&object) => {
                let surface = self.xdg_surfaces.get(&object).copied().unwrap_or(0);
                if let Some(id) = word(0) {
                    let t = Toplevel { xdg_surface: object, surface, app_id: String::new(), size: None, active: false, commits: 0, since: None };
                    self.toplevels.insert(id, t);
                }
            }
            SET_WINDOW_GEOMETRY if self.xdg_surfaces.contains_key(&object) && self.toplevel_of_xdg_surface(object).is_some() => {
                self.settings.refresh(now);
                let caption = self.settings.caption;
                let int = |i: usize| word(i).map(|w| w as i32);
                if let (Some(x), Some(y), Some(w), Some(h)) = (int(0), int(1), int(2), int(3))
                    && caption > 0
                    && h > caption
                {
                    return Pass::Replace(message(object, opcode, &ints(&[x, y + caption, w, h - caption])));
                }
            }
            SET_APP_ID if self.toplevels.contains_key(&object) => {
                let len = word(0).unwrap_or(0) as usize;
                let app_id = args.get(4..4 + len.saturating_sub(1)).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                if let Some(t) = self.toplevels.get_mut(&object) {
                    t.app_id = app_id;
                }
            }
            COMMIT => {
                if let Some(t) = self.toplevel_of_surface(object).and_then(|id| self.toplevels.get_mut(&id)) {
                    t.commits += 1;
                    t.since.get_or_insert(now);
                }
            }
            _ => {}
        }
        Pass::Keep
    }

    /// An event from the compositor: a configure of the size the window had dropped, another made taller
    /// by the caption; ids deleted forgotten.
    fn event(&mut self, object: u32, opcode: u16, args: &[u8], now: Instant) -> Pass {
        let int = |i: usize| args.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).map(i32::from_ne_bytes);
        if object == DISPLAY && opcode == DELETE_ID {
            let Some(id) = int(0).map(|i| i as u32) else {
                return Pass::Keep;
            };
            self.registries.remove(&id);
            self.wm_bases.remove(&id);
            self.xdg_surfaces.remove(&id);
            self.toplevels.remove(&id);
            return Pass::Keep;
        }
        if opcode != CONFIGURE {
            return Pass::Keep;
        }
        let Some(t) = self.toplevels.get_mut(&object) else {
            return Pass::Keep;
        };
        let (Some(w), Some(h)) = (int(0), int(1)) else {
            return Pass::Keep;
        };
        // The states: an array (its length in bytes, then u32s).
        let states_len = int(2).unwrap_or(0) as usize;
        let states = args.get(12..12 + states_len).unwrap_or_default();
        t.active = states.chunks_exact(4).any(|s| s.try_into().ok().map(u32::from_ne_bytes) == Some(ACTIVATED));
        if t.size == Some((w, h)) {
            return Pass::Drop;
        }
        t.size = Some((w, h));
        self.settings.refresh(now);
        let caption = self.settings.caption;
        if caption > 0 && h > 0 {
            let mut changed = args.to_vec();
            changed.splice(4..8, (h + caption).to_ne_bytes());
            return Pass::Replace(message(object, opcode, &changed));
        }
        Pass::Keep
    }

    /// The active toplevel's frame rate, once half a second has passed since it was last told: its app id
    /// and its commits a second.
    fn frame_rate(&mut self, now: Instant) -> Option<(String, f32)> {
        let t = self.toplevels.values_mut().find(|t| t.active && !t.app_id.is_empty())?;
        let since = t.since?;
        let elapsed = now.duration_since(since);
        if elapsed < ROW_EVERY {
            return None;
        }
        let fps = t.commits as f32 / elapsed.as_secs_f32();
        t.commits = 0;
        t.since = Some(now);
        Some((t.app_id.clone(), fps))
    }
}

/// A message: its object, its opcode and its arguments, with its header.
fn message(object: u32, opcode: u16, args: &[u8]) -> Vec<u8> {
    let mut m = object.to_ne_bytes().to_vec();
    m.extend((((args.len() + 8) as u32) << 16 | u32::from(opcode)).to_ne_bytes());
    m.extend_from_slice(args);
    m
}

fn ints(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// The whole messages at the start of `buf`: each `(object, opcode, args)`, and how many bytes they take
/// (a message cut off at the end is left for the next read).
fn messages(buf: &[u8]) -> (Vec<(u32, u16, std::ops::Range<usize>)>, usize) {
    let mut out = Vec::new();
    let mut at = 0;
    let word = |i: usize| buf.get(i..i + 4).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
    while let (Some(object), Some(word)) = (word(at), word(at + 4)) {
        let (size, opcode) = ((word >> 16) as usize, (word & 0xffff) as u16);
        if size < 8 || at + size > buf.len() {
            break;
        }
        out.push((object, opcode, at + 8..at + size));
        at += size;
    }
    (out, at)
}

/// Listens on `<display>-android` beside the compositor's socket (`$XDG_RUNTIME_DIR`, `$WAYLAND_DISPLAY`)
/// and passes each connection through; `settings`: ClamShell's runtime dir (its `android-*` files),
/// `frames`: its MangoHud folder. Returns once the compositor closes a connection (see [`connect`]), or
/// if it can't listen.
pub fn run(settings: PathBuf, frames: PathBuf) -> io::Result<()> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or_else(|| io::Error::other("no XDG_RUNTIME_DIR"))?;
    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| io::Error::other("no WAYLAND_DISPLAY"))?;
    let upstream = runtime.join(&display);
    let path = runtime.join(format!("{display}-android"));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    log::info!("wayland filter: {} for {}", path.display(), upstream.display());
    let (ended, end) = mpsc::channel();
    worker::detached("wayland filter", move || {
        for client in listener.incoming() {
            match client {
                Ok(client) => connect(client, &upstream, &settings, &frames, &ended),
                Err(e) => log::warn!("wayland filter: {e}"),
            }
        }
    })?;
    let _ = end.recv();
    log::info!("wayland filter: the compositor closed a connection: ending");
    Ok(())
}

/// A client connected: its connection to the compositor, and a thread each way. If the compositor ends
/// it (gone, or the client dropped), the filter ends (`ended` is told): Waydroid can't go on without its
/// display (Android's system crashes when it next draws), and its session is bound to the filter's (see
/// the Android script).
fn connect(client: UnixStream, upstream: &Path, settings: &Path, frames: &Path, ended: &mpsc::Sender<()>) {
    let server = match UnixStream::connect(upstream) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("wayland filter: {}: {e}", upstream.display());
            return;
        }
    };
    let objects = Arc::new(Mutex::new(Objects::new(Some(settings.to_path_buf()))));
    // (A clone of the client's end kept here: to shut the connection if the second relay can't start.)
    let (client2, server2, kept) = match (client.try_clone(), server.try_clone(), client.try_clone()) {
        (Ok(c), Ok(s), Ok(k)) => (c, s, k),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return log::warn!("wayland filter: {e}"),
    };
    let (requests, rates) = (objects.clone(), objects.clone());
    let mut log = FrameLog { dir: frames.to_path_buf(), files: HashMap::new() };
    // Which end closed first (true: the compositor's).
    let first: Arc<OnceLock<bool>> = Arc::default();
    let first2 = first.clone();
    let (ended, ended2) = (ended.clone(), ended.clone());
    let end = |first: &OnceLock<bool>, compositor: bool, ended: &mpsc::Sender<()>| {
        if *first.get_or_init(|| compositor) {
            let _ = ended.send(());
        }
    };
    let requests_relay = worker::detached("wayland filter", move || {
        relay(
            &client,
            &server,
            |o, op, args, now| requests.lock().unwrap_or_else(PoisonError::into_inner).request(o, op, args, now),
            |now| {
                if let Some((app, fps)) = rates.lock().unwrap_or_else(PoisonError::into_inner).frame_rate(now) {
                    log.row(&app, fps);
                }
            },
            |client_closed| end(&first, !client_closed, &ended),
        );
    });
    // (The first relay not started: every handle is dropped on return, closing the connection.)
    if let Err(e) = requests_relay {
        return log::warn!("wayland filter: {e}");
    }
    let events_relay = worker::detached("wayland filter", move || {
        relay(
            &server2,
            &client2,
            |o, op, args, now| objects.lock().unwrap_or_else(PoisonError::into_inner).event(o, op, args, now),
            |_| {},
            |compositor_closed| end(&first2, compositor_closed, &ended2),
        );
    });
    if let Err(e) = events_relay {
        log::warn!("wayland filter: {e}");
        let _ = kept.shutdown(std::net::Shutdown::Both);
    }
}

/// The frame rate logs, MangoHud's way (see the module's doc): a file per app, its header first.
struct FrameLog {
    dir: PathBuf,
    files: HashMap<String, File>,
}

impl FrameLog {
    fn row(&mut self, app: &str, fps: f32) {
        let name = format!("android-{}.csv", app.replace(['/', ' '], "_"));
        if !self.files.contains_key(&name) {
            let _ = std::fs::create_dir_all(&self.dir);
            match OpenOptions::new().create(true).truncate(true).write(true).open(self.dir.join(&name)) {
                Ok(mut f) => {
                    let _ = f.write_all(b"fps,frametime\n");
                    self.files.insert(name.clone(), f);
                }
                Err(e) => return log::debug!("wayland filter: {name}: {e}"),
            }
        }
        if let Some(f) = self.files.get_mut(&name) {
            let frame_ms = if fps > 0.0 { 1000.0 / fps } else { 0.0 };
            let _ = writeln!(f, "{fps:.1},{frame_ms:.2}");
        }
    }
}

/// Passes messages from `from` to `to` as `pass` says, with the file descriptors that come with them,
/// until either end closes (then both are shut); `after` is called after each read, `closed` with which
/// end closed (`from`: true) before both are shut.
fn relay(
    from: &UnixStream,
    to: &UnixStream,
    mut pass: impl FnMut(u32, u16, &[u8], Instant) -> Pass,
    mut after: impl FnMut(Instant),
    closed: impl FnOnce(bool),
) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16384];
    let mut out = Vec::new();
    let from_closed = loop {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let n = match rustix::net::recvmsg(from, &mut [IoSliceMut::new(&mut chunk)], &mut ancillary, RecvFlags::CMSG_CLOEXEC) {
            Ok(r) if r.bytes > 0 => r.bytes,
            Err(rustix::io::Errno::INTR) => continue,
            Ok(_) | Err(_) => break true,
        };
        let mut received: Vec<OwnedFd> = Vec::new();
        for message in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(fds) = message {
                received.extend(fds);
            }
        }
        let now = Instant::now();
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
        let (found, used) = messages(&buf);
        out.clear();
        for (object, opcode, args) in found {
            let whole = buf.get(args.start - 8..args.end).unwrap_or_default();
            match pass(object, opcode, buf.get(args).unwrap_or_default(), now) {
                Pass::Keep => out.extend_from_slice(whole),
                Pass::Drop => {}
                Pass::Replace(m) => out.extend(m),
            }
        }
        buf.drain(..used);
        after(now);
        // (The descriptors go with the bytes they came with: the receiver takes them in order, by message.)
        if (!out.is_empty() || !received.is_empty()) && send(to, &out, &received).is_err() {
            break false;
        }
    };
    closed(from_closed);
    let _ = from.shutdown(std::net::Shutdown::Both);
    let _ = to.shutdown(std::net::Shutdown::Both);
}

/// Sends `bytes` with `fds` (with the first part sent), all of it.
fn send(to: &UnixStream, bytes: &[u8], fds: &[OwnedFd]) -> io::Result<()> {
    let borrowed: Vec<BorrowedFd> = fds.iter().map(AsFd::as_fd).collect();
    let mut sent = 0;
    let mut first = true;
    while sent < bytes.len() || (first && !borrowed.is_empty()) {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
        let mut ancillary = SendAncillaryBuffer::new(&mut space);
        if first && !borrowed.is_empty() {
            ancillary.push(SendAncillaryMessage::ScmRights(&borrowed));
        }
        let rest = bytes.get(sent..).unwrap_or_default();
        let n = match rustix::net::sendmsg(to, &[IoSlice::new(rest)], &mut ancillary, SendFlags::NOSIGNAL) {
            Err(rustix::io::Errno::INTR) => continue,
            sent => sent?,
        };
        if n == 0 && rest.is_empty() {
            break;
        }
        sent += n;
        first = false;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn words(ws: &[u32]) -> Vec<u8> {
        ws.iter().flat_map(|w| w.to_ne_bytes()).collect()
    }

    fn string(s: &str) -> Vec<u8> {
        let len = s.len() + 1;
        let mut out = words(&[len as u32]);
        out.extend(s.as_bytes());
        out.resize(4 + len.div_ceil(4) * 4, 0);
        out
    }

    /// The registry's bind of `interface` as id `id`.
    fn bind(interface: &str, id: u32) -> Vec<u8> {
        let mut args = words(&[7]);
        args.extend(string(interface));
        args.extend(words(&[5, id]));
        args
    }

    fn configure(w: i32, h: i32, states: &[u32]) -> Vec<u8> {
        let mut args = ints(&[w, h, (states.len() * 4) as i32]);
        args.extend(words(states));
        args
    }

    /// Objects with a toplevel 11 (xdg_surface 10, wl_surface 9) of app `org.x`.
    fn objects(caption: i32) -> Objects {
        let mut o = Objects::default();
        o.settings.caption = caption;
        let now = Instant::now();
        o.request(DISPLAY, GET_REGISTRY, &words(&[2]), now);
        o.request(2, BIND, &bind("wl_compositor", 3), now);
        o.request(2, BIND, &bind("xdg_wm_base", 4), now);
        o.request(4, GET_XDG_SURFACE, &words(&[10, 9]), now);
        o.request(10, GET_TOPLEVEL, &words(&[11]), now);
        o.request(11, SET_APP_ID, &string("org.x"), now);
        o
    }

    #[test]
    fn a_toplevel_configure_of_the_size_it_had_is_dropped() {
        let mut o = objects(0);
        let now = Instant::now();
        assert_eq!(o.event(11, CONFIGURE, &configure(1920, 1080, &[ACTIVATED]), now), Pass::Keep, "the first");
        assert_eq!(o.event(11, CONFIGURE, &configure(1920, 1080, &[]), now), Pass::Drop, "unfocused: the same size");
        assert_eq!(o.event(11, CONFIGURE, &configure(1240, 1080, &[]), now), Pass::Keep, "resized");
        assert_eq!(o.event(10, CONFIGURE, &words(&[77]), now), Pass::Keep, "xdg_surface's configure (acked): always");
        // Deleted, its id reused for something else: forgotten.
        assert_eq!(o.event(DISPLAY, DELETE_ID, &words(&[11]), now), Pass::Keep);
        assert_eq!(o.event(11, CONFIGURE, &configure(1240, 1080, &[]), now), Pass::Keep);
        assert!(o.toplevels.is_empty());
    }

    #[test]
    fn the_caption_is_out_of_sight_the_app_unscaled() {
        let mut o = objects(57);
        let now = Instant::now();
        // Android told a window the caption taller; the compositor, a window that starts below it.
        let fx = o.event(11, CONFIGURE, &configure(1920, 1080, &[ACTIVATED]), now);
        assert_eq!(fx, Pass::Replace(message(11, CONFIGURE, &configure(1920, 1137, &[ACTIVATED]))));
        assert_eq!(o.event(11, CONFIGURE, &configure(1920, 1080, &[]), now), Pass::Drop, "the same size still, as the compositor gave it");
        let geometry = o.request(10, SET_WINDOW_GEOMETRY, &ints(&[0, 0, 1920, 1137]), now);
        assert_eq!(geometry, Pass::Replace(message(10, SET_WINDOW_GEOMETRY, &ints(&[0, 57, 1920, 1080]))));
    }

    #[test]
    fn the_active_windows_commits_are_its_frame_rate() {
        let mut o = objects(0);
        let t0 = Instant::now();
        o.event(11, CONFIGURE, &configure(1920, 1080, &[ACTIVATED]), t0);
        for i in 0..30 {
            o.request(9, COMMIT, &[], t0 + Duration::from_millis(i * 20));
        }
        assert_eq!(o.frame_rate(t0 + Duration::from_millis(100)), None, "not half a second yet");
        let (app, fps) = o.frame_rate(t0 + Duration::from_millis(600)).unwrap();
        assert_eq!(app, "org.x");
        assert!((fps - 50.0).abs() < 1.0, "{fps}");
        // Not active: none.
        o.event(11, CONFIGURE, &configure(1920, 1080, &[]), t0);
        o.request(9, COMMIT, &[], t0 + Duration::from_millis(700));
        assert_eq!(o.frame_rate(t0 + Duration::from_millis(1300)), None);
    }

    #[test]
    fn messages_are_taken_whole() {
        let mut buf = message(11, 0, &words(&[1, 2, 0]));
        buf.extend(message(10, 0, &words(&[5])));
        let whole = buf.len();
        buf.extend(&message(3, 1, &words(&[1, 2]))[..6]);
        let (found, used) = messages(&buf);
        assert_eq!(used, whole, "the cut-off one left");
        assert_eq!(found.iter().map(|(o, op, r)| (*o, *op, r.len())).collect::<Vec<_>>(), [(11, 0, 12), (10, 0, 4)]);
    }

    #[test]
    fn a_connection_is_passed_through_with_its_fds_and_without_repeated_configures() {
        use std::io::{Read, Write};
        let (client_near, client_far) = UnixStream::pair().unwrap();
        let (server_near, server_far) = UnixStream::pair().unwrap();
        let objects = Arc::new(Mutex::new(Objects::default()));
        let (o1, o2) = (objects.clone(), objects.clone());
        let (cf, sn) = (client_far.try_clone().unwrap(), server_near.try_clone().unwrap());
        std::thread::spawn(move || relay(&cf, &sn, |o, op, a, t| o1.lock().unwrap().request(o, op, a, t), |_| {}, |_| {}));
        std::thread::spawn(move || relay(&server_near, &client_far, |o, op, a, t| o2.lock().unwrap().event(o, op, a, t), |_| {}, |_| {}));
        // The client makes a toplevel, sending an fd along.
        let mut requests = message(DISPLAY, GET_REGISTRY, &words(&[2]));
        requests.extend(message(2, BIND, &bind("xdg_wm_base", 4)));
        requests.extend(message(4, GET_XDG_SURFACE, &words(&[10, 9])));
        requests.extend(message(10, GET_TOPLEVEL, &words(&[11])));
        let file = std::fs::File::open("/dev/null").unwrap();
        send(&client_near, &requests, &[OwnedFd::from(file)]).unwrap();
        let mut got = vec![0u8; requests.len()];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let r = rustix::net::recvmsg(&server_far, &mut [IoSliceMut::new(&mut got)], &mut ancillary, RecvFlags::empty()).unwrap();
        assert_eq!(&got[..r.bytes], &requests[..r.bytes]);
        let fds: usize = ancillary.drain().map(|m| if let RecvAncillaryMessage::ScmRights(f) = m { f.count() } else { 0 }).sum();
        assert_eq!(fds, 1, "the fd passed on");
        // The compositor configures it twice the same, then a resize: two reach the client.
        let events: Vec<u8> = [1920, 1920, 1240].iter().flat_map(|w| message(11, CONFIGURE, &configure(*w, 1080, &[]))).collect();
        (&server_far).write_all(&events).unwrap();
        let one = message(11, CONFIGURE, &configure(1, 1080, &[])).len();
        let mut received = vec![0u8; one * 2];
        (&client_near).read_exact(&mut received).unwrap();
        let expected: Vec<u8> = [1920, 1240].iter().flat_map(|w| message(11, CONFIGURE, &configure(*w, 1080, &[]))).collect();
        assert_eq!(received, expected);
    }
}
