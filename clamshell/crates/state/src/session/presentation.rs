//! Presentation policy is shared by client types. Geometry and buffer transforms belong to the host.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WindowLimits {
    pub min: (u32, u32),
    pub max: (u32, u32),
}
impl WindowLimits {
    pub fn fixed_size(self) -> Option<(u32, u32)> {
        (self.min.0 > 0 && self.min.1 > 0 && self.min == self.max).then_some(self.min)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum WindowRole {
    #[default]
    Normal,
    Dialog,
    Splash,
    Utility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WindowLayout {
    Fullscreen,
    Centred,
}
/// How a window's picture is brought to the screen (see [`crate::settings::Size`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WindowScale {
    /// Fitted, in the picture's shape.
    Fit,
    /// The largest whole multiple of the picture that fits, nearest-neighbour.
    Integer,
    /// Filling the screen, its shape aside.
    Stretch,
    /// At its own size, fitted down if larger.
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowPresentation {
    pub layout: WindowLayout,
    pub scale: WindowScale,
}

/// The screens as the compositor has them (see [`crate::Reading::Screens`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Screens {
    /// Each screen's colour as clients are told it (the top's, the second's): what a curve is shown
    /// within.
    pub colour: [crate::colour::ColourDescription; 2],
    /// The top screen's size in pixels, which a game's resolution is a fraction of.
    pub top_size: (u32, u32),
}

/// How a window's picture is shown: its clarity settings (see [`crate::State::window_clarity`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Clarity {
    pub resolution: crate::settings::Resolution,
    pub scaling: crate::settings::Scaling,
    /// Whether the picture is shown at a whole multiple, nearest-neighbour ([`WindowScale::Integer`]), so
    /// that [`Clarity::scaling`] does nothing.
    pub integer: bool,
    pub anti_aliasing: crate::settings::AntiAliasing,
    /// The sharpening's strength in percent, 0 for none.
    pub sharpen: u16,
}

impl super::windows::WindowFacts {
    /// How the window is shown at `size` (see [`crate::settings::Size`]): dialogs, popups and windows of a
    /// fixed size centred at their own size, other windows filling the screen, a game's scaled to it.
    /// `screen` is the sizes its screen may tell it it has (what each resolution gives it, and an X11
    /// window Xwayland's modes): a window fixed at one of them fills the screen too, as a game Wine shows
    /// as a window of the screen's mode. A size other than Default shows a main window (not a dialog or
    /// popup) filling the screen as it says, whatever its own size.
    pub fn presentation(&self, game: bool, screen: &[(u32, u32)], size: crate::settings::Size) -> WindowPresentation {
        use crate::settings::Size;
        let main = self.parent.is_none() && self.role == WindowRole::Normal;
        let scale = match size {
            Size::Fit => WindowScale::Fit,
            Size::Integer => WindowScale::Integer,
            Size::Stretch => WindowScale::Stretch,
            Size::Default => {
                let fixed = self.limits.fixed_size().is_some_and(|size| !screen.contains(&size));
                let centred = !main || fixed;
                return WindowPresentation {
                    layout: if centred { WindowLayout::Centred } else { WindowLayout::Fullscreen },
                    scale: if game && !centred { WindowScale::Fit } else { WindowScale::Native },
                };
            }
        };
        match main {
            true => WindowPresentation { layout: WindowLayout::Fullscreen, scale },
            false => WindowPresentation { layout: WindowLayout::Centred, scale: WindowScale::Native },
        }
    }
}

impl crate::State {
    pub fn window_presentation(&self, id: i64) -> Option<WindowPresentation> {
        let window = self.session().window(id)?;
        let game = self.session().running_of(window).and_then(|r| self.session().entry(self.shelf().library(), &r.id)).is_some_and(crate::Entry::is_game);
        // What the top screen may tell its windows: a game is told its resolution when it starts, which may
        // have changed since, and an X11 game the modes Xwayland offers, one of which it may switch to. The
        // second's size isn't known here, and games are shown on the top one.
        let mut screen: Vec<(u32, u32)> = self
            .top_size()
            .filter(|_| !self.session().below(id))
            .map(|top| {
                use crate::settings::{Choice, Resolution, Setting};
                Setting::Resolution.choices().iter().map(|value| Resolution::of(*value).size_on(top)).collect()
            })
            .unwrap_or_default();
        if window.x11 && !self.session().below(id) {
            screen.extend(&self.xwayland_modes);
        }
        let size = self.session().running_of(window).map_or_else(Default::default, |r| self.settings_of(&r.id).choice());
        Some(window.presentation(game, &screen, size))
    }

