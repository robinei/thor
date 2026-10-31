//! Routes input, by the screen it is on, to the pure machines: presses, the wheel, pointer motion and
//! releases, the pad and keys.

use clamshell_state::{Event, ListEvent, PadInput, PointerEvent, State};
use xkeysym::Keysym;

use crate::Screen;
use crate::input::focus::{self, PadPress};
use crate::input::nav::Dir;
use crate::input::{End, Source};

use super::Core;

impl Core {
    /// A finger or button comes down at `position` on `screen`. It presses the topmost target there,
    /// which is the last one drawn, so a button on a tile (such as the dock's "+") wins over the tile.
    /// With none there, in the screen's content, it presses nothing ([`clamshell_state::Id::Blank`]),
    /// which may point.
    pub fn press(&mut self, source: Source, screen: Screen, (x, y): (f64, f64), time: u32) {
        let (x, y) = (x as f32, y as f32);
        let found = self.screens[screen].frames.retained.targets.iter().rev().find(|t| t.rect.contains(x, y)).cloned();
        if let Some(touched) = self.screens[screen].touched.clone() {
            self.handle(touched);
        }
        let s = &self.screens[screen];
        let pointing = s.pointing.is_some_and(|pointing| pointing(&self.state));
        let in_content = s.content.is_some_and(|r| r.contains(x, y));
        let points = s.point_area.is_some_and(|r| r.contains(x, y));
        if found.is_none() && !in_content {
            return;
        }
        let now = self.clock.at(time);
        let actions = match source {
            // While the screen is pointing, a finger put down where it may point points too.
            Source::Touch(_) if pointing && points && found.as_ref().is_none_or(|t| t.drag.is_none()) => self.input.point(source, (x, y), now),
            _ => self.input.touch(source, found.as_ref(), points, (x, y), now),
        };
        self.run_actions(actions);
    }

