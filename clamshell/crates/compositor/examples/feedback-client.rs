//! Exercise presentation feedback on a real submitted frame, then an unchanged one.

use smithay::reexports::wayland_protocols::{
    wp::presentation_time::client::{wp_presentation, wp_presentation_feedback},
    xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
};
use std::{io::Write, os::fd::AsFd};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
};

#[derive(Default)]
struct Client {
    configured: bool,
    clock: Option<u32>,
    presented: Vec<(u64, u32)>,
    invalid: bool,
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
impl Dispatch<wp_presentation::WpPresentation, ()> for Client {
    fn event(state: &mut Self, _: &wp_presentation::WpPresentation, event: wp_presentation::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock = Some(clk_id);
        }
    }
}
impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, refresh, seq_hi, seq_lo, flags } => {
                state.presented.push(((u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo), tv_nsec));
                state.invalid |= refresh != 0 || seq_hi != 0 || seq_lo != 0 || flags != WEnum::Value(wp_presentation_feedback::Kind::empty());
            }
            wp_presentation_feedback::Event::Discarded => state.invalid = true,
            _ => {}
        }
    }
}
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_surface::WlSurface);
delegate_noop!(Client: ignore wl_shm::WlShm);
delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Client: ignore wl_buffer::WlBuffer);
delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Client>(&connection)?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let presentation: wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg = shell.get_xdg_surface(&surface, &qh, ());
    let top = xdg.get_toplevel(&qh, ());
    top.set_app_id("clamshell-feedback-fixture".into());
    surface.commit();
    let mut client = Client::default();
    while !client.configured {
        queue.blocking_dispatch(&mut client)?;
    }
    let mut file = tempfile::tempfile()?;
    file.write_all(&vec![0x80; 960 * 540 * 4])?;
    let pool = shm.create_pool(file.as_fd(), 960 * 540 * 4, &qh, ());
    let buffer = pool.create_buffer(0, 960, 540, 960 * 4, wl_shm::Format::Xrgb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    for count in 1..=2 {
        presentation.feedback(&surface, &qh, ());
        surface.commit();
        while client.presented.len() < count && !client.invalid {
            queue.blocking_dispatch(&mut client)?;
        }
        if client.invalid {
            return Err("invalid presentation feedback".into());
        }
    }
    if client.clock != Some(1) || client.presented.windows(2).any(|pair| pair.first() > pair.get(1)) {
        return Err("presentation clock or timestamp order is invalid".into());
    }
    println!("ok: submitted and unchanged frames receive monotonic software presentation feedback");
    Ok(())
}
