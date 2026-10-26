//! A desktop window containing two logical outputs. It never takes control of the host session.

use std::time::Duration;

use clamshell_state::{Effect, State};
use clamshell_ui::{Config, Error, EventSender};
use smithay::{
    backend::{
        renderer::{Frame, Renderer, damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    reexports::winit::{dpi::PhysicalSize, window::WindowAttributes},
    utils::{Rectangle, Transform},
};

use crate::{
    base::Base,
    render::{self, Overlays, UiFrame},
    server::{Process, Server},
};

/// Runs the experimental nested compositor. Launch environments are scoped to its socket; the
/// process environment and the parent's compositor are left alone.
pub fn run(
    state: State,
    config: Config,
    start: impl FnOnce(EventSender, crate::Displays),
    on_effect: impl FnMut(&Effect) + 'static,
    identify: impl Fn(u32) -> Process + 'static,
    mut on_capture: impl FnMut(Result<crate::Capture, String>) + 'static,
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
    let mut colour_pipeline = match crate::colour::render::Pipeline::new(backend.renderer(), server.size) {
        Ok(pipeline) => {
            crate::colour::protocol::install(&server.dh);
            Some(pipeline)
        }
        Err(error) => {
            log::warn!("colour management unavailable: {error}");
            None
        }
    };
    let mut host_colour = colour_pipeline.as_ref().and_then(|_| crate::colour::host::Host::new(backend.window()));
    backend.window().set_cursor_visible(false);
    let mut cursor = crate::cursor::Cursor::new();
    log::info!("nested output format: {:?}", backend.renderer().egl_context().pixel_format());
    let top_size = server.core.size(clamshell_ui::Screen::Top).0;
    let bottom_size = server.core.size(clamshell_ui::Screen::Bottom).0;
    let mut frames = [UiFrame::new(top_size)?, UiFrame::new(bottom_size)?];
    let mut overlays = Overlays::new(top_size)?;
    let mut elements = Vec::new();
    let mut submitted = false;
    let mut refresh_known = false;
    let mut damage = OutputDamageTracker::new(backend.window_size(), 1.0, Transform::Flipped180);
    base.event_loop
        .handle()
        .insert_source(events, move |event, _, server| {
            let result: Result<(), Error> = (|| {
                match event {
                    WinitEvent::Resized { size, .. } => {
                        refresh_known = false;
                        server.resize((size.w, size.h));
                        damage = OutputDamageTracker::new(size, 1.0, Transform::Flipped180);
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
                    WinitEvent::CloseRequested => server.exit = true,
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
                        if let (Some(host), Some(pipeline)) = (&mut host_colour, &mut colour_pipeline) {
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
                        let imported = server.import_dmabufs(backend.renderer());
                        let capturing = std::mem::take(&mut server.screenshot_requested);
                        // Commits timed for no later than this frame, shown at the host's next, go ahead.
                        let interval = Duration::from_secs_f64(1000.0 / f64::from(server.refresh.max(1000)));
                        server.release_timed_commits((Duration::from(server.clock.now()) + interval).into());
                        render::elements(server, &mut frames, &mut overlays, backend.renderer(), &mut elements)?;
                        cursor.elements(server, backend.renderer(), &mut elements)?;
                        let composed_states =
                            if let Some(pipeline) = &mut colour_pipeline { Some(pipeline.compose(server, backend.renderer(), &mut elements)?.0) } else { None };
                        if let Some(histogram) = colour_pipeline.as_mut().and_then(|pipeline| pipeline.take_histogram()) {
                            server.report_histogram(histogram);
                        }
                        // The window's buffer age, which only the colour pipeline's absence uses: asked once
                        // the window is bound, since EGL answers it only for the current surface, and
                        // what was rendered offscreen or imported left none current. After imports, a
                        // whole frame restores the window target even when the scene has not changed.
                        let age = if colour_pipeline.is_none() && submitted && !imported {
                            drop(backend.bind()?);
                            backend.buffer_age().unwrap_or(0)
                        } else {
                            0
                        };
                        let states;
                        {
                            let (renderer, mut framebuffer) = backend.bind()?;
                            states = if let Some(pipeline) = &colour_pipeline {
                                pipeline.present(renderer, &mut framebuffer)?;
                                composed_states.ok_or("colour scene was not composed")?
                            } else {
                                damage.render_output(renderer, &mut framebuffer, age, &elements, [0.0, 0.0, 0.0, 1.0])?.states
                            };
                            if capturing {
                                let captured = if let Some(pipeline) = &colour_pipeline {
                                    pipeline.capture(renderer)
                                } else {
                                    render::capture(renderer, &framebuffer, server.size)
                                };
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
                        submitted = true;
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