    /// A finger or button comes down at `position` on `screen`: it presses the target of the screen's
    /// overlay there, if there is one, and says whether there was. Overlay targets act as they are
    /// pressed ([`PointerEvent::Act`]).
    pub fn press_overlay(&mut self, screen: Screen, (x, y): (f64, f64)) -> bool {
        let found = self.overlays[screen].frames.retained.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).map(|t| t.id.clone());
        let Some(id) = found else { return false };
        self.handle(PointerEvent::Act(id).into());
        true
    }

    /// The wheel on `screen` at `position`, by `notches`. Over a scrolled list it scrolls by a row per
    /// notch. Elsewhere on the screen, it is the view's list's, if the screen declared one (see
    /// `toolkit::PadOut::moves`).
    pub fn wheel(&mut self, screen: Screen, (x, y): (f64, f64), notches: i32) {
        let list = self.screens[screen].frames.retained.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).and_then(|t| t.list);
        if list.is_none()
            && let Some(Some(event)) = self.screens[screen].pad.moves.and_then(|moves| moves(&self.state, crate::toolkit::PadMove::Scroll(notches)))
        {
            return self.handle(event);
        }
        if let Some(list) = list {
            self.handle(ListEvent::Scroll(list.list, notches as f32).into());
        }
    }

    /// When an input event with the host's `time` (milliseconds on its monotonic clock) happened.
    pub fn at(&mut self, time: u32) -> std::time::Instant {
        self.clock.at(time)
    }

    pub fn moved(&mut self, source: Source, (x, y): (f64, f64), time: u32) {
        let now = self.clock.at(time);
        let actions = self.input.moved(source, (x as f32, y as f32), now);
        self.run_actions(actions);
    }

    /// A press ends at `now`. For a finger or the mouse, `now` is its event time from [`Core::at`].
    pub fn up(&mut self, source: Source, end: End, now: std::time::Instant) {
        let actions = self.input.up(source, end, &self.state, now);
        self.run_actions(actions);
    }

    pub fn cancel_touches(&mut self) {
        let actions = self.input.cancel_touches(&self.state);
        self.run_actions(actions);
    }

    /// The pad on ClamShell's screens (see `focus`), unless a dock entry is being moved or the overview of
    /// the program's windows is open, which take the D-pad, A and B:
    /// - directions move focus;
    /// - A acts on the focused target, and B goes back (see `focus::back`);
    /// - Y, X and Start press what their badges show on the screen the pad is on (see `focus::shortcut`);
    /// - the shoulders page through a view's list or step through the second screen's tabs;
    /// - the triggers go to a view's list's first or last row;
    /// - View moves focus to the other screen while the pad may use both. Otherwise it moves the pad
    ///   between the second screen and whatever else wants it, such as the pad mouse or typing. The pad
    ///   thread sends View from there too.
    pub(crate) fn pad(&mut self, input: PadInput) {
        self.pad_from(input, Source::PadA);
    }

    /// Enter follows the same fresh targets and modes as A, but keeps its own press and release.
    fn pad_from(&mut self, input: PadInput, source: Source) {
        self.handle(Event::PadActive);
        self.fresh();
        // A held A lets go wherever it was pressed, a mode or not, so no press is left held.
        if input == PadInput::A(false) {
            self.up(source, End::Release, std::time::Instant::now());
        }
        // A mode drawn takes the pad's buttons first: a dock tile being moved, or the overview.
        if let Some(mode) = self.mode() {
            if let Some(event) = mode(input) {
                self.handle(event);
            }
            return;
        }
        let dir = match input {
            PadInput::Up => Some(Dir::Up),
            PadInput::Down => Some(Dir::Down),
            PadInput::Left => Some(Dir::Left),
            PadInput::Right => Some(Dir::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            let event = focus::navigate(&self.state, &self.screens, &self.focus_memory, dir);
            return self.handle_all(event);
        }
        match input {
            PadInput::A(true) => self.press_focus(source),
            PadInput::A(false) => {}
            // The Back button is B on ClamShell's screens.
            PadInput::B | PadInput::Back => self.handle_all(focus::back(&self.state, &self.screens)),
            PadInput::Y | PadInput::X | PadInput::Start => self.handle_all(focus::shortcut(&self.state, &self.screens, input)),
            PadInput::Shoulder(step) => self.handle_all(focus::shoulder(&self.state, &self.screens, step)),
            PadInput::Trigger(step) => self.handle_all(focus::trigger(&self.state, &self.screens, step)),
            PadInput::Switch if self.state.pad().on_top() => {
                let event = focus::switch_screen(&self.state, &self.screens, &mut self.focus_memory);
                self.handle_all(event);
            }
            // Move the pad between the second screen and whatever else wants it. When it comes back to
            // the screen, the focus returns to where it was.
            PadInput::Switch => {
                let on_ui = |s: &State| s.pad().route() == clamshell_state::PadRoute::Ui;
                if on_ui(&self.state) {
                    self.focus_memory.bottom = self.state.pointer().focus().cloned();
                }
                self.handle(Event::PadSwitch);
                if on_ui(&self.state) {
                    let event = focus::focus_bottom(&self.state, &self.screens, &self.focus_memory);
                    self.handle_all(event);
                }
            }
            PadInput::Up | PadInput::Down | PadInput::Left | PadInput::Right => {}
        }
    }

    /// A or Enter on the focused target.
    fn press_focus(&mut self, source: Source) {
        match focus::press(&self.state, &self.screens, &self.focus_memory) {
            Some(PadPress::Press(id, hold)) => {
                let actions = self.input.press(source, id, hold, std::time::Instant::now());
                self.run_actions(actions);
            }
            Some(PadPress::Focus(event)) => self.handle(event),
            None => {}
        }
    }

    /// The mode a frame drew that takes the pad's buttons, if one did: the overlays' first, as they are over
    /// the screens, and the main screen's only while the pad may use it (it isn't drawn under a window).
    fn mode(&self) -> Option<fn(PadInput) -> Option<Event>> {
        let top = self.state.pad().on_top().then_some(self.screens.top.pad.mode).flatten();
        self.overlays.top.pad.mode.or(self.overlays.bottom.pad.mode).or(self.screens.bottom.pad.mode).or(top)
    }

    /// A key on a desktop, where keys stand in for the pad (see `focus::desktop_key`). Enter is A: in a
    /// mode, the mode's.
    pub fn key(&mut self, sym: Keysym, down: bool) {
        match focus::desktop_key(sym) {
            Some(PadInput::A(_)) => self.pad_from(PadInput::A(down), Source::Enter),
            Some(input) if down => self.pad(input),
            _ => {}
        }
    }
}
