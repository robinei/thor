//! A Wayland proxy for Waydroid (`clamshell --wayland-filter`). It listens on a socket of its own,
//! `<display>-android`, beside the compositor's. It passes each client's connection through to the
//! compositor, messages and file descriptors alike, except for what it changes for Android's windows:
//!
//! - A toplevel configure with the size the window already has is dropped. Waydroid answers every
//!   configure with a display hotplug, which clears its windows. A window is drawn again only with its
//!   app's next frame, and an app at rest draws none. A compositor configures a window, with its size unchanged,
//!   whenever it is hidden, shown, focused or unfocused (`suspended`, `activated`). So an Android app's
//!   window was lost whenever it went out of sight or lost focus. A resize still reaches Waydroid.
//! - Android's caption, with its back, minimise, maximise and close buttons drawn into the top of the
//!   window, is put out of sight. Android is told the window is that much taller, and the compositor is
//!   told, through the window geometry, that the window starts below the caption. So the app's own area
//!   fills the window, unscaled. The caption's height, in pixels at Android's density, is in
//!   `android-caption` in ClamShell's runtime dir. The Android script writes it. Without the file the
//!   caption is shown.
//!
//! There is no frame rate cap. Waydroid paces Android by a vsync of its own, whose period is fixed from
//! the outputs' refresh when its session starts, and not by the compositor's frame callbacks.
//!
//! The proxy reads only what it must: the registry (`get_registry`), the bound `xdg_wm_base`, its
//! `xdg_surface`s and their `wl_surface`s, and their toplevels (app id, size, activated). Ids are
//! forgotten as the compositor deletes them (`wl_display.delete_id`).

mod objects;
mod wire;

use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use objects::Objects;
use wire::relay;

use crate::os::worker;

/// Listens on `<display>-android` beside the compositor's socket (`$XDG_RUNTIME_DIR`, `$WAYLAND_DISPLAY`)
/// and passes each connection through. `settings` is ClamShell's runtime dir with its `android-*` files.
/// Returns once the compositor closes a connection (see
/// [`connect`]), or if it can't listen.
pub fn run(settings: PathBuf) -> io::Result<()> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or_else(|| io::Error::other("no XDG_RUNTIME_DIR"))?;
    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| io::Error::other("no WAYLAND_DISPLAY"))?;
    let upstream = runtime.join(&display);
    let path = runtime.join(format!("{display}-android"));
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != io::ErrorKind::NotFound
    {
        log::warn!("wayland filter: removing {}: {e}", path.display());
    }
    let listener = UnixListener::bind(&path)?;
    log::info!("wayland filter: {} for {}", path.display(), upstream.display());
    let (ended, end) = mpsc::channel();
    worker::lasting(clamshell_state::Thread::WaylandFilter.name(), move || {
        for client in listener.incoming() {
            match client {
                Ok(client) => connect(client, &upstream, &settings, &ended),
                Err(e) => log::warn!("wayland filter: {e}"),
            }
        }
    })?;
    match end.recv() {
        Ok(()) => log::info!("wayland filter: the compositor closed a connection: ending"),
        Err(e) => log::warn!("wayland filter: the listener ended: {e}"),
    }
    Ok(())
}

