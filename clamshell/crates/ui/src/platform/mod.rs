//! The Wayland side: the host of the [`Core`] as a Wayland client. This module holds the event loop and
//! the views, each a surface showing one of the core's screens, drawn into shm buffers. The submodules
//! handle the compositor's events ([`handlers`]), idle and output power ([`display`]), the input method
//! ([`text_input`]) and systemd's notifications ([`notify`]).

mod app;
mod display;
mod handlers;
mod notify;
mod text_input;
mod view;

use std::time::{Duration, Instant};

use clamshell_state::{Effect, Event, State};
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::{EventLoop, channel, ping},
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

use crate::core::Core;
use crate::screens::Screens;
use crate::{Config, Error, EventSender};
use view::View;

/// Runs the UI until it quits. `start` gets a sender for the system threads. `on_effect` carries out what
/// the state asks for, such as launching games.
pub fn run(state: State, config: Config, start: impl FnOnce(EventSender), on_effect: impl FnMut(&Effect) + 'static) -> Result<(), Error> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();
    let shells = Shells::bind(&globals, &qh)?;
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    // The core's threads wake the loop when a font or an image comes back, and the core takes it in on
    // its next run.
    let (wake, woken) = ping::make_ping()?;
    event_loop.handle().insert_source(woken, |_, _, _| {}).map_err(|e| e.error)?;
    let core = Core::new(state, config.touch_layout.clone(), move || wake.ping())?;
    let mut app = App::new(&globals, &qh, core, &config, Box::new(on_effect))?;

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
    let (sender, receiver) = EventSender::channel();
    event_loop
        .handle()
        .insert_source(receiver, |event, _, app: &mut App| {
            if let channel::Event::Msg(event) = event {
                app.core.handle(event);
            }
        })
        .map_err(|e| e.error)?;
    start(sender);
    // Now that everything runs, start what the current state needs, such as the tab's watch, brightness
    // and the pad.
    app.core.handle(Event::Start);
    app.carry_out();

    // systemd is told the program is up once the screens are drawn, and that the loop is still turning
    // (see notify). The loop sleeps until something happens or the core's next deadline.
    let mut notifier = notify::Notifier::new();
    while !app.exit {
        let timeout = app.core.deadline().map(|at| until(at, Instant::now()));
        event_loop.dispatch(timeout, &mut app)?;
        app.turn();
        notifier.turned();
    }
    Ok(())
}

/// The time from `now` to `at`, rounded up to whole milliseconds, so that the loop doesn't wake just
/// before a deadline.
fn until(at: Instant, now: Instant) -> Duration {
    let wait = at.saturating_duration_since(now);
    Duration::from_millis(u64::try_from(wait.as_micros().div_ceil(1000)).unwrap_or(u64::MAX))
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
    bottom_output: Option<smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput>,
    shm: Shm,
    pool: SlotPool,
    qh: QueueHandle<App>,
    /// Made once the outputs are known (see `run`).
    views: Screens<Option<View>>,
    core: Core,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// The compositor's cursor shapes and our pointer's shape. We set the arrow over our surfaces,
    /// because otherwise sway shows no cursor there.
    cursor_shapes: Option<smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager>,
    pub(crate) cursor: Option<smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    touch: Option<wl_touch::WlTouch>,
    /// How the bottom view is shown (see `App::place_views`).
    bottom_kind: BottomKind,
    text_input: text_input::TextInput,
    /// The compositor's idle report, the screens' power and the main screen's refresh rate.
    display: display::Display,
    /// Carries out the effects that aren't the UI's own, through the system crate.
    on_effect: Box<dyn FnMut(&Effect)>,
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
        state.apply(clamshell_state::session::windows::changes(state.session().windows(), &[window]), &mut effects);
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
