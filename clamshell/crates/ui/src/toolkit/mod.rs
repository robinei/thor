//! What every screen is drawn with. [`Frame`] is the drawing context: it holds the canvas, the state,
//! the targets the screen registers and the size of the area being drawn in. Built on it are [`style`]
//! (palette, sizes, looks), [`widgets`] (headers, menus, rows, sliders, switches, tiles),
//! [`format`](mod@format) (text without allocation) and [`scroll`] (scrolled lists). The screens
//! themselves are in `views`.

mod clock;
pub mod format;
mod icons;
pub mod scroll;
pub mod style;
pub mod widgets;

use clamshell_state::{Event, Id, ItemKey, ListId, PadInput, PointerEvent, State, Tab};

use crate::gfx::images::Images;
use crate::gfx::{Canvas, Rect};
pub use clock::{LocalTime, Scratch};
use style::{Look, PRESSED, RADIUS_S, Text, Tone};

/// Something drawn that can be tapped or focused. Its rect is in layout units while drawing, and in the
/// surface's logical pixels, moved by the burn-in offset, once the frame is done.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub rect: Rect,
    pub id: Id,
    /// Where a finger on it sets a value, if it does.
    pub track: Option<Track>,
    /// The scrolled list it is in, if any. A vertical drag on it scrolls the list.
    pub list: Option<scroll::InList>,
    /// The list item it shows, if it is one, as it was when drawn. A press on it carries the key, so it
    /// acts on that item even if the list changes before the press.
    pub item: Option<ItemKey>,
    /// How the pad reaches it.
    pub pad: PadUse,
}

/// How the pad reaches a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadUse {
    /// By moving to it.
    #[default]
    Moves,
    /// Not at all: it is for fingers, such as a list's area (the pad goes to its rows) or a dock tile's
    /// tray mark (the pad uses the tile's menu).
    Touch,
    /// Only by holding A on the selection, as the Library's details' actions are; then the pad stays
    /// among them until B.
    Held,
    /// By moving to it, and on it the pad's moves are its list's (see [`PadOut::moves`]).
    List,
}

/// The span along x over which a finger on a target sets a value, in the same units as the target's
/// rect. For a level row it is the row's track, 0 at `x0` and 100 at `x0 + w`. For the bar's volume and
/// brightness it is the whole screen width that their slider runs along.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Track {
    pub x0: f32,
    pub w: f32,
}

impl Track {
    /// The value (0-100) a finger at `x` sets.
    pub fn value_at(&self, x: f32) -> u8 {
        let v = (x - self.x0) / self.w.max(1.0) * 100.0;
        v.round().clamp(0.0, 100.0) as u8
    }

    /// `x` relative to where the track starts.
    pub fn along(&self, x: f32) -> f32 {
        x - self.x0
    }
}

/// What the pad's buttons do on a screen, as its views declare while drawing what they act on. The pad's
/// input reads it from the last frame (see `input::focus`), so a button does what the screen shows.
#[derive(Debug, Clone, Default)]
pub struct PadOut {
    /// B and the Back button, and whether it is over the whole screen, as an editor's is, rather than
    /// the screen's own. The last declared wins: what is drawn inside goes back first.
    pub back: Option<(Event, bool)>,
    /// Where the focus goes on the screen when nothing better is known.
    pub start: Option<Id>,
    /// What Y, X and Start press, as their badges show.
    pub keys: [Option<Id>; 3],
    /// What the view's list does with the pad's moves while the focus is on its targets ([`PadUse::List`]):
    /// the directions, and the shoulders' pages and the triggers' ends while the focus is on the screen.
    pub moves: Option<ListMoves>,
    /// The bar's tabs, left to right, each with its button's target, which the shoulders step through.
    pub tabs: [Option<(Tab, Id)>; Tab::ALL.len()],
    /// A mode of the view's, such as a dock tile being moved, that takes the pad's buttons before anything
    /// else does: what each does in it.
    pub mode: Option<fn(PadInput) -> Option<Event>>,
}

/// A move of the pad's that a view's list may take (see [`PadOut::moves`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadMove {
    Dir(crate::input::nav::Dir),
    /// A shoulder: a page on, or back with -1.
    Page(i32),
    /// A trigger: to the end, or the start with -1.
    End(i32),
    /// The wheel, by notches, over the screen where no scrolled list is.
    Scroll(i32),
}

/// What a list does with a move: `None` if it doesn't take it, which then goes by direction as on any
/// screen; else what it does, if anything.
pub type ListMoves = fn(&State, PadMove) -> Option<Option<Event>>;

