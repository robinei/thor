//! ClamShell's compositor in a window on a desktop (winit), for development: the window is the whole scene,
//! both screens side by side at the device's sizes, drawn through the same colour pipeline as on the
//! device and paced by the host monitor's refresh. It is an ordinary client of the desktop and never takes
//! over its session: the pointer is captured only through the window, and the host's colour management is
//! told what the window shows.

use std::time::Duration;

use crate::EventSender;
use clamshell_state::{Effect, State};
use clamshell_ui::{Config, Error};
use smithay::{
    backend::{
        renderer::{Frame, Renderer, element::RenderElementStates, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    reexports::winit::{dpi::PhysicalSize, window::WindowAttributes},
    utils::{Rectangle, Transform},
};

use crate::{
    base::Base,
    clients::Process,
    render::{self, UiFrame},
    server::Server,
};

/// Runs the compositor in a window on the desktop until ClamShell quits. The programs it launches are
/// given its own socket and display ([`crate::Displays`]); ClamShell's environment and the desktop's
/// compositor are left alone.
pub fn run(
    state: State,
    config: Config,
    start: impl FnOnce(EventSender, crate::Displays),
    on_effect: impl FnMut(&Effect) + 'static,
    identify: impl Fn(u32) -> Process + Send + Sync + 'static,
    mut on_capture: impl FnMut(Result<clamshell_state::ScreenshotPixels, String>) + 'static,
    quit: crate::QuitSignals,
) -> Result<(), Error> {
    let mut base = Base::new(state, config, false, identify, on_effect, quit)?;
    let server = &mut base.server;
    // The screens are the device's own pixels, on its panels, if the profile knows their size.
    let profile = *server.core.state().profile();
    server.screens.millimetres = [crate::screens::shown_millimetres(None, &profile.top), crate::screens::shown_millimetres(None, &profile.bottom)];

    let attributes =
        // In the host's pixels, so the screens are as many pixels as the device's, whatever the host's scale.
        WindowAttributes::default().with_title("ClamShell nested").with_surface_size(PhysicalSize::new(server.screens.size.0 as u32, server.screens.size.1 as u32));
    let (mut backend, events) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    server.advertise_dmabuf(backend.renderer());
    // The device's drawing path, the only one: nested shows what the device would.
    let mut pipeline = crate::colour::render::Pipeline::new(backend.renderer(), server)
        .map_err(|error| format!("nested compositor: the colour pipeline can't start, and nested draws only through it: {error}"))?;
    crate::colour::protocol::install(&server.dh);
    let mut host_colour = crate::colour::host::Host::new(backend.window());
    if host_colour.is_none() {
        log::info!("nested compositor: the host's colour management can't be reached (not a Wayland window), so the window is sRGB");
    }
    backend.window().set_cursor_visible(false);
    let mut cursor = crate::cursor::Cursor::new();
    log::info!("nested output format: {:?}", backend.renderer().egl_context().pixel_format());
    let mut frames: [UiFrame; 4] = std::array::from_fn(|_| UiFrame::new());
    let mut elements = Vec::new();
    // Each screen's elements' states (the top's, the second's).
    let mut states = [RenderElementStates::default(), RenderElementStates::default()];
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
                        server.pointer.host_focused = focused;
                        if !focused {
                            server.pointer.cursor_activity.hide();
                            server.touch_cancel();
                        }
                    }
                    WinitEvent::Input(event) => {
                        // The window is the whole scene, its touch and pointer positions within it.
                        let whole = Rectangle::from_size((f64::from(server.screens.size.0), f64::from(server.screens.size.1)).into());
                        server.input(event, crate::input::Host { absolute_cursor: true, touch_frames: false }, |_| (whole, Transform::Normal));
                    }
                    // Through the state's quit, which saves where shelf and the second screen are.
                    WinitEvent::CloseRequested => server.core.handle(clamshell_state::Event::Quit),
                    WinitEvent::Redraw => {
                        // The host tells which monitor shows the window only after it is shown, so ask
                        // at each frame until it does, and again after a resize.
                        if !refresh_known && let Some(refresh) = host_refresh(backend.window()) {
                            refresh_known = true;
                            if refresh != server.screens.refresh {
                                log::info!("nested compositor: host refresh {} Hz", f64::from(refresh) / 1000.0);
                                server.screens.refresh = refresh;
                                server.resize(server.screens.size);
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
                        let interval = Duration::from_secs_f64(1000.0 / f64::from(server.screens.refresh.max(1000)));
                        server.release_timed_commits((Duration::from(server.clock.now()) + interval).into());
                        for (down, states) in [false, true].into_iter().zip(&mut states) {
                            render::elements(server, &mut frames, &mut cursor, backend.renderer(), &mut elements, down, None, Default::default())?;
                            pipeline.compose(server, backend.renderer(), down, &mut elements, states, false)?;
                        }
                        // What a screenshot was drawn with goes a while after it (see `crate::hold`); a host
                        // frame comes often enough to drop it near its time.
                        let mut trims = crate::hold::Trims::default();
                        pipeline.trim(std::time::Instant::now(), &mut trims);
                        trims.finish();
                        if let Some(histogram) = pipeline.take_histogram() {
                            server.report_histogram(histogram);
                        }
                        {
                            let (renderer, mut framebuffer) = backend.bind()?;
                            pipeline.present(renderer, &mut framebuffer, server.screens.size)?;
                            if capturing {
                                let captured = pipeline.capture(renderer, server.screens.size);
                                if let Err(error) = &captured {
                                    log::warn!("compositor screenshot: {error}");
                                }
                                // bind() is lazy. Begin an empty frame to actually restore the
                                // window EGL target after the surfaceless readback mapping.
                                // No drawing is submitted by this frame; swap supplies presentation synchronization.
                                drop(renderer.render(&mut framebuffer, server.screens.size.into(), Transform::Flipped180)?.finish()?);
                                on_capture(captured);
                            }
                        }
                        backend.submit(Some(&[Rectangle::from_size(backend.window_size())]))?;
                        for (down, states) in [false, true].into_iter().zip(&states) {
                            server.frame_queued(states, Some(down));
                            server.frame_shown(states, Some(down));
                            server.present_submission(states, server.clock.now());
                        }
                        // The window redraws at each of the host's frames, so a held callback goes at a later one.
                        let refresh = Duration::from_secs_f64(1000.0 / f64::from(server.screens.refresh.max(1000)));
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
