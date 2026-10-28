//! Apps' file choosers, picked in the Library (see [`crate::portal`]).

use clamshell_library::Builtin;

use crate::library_view::pick::PickFor;
use crate::out::Out;
use crate::portal::{PortalEvent, Waiting};
use crate::{Effect, State};

impl State {
    pub(crate) fn on_portal(&mut self, e: PortalEvent, out: &mut Out) {
        match e {
            // Picked when its program is in front (`follow_portal`).
            PortalEvent::Asked(request) => {
                let program = self.program_of(&request.asker);
                if program.is_none() && !request.asker.is_empty() {
                    out.log.push(format!("portal: request {}: process {:?} has no window: the program in front's", request.id, request.asker.first()));
                }
                // In front: the window's program, or while an app's pick shows over it, that app's.
                let program = program.or_else(|| self.session.top_window().and_then(|w| w.group.clone())).or_else(|| self.portal.back.clone());
                self.portal.asked.push(Waiting { request, program });
            }
            PortalEvent::Closed(id) => {
                self.portal.asked.retain(|w| w.request.id != id);
                if self.picking_for_app() == Some(id) {
                    self.library_view.end_pick();
                    self.portal_back();
                }
            }
        }
    }

    /// The program of the process `asker` names first: the group of the window whose process is the
    /// nearest of its parents (itself first).
    fn program_of(&self, asker: &[u32]) -> Option<crate::session::windows::Group> {
        let windows = self.session.windows();
        let nearest = |w: &&crate::session::windows::Window| w.pids.first().and_then(|own| asker.iter().position(|p| p == own));
        windows
            .iter()
            .filter(|w| w.group.is_some())
            .min_by_key(|w| nearest(w).unwrap_or(usize::MAX))
            .filter(|w| nearest(w).is_some())
            .and_then(|w| w.group.clone())
    }

    /// Runs after every event. An app's pick answered is sent back and its program comes back in front;
    /// one left for something else (another program, or a view other than the Library) is set aside, or
    /// answered with nothing if it is no program's. Then a request whose program is in front is picked,
    /// the Library over it, once no other pick goes on. Without a Files tab to pick in, a request is
    /// answered at once with nothing.
    pub(crate) fn follow_portal(&mut self, out: &mut Out) {
        // Order: set aside before the answers are taken, since one that is no program's is answered so.
        if self.picking_for_app().is_some() && self.session.shown_builtin() != Some(Builtin::Library) {
            self.set_pick_aside();
        }
        if let Some((request, paths)) = self.library_view.take_answered() {
            self.portal.asked.retain(|w| w.request.id != request);
            out.effects.push(Effect::FileChosen { request, paths });
            self.portal_back();
        }
        let front = self.session.top_window().and_then(|w| w.group.clone());
        while self.library_view.pick().is_none()
            && let Some(i) = self.portal.asked.iter().position(|w| w.program.is_none() || w.program == front)
        {
            let Some(request) = self.portal.asked.get(i).map(|w| w.request.clone()) else { break };
            let id = request.id;
            if self.library_view.start_pick(request.takes(), PickFor::App { request: id, title: request.title.clone() }) {
                if let Some(folder) = &request.folder
                    && let Err(e) = self.library_view.go_to(folder)
                {
                    out.log.push(format!("portal: request {id}: not started in {}: {e}", folder.display()));
                }
                self.portal.back = front;
                self.show_builtin(Builtin::Library);
                break;
            }
            out.log.push(format!("portal: request {id} answered with nothing: the library has no Files tab to pick in"));
            self.portal.asked.remove(i);
            out.effects.push(Effect::FileChosen { request: id, paths: Vec::new() });
        }
    }

    /// Leaves an app's pick for something else: it waits for its program to be in front again. One that
    /// is no program's is answered with nothing (it would come straight back).
    pub(crate) fn set_pick_aside(&mut self) {
        let Some(id) = self.picking_for_app() else { return };
        let programs = self.portal.asked.iter().find(|w| w.request.id == id).is_some_and(|w| w.program.is_some());
        if programs {
            self.library_view.end_pick();
        } else {
            self.library_view.cancel_pick();
        }
        self.portal.back = None;
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
    pub(crate) fn picking_for_app(&self) -> Option<u32> {
        match self.library_view.pick().map(|p| &p.purpose) {
            Some(PickFor::App { request, .. }) => Some(*request),
            _ => None,
        }
    }

    /// Shows again the program whose request was picked, if the Library still shows and the program still
    /// has windows on the top screen.
    fn portal_back(&mut self) {
        let Some(group) = self.portal.back.take() else { return };
        let shown = self.session.windows().iter().any(|w| w.group.as_ref() == Some(&group) && !self.session.below(w.id));
        if shown && self.session.shown_builtin() == Some(Builtin::Library) {
            self.session.focus(&group);
        }
    }
}
