//! The settings tab's handlers: its tiles, their menus, and the profile pickers and keymap screens
//! opened from it.

use crate::out::Out;
use crate::profiles::picker::{Kind as ProfileKind, ProfileEvent, ProfileId};
use crate::settings::Setting;
use crate::{KeymapEvent, KeymapId, RowPart, SettingsId, State};

impl State {
    /// A tap or long press in the settings tab.
    pub(crate) fn on_settings(&mut self, id: SettingsId, long: bool, out: &mut Out) {
        match id {
            // A long press opens the tile's menu: a profile tile's picker, or the values.
            SettingsId::Tile(Setting::Keymap) if long => self.keymapping.open_picker(),
            SettingsId::Tile(Setting::Curve) if long => self.curving.open_picker(),
            SettingsId::Tile(setting) if long => self.prefs.open_menu(setting),
            SettingsId::Tile(setting @ (Setting::Keymap | Setting::Curve)) => self.next_profile(ProfileKind::of(setting), out),
            SettingsId::Tile(setting) => {
                let (len, now) = (self.menu_values_len(setting), self.panel().tile_value(setting));
                let at = (0..len).find(|i| self.menu_value(setting, *i) == Some(now));
                if let Some(next) = self.menu_value(setting, at.map_or(0, |i| (i + 1) % len.max(1))) {
                    self.change(setting, Some(next), out);
                }
            }
            SettingsId::Back => self.prefs.close_menu(),
            SettingsId::Env => {
                let entry = self.panel().editing().map(|g| g.id.clone());
                self.prefs.open_env_editor(entry, self.bottom.editors_mut());
            }
            SettingsId::Scope(level) => self.prefs.set_level(level),
            SettingsId::Reset => {
                if let Some(id) = self.panel().editing().map(|g| g.id.clone()) {
                    self.prefs.reset(&id);
                }
                out.effects.push(self.prefs.save_effect());
            }
        }
    }

    /// The values a tile steps through and its menu lists, as their number and the one at `i`: the
    /// screen's announced rates after "as is" for the refresh rate (see [`crate::presentation::modes::DisplayModes`]), the setting's
    /// choices for the others. Drawing reads them every frame, so nothing is allocated.
    fn menu_values_len(&self, setting: Setting) -> usize {
        match setting {
            Setting::Refresh => 1 + self.presentation.display_modes().rates().len(),
            _ => setting.choices().len(),
        }
    }

    fn menu_value(&self, setting: Setting, i: usize) -> Option<u16> {
        match (setting, i) {
            (Setting::Refresh, 0) => Some(0),
            (Setting::Refresh, i) => self.presentation.display_modes().rates().get(i - 1).copied(),
            _ => setting.choices().get(i).copied(),
        }
    }

    /// The rows of the open tile's menu: its values, then Reset when there is something to reset.
    pub(crate) fn menu_rows_len(&self) -> usize {
        self.prefs.menu().map_or(0, |setting| self.menu_values_len(setting) + usize::from(self.panel().resettable(setting)))
    }

    /// Row `i` of the open tile's menu: a value, or with `None`, Reset.
    pub fn menu_row(&self, i: usize) -> Option<Option<u16>> {
        let setting = self.prefs.menu()?;
        match self.menu_value(setting, i) {
            Some(value) => Some(Some(value)),
            None => (i == self.menu_values_len(setting) && self.panel().resettable(setting)).then_some(None),
        }
    }

    /// A row of the open tile's menu: its value is set where the panel edits, or Reset drops the level's
    /// own; either closes the menu.
    pub(crate) fn on_menu_row(&mut self, i: usize, out: &mut Out) {
        let (Some(setting), Some(value)) = (self.prefs.menu(), self.menu_row(i)) else {
            return;
        };
        self.change(setting, value, out);
        self.prefs.close_menu();
    }

    /// A profile tile's tap: the next of `kind`'s profiles where the panel edits, after Default (or Off),
    /// the level's own, and the named ones in order, then Default again.
    fn next_profile(&mut self, kind: ProfileKind, out: &mut Out) {
        let entry = self.panel().editing().map(|g| g.id.clone());
        let now = self.panel().profile_chosen(kind);
        let next = match kind {
            ProfileKind::Keymap => next_profile(self.keymapping.keymaps(), entry.as_deref(), now),
            ProfileKind::Curve => next_profile(self.curving.curves(), entry.as_deref(), now),
        };
        self.choose_profile(kind, next, out);
    }