    /// The tone curve for window `id`'s content: the curve chosen for the program running it, in its
    /// settings in force (see [`crate::settings::Setting::Curve`]). None for Default colours, for a curve
    /// that is gone, and for a window no launch runs.
    pub fn window_curve(&self, id: i64) -> Option<crate::curve::Curve> {
        let window = self.session().window(id)?;
        let running = self.session().running_of(window)?;
        let chosen = self.settings_of(&running.id).get(crate::settings::Setting::Curve);
        self.curving().in_force(chosen).copied()
    }

    /// The HUD for window `id`: the one in the settings in force for the program running it. Off for a
    /// window no launch runs.
    pub fn window_hud(&self, id: i64) -> crate::settings::Hud {
        let Some(running) = self.session().window(id).and_then(|window| self.session().running_of(window)) else {
            return crate::settings::Hud::Off;
        };
        self.settings_of(&running.id).choice()
    }

    /// Whether the Resolution set for the program the settings panel edits waits for its next start: it runs,
    /// and was told another when it started.
    pub fn resolution_waits(&self) -> bool {
        let Some(program) = self.panel().editing().map(|p| p.id.clone()) else { return false };
        let set: crate::settings::Resolution = crate::settings::Choice::of(self.panel().tile_value(crate::settings::Setting::Resolution));
        self.session().running(&program).and_then(|r| r.told).is_some_and(|told| told != set)
    }

    /// The HDR content peak set for window `id`'s program, in the content's nits (see
    /// [`crate::settings::Setting::HdrPeak`]): 0 takes what it declares, as for a window no launch runs.
    pub fn window_hdr_peak(&self, id: i64) -> u16 {
        let running = self.session().window(id).and_then(|window| self.session().running_of(window));
        running.map_or(0, |running| self.settings_of(&running.id).get(crate::settings::Setting::HdrPeak))
    }

    /// How window `id`'s picture is shown (`docs/picture.md`): the clarity settings in force for the program
    /// running it; the defaults (native, bilinear, nothing more) for a window no launch runs.
    pub fn window_clarity(&self, id: i64) -> Clarity {
        use crate::settings::Setting;
        let Some(running) = self.session().window(id).and_then(|window| self.session().running_of(window)) else {
            return Clarity::default();
        };
        let v = self.settings_of(&running.id);
        let integer = self.window_presentation(id).is_some_and(|p| p.scale == WindowScale::Integer);
        Clarity { resolution: v.choice(), scaling: v.choice(), integer, anti_aliasing: v.choice(), sharpen: v.get(Setting::Sharpen) }
    }

    /// The game a client's process is (see [`crate::session::Session::running_of_process`]): its resolution,
    /// and whether it shows a window on the second screen (a dual-screen emulator, by its rule to move one
    /// down). None for a process of anything else.
    pub fn process_game(&self, unit: Option<&str>, steam_app: Option<u32>) -> Option<(crate::settings::Resolution, bool)> {
        let id = self.session().entry_of_process(unit, steam_app)?;
        let game = self.session().entry(self.shelf().library(), &id).is_some_and(crate::Entry::is_game);
        let dual = self.session().window_rules().list().iter().any(|rule| *rule.owner == *id);
        game.then(|| (self.settings_of(&id).choice(), dual))
    }

