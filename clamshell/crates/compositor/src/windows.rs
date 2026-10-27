//! Clients' windows: their facts told to the state, and how the state places them told back to them.

use clamshell_state::{State, WindowEvent, session::windows::WindowFacts};
use smithay::{
    desktop::Window as SurfaceWindow,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, SERIAL_COUNTER, Size},
    wayland::{
        compositor::with_states,
        shell::xdg::{SurfaceCachedState, ToplevelSurface, XdgToplevelSurfaceData},
    },
};

use crate::clients::Process;
use crate::server::Server;

pub(crate) struct ClientWindow {
    pub id: i64,
    pub window: crate::surface::Surface,
    pub opened: bool,
    pub mapped: bool,
    pub title_dirty: bool,
    /// What the state says of how it is shown, read once per state revision (see [`Shown`]).
    pub shown: Shown,
    pub frames: crate::frames::Frames,
}

/// How the state says a window is shown: its presentation and its entry's settings for the picture.
/// Each is a search of the library for the window's entry, so they are read once per state revision
/// ([`Server::sync_windows`]), not on every frame and commit.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Shown {
    pub presentation: Option<clamshell_state::presentation::WindowPresentation>,
    pub clarity: clamshell_state::presentation::Clarity,
    pub curve: Option<clamshell_state::curve::Curve>,
    pub hdr_peak: u16,
    pub fps_cap: Option<u16>,
}

impl Shown {
    pub fn of(state: &State, id: i64) -> Shown {
        Shown {
            presentation: state.window_presentation(id),
            clarity: state.window_clarity(id),
            curve: state.window_curve(id),
            hdr_peak: state.window_hdr_peak(id),
            fps_cap: state.window_fps_cap(id),
        }
    }
}

impl Server {
    pub fn surface_id(&self, surface: &WlSurface) -> Option<i64> {
        self.windows.iter().find(|w| w.window.root().as_ref() == Some(surface)).map(|w| w.id)
    }

    pub fn add_toplevel(&mut self, surface: ToplevelSurface) {
        if self.surface_id(surface.wl_surface()).is_some() {
            self.reject_client(surface.wl_surface(), "wl_surface already has a toplevel");
            return;
        }
        let Some(next) = self.next_id.checked_add(1) else {
            log::error!("compositor: window ids exhausted");
            surface.send_close();
            return;
        };
        self.windows.push(ClientWindow {
            id: self.next_id,
            window: crate::surface::Surface::Wayland(SurfaceWindow::new_wayland_window(surface)),
            opened: false,
            mapped: false,
            title_dirty: false,
            shown: Shown::of(self.core.state(), self.next_id),
            frames: Default::default(),
        });
        self.next_id = next;
    }

