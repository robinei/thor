//! The lights' screens, shown over the settings tab: the picker ([`crate::profiles::picker`]) and a set's
//! edit screen. The edit screen shows the two sticks lit, Linked, a slider for each part of a colour
//! ([`LightParam`]) and "Charging shows the battery". Linked, the sliders set both sticks; unlinked, they
//! set the stick picked (a tap on it), and the other keeps its colour. Linking again copies the picked
//! stick's colour to the other. A slider changes the set as it moves, so the sticks show each change at
//! once, and the sets are saved when the finger lifts; the switches save at once.

use super::{LightParam, LightSets, Lights, Stick};
use crate::effect::{Effect, Save};
use crate::levels::slider::{Control, Slider};
use crate::profiles::Profiles;
use crate::profiles::picker::{self, Kind, Owner, PickerScreen};

/// Which of the lights' screens is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightsScreen {
    /// One of the picker's screens.
    Picker(PickerScreen),
    /// A set's edit screen.
    Edit(u16),
}

/// The edit screen's buttons, but Back (the picker's, [`crate::profiles::picker::ProfileId::Back`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LightsId {
    /// The Linked switch.
    Linked,
    /// The "Charging shows the battery" switch.
    Battery,
    /// A stick in the picture: picks the stick the sliders set while unlinked.
    Stick(Stick),
}

/// The sets of lights and their screens: every set, the screen open over the settings tab, and the stick
/// the sliders set while unlinked.
#[derive(Debug, Clone)]
pub struct Lighting {
    sets: LightSets,
    open: Option<LightsScreen>,
    stick: Stick,
}

impl Default for Lighting {
    fn default() -> Lighting {
        Lighting { sets: LightSets::default(), open: None, stick: Stick::Left }
    }
}

impl Lighting {
    pub(crate) fn new(sets: LightSets) -> Lighting {
        Lighting { sets, ..Lighting::default() }
    }

    pub fn sets(&self) -> &LightSets {
        &self.sets
    }

    /// The lights' screen open over the settings tab, if one is.
    pub fn screen(&self) -> Option<LightsScreen> {
        self.open
    }

    /// The stick the sliders set while unlinked.
    pub fn stick(&self) -> Stick {
        self.stick
    }

    /// A set's name: "Own lights" for a game's own, and "Lights off" for none or for one that is gone.
    pub fn name(&self, id: u16) -> &str {
        self.sets.shown_name(id, Kind::Lights)
    }

    /// Set `id`, or Off (all lights off) for 0 and for a set that is gone.
    pub fn in_force(&self, id: u16) -> Lights {
        self.sets.get(id).map(|p| p.body).unwrap_or_default()
    }

    /// The set on the edit screen, if it is open.
    pub fn editing(&self) -> Option<u16> {
        match self.open {
            Some(LightsScreen::Edit(id)) => Some(id),
            _ => None,
        }
    }

    /// Goes back one screen: from the edit screen to the picker, within the picker, and from the picker to
    /// the settings.
    pub(crate) fn back(&mut self) {
        self.open = match self.open {
            None => None,
            Some(LightsScreen::Picker(p)) => picker::back(p).map(LightsScreen::Picker),
            Some(LightsScreen::Edit(_)) => Some(LightsScreen::Picker(PickerScreen::List)),
        };
    }

    /// A finger is held or slid on a part's row of the edit screen: its slider, from the picked stick's
    /// value. None off the edit screen.
    pub(crate) fn slider(&self, param: LightParam, x: f32, width: f32) -> Option<Slider> {
        let id = self.editing()?;
        let colour = self.sets.get(id)?.body.colour(self.stick);
        let (lo, hi, _) = param.range();
        Some(Slider::ranged(Control::Light(param), param.position(param.get(&colour)), param.position(lo), param.position(hi), x, width))
    }

