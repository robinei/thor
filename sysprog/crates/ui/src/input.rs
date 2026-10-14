//! Input: presses from touch, the mouse and the pad's A (Enter on the desktop), with holds (key repeat,
//! long presses) on a timer; pad-style focus movement over both screens; the desktop keys that stand in
//! for the pad.

use smithay_client_toolkit::reexports::{
    calloop::timer::{TimeoutAction, Timer},
    client::protocol::wl_surface::WlSurface,
};
use smithay_client_toolkit::seat::keyboard::Keysym;
use std::time::Duration;
use sysprog_core::{Event, Hold, Id};

use crate::draw::Rect;
use crate::nav::{self, Dir, NavTarget};
use crate::{App, Screen};

/// A press held this long is a hold: keys start repeating, modifiers lock.
const LONG_PRESS: Duration = Duration::from_millis(450);
/// Key repeat interval while held (25 per second).
const REPEAT: Duration = Duration::from_millis(40);

/// Where a press comes from: each touch point, the mouse, or the pad's A (Enter on the desktop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    Touch(i32),
    Mouse,
    PadA,
}

/// A press in progress, on the target it started on.
pub(crate) struct Press {
    id: Id,
    /// The hold timer already acted (repeated or long-pressed): the release adds nothing.
    held: bool,
}

impl App {
    /// What the last frame drew at `position` on `surface`.
    pub(crate) fn target_at(&mut self, surface: &WlSurface, (x, y): (f64, f64)) -> Option<Id> {
        let view = self.view_mut(surface)?;
        view.targets.iter().find(|t| t.rect.contains(x as f32, y as f32)).map(|t| t.id.clone())
    }

    /// A press starts: light the target, and arm the hold timer. A tap acts on release; a hold acts
    /// from the timer (repeat, or one long press).
    pub(crate) fn down(&mut self, source: Source, id: Id) {
        if self.presses.contains_key(&source) {
            return;
        }
        if matches!(source, Source::Touch(_) | Source::Mouse) {
            self.handle(Event::HideFocus);
        }
        self.presses.insert(source, Press { id: id.clone(), held: false });
        self.handle(Event::Down(id));
        self.cancel_hold_timer();
        let timer = Timer::from_duration(LONG_PRESS);
        match self.loop_handle.insert_source(timer, move |_, _, app: &mut App| app.on_hold(source)) {
            Ok(token) => self.hold_timer = Some((source, token)),
            Err(e) => log::warn!("hold timer: {}", e.error),
        }
    }

    fn on_hold(&mut self, source: Source) -> TimeoutAction {
        let Some(press) = self.presses.get_mut(&source) else {
            self.hold_timer = None;
            return TimeoutAction::Drop;
        };
        press.held = true;
        let id = press.id.clone();
        match self.state.hold(&id) {
            Hold::Repeat => {
                self.handle(Event::Activate(id));
                TimeoutAction::ToDuration(REPEAT)
            }
            Hold::Long => {
                self.hold_timer = None;
                self.handle(Event::LongPress(id));
                TimeoutAction::Drop
            }
        }
    }

    /// A press ends: a tap activates; a hold already acted.
    pub(crate) fn up(&mut self, source: Source, activate: bool) {
        if let Some((timer_source, token)) = self.hold_timer
            && timer_source == source
        {
            self.loop_handle.remove(token);
            self.hold_timer = None;
        }
        let Some(press) = self.presses.remove(&source) else {
            return;
        };
        if activate && !press.held {
            self.handle(Event::Activate(press.id.clone()));
        }
        self.handle(Event::Up(press.id));
    }

    fn cancel_hold_timer(&mut self) {
        if let Some((_, token)) = self.hold_timer.take() {
            self.loop_handle.remove(token);
        }
    }

    /// All touch points gone without a tap (cancelled, or the touch device went away).
    pub(crate) fn cancel_touches(&mut self) {
        let touches: Vec<Source> = self.presses.keys().copied().filter(|s| matches!(s, Source::Touch(_))).collect();
        for source in touches {
            self.up(source, false);
        }
    }

    /// Pad-style focus movement over both screens' targets.
    fn navigate(&mut self, dir: Dir) {
        // In shelf's list: up and down move through the games (scrolling), left and right switch tabs;
        // down past the last game goes on to the second screen.
        if matches!(self.state.focus, Some(Id::Game(_)) | Some(Id::Tab(_))) {
            let len = self.state.collection().map_or(0, |c| c.games.len());
            let at_end = self.state.cursor().index + 1 >= len;
            let event = match dir {
                Dir::Up => Some(Event::ListStep(-1)),
                Dir::Down if !at_end => Some(Event::ListStep(1)),
                Dir::Left => Some(Event::TabStep(-1)),
                Dir::Right => Some(Event::TabStep(1)),
                Dir::Down => None,
            };
            if let Some(event) = event {
                self.handle(event);
                return;
            }
        }
        let mut all = Vec::new();
        for view in self.views() {
            let (w, h) = (view.size.0.max(1) as f32, view.size.1.max(1) as f32);
            let y0 = match view.screen {
                Screen::Top => 0.0,
                Screen::Bottom => 1.0,
            };
            all.extend(view.targets.iter().map(|t| NavTarget {
                screen: view.screen,
                rect: Rect::new(t.rect.x / w, y0 + t.rect.y / h, t.rect.w / w, t.rect.h / h),
                id: t.id.clone(),
            }));
        }
        if let Some(id) = nav::step(&all, self.state.focus.as_ref(), dir) {
            self.handle(Event::Focus(id));
        }
    }

    /// The keyboard stands in for the pad on the desktop: arrows move focus, Enter/Space is A (press,
    /// hold), Escape quits.
    pub(crate) fn key(&mut self, sym: Keysym, down: bool) {
        match (sym, down) {
            (Keysym::Left, true) => self.navigate(Dir::Left),
            (Keysym::Right, true) => self.navigate(Dir::Right),
            (Keysym::Up, true) => self.navigate(Dir::Up),
            (Keysym::Down, true) => self.navigate(Dir::Down),
            (Keysym::Return | Keysym::KP_Enter | Keysym::space, true) => {
                if let Some(id) = self.state.focus.clone() {
                    self.down(Source::PadA, id);
                }
            }
            (Keysym::Return | Keysym::KP_Enter | Keysym::space, false) => self.up(Source::PadA, true),
            (Keysym::Escape, true) => self.handle(Event::Quit),
            // The pad's Y: the selected game in or out of the favourites.
            (Keysym::f, true) => self.handle(Event::ToggleFavourite),
            _ => {}
        }
    }
}
