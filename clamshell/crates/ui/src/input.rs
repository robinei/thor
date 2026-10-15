//! Input: presses from touch, the mouse and the pad's A (Enter on the desktop), with holds (key repeat,
//! long presses) on a timer; pad-style focus movement over both screens; the desktop keys that stand in
//! for the pad.

use smithay_client_toolkit::reexports::{
    calloop::RegistrationToken,
    calloop::timer::{TimeoutAction, Timer},
    client::protocol::wl_surface::WlSurface,
};
use smithay_client_toolkit::seat::keyboard::Keysym;
use std::time::Duration;
use clamshell_core::controls::{Control, SLIDE_AFTER, SLIDE_MOVE};
use clamshell_core::mixer::Level;
use clamshell_core::{Event, Hold, Id, Mode, PadInput, PadRoute};

use crate::draw::Rect;
use crate::views::levels;
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

/// A finger on volume or brightness that may become a slide: where it came down, where it is now (in
/// the second screen's layout units), and the timer that starts the slide.
pub(crate) struct PendingSlide {
    source: Source,
    control: Control,
    down_x: f32,
    x: f32,
    width: f32,
    timer: Option<RegistrationToken>,
}

/// A finger setting a level row: the row's track, in the view's logical pixels.
pub(crate) struct LevelDrag {
    source: Source,
    level: Level,
    track: Rect,
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
    pub(crate) fn target_at(&mut self, surface: &WlSurface, (x, y): (f64, f64)) -> Option<(Id, Rect)> {
        let view = self.view_mut(surface)?;
        view.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).map(|t| (t.id.clone(), t.rect))
    }

    /// A finger or button comes down at `position` on `surface`: a press on what is there. On volume or
    /// brightness a slide may follow (held, or moved sideways); on a level row the finger sets it.
    pub(crate) fn press_at(&mut self, source: Source, surface: &WlSurface, position: (f64, f64)) {
        let Some((id, rect)) = self.target_at(surface, position) else {
            return;
        };
        let origin_w = self.view_mut(surface).map(|v| (v.origin.0, v.layout_w));
        let busy = self.presses.contains_key(&source);
        self.down(source, id.clone());
        if busy {
            return;
        }
        match id {
            Id::Control(control) => {
                let Some((origin, width)) = origin_w else {
                    return;
                };
                let x = position.0 as f32 - origin;
                let timer = Timer::from_duration(SLIDE_AFTER);
                let token = self
                    .loop_handle
                    .insert_source(timer, move |_, _, app: &mut App| {
                        // This timer is done (dropped below), not to be removed from inside itself.
                        if let Some(p) = app.pending_slide.as_mut() {
                            p.timer = None;
                        }
                        app.start_slide(source);
                        TimeoutAction::Drop
                    })
                    .map_err(|e| log::warn!("slide timer: {}", e.error))
                    .ok();
                self.cancel_pending_slide();
                self.pending_slide = Some(PendingSlide { source, control, down_x: x, x, width, timer: token });
            }
            Id::Level(level) => {
                self.level_drag = Some(LevelDrag { source, level, track: rect });
                self.handle(Event::SetLevel(level, levels::value_at(rect, position.0 as f32)));
            }
            _ => {}
        }
    }

    /// The slide a finger on volume or brightness was waiting for: the slider comes up under it, and
    /// the press won't be a tap.
    fn start_slide(&mut self, source: Source) {
        let Some(pending) = self.pending_slide.take().filter(|p| p.source == source) else {
            return;
        };
        if let Some(token) = pending.timer {
            self.loop_handle.remove(token);
        }
        if let Some(press) = self.presses.get_mut(&source) {
            press.held = true;
        }
        self.slider_source = Some(source);
        self.handle(Event::SlideStart { control: pending.control, x: pending.x, width: pending.width });
    }

    fn cancel_pending_slide(&mut self) {
        if let Some(token) = self.pending_slide.take().and_then(|p| p.timer) {
            self.loop_handle.remove(token);
        }
    }

    /// A finger or the mouse moved: a slide may start, the slider or a level row follows the press on it.
    pub(crate) fn moved(&mut self, source: Source, position: (f64, f64)) {
        let origin = self.bottom.as_ref().map_or(0.0, |v| v.origin.0);
        let x = position.0 as f32 - origin;
        if let Some(pending) = self.pending_slide.as_mut().filter(|p| p.source == source) {
            pending.x = x;
            if (x - pending.down_x).abs() >= SLIDE_MOVE {
                self.start_slide(source);
            }
            return;
        }
        if let Some(drag) = self.level_drag.as_ref().filter(|d| d.source == source) {
            let event = Event::SetLevel(drag.level, levels::value_at(drag.track, position.0 as f32));
            self.handle(event);
            return;
        }
        if self.slider_source != Some(source) {
            return;
        }
        self.handle(Event::SlideTo { x });
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
        if self.pending_slide.as_ref().is_some_and(|p| p.source == source) {
            self.cancel_pending_slide();
        }
        if self.level_drag.as_ref().is_some_and(|d| d.source == source) {
            self.level_drag = None;
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
        // Away from shelf, only the second screen: to it first if the focus is elsewhere.
        let top = self.state.pad_on_top();
        if !top && self.focus_screen() != Screen::Bottom {
            self.focus_bottom();
            return;
        }
        // On a level row, left and right step it.
        if let (Some(Id::Level(level)), Dir::Left | Dir::Right) = (&self.state.focus, dir) {
            let step = if dir == Dir::Left { -1 } else { 1 };
            self.handle(Event::LevelStep(*level, step));
            return;
        }
        let mut all = Vec::new();
        for view in self.views().filter(|v| top || v.screen == Screen::Bottom) {
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

    /// The pad on ClamShell's screens: directions move focus (crossing screens at the edges while shelf
    /// is in front), A acts on the focused target, B cancels the editor, Y toggles the favourite, the
    /// shoulders step shelf's tabs (top screen) or the bar's tabs (second screen). View moves focus to
    /// the other screen with shelf in front; else it moves the pad between the second screen and what
    /// else wants it (the pad mouse, typing; the pad thread sends View from there too).
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
                if self.state.editor.is_some() {
                    self.handle(Event::Activate(Id::EditorCancel));
                }
            }
            PadInput::Y => self.handle(Event::ToggleFavourite),
            PadInput::Shoulder(step) => {
                if self.state.pad_on_top() && self.focus_screen() == Screen::Top {
                    self.handle(Event::TabStep(step));
                } else {
                    // The bar's tabs, left to right.
                    let modes = Mode::TABS;
                    let at = modes.iter().position(|m| *m == self.state.mode).unwrap_or(0) as i32;
                    let next = (at + step).rem_euclid(modes.len() as i32) as usize;
                    if let Some(&mode) = modes.get(next) {
                        self.handle(Event::Activate(Id::Mode(mode)));
                        self.handle(Event::Focus(Id::Mode(mode)));
                    }
                }
            }
            PadInput::Switch if self.state.pad_on_top() => self.switch_screen(),
            PadInput::Switch => {
                if self.state.pad_route == PadRoute::Ui
                    && let Some(slot) = self.last_focus.get_mut(1)
                {
                    *slot = self.state.focus.clone();
                }
                self.handle(Event::PadSwitch);
                if self.state.pad_route == PadRoute::Ui {
                    self.focus_bottom();
                }
            }
        }
    }

    /// Focus onto the second screen: where it last was there if still drawn, else the open tab.
    fn focus_bottom(&mut self) {
        let drawn = |app: &App, id: &Id| app.views().any(|v| v.screen == Screen::Bottom && v.targets.iter().any(|t| &t.id == id));
        let remembered = self.last_focus.get(1).cloned().flatten().filter(|id| drawn(self, id));
        let current = self.state.focus.clone().filter(|id| drawn(self, id));
        let target = current.or(remembered).unwrap_or(Id::Mode(self.state.mode));
        self.handle(Event::Focus(target));
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
