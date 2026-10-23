//! Routes input to the pure machines: presses, the wheel, pointer motion and releases, the pad and keys.

use clamshell_state::{Event, Id, ListEvent, PadInput, PointerEvent, ShelfEvent, State};
use smithay_client_toolkit::seat::keyboard::Keysym;

use crate::Screen;
use crate::input::focus::{self, PadPress};
use crate::input::nav::Dir;
use crate::input::{End, Source};

use super::App;
use crate::input;

impl App {
    /// A finger or button comes down at `position` on `screen`. It presses the topmost target there,
    /// which is the last one drawn, so a button on a tile (such as the dock's "+") wins over the tile.
    pub(super) fn press_at(&mut self, source: Source, screen: Screen, (x, y): (f64, f64), time: u32) {
        let Some(view) = &self.views[screen] else {
            return;
        };
        let found = view.retained.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).cloned();
        // With no target there, below the second screen's bar, the press is on the blank, which may
        // point.
        let bottom = screen == Screen::Bottom;
        if bottom {
            self.handle(Event::BottomTouched);
        }
        let Some(view) = &self.views[screen] else {
            return;
        };
        let below_bar = bottom && view.body.is_some_and(|b| b.contains(x as f32, y as f32));
        let Some(target) = found
            .or_else(|| below_bar.then(|| crate::toolkit::Target { rect: crate::gfx::Rect::default(), id: Id::Blank, track: None, list: None, item: None }))
        else {
            return;
        };
        let now = self.clock.at(time);
        let actions = match source {
            // While the second screen is pointing, a finger put down there points too.
            Source::Touch(_) if self.state.bottom().pointing() && bottom && input::points(&target.id) => {
                self.input.point(source, &target, (x as f32, y as f32), now)
            }
            _ => self.input.touch(source, &target, (x as f32, y as f32), now),
        };
        self.run_actions(actions);
    }

    /// The wheel on `screen` at `position`, by `notches`. Over the main screen it scrolls the shelf's
    /// list by a game per notch. Over a list on the second screen, it scrolls by a row per notch.
    pub(super) fn wheel(&mut self, screen: Screen, (x, y): (f64, f64), notches: i32) {
        if screen == Screen::Top {
            return self.handle(Event::Shelf(ShelfEvent::ListStep(notches)));
        }
        let Some(view) = &self.views[screen] else {
            return;
        };
        let list = view.retained.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).and_then(|t| t.list);
        if let Some(list) = list {
            self.handle(ListEvent::Scroll(list.list, notches as f32).into());
        }
    }

    /// When an input event with the compositor's `time` happened.
    pub(super) fn at(&mut self, time: u32) -> std::time::Instant {
        self.clock.at(time)
    }

    pub(super) fn moved(&mut self, source: Source, (x, y): (f64, f64), time: u32) {
        let now = self.clock.at(time);
        let actions = self.input.moved(source, (x as f32, y as f32), now);
        self.run_actions(actions);
    }

    /// A press ends at `now`. For a finger or the mouse, `now` is its event time from [`App::at`].
    pub(super) fn up(&mut self, source: Source, end: End, now: std::time::Instant) {
        let actions = self.input.up(source, end, &self.state, now);
        self.run_actions(actions);
    }

    pub(super) fn cancel_touches(&mut self) {
        let actions = self.input.cancel_touches(&self.state);
        self.run_actions(actions);
    }

    /// The pad on ClamShell's screens (see `focus`):
    /// - directions move focus;
    /// - A acts on the focused target, and B goes back (see `focus::back`);
    /// - Y, X and Start do the shelf details' actions while the pad is on the shelf (see
    ///   `focus::shortcut`). On the keyboard, X is Backspace;
    /// - the shoulders page through the shelf or step through the second screen's tabs;
    /// - the triggers go to the shelf's first or last entry;
    /// - View moves focus to the other screen while the shelf is in front. Otherwise it moves the pad
    ///   between the second screen and whatever else wants it, such as the pad mouse or typing. The pad
    ///   thread sends View from there too.
    pub(super) fn pad(&mut self, input: PadInput) {
        // While a dock entry is being moved, the D-pad moves it and A or B ends the move.
        if self.state.dock().moving().is_some() && self.state.bottom().tab() == clamshell_state::Tab::Dock {
            let step = |dir| Some(clamshell_state::DockId::Step(dir));
            use clamshell_state::keymap::Dir as Step;
            let id = match input {
                PadInput::Up => step(Step::Up),
                PadInput::Down => step(Step::Down),
                PadInput::Left => step(Step::Left),
                PadInput::Right => step(Step::Right),
                PadInput::A(true) | PadInput::B => Some(clamshell_state::DockId::Done),
                _ => None,
            };
            if let Some(id) = id {
                self.handle(PointerEvent::Activate(Id::Dock(id)).into());
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
            let event = focus::navigate(&self.state, &self.views, &self.focus_memory, dir);
            return self.handle_all(event);
        }
        match input {
            PadInput::A(true) => self.press_focus(Source::PadA),
            PadInput::A(false) => self.up(Source::PadA, End::Release, std::time::Instant::now()),
            PadInput::B => self.handle_all(focus::back(&self.state)),
            PadInput::Y | PadInput::X | PadInput::Start => self.handle_all(focus::shortcut(&self.state, &self.views, input)),
            // The Back button does what the screen has for back, such as the edit screen's other layer.
            PadInput::Back => {
                if let Some(id) = self.state.pad_back() {
                    self.handle(PointerEvent::Activate(id).into());
                }
            }
            PadInput::Shoulder(step) => self.handle_all(focus::shoulder(&self.state, &self.views, step)),
            PadInput::Trigger(step) => self.handle_all(focus::trigger(&self.state, &self.views, step)),
            PadInput::Switch if self.state.pad().on_top() => {
                let event = focus::switch_screen(&self.state, &self.views, &mut self.focus_memory);
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
                    let event = focus::focus_bottom(&self.state, &self.views, &self.focus_memory);
                    self.handle(event);
                }
            }
            PadInput::Up | PadInput::Down | PadInput::Left | PadInput::Right => {}
        }
    }

    /// A or Enter on the focused target.
    fn press_focus(&mut self, source: Source) {
        match focus::press(&self.state, &self.views, &self.focus_memory) {
            Some(PadPress::Press(id)) => {
                let actions = self.input.press(source, id, std::time::Instant::now());
                self.run_actions(actions);
            }
            Some(PadPress::Focus(event)) => self.handle(event),
            None => {}
        }
    }

    /// A key on a desktop (see `focus::desktop_key`).
    pub(super) fn key(&mut self, sym: Keysym, down: bool) {
        if !self.bottom_kind.desktop_keys() {
            return;
        }
        match focus::desktop_key(sym) {
            Some(PadInput::A(_)) if down => self.press_focus(Source::Enter),
            Some(PadInput::A(_)) => self.up(Source::Enter, End::Release, std::time::Instant::now()),
            Some(input) if down => self.pad(input),
            _ => {}
        }
    }
}
