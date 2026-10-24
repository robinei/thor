//! Parametric PQ, linear and SDR EGL pixels for the real-client colour tests.

use std::time::Duration;

use smithay::{
    backend::{
        renderer::{Frame, Renderer, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    reexports::{calloop::EventLoop, winit::window::WindowAttributes},
    utils::{Rectangle, Transform},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "pq".into());
    let mut events = EventLoop::<bool>::try_new()?;
    let attributes = WindowAttributes::default().with_title("ClamShell colour fixture").with_decorations(false);
    let (mut backend, source) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    let value = match mode.as_str() {
        "pq" => 0.508_078_4,
        "linear" => 0.18,
        "srgb" => 128.0 / 255.0,
        _ => return Err("expected pq, linear or srgb".into()),
    };
    let _description = describe(backend.window(), &mode)?;
    events
        .handle()
        .insert_source(source, move |event, _, finished| {
            let result: Result<(), Box<dyn std::error::Error>> = (|| {
                match event {
                    WinitEvent::CloseRequested => *finished = true,
                    WinitEvent::Redraw => {
                        let size = backend.window_size();
                        {
                            let (renderer, mut framebuffer) = backend.bind()?;
                            let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
                            frame.clear([value, value, value, 1.0].into(), &[Rectangle::from_size(size)])?;
                            drop(frame.finish()?);
                        }
                        backend.submit(None)?;
                        backend.window().request_redraw();
                    }
                    _ => {}
                }
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("EGL fixture: {error}");
                *finished = true;
            }
        })
        .map_err(|error| error.error)?;
    let mut finished = false;
    while !finished {
        events.dispatch(Some(Duration::from_secs(1)), &mut finished)?;
    }
    Ok(())
}

use smithay::reexports::wayland_protocols::wp::color_management::v1::client::{
    wp_color_management_surface_v1::WpColorManagementSurfaceV1,
    wp_color_manager_v1::{self as manager, WpColorManagerV1},
    wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
    wp_image_description_v1::{self as image, WpImageDescriptionV1},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};
#[derive(Default)]
struct Client {
    ready: bool,
    failed: bool,
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WpImageDescriptionV1, ()> for Client {
    fn event(state: &mut Self, _: &WpImageDescriptionV1, event: image::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            image::Event::Ready { .. } => state.ready = true,
            image::Event::Failed { .. } => state.failed = true,
            _ => {}
        }
    }
}
delegate_noop!(Client:ignore WpColorManagerV1);
delegate_noop!(Client:ignore WpColorManagementSurfaceV1);
delegate_noop!(Client:ignore WpImageDescriptionCreatorParamsV1);
fn describe(window: &dyn smithay::reexports::winit::window::Window, mode: &str) -> Result<WpImageDescriptionV1, Box<dyn std::error::Error>> {
    let window = window.cast_ref::<winit_wayland::Window>().ok_or("Wayland host required")?;
    let surface = window.surface();
    let connection = Connection::from_backend(surface.backend().upgrade().ok_or("Wayland connection gone")?);
    let (globals, mut queue) = registry_queue_init::<Client>(&connection)?;
    let qh = queue.handle();
    let manager: WpColorManagerV1 = globals.bind(&qh, 1..=1, ())?;
    let control = manager.get_surface(surface, &qh, ());
    let creator = manager.create_parametric_creator(&qh, ());
    creator.set_primaries_named(if mode == "pq" { manager::Primaries::Bt2020 } else { manager::Primaries::Srgb });
    creator.set_tf_named(match mode {
        "pq" => manager::TransferFunction::St2084Pq,
        "linear" => manager::TransferFunction::ExtLinear,
        _ => manager::TransferFunction::Srgb,
    });
    let description = creator.create(&qh, ());
    let mut client = Client::default();
    while !client.ready && !client.failed {
        queue.roundtrip(&mut client)?;
    }
    if client.failed {
        return Err("colour description failed".into());
    }
    control.set_image_description(&description, manager::RenderIntent::Perceptual);
    connection.flush()?;
    Ok(description)
}
