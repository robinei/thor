//! A game as Wine shows one, drawing a test pattern: a fullscreen toplevel of a fixed size, the screen's
//! mode, and the picture in a subsurface above it scaled to that size by a viewport. It prints the outputs
//! it sees (`output NAME`) and the top's modes (`mode W H`, the last the current one), and reads commands:
//! `render W H` draws the picture at that size, `quit` ends it.
//!
//! The pattern shows what scaling, anti-aliasing and sharpening do. Kept fixed for the checks: corner
//! blocks of 16 pixels (red top left, green top right, blue bottom left, yellow bottom right), a 1-pixel
//! white border and a 1-pixel magenta cross through the middle. In the quarters it makes: top left,
//! hard-edged stripes at 5, 20, 45 and 70 degrees (aliased staircases, for anti-aliasing); top right, a
//! hard-edged star of 36 spokes (aliasing and moiré, and how an upscale keeps detail); bottom left,
//! 1-pixel gratings, vertical, then 2-pixel, horizontal, and a 1-pixel checker (the finest detail, for
//! upscaling and sharpening); bottom right, a grey ramp over a hard step and a soft one (banding, and
//! sharpening's halos). `tests/sdl-pattern.c` draws the same.

use smithay::reexports::wayland_protocols::{
    wp::viewporter::client::{wp_viewport, wp_viewporter},
    xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
};
use std::{
    io::{BufRead, Write},
    os::fd::AsFd,
    sync::{Arc, Mutex},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface},
};

/// An output: its name, and the modes it was told were current, in order.
type Seen = (wl_output::WlOutput, String, Vec<(i32, i32)>);

#[derive(Default)]
struct Client {
    outputs: Vec<Seen>,
    configured: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(_: &mut Self, proxy: &xdg_wm_base::XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(state: &mut Self, proxy: &xdg_surface::XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            proxy.ack_configure(serial);
            state.configured = true;
        }
    }
}
impl Dispatch<wl_output::WlOutput, ()> for Client {
    fn event(state: &mut Self, proxy: &wl_output::WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some((_, name, modes)) = state.outputs.iter_mut().find(|(output, ..)| output == proxy) else { return };
        match event {
            wl_output::Event::Name { name: given } => *name = given,
            wl_output::Event::Mode { flags: WEnum::Value(flags), width, height, .. } if flags.contains(wl_output::Mode::Current) => {
                modes.push((width, height));
            }
            _ => {}
        }
    }
}
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(Client: ignore wl_subsurface::WlSubsurface);
delegate_noop!(Client: ignore wl_surface::WlSurface);
delegate_noop!(Client: ignore wl_shm::WlShm);
delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Client: ignore wl_buffer::WlBuffer);
delegate_noop!(Client: ignore wp_viewporter::WpViewporter);
delegate_noop!(Client: ignore wp_viewport::WpViewport);
delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);

/// The pattern's colour at pixel (`x`, `y`) of `w`×`h` (see the module's comment).
fn pixel(x: i32, y: i32, w: i32, h: i32) -> [u8; 3] {
    let corner = |right: bool, bottom: bool| (if right { x >= w - 16 } else { x < 16 }) && (if bottom { y >= h - 16 } else { y < 16 });
    let grey = |v: u8| [v, v, v];
    let bw = |on: bool| grey(if on { 255 } else { 0 });
    let (cx, cy) = (w / 2, h / 2);
    if corner(false, false) {
        [255, 0, 0]
    } else if corner(true, false) {
        [0, 255, 0]
    } else if corner(false, true) {
        [0, 0, 255]
    } else if corner(true, true) {
        [255, 255, 0]
    } else if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
        grey(255)
    } else if x == cx || y == cy {
        [255, 0, 255]
    } else if x < cx && y < cy {
        // Stripes 12 pixels apart, at an angle by column band.
        let angle = match x * 4 / cx.max(1) {
            ..=0 => 5.0f64,
            1 => 20.0,
            2 => 45.0,
            _ => 70.0,
        }
        .to_radians();
        let along = f64::from(x) * angle.sin() - f64::from(y) * angle.cos();
        bw((along / 6.0).floor().rem_euclid(2.0) == 0.0)
    } else if y < cy {
        let (dx, dy) = (f64::from(x - (cx + w) / 2), f64::from(y - cy / 2));
        if dx.hypot(dy) > f64::from((w - cx).min(cy)) * 0.45 {
            grey(128)
        } else {
            bw(((dy.atan2(dx) / std::f64::consts::TAU + 1.0) * 72.0).floor() as i64 % 2 == 0)
        }
    } else if x < cx {
        let band = ((y - cy) * 4 / (h - cy).max(1)).clamp(0, 3);
        bw(match band {
            0 => x % 2 == 0,
            1 => x % 4 < 2,
            2 => y % 2 == 0,
            _ => (x + y) % 2 == 0,
        })
    } else {
        let (u, v) = (x - cx, y - cy);
        let (bw_, bh) = ((w - cx).max(1), (h - cy).max(1));
        if v < bh / 2 {
            grey((u * 255 / bw_) as u8)
        } else if u < bw_ / 2 {
            bw(u >= bw_ / 4)
        } else {
            // A step smoothed over 16 pixels.
            let t = (f64::from(u - bw_ * 3 / 4) / 16.0 + 0.5).clamp(0.0, 1.0);
            grey((t * t * (3.0 - 2.0 * t) * 255.0).round() as u8)
        }
    }
}

