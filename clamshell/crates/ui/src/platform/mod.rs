//! The Wayland side. This module holds the app's event loop and its views, each a surface drawn into shm
//! buffers. The submodules handle the compositor's events ([`handlers`]), idle and output power
//! ([`display`]), the input method ([`text_input`]) and systemd's notifications ([`notify`]).

mod app;
mod display;
mod handlers;
mod notify;
mod routing;
mod text_input;
mod view;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use clamshell_state::bottom::touch::TouchLayout;
use clamshell_state::{Effect, Event, Id, State};
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::{EventLoop, LoopHandle, RegistrationToken, channel},
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, QueueHandle,
            globals::{GlobalList, registry_queue_init},
            protocol::{wl_keyboard, wl_pointer, wl_touch},
        },
    },
    registry::RegistryState,
    seat::SeatState,
    shell::{wlr_layer::LayerShell, xdg::XdgShell},
    shm::{Shm, slot::SlotPool},
};

use crate::gfx::DrawCache;
use crate::input::{Input, Source};
use crate::screens::Screens;
use crate::{Config, Error, EventSender, gfx, input};
use view::View;

/// Runs the UI until it quits. `start` gets a sender for the system threads. `on_effect` carries out what
/// the state asks for, such as launching games.
pub fn run(state: State, config: Config, start: impl FnOnce(EventSender), on_effect: impl FnMut(&Effect) + 'static) -> Result<(), Error> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();
    let shells = Shells::bind(&globals, &qh)?;
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let images = install_images(&event_loop.handle())?;
    let mut app = App::new(&globals, &qh, state, &config, images, event_loop.handle(), Box::new(on_effect))?;

    // Learn the outputs and their names before placing the bottom view. These roundtrips dispatch into
    // the app, which is why the app is made before its views.
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;
    // Frames are drawn straight into shm buffers in tiny-skia's byte order, which sway accepts.
    if !app.shm.formats().contains(&smithay_client_toolkit::reexports::client::protocol::wl_shm::Format::Abgr8888) {
        return Err("the compositor doesn't take ABGR8888 shm buffers (sway does)".into());
    }
    if let Some(seat) = app.seat_state.seats().next() {
        app.text_input.seat(&seat, &qh);
        app.display.seat(&seat, &qh);
    }
    app.place_views(&shells, &qh, &config);

    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).map_err(|e| e.error)?;
    let (sender, receiver) = channel::channel();
    event_loop
        .handle()
        .insert_source(receiver, |event, _, app: &mut App| {
            if let channel::Event::Msg(event) = event {
                app.handle(event);
            }
        })
        .map_err(|e| e.error)?;
    start(EventSender(sender));
    // Now that everything runs, start what the current state needs, such as the tab's watch, brightness
    // and the pad.
    app.handle(Event::Start);

    // systemd is told the program is up once the screens are drawn, and that the loop is still turning
    // (see notify).
    let mut notifier = notify::Notifier::new();
    while !app.exit {
        event_loop.dispatch(None, &mut app)?;
        app.redraw();
        notifier.turned();
    }
    Ok(())
}

/// The shells the views are made with.
struct Shells {
    compositor: CompositorState,
    xdg: XdgShell,
    layer: Option<LayerShell>,
}

impl Shells {
    fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Result<Shells, Error> {
        Ok(Shells {
            compositor: CompositorState::bind(globals, qh)?,
            xdg: XdgShell::bind(globals, qh)?,
            layer: LayerShell::bind(globals, qh).inspect_err(|e| log::info!("no layer shell protocol: {e}")).ok(),
        })
    }
}

/// Sets up image loading. Requests go to a decoding thread, and results come back through the event loop.
fn install_images(handle: &LoopHandle<'static, App>) -> Result<crate::gfx::images::Images, Error> {
    let (requests, worker) = std::sync::mpsc::channel();
    let (done, results) = channel::channel();
    crate::gfx::images::spawn_worker(worker, move |key, pixmap| done.send((key, pixmap)).is_ok())?;
    handle
        .insert_source(results, |event, _, app: &mut App| {
            if let channel::Event::Msg((key, pixmap)) = event {
                app.images.arrived(key, pixmap);
                app.changed();
            }
        })
        .map_err(|e| e.error)?;
    Ok(crate::gfx::images::Images::new(requests))
}

/// Sets up finding fonts for characters the built-in one lacks. They are found on a thread and come back
/// through the event loop.
fn install_fonts(handle: &LoopHandle<'static, App>) -> Result<DrawCache, Error> {
    let (requests, finder) = std::sync::mpsc::channel();
    let (done, results) = channel::channel();
    gfx::spawn_font_finder(finder, move |found| done.send(found).is_ok())?;
    handle
        .insert_source(results, |event, _, app: &mut App| {
            if let channel::Event::Msg(found) = event {
                app.fonts.found(found);
                app.changed();
            }
        })
        .map_err(|e| e.error)?;
    Ok(DrawCache::with_finder(requests))
}