    pub fn facts(&self, surface: &ToplevelSurface, id: i64) -> WindowFacts {
        let (app_id, title, parent, dialog) = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .map(|data| {
                    let dialog = data.dialog_hint != smithay::wayland::shell::xdg::dialog::ToplevelDialogHint::Unknown;
                    (data.app_id.clone().unwrap_or_default(), data.title.clone().unwrap_or_default(), data.parent.clone(), dialog)
                })
                .unwrap_or_default()
        });
        let process = self
            .core
            .state()
            .session()
            .window(id)
            .map(|window| Process { unit: window.unit.clone(), pids: window.pids.clone(), steam_app: None })
            .unwrap_or_else(|| {
                // Its client's, identified before it was admitted.
                surface.wl_surface().client().and_then(|c| c.get_data::<crate::clients::ClientState>()?.process.clone()).unwrap_or_default()
            });
        let limits = Self::size_limits(surface);
        WindowFacts {
            id,
            app_id,
            title,
            colour: self.windows.iter().find(|w| w.id == id).and_then(|client| self.window_colour(&client.window)),
            parent: parent.and_then(|p| self.surface_id(&p)),
            unit: process.unit,
            pids: process.pids,
            limits,
            role: if dialog { clamshell_state::presentation::WindowRole::Dialog } else { clamshell_state::presentation::WindowRole::Normal },
            ..WindowFacts::default()
        }
    }

    fn size_limits(surface: &ToplevelSurface) -> clamshell_state::presentation::WindowLimits {
        with_states(surface.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            clamshell_state::presentation::WindowLimits {
                min: (current.min_size.w.max(0) as u32, current.min_size.h.max(0) as u32),
                max: (current.max_size.w.max(0) as u32, current.max_size.h.max(0) as u32),
            }
        })
    }

    /// A window's colour, as the state has it: the description of the surface holding its picture (see
    /// `Surface::picture`: Wine declares a game's on the subsurface it draws in), else its toplevel's.
    pub(crate) fn window_colour(&self, window: &crate::surface::Surface) -> Option<std::sync::Arc<clamshell_state::colour::ColourDescription>> {
        use smithay::backend::renderer::element::Id;
        let picture = window.picture_surface().map(|surface| Id::from_wayland_resource(&surface));
        let root = window.root().map(|surface| Id::from_wayland_resource(&surface));
        picture.and_then(|id| self.colour.surfaces.get(&id)).or_else(|| root.and_then(|id| self.colour.surfaces.get(&id))).cloned()
    }

    pub fn open_on_commit(&mut self, surface: &WlSurface) {
        let Some(index) = self.windows.iter().position(|w| w.window.root().as_ref() == Some(surface)) else { return };
        let Some(client) = self.windows.get(index) else { return };
        let Some(toplevel) = client.window.toplevel().cloned() else { return };
        let id = client.id;
        if !client.opened {
            let facts = self.facts(&toplevel, id);
            let colour = facts.colour.as_ref().map_or("unspecified".to_string(), |c| format!("{:?} {:?}", c.primaries, c.transfer));
            let (app_id, title) = (facts.app_id.clone(), facts.title.clone());
            self.core.handle(WindowEvent::Opened(facts).into());
            let focused = self.core.state().session().top_window().is_some_and(|w| w.id == id);
            log::info!("compositor: window {id} opens: {app_id:?} {title:?}, focused {focused}, colour {colour}");
            if let Some(client) = self.windows.get_mut(index) {
                client.opened = true;
            }
        } else {
            let limits = Self::size_limits(&toplevel);
            let colour = self.windows.get(index).and_then(|client| self.window_colour(&client.window));
            let old = self.core.state().session().window(id).map(|old| (old.limits, old.colour.clone()));
            if let Some((_, old_colour)) = &old
                && *old_colour != colour
            {
                let shown = colour.as_ref().map(|c| format!("{:?} {:?}", c.primaries, c.transfer));
                log::info!("compositor: window {id}'s colour is now {}", shown.as_deref().unwrap_or("unspecified"));
            }
            if old.is_some_and(|(old_limits, old_colour)| old_limits != limits || old_colour != colour) {
                self.core.handle(WindowEvent::Changed(self.facts(&toplevel, id)).into());
            }
        }
        self.sync_windows();
        if !toplevel.is_initial_configure_sent() {
            toplevel.send_configure();
        }
    }

    pub fn sync_windows(&mut self) {
        let revision = self.core.state_revision();
        if self.presentation_revision != Some(revision) {
            for client in &mut self.windows {
                client.shown = Shown::of(self.core.state(), client.id);
            }
            self.presentation_revision = Some(revision);
        }
        self.place();
        self.sync_x11_popups();
        self.validate_capture();
        self.cancel_hidden_touches();
        let focus = self.core.state().session().keyboard_focus();
        for client in &self.windows {
            let Some(window) = self.core.state().session().window(client.id) else { continue };
            if let Some(surface) = client.window.toplevel() {
                let area = self.area(self.core.state().session().below(client.id));
                let Some(policy) = client.shown.presentation else { continue };
                let fill = fill_size(client.shown.clarity, area.size);
                let size = crate::geometry::configure_size(window, policy, area.size, fill, client.window.geometry().size);
                surface.with_pending_state(|pending| {
                    pending.size = size;
                    pending.bounds = Some(area.size);
                    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                    if policy.layout == clamshell_state::presentation::WindowLayout::Fullscreen {
                        pending.states.set(State::Fullscreen);
                    } else {
                        pending.states.unset(State::Fullscreen);
                    }
                    if focus == Some(client.id) {
                        pending.states.set(State::Activated);
                    } else {
                        pending.states.unset(State::Activated);
                    }
                });
                if surface.is_initial_configure_sent() {
                    surface.send_pending_configure();
                }
            }
            let Some(root) = client.window.root() else { continue };
            let output = if self.core.state().session().below(client.id) { &self.screens.outputs[1] } else { &self.screens.outputs[0] };
            for candidate in &self.screens.outputs {
                if candidate == output {
                    candidate.enter(&root);
                } else {
                    candidate.leave(&root);
                }
            }
        }
        let surface = focus.and_then(|id| self.windows.iter().find(|w| w.id == id && w.mapped)).and_then(|w| w.window.root());
        let surface = self.menu_focus(surface);
        if let Some(keyboard) = self.seat.get_keyboard()
            && keyboard.current_focus() != surface
        {
            self.core.handle(clamshell_state::Event::TextInput(false));
            if surface.is_none() {
                self.protocols.activation.retain_tokens(|_, _| false);
                smithay::wayland::selection::data_device::set_data_device_focus(&self.dh, &self.seat, None);
                smithay::wayland::selection::primary_selection::set_primary_focus(&self.dh, &self.seat, None);
            }
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }

    /// Tells the state the titles Wayland windows changed since the last frame.
    pub(crate) fn flush_titles(&mut self) {
        for index in 0..self.windows.len() {
            let pending = self.windows.get_mut(index).and_then(|client| {
                std::mem::take(&mut client.title_dirty).then(|| client.window.toplevel().cloned().map(|surface| (client.id, surface))).flatten()
            });
            if let Some((id, surface)) = pending {
                let title = with_states(surface.wl_surface(), |states| {
                    states.data_map.get::<XdgToplevelSurfaceData>().and_then(|data| data.lock().ok()).and_then(|data| {
                        let title = data.title.as_deref().unwrap_or_default();
                        self.core.state().session().window(id).filter(|old| old.title != title).map(|_| title.to_owned())
                    })
                });
                if let Some(title) = title {
                    self.core.handle(WindowEvent::Title { window: id, title }.into());
                }
            }
        }
    }
}

/// What a fullscreen window fills in an area of `area`: the area, or the resolution its game is set to (a
/// fraction of it, `docs/picture.md`), which the window is then scaled up from.
pub(crate) fn fill_size(clarity: clamshell_state::presentation::Clarity, area: Size<i32, Logical>) -> Size<i32, Logical> {
    let (w, h) = clarity.resolution.size_on((area.w.max(0) as u32, area.h.max(0) as u32));
    (w as i32, h as i32).into()
}
