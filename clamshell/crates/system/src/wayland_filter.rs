//! A Wayland proxy for Waydroid (`clamshell --wayland-filter`): it listens on a socket of its own beside
//! the compositor's (`<display>-android`), and passes each client's connection through to the
//! compositor, messages and file descriptors alike, but for one kind of event: an `xdg_toplevel.configure`
//! of the size the window already has.
//!
//! Waydroid answers every toplevel configure with a display hotplug, which clears its windows; a window is
//! drawn again only with its app's next frame, and an app at rest draws none. Sway configures a window as
//! it is hidden, shown, focused or unfocused (its `suspended` and `activated` states), its size unchanged,
//! so an Android app's window was lost whenever it went out of sight or lost focus. Without those
//! configures (Waydroid ignores the states anyway) its windows stay. A resize still reaches it.
//!
//! The proxy reads only what it must to know which objects are toplevels: the registry (`get_registry`),
//! `xdg_wm_base` (bound), its `xdg_surface`s, their toplevels; ids are forgotten as the compositor deletes
//! them (`wl_display.delete_id`).

use std::collections::{HashMap, HashSet};
use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags};

/// The display object's id, and the opcodes read: its `get_registry` request and `delete_id` event; the
/// registry's `bind`; `xdg_wm_base.get_xdg_surface`; `xdg_surface.get_toplevel`; `xdg_toplevel.configure`.
const DISPLAY: u32 = 1;
const GET_REGISTRY: u16 = 1;
const DELETE_ID: u16 = 1;
const BIND: u16 = 0;
const GET_XDG_SURFACE: u16 = 2;
const GET_TOPLEVEL: u16 = 1;
const CONFIGURE: u16 = 0;

/// File descriptors a message may carry at most (libwayland's).
const MAX_FDS: usize = 28;

/// What the proxy knows of a connection's objects.
#[derive(Debug, Default)]
pub struct Objects {
    registries: HashSet<u32>,
    wm_bases: HashSet<u32>,
    xdg_surfaces: HashSet<u32>,
    /// Each toplevel, and the size of the last configure passed on (none yet: the first goes).
    toplevels: HashMap<u32, Option<(i32, i32)>>,
}

impl Objects {
    /// A request from the client: the objects it makes that the proxy follows.
    fn request(&mut self, object: u32, opcode: u16, args: &[u8]) {
        let word = |i: usize| args.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
        match opcode {
            GET_REGISTRY if object == DISPLAY => self.registries.extend(word(0)),
            BIND if self.registries.contains(&object) => {
                // name, interface (a string: its length, then its bytes padded to 4), version, id.
                let Some(len) = word(1).map(|l| l as usize) else {
                    return;
                };
                let interface = args.get(8..8 + len.saturating_sub(1)).unwrap_or_default();
                let after = 8 + len.div_ceil(4) * 4;
                let id = args.get(after + 4..after + 8).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
                if interface == b"xdg_wm_base" {
                    self.wm_bases.extend(id);
                }
            }
            GET_XDG_SURFACE if self.wm_bases.contains(&object) => self.xdg_surfaces.extend(word(0)),
            GET_TOPLEVEL if self.xdg_surfaces.contains(&object) => {
                if let Some(id) = word(0) {
                    self.toplevels.insert(id, None);
                }
            }
            _ => {}
        }
    }

    /// An event from the compositor: whether it is passed on (not a toplevel configure of the size it
    /// had); ids deleted forgotten.
    fn event(&mut self, object: u32, opcode: u16, args: &[u8]) -> bool {
        let int = |i: usize| args.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).map(i32::from_ne_bytes);
        if object == DISPLAY && opcode == DELETE_ID {
            if let Some(id) = int(0).map(|i| i as u32) {
                self.registries.remove(&id);
                self.wm_bases.remove(&id);
                self.xdg_surfaces.remove(&id);
                self.toplevels.remove(&id);
            }
            return true;
        }
        if opcode != CONFIGURE {
            return true;
        }
        let Some(last) = self.toplevels.get_mut(&object) else {
            return true;
        };
        let (Some(w), Some(h)) = (int(0), int(1)) else {
            return true;
        };
        if *last == Some((w, h)) {
            return false;
        }
        *last = Some((w, h));
        true
    }
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
/// and passes each connection through; returns only if it can't listen.
pub fn run() -> io::Result<()> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or_else(|| io::Error::other("no XDG_RUNTIME_DIR"))?;
    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| io::Error::other("no WAYLAND_DISPLAY"))?;
    let upstream = runtime.join(&display);
    let path = runtime.join(format!("{display}-android"));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    log::info!("wayland filter: {} for {}", path.display(), upstream.display());
    for client in listener.incoming() {
        match client {
            Ok(client) => connect(client, &upstream),
            Err(e) => log::warn!("wayland filter: {e}"),
        }
    }
    Ok(())
}

