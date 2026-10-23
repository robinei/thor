//! Where the pad's focus goes on each shot of the snapshot scenario, worked out from the shot's frames
//! and state by the UI's own pad functions (clamshell-ui's `PadView`). For each target the pad may focus
//! on the shot's screen it gives where up, down, left and right lead, with where the pad starts and what
//! B does. The test keeps this as text in tests/focus.txt, checks rules every screen must keep, and draws
//! the moves over the shot when they change.
//!
//! The state isn't `Clone`, so the state with the focus on a target is made by applying the events that
//! led to the shot to a fresh state again, then the focus.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::Path;

use clamshell_state::{Event, Id, PadRoute, PointerEvent, ShelfId, State};
use clamshell_ui::{Dir, PadFrames, PadTarget, PadView, Screen};

use super::trace;

pub const DIRS: [(Dir, &str); 4] = [(Dir::Up, "up"), (Dir::Down, "down"), (Dir::Left, "left"), (Dir::Right, "right")];

/// How many frames a shot's map may record for states that moves lead to, beyond the shot's own. Moves
/// through a scrolled list record one each.
const MAX_FRAMES: usize = 400;

/// Where a move leads.
#[derive(Debug, Clone, PartialEq)]
pub enum Move {
    /// Nothing happens, or the focus stays where it is.
    Stays,
    /// The focus goes to this target.
    To(Id),
    /// This event, which moves the focus to the target given, or leaves it where it is (as a level's step
    /// does).
    Event(Event, Option<Id>),
}

impl Move {
    /// Where the focus is after the move, if it moved.
    fn lands(&self) -> Option<&Id> {
        match self {
            Move::Stays | Move::Event(_, None) => None,
            Move::To(id) | Move::Event(_, Some(id)) => Some(id),
        }
    }

    fn text(&self) -> String {
        match self {
            Move::Stays => "-".into(),
            Move::To(id) => format!("{id:?}"),
            Move::Event(e, None) => format!("[{}]", trace::event(e)),
            Move::Event(e, Some(id)) => format!("[{}] {id:?}", trace::event(e)),
        }
    }
}

/// A shot's map.
pub struct Map {
    pub name: String,
    /// The map as text, starting with a `== name` line.
    pub text: String,
    /// The shot screen's targets the pad may focus, with their moves in the order of [`DIRS`].
    pub targets: Vec<(PadTarget, [Move; 4])>,
    /// Everything drawn on the shot's screen.
    pub drawn: Vec<PadTarget>,
    pub start: Option<Id>,
    /// The rules the map breaks, one line each.
    pub failures: Vec<String>,
}

/// A state the map reaches: the events after the shot's that make it, and the frame it is moved over
/// in (an index into the recorded views).
struct Node {
    prefix: Vec<Event>,
    view: usize,
}

/// What maps a shot.
pub struct Mapper<'a> {
    fresh: &'a dyn Fn() -> State,
    frames: PadFrames,
    size: fn(Screen) -> (u32, u32),
}

