//! What the UI and the rules ask that needs more than one area: an entry, the settings in force, the
//! settings panel, the profiles' names.

use crate::bottom::screen::Power;
use crate::profiles::picker::{Kind as ProfileKind, PickerCx};
use crate::settings::{BottomWhenIdle, Values};
use crate::{Entry, Panel, Resume, State};

impl State {
    /// A library entry by its id. An entry that is still running after leaving the library, because its
    /// file was deleted, is returned as it was.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.session.entry(self.library_view.library(), id)
    }

    /// Whether the dock menu offers Open another for `id`. It does when `id` is running and another launch
    /// of it makes sense (see [`crate::Entry::another`]).
    pub fn opens_another(&self, id: &str) -> bool {
        // Not for an entry handed over to another program, such as Android's: launching it again just
        // shows the one already running.
        self.session.running(id).is_some() && self.entry(id).is_some_and(|g| g.another && !g.runner.as_ref().is_some_and(|r| r.handover))
    }

    /// The settings in force: those of the program in force (see [`crate::session::program`]), or else
    /// the global ones.
    pub fn in_force(&self) -> Values {
        self.session.program().as_ref().map_or(self.prefs.settings().global, |id| self.settings_of(id))
    }

    /// The level that entry `id` takes its settings from before its own, which is its runner's, if it
    /// has one (see [`crate::Session::settings_entry`]).
    fn runner_of(&self, id: &str) -> Option<&str> {
        self.session.settings_entry(self.library_view.library(), id).and_then(|g| g.runner.as_ref()).map(|r| &*r.key)
    }

    /// The settings entry or level `id` runs with: the global ones, then its runner's, then its own.
    pub(crate) fn settings_of(&self, id: &str) -> Values {
        self.prefs.settings().of_in(id, self.runner_of(id).as_slice())
    }

    /// The kinds of profile, for the settings panel.
    pub(crate) fn kinds(&self) -> crate::settings::panel::Kinds<'_> {
        crate::settings::panel::Kinds { keymapping: &self.keymapping, curving: &self.curving, lighting: &self.lighting }
    }

    /// The settings panel as it is now (see [`Panel::new`]).
    pub fn panel(&self) -> Panel<'_> {
        Panel::new(self.subject(), &self.session, &self.library_view, &self.prefs, self.kinds(), self.device_view.controllers().count())
    }

    /// Whether the second screen is dark (see [`crate::Bottom::dark`]). A window shown there keeps it lit:
    /// it was put there to be watched.
    pub fn bottom_dark(&self) -> bool {
        !self.bottom.shows_windows(&self.session) && self.bottom.dark(self.session.program().is_some(), self.in_force().choice::<BottomWhenIdle>())
    }

    /// What the second screen's power follows, as things are now.
    pub(crate) fn power(&self) -> Power {
        Power {
            any_down: self.session.bottom_window().is_some(),
            in_game: self.session.program().is_some(),
            when_idle: self.in_force().choice::<BottomWhenIdle>(),
            discharging: self.sys.status().discharging,
            refreshing: self.device_view.calibrating(),
        }
    }

    /// Where the Library and the second screen are, to come back to.
    pub(crate) fn resume(&self) -> Resume {
        Resume::of(&self.library_view, &self.bottom)
    }

    /// A hash of the place as [`State::resume`] saves it: the Library's (see `LibraryView::hash_place`) and the second
    /// screen's chosen tab.
    pub(crate) fn place_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        self.library_view.hash_place(&mut hasher);
        self.bottom.chosen_tab().hash(&mut hasher);
        hasher.finish()
    }

    /// Whether the pad listens for inputs (see [`crate::keymap::screens::Keymapping::listening`]).
    pub(crate) fn listening(&self) -> bool {
        self.keymapping.listening(self.bottom.tab(), self.bottom.editor().is_some())
    }

    /// Profile `id` of `kind`'s name as shown (see [`crate::profiles::Profiles::shown_name`]).
    pub fn profile_name(&self, kind: ProfileKind, id: u16) -> &str {
        match kind {
            ProfileKind::Keymap => self.keymapping.name(id),
            ProfileKind::Curve => self.curving.name(id),
            ProfileKind::Lights => self.lighting.name(id),
        }
    }

    /// Whether profile `id` of `kind` is a game's own.
    pub fn profile_is_own(&self, kind: ProfileKind, id: u16) -> bool {
        match kind {
            ProfileKind::Keymap => self.keymapping.is_own(id),
            ProfileKind::Curve => self.curving.curves().is_own(id),
            ProfileKind::Lights => self.lighting.sets().is_own(id),
        }
    }

    /// Whether profile `id` of `kind` is there.
    pub fn profile_exists(&self, kind: ProfileKind, id: u16) -> bool {
        match kind {
            ProfileKind::Keymap => self.keymapping.keymaps().get(id).is_some(),
            ProfileKind::Curve => self.curving.curves().get(id).is_some(),
            ProfileKind::Lights => self.lighting.sets().get(id).is_some(),
        }
    }

    /// What the stick lights show now, left then right (see [`crate::lights`]): while a set is on its edit
    /// screen in the settings tab, that set's colours, so that each change shows as it is made; else the
    /// set the settings in force choose, showing the charge while charging if it says so.
    pub fn lights_shown(&self) -> [crate::lights::Lit; 2] {
        if self.bottom.tab() == crate::Tab::Settings
            && let Some(id) = self.lighting.editing()
        {
            return self.lighting.in_force(id).shown(None);
        }
        let lights = self.lighting.in_force(self.in_force().get(crate::settings::Setting::Lights));
        let status = self.sys.status();
        lights.shown(status.battery.filter(|_| status.charging))
    }

    /// What the keymaps' screens read from the settings panel (see [`PickerCx`]).
    pub(crate) fn keymap_cx(&self) -> PickerCx {
        self.picker_cx(ProfileKind::Keymap)
    }

    /// What `kind`'s picker reads from the settings panel (see [`PickerCx`]).
    pub(crate) fn picker_cx(&self, kind: ProfileKind) -> PickerCx {
        let panel = self.panel();
        PickerCx { entry: panel.editing().map(|g| g.id.clone()), chosen: panel.profile_chosen(kind) }
    }
}
