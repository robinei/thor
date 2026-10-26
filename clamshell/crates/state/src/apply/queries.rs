//! What the UI and the rules ask that needs more than one area: an entry, the settings in force, the
//! settings panel, the pad's Back, the profiles' names.

use crate::bottom::screen::Power;
use crate::profiles::picker::{Kind as ProfileKind, PickerCx, ProfileId};
use crate::settings::{BottomWhenIdle, Values};
use crate::{Entry, Panel, Resume, SettingsId, State};

impl State {
    /// A library entry by its id. An entry that is still running after leaving the library, because its
    /// file was deleted, is returned as it was.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.session.entry(self.shelf.library(), id)
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
    /// has one.
    fn runner_of(&self, id: &str) -> Option<&str> {
        self.entry(id).and_then(|g| g.runner.as_ref()).map(|r| &*r.key)
    }

    /// The settings entry or level `id` runs with: the global ones, then its runner's, then its own.
    pub(crate) fn settings_of(&self, id: &str) -> Values {
        self.prefs.settings().of_in(id, self.runner_of(id).as_slice())
    }

    /// The settings panel as it is now (see [`Panel::new`]).
    pub fn panel(&self) -> Panel<'_> {
        Panel::new(&self.session, &self.shelf, &self.prefs, &self.keymapping, &self.curving)
    }

    /// Whether the second screen is dark (see [`crate::Bottom::dark`]).
    pub fn bottom_dark(&self) -> bool {
        self.bottom.dark(self.session.program().is_some(), self.in_force().choice::<BottomWhenIdle>())
    }

    /// What the second screen's power follows, as things are now.
    pub(crate) fn power(&self) -> Power {
        Power {
            any_down: self.session.bottom_window().is_some(),
            in_game: self.session.program().is_some(),
            when_idle: self.in_force().choice::<BottomWhenIdle>(),
            discharging: self.sys.status().discharging,
            refreshing: self.device.calibrating(),
        }
    }

    /// What the pad's Back button does on ClamShell's screens, as the target it activates. In the editor
    /// it is Cancel. In the dock's menu or move mode it is the dock's Back. On the keymap screens it goes
    /// back one screen, or gives up picking a control. Nothing else has a back of its own. On the edit
    /// screen while the pad listens, Back comes in as an input to remap.
    pub fn pad_back(&self) -> Option<crate::Id> {
        if self.bottom.editor().is_some() {
            return Some(crate::Id::Editor(crate::EditorId::Cancel));
        }
        if (self.dock.menu().is_some() || self.dock.moving().is_some()) && self.bottom.tab() == crate::Tab::Dock {
            return Some(crate::Id::Dock(crate::DockId::Back));
        }
        if self.curving.screen_in(self.bottom.tab()).is_some() {
            return Some(crate::Id::Profile(ProfileKind::Curve, ProfileId::Back));
        }
        if self.prefs.menu_in(self.bottom.tab()).is_some() {
            return Some(crate::Id::Settings(SettingsId::Back));
        }
        self.keymapping.screen_in(self.bottom.tab()).map(|_| crate::Id::Profile(ProfileKind::Keymap, ProfileId::Back))
    }

    /// Where shelf and the second screen are, to come back to.
    pub(crate) fn resume(&self) -> Resume {
        Resume::of(&self.shelf, &self.bottom)
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
        }
    }

    /// Whether profile `id` of `kind` is a game's own.
    pub fn profile_is_own(&self, kind: ProfileKind, id: u16) -> bool {
        match kind {
            ProfileKind::Keymap => self.keymapping.is_own(id),
            ProfileKind::Curve => self.curving.curves().is_own(id),
        }
    }

    /// Whether profile `id` of `kind` is there.
    pub fn profile_exists(&self, kind: ProfileKind, id: u16) -> bool {
        match kind {
            ProfileKind::Keymap => self.keymapping.keymaps().get(id).is_some(),
            ProfileKind::Curve => self.curving.curves().get(id).is_some(),
        }
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