impl<'a> Mapper<'a> {
    pub fn new(fresh: &'a dyn Fn() -> State, size: fn(Screen) -> (u32, u32)) -> Mapper<'a> {
        Mapper { fresh, frames: PadFrames::new(size(Screen::Top), size(Screen::Bottom)), size }
    }

    /// The state after `log` and then `more`.
    fn replay(&self, log: &[Event], more: &[Event]) -> State {
        let mut state = (self.fresh)();
        let mut effects = Vec::new();
        for event in log.iter().chain(more) {
            effects.clear();
            state.apply(event.clone(), &mut effects);
        }
        state
    }

    /// Whether `id` is drawn in `view` where it can be seen: a target of it lies on its screen and no
    /// target drawn later covers its middle.
    fn shown(&self, view: &PadView, id: &Id) -> bool {
        Screen::BOTH.into_iter().any(|screen| {
            let drawn = view.drawn(screen);
            let (w, h) = (self.size)(screen);
            drawn.iter().enumerate().any(|(i, t)| {
                let [x, y, tw, th] = t.rect;
                let on_screen = tw > 0.0 && th > 0.0 && x < w as f32 && y < h as f32 && x + tw > 0.0 && y + th > 0.0;
                let (cx, cy) = (x + tw / 2.0, y + th / 2.0);
                let covered = drawn.iter().skip(i + 1).any(|o| {
                    let [ox, oy, ow, oh] = o.rect;
                    !matches!(o.id, Id::List(_)) && o.id != *id && cx > ox && cx < ox + ow && cy > oy && cy < oy + oh
                });
                &t.id == id && on_screen && !covered
            })
        })
    }

    /// Maps the shot `name` of `screen`, whose state is `state`, made by `log`.
    pub fn map(&mut self, state: &State, log: &[Event], name: &str, screen: Screen) -> Map {
        if state.pad().route() == PadRoute::Listen {
            // The pad's inputs are chosen on the screen rather than moving the focus.
            let text = format!("== {name} ({}; the pad listening for inputs)\n", screen_name(screen));
            return Map { name: name.to_owned(), text, targets: Vec::new(), drawn: Vec::new(), start: None, failures: Vec::new() };
        }
        let view = self.frames.record(state);
        // A target drawn in pieces has one entry per piece; the pad moves from its first.
        let mut ids = HashSet::new();
        let focusable: Vec<PadTarget> = view.focusable(state).into_iter().filter(|t| ids.insert(t.id.clone())).collect();
        let drawn = view.drawn(screen);
        let start = view.start(state);
        let mut views = vec![view];
        let mut failures = Vec::new();
        let mut nodes: HashMap<Id, Node> =
            focusable.iter().map(|t| (t.id.clone(), Node { prefix: vec![PointerEvent::Focus(t.id.clone()).into()], view: 0 })).collect();
        let mut moves: HashMap<Id, [Move; 4]> = HashMap::new();
        for t in &focusable {
            let m = self.moves(log, &t.id, &mut nodes, &mut views, &mut failures);
            moves.insert(t.id.clone(), m);
        }
        // Every target is reached from where the pad starts.
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        match &start {
            Some(id) => {
                nodes.entry(id.clone()).or_insert_with(|| Node { prefix: vec![PointerEvent::Focus(id.clone()).into()], view: 0 });
                seen.insert(id.clone());
                queue.push_back(id.clone());
            }
            None => failures.push("the pad has nowhere to start".into()),
        }
        while let Some(id) = queue.pop_front() {
            if !moves.contains_key(&id) {
                if views.len() > MAX_FRAMES {
                    failures.push(format!("more than {MAX_FRAMES} frames to follow the moves; stopped at {id:?}"));
                    break;
                }
                let m = self.moves(log, &id, &mut nodes, &mut views, &mut failures);
                moves.insert(id.clone(), m);
            }
            for to in moves.get(&id).into_iter().flatten().filter_map(Move::lands) {
                if seen.insert(to.clone()) {
                    queue.push_back(to.clone());
                }
            }
        }
        // Except shelf's collections and its way up out of a group: left and right and B do their work.
        let by_button = |id: &Id| matches!(id, Id::Shelf(ShelfId::Collection(_) | ShelfId::Up));
        let unreached: Vec<String> = focusable.iter().filter(|t| !seen.contains(&t.id) && !by_button(&t.id)).map(|t| format!("{:?}", t.id)).collect();
        if !unreached.is_empty() && start.is_some() {
            failures.push(format!("not reached from {:?}: {}", start, unreached.join(", ")));
        }
        // No target where the focus can't leave, unless it is the only one.
        if focusable.len() > 1 {
            for t in &focusable {
                if moves.get(&t.id).is_some_and(|m| m.iter().all(|m| m.lands().is_none_or(|to| to == &t.id))) {
                    failures.push(format!("a trap: no move leaves {:?}", t.id));
                }
            }
        }
        // With shelf in front, up and down step through its list.
        if PadView::both_screens(state) && !state.shelf().in_actions(state.pointer()) {
            self.check_shelf_list(state, &focusable, &moves, &mut failures);
        }
        let targets: Vec<(PadTarget, [Move; 4])> =
            focusable.into_iter().filter(|t| t.screen == screen).map(|t| (t.clone(), moves.get(&t.id).cloned().unwrap_or_else(stays))).collect();
        let text = text(state, name, screen, start.as_ref(), &targets);
        Map { name: name.to_owned(), text, targets, drawn, start, failures }
    }

    /// The four moves from the focus on `id`. A move to a target not shown in the frame the moves are
    /// made over is followed into a frame recorded for the state it leads to, where the target has to be
    /// shown, as when a list scrolls to its next row.
    fn moves(&mut self, log: &[Event], id: &Id, nodes: &mut HashMap<Id, Node>, views: &mut Vec<PadView>, failures: &mut Vec<String>) -> [Move; 4] {
        let Some(node) = nodes.get(id) else {
            return stays();
        };
        let (prefix, at) = (node.prefix.clone(), node.view);
        let state = self.replay(log, &prefix);
        let mut out = stays();
        for (slot, (dir, word)) in out.iter_mut().zip(DIRS) {
            let Some(view) = views.get(at) else {
                break;
            };
            let event = view.navigate(&state, dir);
            let m = match event {
                None => Move::Stays,
                Some(Event::Pointer(PointerEvent::Focus(to))) if &to == id => Move::Stays,
                Some(Event::Pointer(PointerEvent::Focus(to))) => Move::To(to),
                Some(e) => {
                    let after = self.replay(log, &[prefix.as_slice(), std::slice::from_ref(&e)].concat());
                    match after.pointer().focus() {
                        Some(f) if f != id => Move::Event(e, Some(f.clone())),
                        _ => Move::Event(e, None),
                    }
                }
            };
            if let Some(to) = m.lands().cloned()
                && !nodes.contains_key(&to)
            {
                // Moved to something not shown in this frame, or by an event that may scroll: follow it
                // into the frame of the state it leads to.
                let step = match &m {
                    Move::Event(e, _) => e.clone(),
                    _ => PointerEvent::Focus(to.clone()).into(),
                };
                let prefix = [prefix.as_slice(), std::slice::from_ref(&step)].concat();
                let shown_here = matches!(m, Move::To(_)) && views.get(at).is_some_and(|v| self.shown(v, &to));
                let view = if shown_here {
                    at
                } else {
                    let after = self.replay(log, &prefix);
                    let recorded = self.frames.record(&after);
                    if !self.shown(&recorded, &to) {
                        failures.push(format!("{word} from {id:?} lands on {to:?}, which isn't shown"));
                    }
                    views.push(recorded);
                    views.len() - 1
                };
                nodes.insert(to, Node { prefix, view });
            } else if let Some(to) = m.lands()
                && at == 0
                && nodes.get(to).is_some_and(|n| n.view == 0)
                && views.first().is_some_and(|v| !self.shown(v, to))
            {
                failures.push(format!("{word} from {id:?} lands on {to:?}, which isn't shown"));
            }
            *slot = m;
        }
        out
    }

    /// With shelf in front, up and down on an entry go to the entry above and below; up stays on the
    /// first, and down leaves the list from the last.
    fn check_shelf_list(&self, state: &State, focusable: &[PadTarget], moves: &HashMap<Id, [Move; 4]>, failures: &mut Vec<String>) {
        let entry = |i: usize| state.shelf().entry_at(i).map(|e| Id::Shelf(ShelfId::Entry(e.id.clone())));
        let len = state.shelf().list().len();
        for t in focusable {
            let Id::Shelf(ShelfId::Entry(_)) = &t.id else {
                continue;
            };
            let Some(i) = (0..len).find(|&i| entry(i).as_ref() == Some(&t.id)) else {
                continue;
            };
            let Some([up, down, _, _]) = moves.get(&t.id) else {
                continue;
            };
            let after = |m: &Move| m.lands().cloned().unwrap_or_else(|| t.id.clone());
            let want_up = i.checked_sub(1).and_then(entry).unwrap_or_else(|| t.id.clone());
            if after(up) != want_up {
                failures.push(format!("up from {:?} (entry {i}) goes to {:?}, not {want_up:?}", t.id, after(up)));
            }
            let down_to = after(down);
            if i + 1 < len {
                let want = entry(i + 1);
                if Some(&down_to) != want.as_ref() {
                    failures.push(format!("down from {:?} (entry {i}) goes to {down_to:?}, not {want:?}", t.id));
                }
            } else if matches!(down_to, Id::Shelf(ShelfId::Entry(_))) {
                failures.push(format!("down from the last entry {:?} stays in the list, at {down_to:?}", t.id));
            }
        }
    }
}

fn stays() -> [Move; 4] {
    [Move::Stays, Move::Stays, Move::Stays, Move::Stays]
}

fn screen_name(screen: Screen) -> &'static str {
    match screen {
        Screen::Top => "top",
        Screen::Bottom => "bottom",
    }
}