    /// A part's slider moved to `position`: the picked stick's colour, and linked the other's too, follow
    /// at once (the sets are saved when the finger lifts).
    pub(crate) fn slid(&mut self, param: LightParam, position: u8) {
        let Some(id) = self.editing() else {
            return;
        };
        let stick = self.stick;
        self.sets.edit(id, |p| {
            let before = p.body;
            param.set(p.body.colour_mut(stick), param.value(position));
            if p.body.linked {
                let colour = p.body.colour(stick);
                p.body.left = colour;
                p.body.right = colour;
            }
            p.body != before
        });
    }

    /// The edit screen's buttons. A switch saves the sets.
    pub(crate) fn on(&mut self, id: LightsId) -> Option<Effect> {
        let set = self.editing()?;
        let stick = self.stick;
        let changed = match id {
            LightsId::Stick(picked) => {
                self.stick = picked;
                false
            }
            // Linking copies the picked stick's colour to the other.
            LightsId::Linked => self.sets.edit(set, |p| {
                p.body.linked = !p.body.linked;
                if p.body.linked {
                    let colour = p.body.colour(stick);
                    p.body.left = colour;
                    p.body.right = colour;
                }
                true
            }),
            LightsId::Battery => self.sets.edit(set, |p| {
                p.body.battery = !p.body.battery;
                true
            }),
        };
        changed.then(|| self.save_effect())
    }

    pub(crate) fn save_effect(&self) -> Effect {
        Effect::Save(Save::Lights(self.sets.clone()))
    }
}

impl Owner for Lighting {
    type Body = Lights;
    const KIND: Kind = Kind::Lights;
    fn profiles(&self) -> &Profiles<Lights> {
        &self.sets
    }
    fn profiles_mut(&mut self) -> &mut Profiles<Lights> {
        &mut self.sets
    }
    fn picker(&self) -> Option<PickerScreen> {
        match self.open {
            Some(LightsScreen::Picker(p)) => Some(p),
            _ => None,
        }
    }
    fn set_picker(&mut self, screen: Option<PickerScreen>) {
        self.open = screen.map(LightsScreen::Picker);
    }
    /// The edit screen opens with the left stick picked.
    fn open_edit(&mut self, id: u16) {
        self.open = Some(LightsScreen::Edit(id));
        self.stick = Stick::Left;
    }
    fn save(&mut self) -> Effect {
        self.save_effect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::lights::{Colour, Lit};
    use crate::profiles::picker::{Kind as ProfileKind, PickerRow, ProfileId};
    use crate::testkit::*;
    use crate::*;

    fn row(i: u16) -> Id {
        ListId::Lights.row(i)
    }

    fn picker(s: &mut State) {
        tap(s, Id::Tab(Tab::Settings));
        long(s, Id::Settings(SettingsId::Tile(Setting::Lights)));
    }

    fn profile(s: &mut State, id: ProfileId) -> Vec<Effect> {
        tap(s, Id::Profile(ProfileKind::Lights, id))
    }

    fn chosen(s: &State) -> Option<u16> {
        s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Lights)
    }

    fn saved(fx: &[Effect]) -> Option<&LightSets> {
        fx.iter().find_map(|e| if let Effect::Save(Save::Lights(l)) = e { Some(l) } else { None })
    }

    fn sent(fx: &[Effect]) -> Option<[Lit; 2]> {
        fx.iter().find_map(|e| if let Effect::Lights(l) = e { Some(*l) } else { None })
    }

    fn slide(s: &mut State, param: LightParam, to: f32) -> Vec<Effect> {
        s.applied(Slide::Start { control: Control::Light(param), x: 300.0, width: 620.0 }.into());
        let moved = s.applied(Slide::To { x: to }.into());
        assert!(saved(&moved).is_none(), "saved only when lifted: {moved:?}");
        [moved, s.applied(Slide::End.into())].concat()
    }

    fn edit(s: &mut State, i: u16) {
        picker(s);
        tap(s, Id::Row { list: ListId::Lights, i, part: RowPart::Edit });
    }

