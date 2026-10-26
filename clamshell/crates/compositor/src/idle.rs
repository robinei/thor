//! The compositor's part of idling: when input last came, and whether a shown window inhibits idling
//! (idle-inhibit, as a video player asks while it plays). The state is told when the compositor turns quiet
//! (no input for `NOTIFY_AFTER` seconds and nothing inhibiting) and when input or an inhibitor wakes it;
//! its idle stage decides the rest. An inhibitor counts only while its surface has a buffer and its window
//! is shown.

use crate::server::Server;
use clamshell_state::{
    Event,
    device::idle::{IdleEvent, NOTIFY_AFTER},
};
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    wayland::{compositor::get_parent, idle_inhibit::IdleInhibitHandler},
};
use std::time::{Duration, Instant};

pub(crate) struct Idle {
    pub inhibitors: Vec<WlSurface>,
    last_input: Instant,
    inhibited: bool,
    pub quiet: Option<bool>,
}
impl Idle {
    pub fn new() -> Self {
        Self { inhibitors: Vec::new(), last_input: Instant::now(), inhibited: false, quiet: None }
    }
    pub fn input(&mut self, now: Instant) {
        self.last_input = now;
    }
    fn update(&mut self, now: Instant, inhibited: bool) -> Option<IdleEvent> {
        if self.inhibited && !inhibited {
            self.last_input = now;
        }
        self.inhibited = inhibited;
        let quiet = !inhibited && now.saturating_duration_since(self.last_input) >= Duration::from_secs(NOTIFY_AFTER);
        if self.quiet == Some(quiet) {
            return None;
        }
        self.quiet = Some(quiet);
        Some(if quiet { IdleEvent::CompositorQuiet } else { IdleEvent::CompositorInput })
    }
}
impl IdleInhibitHandler for Server {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle.inhibitors.push(surface);
    }
    fn uninhibit(&mut self, surface: WlSurface) {
        if let Some(index) = self.idle.inhibitors.iter().position(|old| old == &surface) {
            self.idle.inhibitors.remove(index);
        }
    }
}
impl Server {
    pub(crate) fn update_idle(&mut self, now: Instant) {
        self.idle.inhibitors.retain(Resource::is_alive);
        let inhibited = self.idle.inhibitors.iter().any(|surface| {
            if !with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false) {
                return false;
            }
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root).or_else(|| match self.popups.find_popup(&root) {
                Some(smithay::desktop::PopupKind::Xdg(popup)) => popup.get_parent_surface(),
                _ => None,
            }) {
                root = parent;
            }
            self.surface_id(&root).is_some_and(|id| self.visible(id) && self.windows.iter().any(|client| client.id == id && client.mapped))
        });
        if let Some(event) = self.idle.update(now, inhibited) {
            self.core.handle(Event::Idle(event));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_input_and_inhibition_report_only_transitions_and_restart_the_timeout() {
        let mut idle = Idle::new();
        let start = idle.last_input;
        assert_eq!(idle.update(start, false), Some(IdleEvent::CompositorInput));
        assert_eq!(idle.update(start + Duration::from_secs(9), false), None);
        assert_eq!(idle.update(start + Duration::from_secs(10), false), Some(IdleEvent::CompositorQuiet));
        assert_eq!(idle.update(start + Duration::from_secs(20), false), None);
        idle.input(start + Duration::from_secs(21));
        assert_eq!(idle.update(start + Duration::from_secs(21), false), Some(IdleEvent::CompositorInput));
        assert_eq!(idle.update(start + Duration::from_secs(31), false), Some(IdleEvent::CompositorQuiet));
        assert_eq!(idle.update(start + Duration::from_secs(32), true), Some(IdleEvent::CompositorInput));
        assert_eq!(idle.update(start + Duration::from_secs(500), true), None);
        assert_eq!(idle.update(start + Duration::from_secs(600), false), None);
        assert_eq!(idle.update(start + Duration::from_secs(609), false), None);
        assert_eq!(idle.update(start + Duration::from_secs(610), false), Some(IdleEvent::CompositorQuiet));
    }
}
