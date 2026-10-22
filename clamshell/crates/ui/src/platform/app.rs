//! The app's own work. It places the views on the outputs, applies events and marks the views they
//! touched, draws frames when they are due, and carries out the input machines' actions and arms their
//! timers.

use std::collections::HashMap;
use std::time::Duration;

use clamshell_core::{Dirty, Effect, Event, ShelfEvent, State};
use smithay_client_toolkit::{
    output::OutputState,
    reexports::{
        calloop::{
            LoopHandle,
            timer::{TimeoutAction, Timer},
        },
        client::{QueueHandle, globals::GlobalList, protocol::wl_surface::WlSurface},
    },
    registry::RegistryState,
    seat::SeatState,
    shm::{Shm, slot::SlotPool},
};

use super::view::View;
use crate::input::{Action, Input, Source};
use crate::screens::Screens;
use crate::{Config, Error, Screen, render};

use super::{App, BottomKind, Shells, display, install_fonts, text_input, view};
use crate::input;

impl App {
    pub(super) fn new(
        globals: &GlobalList,
        qh: &QueueHandle<App>,
        state: State,
        config: &Config,
        images: crate::gfx::images::Images,
        loop_handle: LoopHandle<'static, App>,
        on_effect: Box<dyn FnMut(&Effect)>,
    ) -> Result<App, Error> {
        let shm = Shm::bind(globals, qh)?;
        let pool = SlotPool::new(4 << 20, &shm)?;
        Ok(App {
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            shm,
            pool,
            qh: qh.clone(),
            views: Screens::default(),
            state,
            fonts: install_fonts(&loop_handle)?,
            keyboard: None,
            pointer: None,
            cursor_shapes: smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager::bind(globals, qh)
                .inspect_err(|e| log::info!("no cursor shape protocol: {e}"))
                .ok(),
            cursor: None,
            touch: None,
            touch_layout: config.touch_layout.clone(),
            images,
            input: Input::default(),
            timers: HashMap::new(),
            contacts: Vec::new(),
            clock: input::EventClock::default(),
            focus_memory: Screens::default(),
            desktop_keys: true,
            text_input: text_input::TextInput::bind(globals, qh),
            display: display::Display::bind(globals, qh, &config.bottom_output),
            loop_handle,
            on_effect,
            exit: false,
        })
    }

    /// Places the top view as a window, and the bottom one as `BottomKind::choose` says.
    pub(super) fn place_views(&mut self, shells: &Shells, qh: &QueueHandle<App>, config: &Config) {
        self.views.top = Some(View::window(&shells.compositor, &shells.xdg, qh, Screen::Top, config.top_size));
        self.display.all_on(self.output_state.outputs(), qh);
        let output = self.output_state.outputs().find(|o| self.output_state.info(o).and_then(|i| i.name).as_deref() == Some(config.bottom_output.as_str()));
        let kind = BottomKind::choose(output.is_some(), shells.layer.is_some(), config.bottom_panel);
        self.desktop_keys = kind.desktop_keys();
        self.views.bottom = Some(match (kind, &shells.layer) {
            (BottomKind::Output, Some(layer_shell)) => {
                log::info!("bottom view: layer surface on {}", config.bottom_output);
                View::layer(&shells.compositor, layer_shell, qh, output.as_ref(), None)
            }
            (BottomKind::Panel, Some(layer_shell)) => {
                log::info!("bottom view: a panel in the corner (it never takes keyboard focus)");
                View::layer(&shells.compositor, layer_shell, qh, None, Some(config.bottom_size))
            }
            _ => {
                log::info!("bottom view: a window (no output {})", config.bottom_output);
                View::window(&shells.compositor, &shells.xdg, qh, Screen::Bottom, config.bottom_size)
            }
        });
    }

    pub(super) fn view_mut(&mut self, surface: &WlSurface) -> Option<&mut View> {
        self.views.iter_mut().filter_map(|(_, v)| v.as_mut()).find(|v| v.shell.surface() == surface)
    }

    pub(super) fn handle(&mut self, event: Event) {
        // The pad driving the screens is input, handled here like keys and touches.
        if let Event::Pad(input) = event {
            return self.pad(input);
        }
        let (dirty, effects) = self.state.apply(event);
        self.mark(dirty);
        let released = self.input.pad_route(self.state.pad().route(), &self.state);
        self.run_actions(released);
        for effect in &effects {
            match effect {
                Effect::Quit => self.exit = true,
                // The compositor controls the screens' power, and we ask it through Wayland. A bottom
                // screen turned off with Home+Start is left out, so turning the screens on again leaves
                // it off.
                Effect::Screens(on) => {
                    let bottom_off = self.state.bottom().off().then(|| self.display.bottom_output().to_owned());
                    let outputs: Vec<_> =
                        self.output_state.outputs().filter(|o| bottom_off.is_none() || self.output_state.info(o).and_then(|i| i.name) != bottom_off).collect();
                    self.display.power(*on, outputs.into_iter(), &self.qh);
                }
                other => {
                    // The main screen's refresh rate follows the settings in force. It is also set
                    // through Wayland.
                    if let Effect::Apply { settings, .. } | Effect::Launch { settings, .. } = other {
                        self.display.refresh(settings.get(clamshell_core::settings::Setting::Refresh), &self.qh);
                    }
                    (self.on_effect)(other)
                }
            }
        }
    }