    fn body(s: &State, id: u16) -> Lights {
        s.lighting().in_force(id)
    }

    fn new_state() -> State {
        State::new(Init { lights: LightSets::initial(), ..init() })
    }

    #[test]
    fn the_tile_opens_the_picker_and_a_row_chooses_lights_for_the_game() {
        let mut s = new_state();
        picker(&mut s);
        assert_eq!(s.lighting().screen(), Some(LightsScreen::Picker(PickerScreen::List)));
        assert_eq!(s.lists().len(ListId::Lights), 4, "the game's own (made from it), Off, then the two to start from");
        assert_eq!(s.panel().profile_row(ProfileKind::Lights, 1), Some(PickerRow::Default));
        assert_eq!(s.panel().profile_row(ProfileKind::Lights, 2), Some(PickerRow::Named(1, "Ember")));
        tap(&mut s, row(2));
        assert_eq!(s.lighting().screen(), None, "chosen: the picker closes");
        assert_eq!(chosen(&s), Some(1));
        assert_eq!(s.prefs().settings().users(Setting::Lights, 1).entries, 1);
        // Off for the game.
        picker(&mut s);
        tap(&mut s, row(1));
        assert_eq!(chosen(&s), Some(0));
        assert_eq!(s.lighting().name(0), "Lights off");
        assert_eq!(s.lighting().in_force(0), Lights::default());
    }

