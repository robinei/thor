//! Apps' file choosers, picked in the Library (see [`crate::portal`]).

use clamshell_library::Builtin;

use crate::library_view::pick::PickFor;
use crate::out::Out;
use crate::portal::PortalEvent;
use crate::{Effect, State};

impl State {
    pub(crate) fn on_portal(&mut self, e: PortalEvent) {
        match e {
            // Picked when its turn comes (`follow_portal`).
            PortalEvent::Asked(request) => self.portal.asked.push(request),
            PortalEvent::Closed(id) => {
                self.portal.asked.retain(|r| r.id != id);
                if self.picking_for_app() == Some(id) {
                    self.library_view.end_pick();
                    self.portal_back();
                }
            }
        }
    }

    /// Runs after every event. An app's pick answered is sent back and what was in front before it comes
    /// back; then the next request waiting is picked, the Library in front, once no other pick goes on.
    /// Without a Files tab to pick in, a request is answered at once with nothing chosen.
    pub(crate) fn follow_portal(&mut self, out: &mut Out) {
        if let Some((request, path)) = self.library_view.take_answered() {
            self.portal.asked.retain(|r| r.id != request);
            out.effects.push(Effect::FileChosen { request, path });
            self.portal_back();
        }
        while self.library_view.pick().is_none()
            && let Some(request) = self.portal.asked.first()
        {
            let (id, title) = (request.id, request.title.clone());
            if self.library_view.start_pick(request.takes(), PickFor::App { request: id, title }) {
                self.portal.back = self.session.top_window().and_then(|w| w.group.clone());
                self.show_builtin(Builtin::Library);
                break;
            }
            // (The portal thread logs it, as it does every answer.)
            self.portal.asked.remove(0);
            out.effects.push(Effect::FileChosen { request: id, path: None });
        }
    }

    /// The portal thread started again: the requests it held are gone, and a pick for one ends.
    pub(crate) fn portal_restarted(&mut self) {
        self.portal.asked.clear();
        if self.picking_for_app().is_some() {
            self.library_view.end_pick();
            self.portal_back();
        }
    }

    /// The request picked for, if an app's pick goes on.
    fn picking_for_app(&self) -> Option<u32> {
        match self.library_view.pick().map(|p| &p.purpose) {
            Some(PickFor::App { request, .. }) => Some(*request),
            _ => None,
        }
    }

    /// Shows again the group that was in front when the pick began, if the Library still shows and the group
    /// still has windows on the top screen.
    fn portal_back(&mut self) {
        let Some(group) = self.portal.back.take() else { return };
        let shown = self.session.windows().iter().any(|w| w.group.as_ref() == Some(&group) && !self.session.below(w.id));
        if shown && self.session.shown_builtin() == Some(Builtin::Library) {
            self.session.focus(&group);
        }
    }
}