    /// The frame rate cap for window `id`: the one in the settings in force for the program running it.
    /// None for no cap, and for a window no launch runs.
    pub fn window_fps_cap(&self, id: i64) -> Option<u16> {
        let window = self.session().window(id)?;
        let running = self.session().running_of(window)?;
        Some(self.settings_of(&running.id).get(crate::settings::Setting::FpsCap)).filter(|cap| *cap > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::windows::WindowFacts;
    use crate::settings::Size;

    #[test]
    fn a_size_shows_main_windows_as_it_says_whatever_their_size_but_not_dialogs() {
        let launcher = WindowFacts { limits: WindowLimits { min: (500, 300), max: (500, 300) }, ..WindowFacts::default() };
        assert_eq!(launcher.presentation(true, &[], Size::Default).layout, WindowLayout::Centred, "by default, at its own size");
        for (size, scale) in [(Size::Fit, WindowScale::Fit), (Size::Integer, WindowScale::Integer), (Size::Stretch, WindowScale::Stretch)] {
            assert_eq!(launcher.presentation(true, &[], size), WindowPresentation { layout: WindowLayout::Fullscreen, scale });
            assert_eq!(launcher.presentation(false, &[], size).scale, scale, "any program's");
        }
        let dialog = WindowFacts { parent: Some(1), ..WindowFacts::default() };
        assert_eq!(dialog.presentation(true, &[], Size::Stretch), WindowPresentation { layout: WindowLayout::Centred, scale: WindowScale::Native });
    }

    #[test]
    fn a_windows_fps_cap_is_the_one_set_for_its_program() {
        use crate::testkit::*;
        use crate::{Id, SettingsId, Tab, settings::Setting};
        let mut s = state();
        launch(&mut s, "zelda-lttp");
        show_window(&mut s, "zelda-lttp", 7);
        assert_eq!(s.window_fps_cap(7), None, "no cap");
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Tile(Setting::FpsCap)));
        assert_eq!(s.window_fps_cap(7), Some(30));
        assert_eq!(s.window_fps_cap(99), None, "no such window");
    }
    /// A game is told its resolution when it starts: a change waits for its next start, and the tile says
    /// so until the setting is back to what it was told, or the game starts again.
    #[test]
    fn a_resolution_change_waits_for_the_games_next_start() {
        use crate::testkit::*;
        use crate::{Id, SettingsId, Tab, settings::Resolution, settings::Setting};
        let mut s = state();
        let launch = launch(&mut s, "zelda-lttp");
        tap(&mut s, Id::Tab(Tab::Settings));
        let unit = Some(launch.unit());
        s.applied(crate::Event::Reading(crate::Reading::Told { unit: unit.clone(), steam_app: None, resolution: Resolution::Native }));
        assert!(!s.resolution_waits(), "as it was told");
        tap(&mut s, Id::Settings(SettingsId::Tile(Setting::Resolution)));
        assert!(s.resolution_waits(), "changed while it runs");
        // Started again with it: told the new one.
        let set: Resolution = crate::settings::Choice::of(s.panel().tile_value(Setting::Resolution));
        s.applied(crate::Event::Reading(crate::Reading::Told { unit, steam_app: None, resolution: set }));
        assert!(!s.resolution_waits());
    }
    /// A launch's program can connect before systemd lists its unit, as a service's program starts with
    /// it: it is its game's already, and the resolution it was told is kept once it runs.
    #[test]
    fn a_game_connecting_before_its_unit_is_listed_is_its_entrys() {
        use crate::testkit::*;
        use crate::{Effect, settings::Resolution};
        let mut s = state();
        let mut fx = tap(&mut s, entry("zelda-lttp"));
        fx.extend(tap(&mut s, entry("zelda-lttp")));
        let unit = fx.iter().find_map(|e| if let Effect::Launch { instance, .. } = e { Some(instance.unit()) } else { None });
        assert!(s.process_game(unit.as_deref(), None).is_some(), "a game, though its unit isn't listed yet");
        s.applied(crate::Event::Reading(crate::Reading::Told { unit, steam_app: None, resolution: Resolution::Half }));
        start(&mut s, "zelda-lttp");
        assert_eq!(s.session().running_entries().iter().find(|r| &*r.id == "zelda-lttp").and_then(|r| r.told), Some(Resolution::Half));
    }
    #[test]
    fn dialogs_and_fixed_windows_are_centred_and_games_fit_ignored_sizes() {
        let mut window = WindowFacts::default();
        assert_eq!(window.presentation(false, &[], Size::Default), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Native });
        assert_eq!(window.presentation(true, &[], Size::Default), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Fit });
        window.parent = Some(7);
        assert_eq!(window.presentation(true, &[], Size::Default), WindowPresentation { layout: WindowLayout::Centred, scale: WindowScale::Native });
        window.parent = None;
        window.limits = WindowLimits { min: (640, 480), max: (640, 480) };
        assert_eq!(window.limits.fixed_size(), Some((640, 480)));
        assert_eq!(window.presentation(true, &[], Size::Default).layout, WindowLayout::Centred);
        window.limits.max = (0, 0);
        assert_eq!(window.limits.fixed_size(), None);
        assert_eq!(window.presentation(true, &[], Size::Default).layout, WindowLayout::Fullscreen);
        for role in [WindowRole::Dialog, WindowRole::Splash, WindowRole::Utility] {
            window.role = role;
            assert_eq!(window.presentation(false, &[], Size::Default).layout, WindowLayout::Centred);
        }
    }
    #[test]
    fn a_game_window_fixed_at_its_screens_mode_fills_the_screen() {
        // As Wine shows a game: a window fixed at the mode.
        let window = WindowFacts { limits: WindowLimits { min: (1200, 675), max: (1200, 675) }, ..WindowFacts::default() };
        let screen = [(1200, 675), (1000, 562)];
        assert_eq!(window.presentation(true, &screen, Size::Default), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Fit });
        let lower = WindowFacts { limits: WindowLimits { min: (1000, 562), max: (1000, 562) }, ..WindowFacts::default() };
        assert_eq!(lower.presentation(true, &screen, Size::Default).scale, WindowScale::Fit, "at the game's resolution, scaled up");
        let small = WindowFacts { limits: WindowLimits { min: (800, 600), max: (800, 600) }, ..WindowFacts::default() };
        assert_eq!(small.presentation(true, &screen, Size::Default).layout, WindowLayout::Centred, "another fixed size stays centred");
    }
}
