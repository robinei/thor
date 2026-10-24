//! A real Wayland EGL client for the nested smoke test, using only safe renderer APIs.

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
    let mut events = EventLoop::<bool>::try_new()?;
    let attributes = WindowAttributes::default().with_title("ClamShell EGL fixture").with_decorations(false);
    let (mut backend, source) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
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
                            frame.clear([128.0 / 255.0, 32.0 / 255.0, 64.0 / 255.0, 1.0].into(), &[Rectangle::from_size(size)])?;
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