/// The map as text.
fn text(state: &State, name: &str, screen: Screen, start: Option<&Id>, targets: &[(PadTarget, [Move; 4])]) -> String {
    let screen = screen_name(screen);
    let pad = if PadView::both_screens(state) && state.shelf().in_actions(state.pointer()) {
        "shelf's actions"
    } else if PadView::both_screens(state) {
        "both screens"
    } else {
        "the second screen"
    };
    let back = PadView::back(state).map_or("-".into(), |e| trace::event(&e));
    let mut out = format!("== {name} ({screen}; the pad on {pad})\nstart {}; back {back}\n", start.map_or("-".into(), |id| format!("{id:?}")));
    for (t, moves) in targets {
        let _ = write!(out, "{:?}:", t.id);
        for ((_, word), m) in DIRS.iter().zip(moves) {
            let _ = write!(out, " {word} {}", m.text());
        }
        out.push('\n');
    }
    out
}

/// Draws `map` over the shot's picture at `png` into `out`: from each target an arrow to where each
/// direction leads (up cyan, down magenta, left yellow, right green; a short one out of the screen for a
/// target elsewhere), and a red box around where the pad starts.
pub fn overlay(map: &Map, png: &Path, out: &Path) -> Result<(), image::ImageError> {
    let mut image = image::open(png)?.to_rgba8();
    let rect_of = |id: &Id| map.drawn.iter().rev().find(|t| &t.id == id).map(|t| t.rect);
    for (t, moves) in &map.targets {
        for ((dir, _), m) in DIRS.iter().zip(moves) {
            let Some(to) = m.lands() else {
                continue;
            };
            let color = match dir {
                Dir::Up => [0, 200, 255],
                Dir::Down => [255, 80, 220],
                Dir::Left => [255, 210, 0],
                Dir::Right => [80, 255, 80],
            };
            let (from, end) = ends(*dir, t.rect, rect_of(to));
            arrow(&mut image, from, end, color);
        }
    }
    if let Some([x, y, w, h]) = map.start.as_ref().and_then(rect_of) {
        for d in 0..4 {
            let d = d as f32;
            let red = [255, 40, 40];
            let (x0, y0, x1, y1) = (x - d, y - d, x + w + d, y + h + d);
            line(&mut image, (x0, y0), (x1, y0), red, 1.0);
            line(&mut image, (x1, y0), (x1, y1), red, 1.0);
            line(&mut image, (x1, y1), (x0, y1), red, 1.0);
            line(&mut image, (x0, y1), (x0, y0), red, 1.0);
        }
    }
    image.save(out)
}