    /// Changes `setting` where the panel edits, in the game's overrides or the global settings. It is set
    /// to `value`, or back to the default for `None`. The change is saved, and applied after the event
    /// if the settings in force changed (see [`crate::sync`]).
    pub(crate) fn change(&mut self, setting: Setting, value: Option<u16>, out: &mut Out) {
        let entry = self.panel().editing().map(|g| g.id.clone());
        self.prefs.set(entry.as_ref(), setting, value);
        out.effects.push(self.prefs.save_effect());
    }

    /// Chooses profile `id` of `kind` where the panel edits, and saves the settings.
    fn choose_profile(&mut self, kind: ProfileKind, id: u16, out: &mut Out) {
        let entry = self.panel().editing().map(|g| g.id.clone());
        self.prefs.choose(entry.as_ref(), kind.setting(), id);
        out.effects.push(self.prefs.save_effect());
    }

    /// The keymaps' screens' buttons, but Back (the picker's, [`ProfileId::Back`]).
    pub(crate) fn on_keymap(&mut self, id: KeymapId, long: bool, out: &mut Out) {
        let cx = self.keymap_cx();
        self.keymapping.on(id, long, &cx, self.bottom.editors_mut(), out);
    }

    /// A kind's picker's buttons. Back goes back a screen; the others are the picker's.
    pub(crate) fn on_profile(&mut self, kind: ProfileKind, id: ProfileId, out: &mut Out) {
        match kind {
            ProfileKind::Keymap if id == ProfileId::Back => self.keymapping.back(&mut self.keyboard),
            ProfileKind::Keymap => self.keymapping.on_picker(id, self.prefs.settings(), self.bottom.editors_mut(), out),
            ProfileKind::Curve if id == ProfileId::Back => self.curving.back(),
            ProfileKind::Curve => self.curving.on_picker(id, self.prefs.settings(), self.bottom.editors_mut(), out),
        }
    }

    /// A row of the picker (see [`crate::keymap::screens::Keymapping::on_row`]).
    pub(crate) fn on_keymap_row(&mut self, i: usize, part: RowPart, long: bool, out: &mut Out) {
        let cx = self.keymap_cx();
        self.keymapping.on_row(i, part, long, &cx, self.bottom.editors_mut(), out);
    }

    /// A row of the list of layers, as the button it stands for.
    pub(crate) fn on_layer_row(&mut self, i: usize, part: RowPart, long: bool, out: &mut Out) {
        if let Some(id) = self.keymapping.layer_row_id(i, part) {
            self.on_keymap(id, long, out);
        }
    }

    /// What the keymaps' screens ask of the keyboard.
    pub(crate) fn keymap_event(&mut self, event: KeymapEvent, out: &mut Out) {
        match event {
            KeymapEvent::Capture => self.keymapping.capture(&mut self.keyboard, out),
        }
    }

    /// What a kind's picker asks of the settings.
    pub(crate) fn profile_event(&mut self, kind: ProfileKind, event: ProfileEvent, out: &mut Out) {
        match event {
            ProfileEvent::Choose(id) => self.choose_profile(kind, id, out),
            // The settings are saved before the profiles.
            ProfileEvent::MadeOwn(id) => {
                self.choose_profile(kind, id, out);
                out.effects.push(match kind {
                    ProfileKind::Keymap => self.keymapping.save(),
                    ProfileKind::Curve => self.curving.save_effect(),
                });
            }
            ProfileEvent::Deleted(id) => {
                self.prefs.forget(kind.setting(), id);
                out.effects.push(self.prefs.save_effect());
            }
        }
    }
}

/// The profile after `now` that a profile tile steps to for `entry`'s level, through Default (or Off,
/// 0), the level's own if it has one, then the named profiles in order, and back to Default.
fn next_profile<B: Clone + Default + PartialEq>(profiles: &crate::profiles::Profiles<B>, entry: Option<&str>, now: u16) -> u16 {
    let own = entry.and_then(|e| profiles.own_of(e)).map(|p| p.id);
    let order = || std::iter::once(0).chain(own).chain(profiles.named().map(|p| p.id));
    order().position(|id| id == now).and_then(|i| order().nth(i + 1)).unwrap_or(0)
}