/// A button of the pad that a badge stands for (see [`widgets::key_action`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadKey {
    B,
    Y,
    X,
    Start,
}

impl PadKey {
    /// Its name on the badge.
    pub fn name(self) -> &'static str {
        match self {
            PadKey::B => "B",
            PadKey::Y => "Y",
            PadKey::X => "X",
            PadKey::Start => "Start",
        }
    }

    /// Its place in [`PadOut::keys`], for the buttons there.
    pub fn index(self) -> Option<usize> {
        match self {
            PadKey::B => None,
            PadKey::Y => Some(0),
            PadKey::X => Some(1),
            PadKey::Start => Some(2),
        }
    }
}

/// What a frame tells the rest of the program besides its picture and targets.
#[derive(Debug, Clone, Default)]
pub struct FrameOut {
    /// An event the frame tells the state once it is drawn, such as how many rows a list shows when that
    /// changed. The last reported in a frame wins; each frame's is told.
    pub report: Option<Event>,
    /// The area below the bar on the second screen, where a finger may point (see clamshell-state's
    /// `touch`).
    pub touchpad_area: Option<Rect>,
    /// How many rows each scrolled list drawn shows, indexed by [`ListId::index`]. The state keeps the
    /// lists' offsets and the pad's focus within them.
    pub lists: [Option<f32>; ListId::COUNT],
    pub pad: PadOut,
}

/// One screen's drawing pass. It holds the canvas, the state it shows, the images and kept text, the
/// targets it registers and what it reports.
pub struct Frame<'f, 'c> {
    pub c: &'f mut Canvas<'c>,
    pub s: &'f State,
    pub images: &'f mut Images,
    pub scratch: &'f mut Scratch,
    pub targets: &'f mut Vec<Target>,
    pub out: FrameOut,
    /// The area the screen's content goes in; on the second screen, the area below the bar. Sizes are
    /// fractions of its height (see [`Frame::pad`], [`Frame::text`]).
    pub body: Rect,
    /// The target pressed, where the item it came down on is now (see `Lists::pressed`).
    pressed: Option<Id>,
}

impl<'f, 'c> Frame<'f, 'c> {
    pub fn new(c: &'f mut Canvas<'c>, s: &'f State, images: &'f mut Images, scratch: &'f mut Scratch, targets: &'f mut Vec<Target>) -> Self {
        targets.clear();
        let pressed = s.lists().pressed();
        Frame { c, s, images, scratch, targets, out: FrameOut::default(), body: Rect::new(0.0, 0.0, 0.0, 0.0), pressed }
    }

    /// The area the screen's content goes in.
    pub fn body(&self) -> Rect {
        self.body
    }

    /// The padding, and the unit other spacing is counted in (see [`style::pad`]).
    pub fn pad(&self) -> f32 {
        style::pad(self.body.h)
    }

    /// A text size (see [`Text`]).
    pub fn text(&self, t: Text) -> f32 {
        t.size(self.body.h)
    }

    /// A tappable, focusable area. Draws its background (lit while pressed), then `content`, then the
    /// focus ring if it is focused. `rect` becomes its target.
    pub fn button(&mut self, id: Id, rect: Rect, look: Look, content: impl FnOnce(&mut Self)) {
        self.button_in(id, rect, rect, look, content);
    }

    /// [`Frame::button`] for something drawn smaller than its target. The background and the focus ring
    /// follow `shown`, and the target is `rect`.
    pub fn button_in(&mut self, id: Id, rect: Rect, shown: Rect, look: Look, content: impl FnOnce(&mut Self)) {
        let pressed = self.pressed(&id);
        let background = if pressed && look.press_fill { Some(PRESSED) } else { look.fill };
        if let Some(color) = background {
            let r = shown.inset(look.inset);
            if look.radius > 0.0 {
                self.c.rounded(r, look.radius, color);
            } else {
                self.c.fill(r, color);
            }
        }
        content(self);
        if self.s.pointer().focused(&id) {
            self.ring(shown, look);
        }
        let item = self.s.lists().item_key(&id);
        self.targets.push(Target { rect, id, track: None, list: None, item, pad: PadUse::Moves });
    }

    /// The focus ring of a button of `look` shown at `shown`.
    pub fn ring(&mut self, shown: Rect, look: Look) {
        self.c.outline(shown.inset(look.ring), look.radius.max(RADIUS_S), 2.0, Tone::Accent);
    }