    /// The lights follow the settings in force: chosen for all, they are sent at once; a game coming in
    /// front with its own choice sends its lights, and nothing is sent while they stay the same.
    #[test]
    fn the_lights_in_force_are_sent_when_they_change() {
        let mut s = new_state();
        let ember = body(&s, 1).shown(None);
        assert_ne!(ember, [Lit::OFF; 2]);
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Scope(SettingsLevel::Global)));
        long(&mut s, Id::Settings(SettingsId::Tile(Setting::Lights)));
        let fx = tap(&mut s, row(1));
        assert_eq!(sent(&fx), Some(ember), "{fx:?}");
        assert_eq!(sent(&tap(&mut s, Id::Tab(Tab::Dock))), None, "unchanged: nothing sent");
        // A game whose own choice is Ice.
        let ice: std::sync::Arc<str> = "zelda-lttp".into();
        s.settings_mut().set(Some(&ice), Setting::Lights, Some(2));
        let mut fx = tap(&mut s, entry("zelda-lttp"));
        fx.extend(tap(&mut s, entry("zelda-lttp")));
        fx.extend(start(&mut s, "zelda-lttp"));
        fx.extend(show_window(&mut s, "zelda-lttp", 7));
        let lights: Vec<_> = fx.iter().filter_map(|e| if let Effect::Lights(l) = e { Some(*l) } else { None }).collect();
        assert_eq!(lights, [body(&s, 2).shown(None)], "{fx:?}");
        // Back from sleep, which turns them off, they are set again; so after the thread's restart.
        assert_eq!(sent(&s.applied(Event::Resumed)), Some(body(&s, 2).shown(None)));
        assert_eq!(sent(&s.applied(Event::Restarted(Thread::Lights))), Some(body(&s, 2).shown(None)));
        assert_eq!(sent(&s.applied(Event::Restarted(Thread::Brightness))), None);
    }

    #[test]
    fn a_games_own_lights_are_made_from_the_ones_in_force_and_open_on_their_edit_screen() {
        let mut s = new_state();
        picker(&mut s);
        tap(&mut s, row(3));
        picker(&mut s);
        let fx = tap(&mut s, row(0));
        // The own set copies Ice, opens on its edit screen and is chosen; the settings are saved before
        // the sets.
        let own = s.lighting().sets().own_of("zelda-lttp").map(|p| p.id).unwrap();
        assert_eq!(s.lighting().screen(), Some(LightsScreen::Edit(own)));
        assert_eq!(chosen(&s), Some(own));
        assert_eq!(body(&s, own), body(&s, 2));
        let settings = fx.iter().position(|e| matches!(e, Effect::Save(Save::Settings(_)))).unwrap();
        let lights = fx.iter().position(|e| matches!(e, Effect::Save(Save::Lights(_)))).unwrap();
        assert!(settings < lights, "{fx:?}");
        assert_eq!(s.lighting().name(own), "Own lights");
    }

    /// Linked, a slider sets both sticks, and the lights follow it as it moves, before it is saved: the
    /// edit screen shows the set edited, whatever is chosen. Leaving the screen goes back to the lights
    /// in force.
    #[test]
    fn linked_a_slider_sets_both_sticks_and_the_lights_follow_it_live() {
        let mut s = new_state();
        edit(&mut s, 2);
        assert_eq!(s.lighting().screen(), Some(LightsScreen::Edit(1)));
        let before = body(&s, 1);
        let fx = tap(&mut s, Id::Tab(Tab::Settings));
        assert_eq!(sent(&fx), None);
        s.applied(Slide::Start { control: Control::Light(LightParam::Hue), x: 300.0, width: 620.0 }.into());
        assert_eq!(s.held_slider().map(|sl| sl.start), Some(LightParam::Hue.position(before.left.hue)));
        let fx = s.applied(Slide::To { x: 400.0 }.into());
        let moved = body(&s, 1);
        assert!(moved.left.hue > before.left.hue, "{moved:?}");
        assert_eq!(moved.left, moved.right, "both sticks");
        assert_eq!(sent(&fx), Some(moved.shown(None)), "the lights follow at once: {fx:?}");
        assert!(saved(&fx).is_none());
        let fx = s.applied(Slide::End.into());
        assert_eq!(saved(&fx).and_then(|l| l.get(1)).map(|p| p.body), Some(moved));
        // Back to the picker: the lights in force (Off, nothing chosen) again.
        let fx = profile(&mut s, ProfileId::Back);
        assert_eq!(sent(&fx), Some([Lit::OFF; 2]), "{fx:?}");
    }

    /// Unlinked, a tap on a stick picks the stick the sliders set, and the other keeps its colour.
    /// Linking again copies the picked stick's colour to the other. Each switch saves.
    #[test]
    fn unlinked_the_picked_stick_is_set_and_linking_copies_it_to_the_other() {
        let mut s = new_state();
        edit(&mut s, 2);
        let fx = tap(&mut s, Id::Lights(LightsId::Linked));
        assert!(saved(&fx).is_some_and(|l| l.get(1).is_some_and(|p| !p.body.linked)), "{fx:?}");
        tap(&mut s, Id::Lights(LightsId::Stick(Stick::Right)));
        assert_eq!(s.lighting().stick(), Stick::Right);
        let left = body(&s, 1).left;
        slide(&mut s, LightParam::Saturation, 0.0);
        let set = body(&s, 1);
        assert_eq!(set.left, left, "the left keeps its colour");
        assert_eq!(set.right.saturation, 0);
        assert_eq!(s.lights_shown(), set.shown(None));
        // A slider starts from the picked stick's value.
        tap(&mut s, Id::Lights(LightsId::Stick(Stick::Left)));
        s.applied(Slide::Start { control: Control::Light(LightParam::Saturation), x: 300.0, width: 620.0 }.into());
        assert_eq!(s.held_slider().map(|sl| sl.start), Some(left.saturation));
        s.applied(Slide::End.into());
        // Linking copies the picked stick's (the left's) to the right.
        let fx = tap(&mut s, Id::Lights(LightsId::Linked));
        let set = body(&s, 1);
        assert!(set.linked);
        assert_eq!((set.left, set.right), (left, left));
        assert_eq!(saved(&fx).and_then(|l| l.get(1)).map(|p| p.body), Some(set));
        // The edit screen opens again with the left picked.
        profile(&mut s, ProfileId::Back);
        tap(&mut s, Id::Row { list: ListId::Lights, i: 2, part: RowPart::Edit });
        assert_eq!(s.lighting().stick(), Stick::Left);
    }

    /// With "Charging shows the battery" on, the lights in force show the charge while charging, red to
    /// green, and their colours again when unplugged. On the edit screen they show the colours, so that
    /// a change shows.
    #[test]
    fn charging_shows_the_battery_when_the_set_says_so() {
        let mut s = new_state();
        let charging = |level, on| Reading::Status(Status { battery: Some(level), charging: on, discharging: !on, ..Status::default() });
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Scope(SettingsLevel::Global)));
        long(&mut s, Id::Settings(SettingsId::Tile(Setting::Lights)));
        tap(&mut s, row(1));
        let ember = body(&s, 1);
        assert_eq!(sent(&s.applied(Event::Reading(charging(50, true)))), None, "not asked for: the colours stay");
        long(&mut s, Id::Settings(SettingsId::Tile(Setting::Lights)));
        tap(&mut s, Id::Row { list: ListId::Lights, i: 1, part: RowPart::Edit });
        let fx = tap(&mut s, Id::Lights(LightsId::Battery));
        assert!(saved(&fx).is_some_and(|l| l.get(1).is_some_and(|p| p.body.battery)), "{fx:?}");
        assert_eq!(s.lights_shown(), ember.shown(None), "on the edit screen: the colours");
        let fx = profile(&mut s, ProfileId::Back);
        let half = Colour { hue: 60, saturation: 100, brightness: ember.left.brightness }.lit();
        assert_eq!(sent(&fx), Some([half; 2]), "charging at half: yellow, {fx:?}");
        assert_eq!(sent(&s.applied(Event::Reading(charging(100, true)))).map(|[l, _]| l.rgb), Some([0, 255, 0]), "full: green");
        assert_eq!(sent(&s.applied(Event::Reading(charging(100, false)))), Some(ember.shown(None)), "unplugged: the colours");
    }

    #[test]
    fn deleting_lights_in_use_asks_and_puts_its_games_back_on_off() {
        let mut s = new_state();
        picker(&mut s);
        tap(&mut s, row(2));
        picker(&mut s);
        s.applied(Event::Pointer(PointerEvent::LongPress(row(2))));
        assert_eq!(s.lighting().screen(), Some(LightsScreen::Picker(PickerScreen::Menu(1))));
        profile(&mut s, ProfileId::Delete);
        assert_eq!(s.lighting().screen(), Some(LightsScreen::Picker(PickerScreen::Delete(1))));
        let fx = profile(&mut s, ProfileId::ConfirmDelete);
        assert!(saved(&fx).is_some_and(|l| l.get(1).is_none()), "{fx:?}");
        assert_eq!(chosen(&s), Some(0));
        assert_eq!(s.lighting().in_force(1), Lights::default(), "gone: Off");
        profile(&mut s, ProfileId::Back);
        assert_eq!(s.lighting().screen(), None);
    }

    #[test]
    fn new_lights_are_named_and_off_until_edited() {
        let mut s = new_state();
        picker(&mut s);
        profile(&mut s, ProfileId::New);
        for c in "mine".chars() {
            tap(&mut s, Id::Key(crate::bottom::keyboard::find(crate::bottom::keyboard::Key::Char(c)).unwrap()));
        }
        let fx = tap(&mut s, Id::Editor(EditorId::Save));
        let id = saved(&fx).and_then(|l| l.named().find(|p| p.name == "mine")).map(|p| p.id).unwrap();
        assert_eq!(s.lighting().screen(), Some(LightsScreen::Edit(id)));
        assert_eq!(body(&s, id), Lights::default());
        // Brightness up: lit, linked, at once.
        slide(&mut s, LightParam::Brightness, 5000.0);
        assert_eq!(s.lights_shown().map(|l| l.brightness), [100; 2]);
    }
}