/// A client connected: its connection to the compositor, and a thread each way.
fn connect(client: UnixStream, upstream: &Path) {
    let server = match UnixStream::connect(upstream) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("wayland filter: {}: {e}", upstream.display());
            return;
        }
    };
    let objects = Arc::new(Mutex::new(Objects::default()));
    let ends = (client.try_clone(), server.try_clone());
    let (Ok(client2), Ok(server2)) = ends else {
        return;
    };
    let requests = objects.clone();
    let _ = thread::Builder::new().name("wayland filter".into()).spawn(move || {
        relay(&client, &server, |o, op, args| {
            requests.lock().unwrap_or_else(PoisonError::into_inner).request(o, op, args);
            true
        });
    });
    let _ = thread::Builder::new().name("wayland filter".into()).spawn(move || {
        relay(&server2, &client2, |o, op, args| objects.lock().unwrap_or_else(PoisonError::into_inner).event(o, op, args));
    });
}

/// Passes messages from `from` to `to`, those `keep` says, with the file descriptors that come with
/// them, until either end closes (then both are shut).
fn relay(from: &UnixStream, to: &UnixStream, mut keep: impl FnMut(u32, u16, &[u8]) -> bool) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16384];
    loop {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let n = match rustix::net::recvmsg(from, &mut [IoSliceMut::new(&mut chunk)], &mut ancillary, RecvFlags::CMSG_CLOEXEC) {
            Ok(r) if r.bytes > 0 => r.bytes,
            Ok(_) | Err(_) => break,
        };
        let mut fds: Vec<OwnedFd> = Vec::new();
        for message in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(received) = message {
                fds.extend(received);
            }
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
        let (found, used) = messages(&buf);
        let mut out = Vec::with_capacity(used);
        for (object, opcode, args) in found {
            let message = buf.get(args.start - 8..args.end).unwrap_or_default();
            if keep(object, opcode, buf.get(args).unwrap_or_default()) {
                out.extend_from_slice(message);
            }
        }
        buf.drain(..used);
        // (The descriptors go with the bytes they came with: the receiver takes them in order, by message.)
        if send(to, &out, &fds).is_err() {
            break;
        }
    }
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
        let n = rustix::net::sendmsg(to, &[IoSlice::new(rest)], &mut ancillary, SendFlags::NOSIGNAL)?;
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

    fn message(object: u32, opcode: u16, args: &[u8]) -> Vec<u8> {
        let mut m = object.to_ne_bytes().to_vec();
        m.extend((((args.len() + 8) as u32) << 16 | u32::from(opcode)).to_ne_bytes());
        m.extend_from_slice(args);
        m
    }

    fn words(ws: &[u32]) -> Vec<u8> {
        ws.iter().flat_map(|w| w.to_ne_bytes()).collect()
    }

    /// The registry's bind of `interface` as id `id`.
    fn bind(interface: &str, id: u32) -> Vec<u8> {
        let len = interface.len() + 1;
        let mut args = words(&[7, len as u32]);
        args.extend(interface.as_bytes());
        args.resize(8 + len.div_ceil(4) * 4, 0);
        args.extend(words(&[5, id]));
        args
    }

    #[test]
    fn a_toplevel_configure_of_the_size_it_had_is_dropped() {
        let mut o = Objects::default();
        o.request(DISPLAY, GET_REGISTRY, &words(&[2]));
        o.request(2, BIND, &bind("wl_compositor", 3));
        o.request(2, BIND, &bind("xdg_wm_base", 4));
        o.request(4, GET_XDG_SURFACE, &words(&[10, 9]));
        o.request(10, GET_TOPLEVEL, &words(&[11]));
        let configure = |w: i32, h: i32, states: &[u32]| {
            let mut args = words(&[w as u32, h as u32, (states.len() * 4) as u32]);
            args.extend(words(states));
            args
        };
        assert!(o.event(11, CONFIGURE, &configure(1920, 1080, &[4])), "the first");
        assert!(!o.event(11, CONFIGURE, &configure(1920, 1080, &[])), "unfocused: the same size");
        assert!(!o.event(11, CONFIGURE, &configure(1920, 1080, &[9])), "suspended");
        assert!(o.event(11, CONFIGURE, &configure(1240, 1080, &[])), "resized");
        assert!(o.event(10, CONFIGURE, &words(&[77])), "xdg_surface's configure (acked): always");
        assert!(o.event(3, CONFIGURE, &configure(1, 1, &[])), "another object's opcode 0");
        // Deleted, its id reused for something else: forgotten.
        assert!(o.event(DISPLAY, DELETE_ID, &words(&[11])));
        assert!(o.event(11, CONFIGURE, &configure(1240, 1080, &[])));
        assert!(o.toplevels.is_empty());
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
        thread::spawn(move || {
            relay(&cf, &sn, |o, op, a| {
                o1.lock().unwrap().request(o, op, a);
                true
            })
        });
        thread::spawn(move || relay(&server_near, &client_far, |o, op, a| o2.lock().unwrap().event(o, op, a)));
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
        let configure = |w: u32| message(11, CONFIGURE, &words(&[w, 1080, 0]));
        let mut events = configure(1920);
        events.extend(configure(1920));
        events.extend(configure(1240));
        (&server_far).write_all(&events).unwrap();
        let mut received = vec![0u8; configure(1).len() * 2];
        (&client_near).read_exact(&mut received).unwrap();
        let mut expected = configure(1920);
        expected.extend(configure(1240));
        assert_eq!(received, expected);
    }
}