    pub(super) fn handle_all(&mut self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            self.handle(event);
        }
    }

    pub(super) fn mark(&mut self, dirty: Dirty) {
        let dirty = Screens::from(dirty);
        for (screen, view) in self.views.iter_mut() {
            if let Some(view) = view {
                view.dirty |= dirty[screen];
            }
        }
    }

    /// Draws the views that are due. What the drawing changes, such as rows and lists, may make another
    /// view due. That view is drawn too rather than left until the next event, for a few passes at most.
    pub(super) fn redraw_dirty(&mut self) {
        for _ in 0..3 {
            if !self.draw_due() || !self.views.iter_mut().any(|(_, v)| v.as_ref().is_some_and(View::due)) {
                break;
            }
        }
    }

    /// Draws the views that are due. Returns whether it drew any.
    fn draw_due(&mut self) -> bool {
        let mut drew = false;
        let mut rows_changed = None;
        let mut lists = Vec::new();
        // While a window is shown on the bottom screen, ClamShell's bottom view goes behind it.
        let behind = self.state.bottom().shows_windows(self.state.session());
        if let Some(view) = self.views.iter_mut().find(|(s, _)| *s == Screen::Bottom).and_then(|(_, v)| v.as_mut()) {
            view.set_behind(behind);
        }
        for (screen, view) in self.views.iter_mut() {
            let Some(view) = view.as_mut().filter(|v| v.due()) else {
                continue;
            };
            match view.draw(&mut self.pool, &mut self.fonts, &self.state, &mut self.images, &self.qh) {
                Ok(None) => {}
                Ok(Some(rendered)) => {
                    drew = true;
                    view.body = rendered.touchpad_area;
                    if screen == Screen::Bottom {
                        let scale = view.scale.max(1) as f32;
                        view::publish(&self.touch_layout, (view.size.0 as f32 * scale, view.size.1 as f32 * scale));
                    }
                    rows_changed = rows_changed.or(rendered.list_rows.filter(|&rows| rows != self.state.shelf().rows()));
                    lists.extend(render::list_changes(&self.state, &rendered));
                }
                Err(e) => {
                    log::warn!("drawing {screen:?}: {e}");
                    view.dirty = false;
                }
            }
        }
        // The list's row count follows the layout; the state keeps the selection within it.
        if let Some(rows) = rows_changed {
            self.handle(Event::Shelf(ShelfEvent::Rows(rows)));
        }
        self.handle_all(lists);
        drew
    }

    /// Carries out what the input side asks: events, and the presses' timers on the event loop.
    pub(super) fn run_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Emit(event) => self.handle(event),
                Action::Arm(source, timer, after) => self.arm(source, timer, after),
                Action::Disarm(source, timer) => {
                    if let Some(token) = self.timers.remove(&(source, timer)) {
                        self.loop_handle.remove(token);
                    }
                }
                // Positions are in the second screen's pixels, and a finger's slot comes from its touch id.
                Action::Point(Source::Touch(id), phase, (x, y)) => {
                    let scale = self.views.bottom.as_ref().map_or(1, |v| v.scale.max(1)) as f32;
                    let slot = id.rem_euclid(clamshell_core::bottom::touch::SLOTS) as u8;
                    let contact = clamshell_core::bottom::touch::Contact { slot, phase, x: (x * scale).round() as i32, y: (y * scale).round() as i32 };
                    // A touch frame's events come in one go. Once they are handled, the frame's contacts
                    // are sent on together.
                    if self.contacts.is_empty() {
                        self.loop_handle.insert_idle(|app| {
                            let contacts = std::mem::take(&mut app.contacts);
                            app.handle(clamshell_core::bottom::touch::Pointing::Contacts(contacts).into());
                        });
                    }
                    self.contacts.push(contact);
                }
                Action::Point(..) => {}
            }
        }
    }

    fn arm(&mut self, source: Source, timer: input::Timer, after: Duration) {
        if let Some(token) = self.timers.remove(&(source, timer)) {
            self.loop_handle.remove(token);
        }
        let fire = move |_, _: &mut (), app: &mut App| {
            // The timer is dropped by returning Drop, so here it is only forgotten, not removed from the
            // loop from inside itself.
            app.timers.remove(&(source, timer));
            let actions = app.input.timer(source, timer, &app.state, std::time::Instant::now());
            app.run_actions(actions);
            TimeoutAction::Drop
        };
        match self.loop_handle.insert_source(Timer::from_duration(after), fire) {
            Ok(token) => {
                self.timers.insert((source, timer), token);
            }
            Err(e) => log::warn!("{timer:?} timer: {}", e.error),
        }
    }
}