    /// Whether `id` is held down by a finger or the pad's A. A list item is lit where the item pressed is
    /// now. A control whose slider is up doesn't count, since the finger is then on the slider rather
    /// than the button.
    pub fn pressed(&self, id: &Id) -> bool {
        self.pressed.as_ref() == Some(id) && !self.sliding(id)
    }

    /// Whether `id` is a control whose slider is held: a level's, a keymap's or a curve's parameter's, or
    /// a setting's.
    pub fn sliding(&self, id: &Id) -> bool {
        let Id::Control(control) = id else {
            return false;
        };
        self.s.held_slider().is_some_and(|s| s.control == *control)
    }

    /// A text action (see [`Look::TEXT`]). Draws `label` in `r` at `size`, centred, or from the left with
    /// [`crate::gfx::Align::Start`]. It is in `tone` (blue for an action, see [`Tone::Fn`]), and white
    /// while pressed. The whole of `r` is its target.
    pub fn text_button(&mut self, id: Id, r: Rect, size: f32, tone: Tone, label: &str, align: crate::gfx::Align) {
        let tone = if self.pressed(&id) { Tone::High } else { tone };
        self.button(id, r, Look::TEXT, |f| {
            let w = f.c.text_width(size, label).min(r.w);
            let x = if align == crate::gfx::Align::Start { r.x } else { r.x + (r.w - w) / 2.0 };
            f.c.text_fit(Rect::new(x, r.y, w + 1.0, r.h), size, tone, label, crate::gfx::Keep::Start);
        });
    }

    /// Gives the last target a track. The caller passes it because whatever drew the track knows where
    /// it is.
    pub fn track(&mut self, track: Track) {
        if let Some(t) = self.targets.last_mut() {
            t.track = Some(track);
        }
    }

    /// How the pad reaches the last target drawn.
    pub fn pad_use(&mut self, pad: PadUse) {
        if let Some(t) = self.targets.last_mut() {
            t.pad = pad;
        }
    }

    /// B goes back by pressing `id`, as the screen's own back.
    pub fn back(&mut self, id: Id) {
        self.back_with(PointerEvent::Activate(id).into());
    }

    /// B goes back by `event`, as the screen's own back.
    pub fn back_with(&mut self, event: Event) {
        if !self.out.pad.back.as_ref().is_some_and(|(_, over)| *over) {
            self.out.pad.back = Some((event, false));
        }
    }

    /// B goes back by pressing `id`, over the whole screen (an editor's Cancel).
    pub fn back_over(&mut self, id: Id) {
        self.out.pad.back = Some((PointerEvent::Activate(id).into(), true));
    }

    /// Tells the state `event` once the frame is drawn (see [`FrameOut::report`]).
    pub fn report(&mut self, event: Event) {
        self.out.report = Some(event);
    }

    /// The view's list takes the pad's moves as `moves` says, on its targets marked [`PadUse::List`].
    pub fn list_moves(&mut self, moves: ListMoves) {
        self.out.pad.moves = Some(moves);
    }

    /// How many targets have been drawn: with [`Frame::pad_use_since`], what a widget drew is marked.
    pub fn targets_drawn(&self) -> usize {
        self.targets.len()
    }

    /// How the pad reaches the targets it moves to drawn since `first` (see [`Frame::targets_drawn`]); those
    /// for fingers alone stay so.
    pub fn pad_use_since(&mut self, first: usize, pad: PadUse) {
        for t in self.targets.iter_mut().skip(first).filter(|t| t.pad == PadUse::Moves) {
            t.pad = pad;
        }
    }

    /// The bar's tab `tab` is drawn, after those left of it, with its button's target `id`.
    pub fn bar_tab(&mut self, tab: Tab, id: Id) {
        if let Some(slot) = self.out.pad.tabs.iter_mut().find(|t| t.is_none()) {
            *slot = Some((tab, id));
        }
    }

    /// The view is in a mode that takes the pad's buttons, each as `input` says.
    pub fn mode(&mut self, input: fn(PadInput) -> Option<Event>) {
        self.out.pad.mode = Some(input);
    }

    /// The focus starts on `id` when nothing better is known.
    pub fn start(&mut self, id: Id) {
        self.out.pad.start = Some(id);
    }

    /// `key` presses `id`.
    pub fn key(&mut self, key: PadKey, id: Id) {
        match key.index().and_then(|i| self.out.pad.keys.get_mut(i)) {
            Some(slot) => *slot = Some(id),
            None => self.back(id),
        }
    }

    pub fn finish(self) -> FrameOut {
        self.out
    }
}