/// How the bottom view is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomKind {
    /// A layer surface over the second screen's output, on the device.
    Output,
    /// A layer surface in a desktop's corner, which never takes the keyboard focus.
    Panel,
    /// An ordinary window.
    Window,
}

impl BottomKind {
    /// Chooses from whether the second screen's output exists, whether the compositor has layer shell,
    /// and whether a panel was asked for.
    fn choose(output: bool, layer_shell: bool, panel: bool) -> BottomKind {
        match (output, layer_shell) {
            (true, true) => BottomKind::Output,
            (false, true) if panel => BottomKind::Panel,
            _ => BottomKind::Window,
        }
    }

    /// Keys stand in for the pad everywhere except on the device, where keys reaching shelf are typing.
    fn desktop_keys(self) -> bool {
        self != BottomKind::Output
    }
}

/// The views that can't be seen, and so aren't drawn: the bottom one while it is on the second screen and
/// that screen is off. On a desktop nothing turns it off, so it is drawn whatever the state says. A
/// window covered by another is drawn too: it gets no frame callback while covered, so it commits at most
/// one frame until it is shown again.
fn hidden_views(kind: BottomKind, state: &State) -> Screens<bool> {
    Screens { top: false, bottom: kind == BottomKind::Output && state.bottom().off() }
}

pub(crate) struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    qh: QueueHandle<App>,
    /// Made once the outputs are known (see `run`).
    views: Screens<Option<View>>,
    state: State,
    fonts: DrawCache,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// The compositor's cursor shapes and our pointer's shape. We set the arrow over our surfaces,
    /// because otherwise sway shows no cursor there.
    cursor_shapes: Option<smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager>,
    pub(crate) cursor: Option<smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    touch: Option<wl_touch::WlTouch>,
    touch_layout: Arc<Mutex<TouchLayout>>,
    images: crate::gfx::images::Images,
    input: Input,
    /// The presses' running timers.
    timers: HashMap<(Source, input::Timer), RegistrationToken>,
    /// Turns the compositor's event times into instants, so they say when a finger moved rather than
    /// when the event was handled.
    clock: input::EventClock,
    /// Fingers pointing in the touch frame being handled. They are sent on together once the whole frame
    /// has been handled.
    contacts: Vec<clamshell_state::bottom::touch::Contact>,
    /// Where the pad's focus was on each screen, so View can switch back to it.
    focus_memory: Screens<Option<Id>>,
    /// How the bottom view is shown (see `App::place_views`).
    bottom_kind: BottomKind,
    text_input: text_input::TextInput,
    /// The compositor's idle report, the screens' power and the main screen's refresh rate.
    display: display::Display,
    loop_handle: LoopHandle<'static, App>,
    on_effect: Box<dyn FnMut(&Effect)>,
    /// The effects of the event being handled, kept to be reused for the next one.
    effects: Vec<Effect>,
    /// The events waiting while another is handled.
    queue: app::EventQueue,
    /// The screens' power as last set (see `App::outputs`).
    outputs: (bool, bool),
    exit: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bottom_view_goes_on_the_second_screen_when_there_is_one() {
        assert_eq!(BottomKind::choose(true, true, false), BottomKind::Output);
        assert_eq!(BottomKind::choose(true, true, true), BottomKind::Output);
        assert_eq!(BottomKind::choose(false, true, true), BottomKind::Panel);
        assert_eq!(BottomKind::choose(false, true, false), BottomKind::Window);
        assert_eq!(BottomKind::choose(true, false, true), BottomKind::Window);
        assert!(!BottomKind::Output.desktop_keys() && BottomKind::Panel.desktop_keys() && BottomKind::Window.desktop_keys());
    }

    /// On a desktop the bottom view is drawn while the state has the second screen off, and the top view
    /// while a program's window is in front, so neither freezes where it can be seen.
    #[test]
    fn only_the_second_screens_own_output_hides_a_view() {
        use clamshell_state::device::buttons::DeviceKey;
        use clamshell_state::session::instance::Instance;
        use clamshell_state::session::windows::Window;
        let mut state = State::new(clamshell_state::Init { library: clamshell_library::demo::library(), ..clamshell_state::Init::default() });
        let mut effects = Vec::new();
        let key = state.shelf().library().entries.first().map(|g| g.key.clone()).unwrap_or_default();
        let run = Instance::Run { key, secs: 1, n: 1 };
        state.apply(Event::Units(vec![run.clone()]), &mut effects);
        let window = Window { id: 5, app_id: "game".into(), focused: true, visible: true, workspace: run.workspace(), ..Window::default() };
        state.apply(Event::Windows(vec![window]), &mut effects);
        state.apply(Event::Device(DeviceKey::BottomPower), &mut effects);
        let session = state.session();
        assert!(state.bottom().off() && session.windows().iter().any(|w| w.focused && session.running_of(w).is_some()));
        let hidden = |kind| {
            let h = hidden_views(kind, &state);
            (h.top, h.bottom)
        };
        assert_eq!(hidden(BottomKind::Output), (false, true));
        assert_eq!(hidden(BottomKind::Panel), (false, false));
        assert_eq!(hidden(BottomKind::Window), (false, false));
    }
}