/// Where the arrow for a move `dir` from the rect `from` to the rect `to` starts and ends: from the side
/// it leaves by to the side it arrives at, or a short way out for a target not on the screen. Up and
/// down are drawn a little apart, as are left and right, so that a pair between two targets shows both.
fn ends(dir: Dir, from: [f32; 4], to: Option<[f32; 4]>) -> ((f32, f32), (f32, f32)) {
    let side = |[x, y, w, h]: [f32; 4], leaving: bool| match (dir, leaving) {
        (Dir::Up, true) => (x + w * 0.45, y),
        (Dir::Up, false) => (x + w * 0.45, y + h),
        (Dir::Down, true) => (x + w * 0.55, y + h),
        (Dir::Down, false) => (x + w * 0.55, y),
        (Dir::Left, true) => (x, y + h * 0.55),
        (Dir::Left, false) => (x + w, y + h * 0.55),
        (Dir::Right, true) => (x + w, y + h * 0.45),
        (Dir::Right, false) => (x, y + h * 0.45),
    };
    let start = side(from, true);
    let (dx, dy) = match dir {
        Dir::Up => (0.0, -40.0),
        Dir::Down => (0.0, 40.0),
        Dir::Left => (-40.0, 0.0),
        Dir::Right => (40.0, 0.0),
    };
    (start, to.map_or((start.0 + dx, start.1 + dy), |r| side(r, false)))
}

/// A line `width` pixels wide.
fn line(image: &mut image::RgbaImage, (x0, y0): (f32, f32), (x1, y1): (f32, f32), [r, g, b]: [u8; 3], width: f32) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as usize;
    let half = (width / 2.0).max(0.5);
    for i in 0..=steps {
        let f = i as f32 / steps as f32;
        let (x, y) = (x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
        let (ax, bx, ay, by) = ((x - half).round() as i64, (x + half).round() as i64, (y - half).round() as i64, (y + half).round() as i64);
        for px in ax..bx.max(ax + 1) {
            for py in ay..by.max(ay + 1) {
                if let (Ok(px), Ok(py)) = (u32::try_from(px), u32::try_from(py))
                    && px < image.width()
                    && py < image.height()
                {
                    image.put_pixel(px, py, image::Rgba([r, g, b, 255]));
                }
            }
        }
    }
}

/// A line with a head at `to`.
fn arrow(image: &mut image::RgbaImage, from: (f32, f32), to: (f32, f32), color: [u8; 3]) {
    line(image, from, to, color, 3.0);
    let angle = (to.1 - from.1).atan2(to.0 - from.0);
    for side in [-0.45f32, 0.45] {
        let a = angle + std::f32::consts::PI + side;
        line(image, to, (to.0 + 16.0 * a.cos(), to.1 + 16.0 * a.sin()), color, 3.0);
    }
}
