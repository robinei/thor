//! Input: presses from touch, the mouse and the pad's A (Enter on the desktop), with holds (key repeat,
//! long presses) on a timer; pad-style focus movement over both screens; the desktop keys that stand in
//! for the pad.

use smithay_client_toolkit::reexports::{
    calloop::timer::{TimeoutAction, Timer},
    client::protocol::wl_surface::WlSurface,
};
use smithay_client_toolkit::seat::keyboard::Keysym;
use std::time::Duration;
use clamshell_core::{Event, Hold, Id, Mode, PadInput};

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
    /// What the last frame drew at `position` on `surface`: the topmost (last drawn) target there, so a
    /// button on a tile (the launcher's "+") wins over the tile.
    pub(crate) fn target_at(&mut self, surface: &WlSurface, (x, y): (f64, f64)) -> Option<Id> {
        let view = self.view_mut(surface)?;
        view.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).map(|t| t.id.clone())
    }

    /// A finger or button comes down at `position` on `surface`: a press on what is there, and on volume
    /// or brightness the start of a slide.
    pub(crate) fn press_at(&mut self, source: Source, surface: &WlSurface, position: (f64, f64)) {
        let Some(id) = self.target_at(surface, position) else {
            return;
        };
        let control = match id {
            Id::Control(control) => Some(control),
            _ => None,
        };
        self.down(source, id);
        if let Some(control) = control
            && let Some(view) = self.view_mut(surface)
        {
            let (x, width) = (position.0 as f32 - view.origin.0, view.layout_w);
            self.slider_source = Some(source);
            self.handle(Event::SlideStart { control, x, width });
        }
    }

    /// A finger or the mouse moved: the slider follows the press that started it.
    pub(crate) fn moved(&mut self, source: Source, position: (f64, f64)) {
        if self.slider_source != Some(source) {
            return;
        }
        let origin = self.bottom.as_ref().map_or(0.0, |v| v.origin.0);
        self.handle(Event::SlideTo { x: position.0 as f32 - origin });
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
        if self.slider_source == Some(source) {
            self.slider_source = None;
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

    /// The pad, while shelf is in front: directions move focus (crossing screens at the edges), A acts on
    /// the focused target, B goes back, Y toggles the favourite, the shoulders step tabs (top screen) or
    /// modes (second screen), View/Select moves focus to the other screen.
    pub(crate) fn pad(&mut self, input: PadInput) {
        match input {
            PadInput::Up => self.navigate(Dir::Up),
            PadInput::Down => self.navigate(Dir::Down),
            PadInput::Left => self.navigate(Dir::Left),
            PadInput::Right => self.navigate(Dir::Right),
            PadInput::A(true) => {
                if let Some(id) = self.state.focus.clone() {
                    self.down(Source::PadA, id);
                }
            }
            PadInput::A(false) => self.up(Source::PadA, true),
            PadInput::B => {
                let back = if self.state.editor.is_some() {
                    Some(Id::EditorCancel)
                } else if !Mode::BAR.contains(&self.state.mode) {
                    // Wi-Fi or Battery: their icon again goes back.
                    Some(Id::Mode(self.state.mode))
                } else {
                    None
                };
                if let Some(id) = back {
                    self.handle(Event::Activate(id));
                }
            }
            PadInput::Y => self.handle(Event::ToggleFavourite),
            PadInput::Shoulder(step) => {
                if self.focus_screen() == Screen::Top {
                    self.handle(Event::TabStep(step));
                } else {
                    let modes = Mode::BAR;
                    let at = modes.iter().position(|m| *m == self.state.mode).unwrap_or(0) as i32;
                    let next = (at + step).rem_euclid(modes.len() as i32) as usize;
                    if let Some(&mode) = modes.get(next) {
                        self.handle(Event::Activate(Id::Mode(mode)));
                        self.handle(Event::Focus(Id::Mode(mode)));
                    }
                }
            }
            PadInput::Switch => self.switch_screen(),
        }
    }

    /// The screen the focus is on (the top one when it isn't on anything drawn).
    fn focus_screen(&self) -> Screen {
        let focus = self.state.focus.as_ref();
        self.views().find(|v| v.targets.iter().any(|t| Some(&t.id) == focus)).map_or(Screen::Top, |v| v.screen)
    }

    /// Focus to the other screen: where it last was there if that is still drawn, else shelf's selected
    /// game (top) or the current mode's icon (second screen).
    fn switch_screen(&mut self) {
        let from = self.focus_screen();
        let (to, slot_from, slot_to) = match from {
            Screen::Top => (Screen::Bottom, 0, 1),
            Screen::Bottom => (Screen::Top, 1, 0),
        };
        if let Some(slot) = self.last_focus.get_mut(slot_from) {
            *slot = self.state.focus.clone();
        }
        let drawn = |app: &App, id: &Id| app.views().any(|v| v.screen == to && v.targets.iter().any(|t| &t.id == id));
        let remembered = self.last_focus.get(slot_to).cloned().flatten().filter(|id| drawn(self, id));
        let default = match to {
            Screen::Top => self.state.selected.clone().map(Id::Game),
            Screen::Bottom => Some(Id::Mode(self.state.mode)),
        };
        let first = self.views().find(|v| v.screen == to).and_then(|v| v.targets.first()).map(|t| t.id.clone());
        let target = remembered.or_else(|| default.filter(|id| drawn(self, id))).or(first);
        if let Some(id) = target {
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
            // The pad's View (focus to the other screen), B, and shoulders.
            (Keysym::Tab, true) => self.pad(PadInput::Switch),
            (Keysym::BackSpace, true) => self.pad(PadInput::B),
            (Keysym::q, true) => self.pad(PadInput::Shoulder(-1)),
            (Keysym::e, true) => self.pad(PadInput::Shoulder(1)),
            _ => {}
        }
    }
}