/// The pattern at `w`×`h`, as XRGB8888 (little-endian: blue, green, red, unused).
fn pattern(w: i32, h: i32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = pixel(x, y, w, h);
            pixels.extend_from_slice(&[b, g, r, 255]);
        }
    }
    pixels
}

fn buffer(shm: &wl_shm::WlShm, qh: &QueueHandle<Client>, (w, h): (i32, i32), pixels: &[u8]) -> std::io::Result<wl_buffer::WlBuffer> {
    let file = tempfile::tempfile()?;
    (&file).write_all(pixels)?;
    let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, qh, ());
    pool.destroy();
    Ok(buffer)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Client>(&connection)?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ())?;
    let subcompositor: wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let viewporter: wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ())?;
    let mut client = Client::default();
    for global in globals.contents().clone_list().into_iter().filter(|g| g.interface == wl_output::WlOutput::interface().name) {
        let output = globals.registry().bind::<wl_output::WlOutput, _, _>(global.name, global.version.min(4), &qh, ());
        client.outputs.push((output, String::new(), Vec::new()));
    }
    queue.roundtrip(&mut client)?;
    let mut out = std::io::stdout();
    for (_, name, _) in &client.outputs {
        writeln!(out, "output {name}").ok();
    }
    let top = client.outputs.iter().find(|(_, name, _)| name.ends_with("Top")).or(client.outputs.first()).ok_or("no output")?;
    for (w, h) in &top.2 {
        writeln!(out, "mode {w} {h}").ok();
    }
    let mode = *top.2.last().ok_or("no current mode")?;
    out.flush().ok();

    // The toplevel: fullscreen, fixed at the mode, a black pixel scaled over it.
    let root = compositor.create_surface(&qh, ());
    let xdg = wm.get_xdg_surface(&root, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_app_id("pattern-game".into());
    toplevel.set_title("Pattern".into());
    toplevel.set_min_size(mode.0, mode.1);
    toplevel.set_max_size(mode.0, mode.1);
    toplevel.set_fullscreen(None);
    root.commit();
    while !client.configured {
        queue.blocking_dispatch(&mut client)?;
    }
    xdg.set_window_geometry(0, 0, mode.0, mode.1);
    let root_viewport = viewporter.get_viewport(&root, &qh, ());
    root_viewport.set_destination(mode.0, mode.1);
    root.attach(Some(&buffer(&shm, &qh, (1, 1), &[0, 0, 0, 255])?), 0, 0);
    root.damage_buffer(0, 0, 1, 1);

    // The picture, in a subsurface scaled to the window.
    let picture = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&picture, &root, &qh, ());
    subsurface.set_position(0, 0);
    subsurface.set_desync();
    let viewport = viewporter.get_viewport(&picture, &qh, ());
    viewport.set_destination(mode.0, mode.1);
    let draw = move |size: (i32, i32)| -> std::io::Result<()> {
        picture.attach(Some(&buffer(&shm, &qh, size, &pattern(size.0, size.1))?), 0, 0);
        picture.damage_buffer(0, 0, size.0, size.1);
        picture.commit();
        Ok(())
    };
    draw(mode)?;
    root.commit();
    connection.flush().ok();

    // Commands, from another thread: the picture is drawn there and the connection flushed.
    let ended = Arc::new(Mutex::new(false));
    {
        let (connection, ended) = (connection.clone(), ended.clone());
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let words: Vec<&str> = line.split_whitespace().collect();
                match words.as_slice() {
                    ["render", w, h] => {
                        if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                            if let Err(error) = draw((w, h)) {
                                eprintln!("drawing {w}x{h}: {error}");
                                continue;
                            }
                            connection.flush().ok();
                            println!("drawn {w} {h}");
                        }
                    }
                    ["quit"] => break,
                    _ => eprintln!("unknown command: {line}"),
                }
            }
            if let Ok(mut ended) = ended.lock() {
                *ended = true;
            }
        });
    }
    while !ended.lock().map(|e| *e).unwrap_or(true) {
        // Wakes for events, and at least every 100 ms to see whether the commands ended.
        if let Some(guard) = queue.prepare_read() {
            let fd = guard.connection_fd();
            let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN)];
            let timeout = rustix::event::Timespec { tv_sec: 0, tv_nsec: 100_000_000 };
            if rustix::event::poll(&mut fds, Some(&timeout)).unwrap_or(0) > 0 {
                guard.read().ok();
            }
        }
        queue.dispatch_pending(&mut client).ok();
        connection.flush().ok();
    }
    Ok(())
}
