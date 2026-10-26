//! A desktop window containing two logical outputs. It never takes control of the host session.

use std::time::Duration;

use crate::EventSender;
use clamshell_state::{Effect, State};
use clamshell_ui::{Config, Error};
use smithay::{
    backend::{
        renderer::{Frame, Renderer, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    reexports::winit::{dpi::PhysicalSize, window::WindowAttributes},
    utils::{Rectangle, Transform},
};

use crate::{
    base::Base,
    clients::Process,
    render::{self, Overlays, UiFrame},
    server::Server,
};

/// Runs the experimental nested compositor. Launch environments are scoped to its socket; the
/// process environment and the parent's compositor are left alone.
pub fn run(
    state: State,
    config: Config,
    start: impl FnOnce(EventSender, crate::Displays),
    on_effect: impl FnMut(&Effect) + 'static,
    identify: impl Fn(u32) -> Process + 'static,
    mut on_capture: impl FnMut(Result<clamshell_state::ScreenshotPixels, String>) + 'static,
) -> Result<(), Error> {
    let mut base = Base::new(state, config, false, identify, on_effect)?;
    let server = &mut base.server;
    // The screens are the device's own pixels: its second screen's density, if the profile knows it.
    let density = server.core.state().profile().bottom_pixels_per_mm_hint();
    server.core.set_bottom_density(density.map(|density| (density, density)));

    let attributes =
        // In the host's pixels, so the screens are as many pixels as the device's, whatever the host's scale.
        WindowAttributes::default().with_title("ClamShell nested").with_surface_size(PhysicalSize::new(server.size.0 as u32, server.size.1 as u32));
    let (mut backend, events) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    server.advertise_dmabuf(backend.renderer());
    // The device's drawing path, the only one: nested shows what the device would.
    let mut pipeline = crate::colour::render::Pipeline::new(backend.renderer(), server.size)
        .map_err(|error| format!("nested compositor: the colour pipeline can't start, and nested draws only through it: {error}"))?;
    crate::colour::protocol::install(&server.dh);
    let mut host_colour = crate::colour::host::Host::new(backend.window());
    if host_colour.is_none() {
        log::info!("nested compositor: the host's colour management can't be reached (not a Wayland window), so the window is sRGB");
    }
    backend.window().set_cursor_visible(false);
    let mut cursor = crate::cursor::Cursor::new();
    log::info!("nested output format: {:?}", backend.renderer().egl_context().pixel_format());
    let top_size = server.core.size(clamshell_ui::Screen::Top).0;
    let bottom_size = server.core.size(clamshell_ui::Screen::Bottom).0;
    let mut frames = [UiFrame::new(top_size)?, UiFrame::new(bottom_size)?];
    let mut overlays = Overlays::new(top_size)?;
    let mut elements = Vec::new();
    let mut refresh_known = false;
    base.event_loop
        .handle()
        .insert_source(events, move |event, _, server| {
            let result: Result<(), Error> = (|| {
                match event {
                    WinitEvent::Resized { size, .. } => {
                        refresh_known = false;
                        server.resize((size.w, size.h));
                    }
                    WinitEvent::Focus(focused) => {
                        server.host_focused = focused;
                        if !focused {
                            server.cursor_activity.hide();
                            server.touch_cancel();
                        }
                    }
                    WinitEvent::Input(event) => {
                        // The window is the whole scene, its touch and pointer positions within it.
                        let whole = Rectangle::from_size((f64::from(server.size.0), f64::from(server.size.1)).into());
                        server.input(event, crate::input::Host { absolute_cursor: true, touch_frames: false }, |_| (whole, Transform::Normal));
                    }
                    // Through the state's quit, which saves where shelf and the second screen are.
                    WinitEvent::CloseRequested => server.core.handle(clamshell_state::Event::Quit),
                    WinitEvent::Redraw => {
                        // The host tells which monitor shows the window only after it is shown, so ask
                        // at each frame until it does, and again after a resize.
                        if !refresh_known && let Some(refresh) = host_refresh(backend.window()) {
                            refresh_known = true;
                            if refresh != server.refresh {
                                log::info!("nested compositor: host refresh {} Hz", f64::from(refresh) / 1000.0);
                                server.refresh = refresh;
                                server.resize(server.size);
                            }
                        }
                        server.frame();
                        if let Some(host) = &mut host_colour {
                            if let Some(preferred) = host.turn()? {
                                log::info!("nested compositor: host preferred colour {preferred:?}");
                            }
                            let (peak, average) = server.colour_volume();
                            if let Some(planned) = host.planned_encoding(peak, average) {
                                host.describe(planned);
                            }
                            if let Some(encoding) = host.encoding() {
                                pipeline.set_outputs(backend.renderer(), [encoding; 2])?;
                            }
                            if let Some(advertised) = host.advertised() {
                                server.describe_screens([advertised; 2]);
                            }
                        }
                        sync_capture(server, backend.window());
                        server.import_dmabufs(backend.renderer());
                        let capturing = std::mem::take(&mut server.screenshot_requested);
                        // Commits timed for no later than this frame, shown at the host's next, go ahead.
                        let interval = Duration::from_secs_f64(1000.0 / f64::from(server.refresh.max(1000)));
                        server.release_timed_commits((Duration::from(server.clock.now()) + interval).into());
                        render::elements(server, &mut frames, &mut overlays, backend.renderer(), &mut elements)?;
                        cursor.elements(server, backend.renderer(), &mut elements)?;
                        let (states, _) = pipeline.compose(server, backend.renderer(), &mut elements)?;
                        if let Some(histogram) = pipeline.take_histogram() {
                            server.report_histogram(histogram);
                        }
                        {
                            let (renderer, mut framebuffer) = backend.bind()?;
                            pipeline.present(renderer, &mut framebuffer)?;
                            if capturing {
                                let captured = pipeline.capture(renderer);
                                if let Err(error) = &captured {
                                    log::warn!("compositor screenshot: {error}");
                                }
                                // bind() is lazy. Begin an empty frame to actually restore the
                                // window EGL target after the surfaceless readback mapping.
                                // No drawing is submitted by this frame; swap supplies presentation synchronization.
                                drop(renderer.render(&mut framebuffer, server.size.into(), Transform::Flipped180)?.finish()?);
                                on_capture(captured);
                            }
                        }
                        backend.submit(Some(&[Rectangle::from_size(backend.window_size())]))?;
                        server.frame_queued(&states, None);
                        server.frame_shown(&states, None);
                        server.present_submission(&states, server.clock.now());
                        // The window redraws at each of the host's frames, so a held callback goes at a later one.
                        let refresh = Duration::from_secs_f64(1000.0 / f64::from(server.refresh.max(1000)));
                        server.frame_callbacks(None, refresh, std::time::Instant::now());
                        backend.window().request_redraw();
                    }
                }
                sync_capture(server, backend.window());
                Ok(())
            })();
            if let Err(error) = result {
                log::error!("nested compositor: {error}");
                server.exit = true;
            }
        })
        .map_err(|e| e.error)?;
    // The window redraws at each of the host's frames, so it needs no turns of its own.
    base.run(start, |_, _| None)
}

/// The refresh of the monitor showing the window, in mHz, once the host says.
fn host_refresh(window: &dyn smithay::reexports::winit::window::Window) -> Option<i32> {
    let refresh = window.current_monitor()?.current_video_mode()?.refresh_rate_millihertz()?;
    i32::try_from(refresh.get()).ok()
}

fn sync_capture(server: &mut Server, window: &dyn smithay::reexports::winit::window::Window) {
    use crate::pointer::CaptureMode;
    use smithay::reexports::winit::window::CursorGrabMode;
    server.sync_pointer_capture(|mode| {
        window.set_cursor_grab(match mode {
            CaptureMode::Free => CursorGrabMode::None,
            CaptureMode::Locked => CursorGrabMode::Locked,
            CaptureMode::Confined => CursorGrabMode::Confined,
        })
    });
}
