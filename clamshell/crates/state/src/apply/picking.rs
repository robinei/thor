//! Apps' file choosers, picked in the Library (see [`crate::portal`]).

use crate::library_view::pick::PickFor;
use crate::out::Out;
use crate::portal::PortalEvent;
use crate::{Effect, State};

impl State {
    pub(crate) fn on_portal(&mut self, e: PortalEvent, out: &mut Out) {
        match e {
            // Picked when its program is in front (`follow_portal`).
            PortalEvent::Asked(request) => {
                let program = self.program_of(&request.asker);
                if program.is_none() && !request.asker.is_empty() {
                    out.log(
                        crate::LogLevel::Debug,
                        format!("portal: request {}: process {:?} has no window: the program in front's", request.id, request.asker.first()),
                    );
                }
                // In front: the window's program, or while an app's pick shows over it, that app's.
                let program = program.or_else(|| self.session.top_window().and_then(|w| w.group.clone())).or_else(|| self.portal.back().cloned());
                self.portal.ask(request, program);
            }
            PortalEvent::Closed(id) => {
                self.portal.forget(id);
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
    /// the Library over it, once no other pick shows: a pick of ClamShell's own left behind a window waits
    /// for it (see [`crate::library_view::LibraryView::end_pick`]). Without a Files tab to pick in, a request is
    /// answered at once with nothing.
    pub(crate) fn follow_portal(&mut self, out: &mut Out) {
        // Order: set aside before the answers are taken, since one that is no program's is answered so.
        if self.picking_for_app().is_some() && !self.home_in_front() {
            self.set_pick_aside();
        }
        if let Some((request, paths)) = self.library_view.take_answered() {
            self.portal.forget(request);
            out.effects.push(Effect::FileChosen { request, paths });
            self.portal_back();
        }
        let front = self.session.top_window().and_then(|w| w.group.clone());
        // A program with no window left (closed to its tray, say) can't come in front: its request is
        // picked over whatever is.
        let windowless = |program: &crate::session::windows::Group| !self.session.windows().iter().any(|w| w.group.as_ref() == Some(program));
        // While an app needed now shows (Setup), the requests wait: it is in front of the home app.
        while crate::builtin_apps::needed(self).is_none()
            && self.pick_free()
            && let Some(request) = self.portal.next(front.as_ref(), windowless).cloned()
        {
            let id = request.id;
            if let crate::portal::Asks::Save { name } = &request.asks
                && !name.is_empty()
                && !crate::library_view::pick::file_name(name)
            {
                out.log(crate::LogLevel::Info, format!("portal: request {id}: {name:?} isn't a file's name: Save here waits for one typed"));
            }
            if self.library_view.start_pick(request.takes(), PickFor::App { request: id, title: request.title.clone() }) {
                if let Some(folder) = &request.folder
                    && let Err(e) = self.library_view.go_to(folder)
                {
                    out.log(crate::LogLevel::Info, format!("portal: request {id}: not started in {}: {e}", folder.display()));
                }
                self.portal.set_back(front);
                self.show_app(crate::builtin_apps::home());
                break;
            }
            out.log(crate::LogLevel::Warn, format!("portal: request {id} answered with nothing: the library has no Files tab to pick in"));
            self.portal.forget(id);
            out.effects.push(Effect::FileChosen { request: id, paths: Vec::new() });
        }
    }

    /// Leaves an app's pick for something else: it waits for its program to be in front again. One that
    /// is no program's is answered with nothing (it would come straight back).
    pub(crate) fn set_pick_aside(&mut self) {
        let Some(id) = self.picking_for_app() else { return };
        if self.portal.programs(id) {
            self.library_view.end_pick();
        } else {
            self.library_view.cancel_pick();
        }
        self.portal.set_back(None);
    }

    /// The portal thread started again: the requests it held are gone, and a pick for one ends.
    pub(crate) fn portal_restarted(&mut self) {
        self.portal.restarted();
        if self.picking_for_app().is_some() {
            self.library_view.end_pick();
            self.portal_back();
        }
    }

    /// Whether an app's pick may start: no pick goes on, or one of ClamShell's own that isn't shown.
    fn pick_free(&self) -> bool {
        match self.library_view.pick().map(|p| &p.purpose) {
            None => true,
            Some(PickFor::App { .. }) => false,
            Some(_) => !self.home_in_front(),
        }
    }

    /// Whether the home app is in front, where picks are made.
    fn home_in_front(&self) -> bool {
        matches!(self.top(), crate::Top::App { app, .. } if app == crate::builtin_apps::home())
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
        let Some(group) = self.portal.take_back() else { return };
        let shown = self.session.windows().iter().any(|w| w.group.as_ref() == Some(&group) && !self.session.below(w.id));
        if shown && self.session.shown_app() == Some(crate::builtin_apps::home()) {
            self.session.focus(&group);
        }
    }
}
