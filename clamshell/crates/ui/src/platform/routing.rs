//! Input routed to the pure machines: presses, the wheel, pointer motion and releases, the pad, keys.

use clamshell_core::{Event, Id, ListEvent, PadInput, PointerEvent, State};
use smithay_client_toolkit::{reexports::client::protocol::wl_surface::WlSurface, seat::keyboard::Keysym};

use crate::Screen;
use crate::input::focus::{self, PadPress};
use crate::input::nav::Dir;
use crate::input::{End, Source};

use super::App;
use crate::input;

impl App {
    /// A finger or button comes down at `position` on `surface`: a press on the topmost (last drawn)
    /// target there, so a button on a tile (the dock's "+") wins over the tile.
    pub(super) fn press_at(&mut self, source: Source, surface: &WlSurface, (x, y): (f64, f64), time: u32) {
        let Some(view) = self.view_mut(surface) else {
            return;
        };
        let found = view.retained.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).cloned();
        // (Nothing there, below the second screen's bar: the blank, which may point.)
        let bottom = view.screen == Screen::Bottom;
        if bottom {
            self.handle(Event::BottomTouched);
        }
        let Some(view) = self.view_mut(surface) else {
            return;
        };
        let below_bar = bottom && view.body.is_some_and(|b| b.contains(x as f32, y as f32));
        let Some(target) =
            found.or_else(|| below_bar.then(|| crate::toolkit::Target { rect: crate::gfx::Rect::default(), id: Id::Blank, track: None, list: None }))
        else {
            return;
        };
        let now = self.clock.at(time);
        let actions = match source {
            // (While it points, a finger there points too.)
            Source::Touch(_) if self.state.bottom().pointing() && bottom && input::points(&target.id) => {
                self.input.point(source, &target, (x as f32, y as f32), now)
            }
            _ => self.input.touch(source, &target, (x as f32, y as f32), now),
        };
        self.run_actions(actions);
    }

    /// The wheel over a scrolled list on `surface` at `position`: it scrolls by `notches` rows.
    pub(super) fn wheel(&mut self, surface: &WlSurface, (x, y): (f64, f64), notches: i32) {
        let Some(view) = self.view_mut(surface) else {
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

    /// A press ends at `now` (a finger's or the mouse's: [`App::at`] its event time).
    pub(super) fn up(&mut self, source: Source, end: End, now: std::time::Instant) {
        let actions = self.input.up(source, end, &self.state, now);
        self.run_actions(actions);
    }

    pub(super) fn cancel_touches(&mut self) {
        let actions = self.input.cancel_touches(&self.state);
        self.run_actions(actions);
    }

    /// The pad on ClamShell's screens (see `focus`): directions move focus, A acts on the focused
    /// target, B goes back (see `focus::back`), Y, X and Start shelf's details' actions while the pad
    /// is on shelf (see `focus::shortcut`; X is Backspace on the keyboard), the shoulders
    /// page through shelf (or step the second screen's tabs), the triggers go to shelf's first or last entry.
    /// View moves focus to the other screen with shelf in front; else it moves the pad between the second
    /// screen and what else wants it (the pad mouse, typing; the pad thread sends View from there too).
    pub(super) fn pad(&mut self, input: PadInput) {
        // Moving a dock entry: the D-pad moves it, A or B ends it.
        if self.state.dock().moving().is_some() && self.state.bottom().tab() == clamshell_core::Tab::Dock {
            let step = |dir| Some(clamshell_core::DockId::Step(dir));
            use clamshell_core::keymap::Dir as Step;
            let id = match input {
                PadInput::Up => step(Step::Up),
                PadInput::Down => step(Step::Down),
                PadInput::Left => step(Step::Left),
                PadInput::Right => step(Step::Right),
                PadInput::A(true) | PadInput::B => Some(clamshell_core::DockId::Done),
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
            // The Back button: what the screen has for back (the edit screen's other layer).
            PadInput::Back => {
                if let Some(id) = self.state.pad_back() {
                    self.handle(PointerEvent::Activate(id).into());
                }
            }
            PadInput::Shoulder(step) => self.handle_all(focus::shoulder(&self.state, &self.views, step)),
            PadInput::Trigger(step) => self.handle_all(focus::trigger(&self.state, &self.views, step)),
            PadInput::Switch if self.state.pad_on_top() => {
                let event = focus::switch_screen(&self.state, &self.views, &mut self.focus_memory);
                self.handle_all(event);
            }
            // The pad between the second screen and what else wants it; back on the screen, the focus
            // where it was.
            PadInput::Switch => {
                let on_ui = |s: &State| s.pad().route() == clamshell_core::PadRoute::Ui;
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

    /// A (or Enter) on the focused target.
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
        if !self.desktop_keys {
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