/// Handles a client that connected by opening its connection to the compositor and starting a thread
/// each way. If the compositor ends the connection, because it is gone or it dropped the client, the
/// filter ends and `ended` is told. Waydroid can't go on without its display, since Android's system
/// crashes when it next draws, and its session is bound to the filter's (see the Android script).
fn connect(client: UnixStream, upstream: &Path, settings: &Path, ended: &mpsc::Sender<()>) {
    let server = match UnixStream::connect(upstream) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("wayland filter: {}: {e}", upstream.display());
            return;
        }
    };
    let objects = Arc::new(Mutex::new(Objects::new(Some(settings.to_path_buf()))));
    // Keep a clone of the client's end here, to shut the connection if the second relay can't start.
    let (client2, server2, kept) = match (client.try_clone(), server.try_clone(), client.try_clone()) {
        (Ok(c), Ok(s), Ok(k)) => (c, s, k),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return log::warn!("wayland filter: {e}"),
    };
    let requests = objects.clone();
    // Which end closed first. True means the compositor's.
    let first: Arc<OnceLock<bool>> = Arc::default();
    let first2 = first.clone();
    let (ended, ended2) = (ended.clone(), ended.clone());
    let end = |first: &OnceLock<bool>, compositor: bool, ended: &mpsc::Sender<()>| {
        if *first.get_or_init(|| compositor) && ended.send(()).is_err() {
            log::debug!("wayland filter: already ending");
        }
    };
    let requests_relay = worker::detached("wayland filter", move || {
        relay(&client, &server, |o, op, args, now| crate::os::lock(&requests).request(o, op, args, now), |client_closed| end(&first, !client_closed, &ended));
    });
    // If the first relay didn't start, every handle is dropped on return, which closes the connection.
    if let Err(e) = requests_relay {
        return log::warn!("wayland filter: {e}");
    }
    let events_relay = worker::detached("wayland filter", move || {
        relay(
            &server2,
            &client2,
            |o, op, args, now| crate::os::lock(&objects).event(o, op, args, now),
            |compositor_closed| end(&first2, compositor_closed, &ended2),
        );
    });
    if let Err(e) = events_relay {
        log::warn!("wayland filter: {e}");
        if let Err(e) = kept.shutdown(std::net::Shutdown::Both) {
            log::warn!("wayland filter: closing the client's connection: {e}");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::objects::*;
    use super::wire::*;
    use super::*;
    use std::io::IoSliceMut;
    use std::mem::MaybeUninit;
    use std::os::fd::OwnedFd;
    use std::time::Instant;

    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};

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

    /// Objects with toplevel 11 (xdg_surface 10, wl_surface 9) of app `org.x`.
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
        // Once deleted, the id may be reused for something else, so the toplevel is forgotten.
        assert_eq!(o.event(DISPLAY, DELETE_ID, &words(&[11]), now), Pass::Keep);
        assert_eq!(o.event(11, CONFIGURE, &configure(1240, 1080, &[]), now), Pass::Keep);
        assert!(o.toplevels.is_empty());
    }

    #[test]
    fn the_caption_is_out_of_sight_the_app_unscaled() {
        let mut o = objects(57);
        let now = Instant::now();
        // Android is told the window is the caption's height taller, and the compositor that the window
        // starts below it.
        let fx = o.event(11, CONFIGURE, &configure(1920, 1080, &[ACTIVATED]), now);
        assert_eq!(fx, Pass::Replace(message(11, CONFIGURE, &configure(1920, 1137, &[ACTIVATED]))));
        assert_eq!(o.event(11, CONFIGURE, &configure(1920, 1080, &[]), now), Pass::Drop, "the same size still, as the compositor gave it");
        let geometry = o.request(10, SET_WINDOW_GEOMETRY, &ints(&[0, 0, 1920, 1137]), now);
        assert_eq!(geometry, Pass::Replace(message(10, SET_WINDOW_GEOMETRY, &ints(&[0, 57, 1920, 1080]))));
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
        std::thread::spawn(move || relay(&cf, &sn, |o, op, a, t| o1.lock().unwrap().request(o, op, a, t), |_| {}));
        std::thread::spawn(move || relay(&server_near, &client_far, |o, op, a, t| o2.lock().unwrap().event(o, op, a, t), |_| {}));
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
        // The compositor configures it twice with the same size, then resizes it. Two configures reach the
        // client.
        let events: Vec<u8> = [1920, 1920, 1240].iter().flat_map(|w| message(11, CONFIGURE, &configure(*w, 1080, &[]))).collect();
        (&server_far).write_all(&events).unwrap();
        let one = message(11, CONFIGURE, &configure(1, 1080, &[])).len();
        let mut received = vec![0u8; one * 2];
        (&client_near).read_exact(&mut received).unwrap();
        let expected: Vec<u8> = [1920, 1240].iter().flat_map(|w| message(11, CONFIGURE, &configure(*w, 1080, &[]))).collect();
        assert_eq!(received, expected);
    }
}
