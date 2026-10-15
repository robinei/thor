//! Where the AYN Thor's controls are drawn on the keymap edit screen. They sit on either side of its
//! bottom screen, so they are drawn as two clusters at the screen's edges, each a column of groups in
//! the order of [`LEFT`] and [`RIGHT`]: the shoulders (on the back's top edge), a small round button
//! above the screen's corner (Select, Start), a stick and the D-pad or the face buttons, and a small
//! round button at the screen's bottom corner (Home, Back: drawn, not remappable). The gyro sits in the
//! middle, at the bottom, where the AYN button is under the screen.
//!
//! A remapped control has a line in to its label (between the clusters): it leaves the control's inner
//! side straight across, or, for the controls a line across would pass through others (the outer one of
//! the D-pad, of a stick's directions, of the face buttons), first straight up above its group.

use clamshell_core::keymap::{Button, Dir, Input, Side};

use crate::draw::Rect;

/// A group of controls in a cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// L2, R2.
    Trigger,
    /// L1, R1.
    Bumper,
    /// The small round button above the screen's corner: Select, Start.
    Top,
    Stick,
    Dpad,
    Face,
    /// The small round button at the screen's bottom corner: Home, Back.
    Foot,
}

/// Each side's groups from top to bottom, and the share of the cluster's height each takes.
pub const LEFT: [(Group, f32); 6] = [(Group::Trigger, 0.11), (Group::Bumper, 0.08), (Group::Top, 0.1), (Group::Stick, 0.31), (Group::Dpad, 0.31), (Group::Foot, 0.09)];
pub const RIGHT: [(Group, f32); 6] = [(Group::Trigger, 0.11), (Group::Bumper, 0.08), (Group::Top, 0.1), (Group::Face, 0.31), (Group::Stick, 0.31), (Group::Foot, 0.09)];

/// Sizes in a group, of the smaller of its width and height. A stick: the well's radius, the cap's and
/// the click's (dimple); the click's target's half height (its width is the cap's).
const WELL: f32 = 0.38;
const CAP: f32 = 0.27;
const DIMPLE: f32 = 0.11;
const CLICK: f32 = 0.145;
/// The D-pad: an arm's reach from the centre, its half width, where it starts from the centre.
const ARM_L: f32 = 0.38;
const ARM_HALF: f32 = 0.105;
const ARM_FROM: f32 = 0.13;
/// The face buttons: their distance from the centre and radius.
const FACE_OFF: f32 = 0.33;
const FACE_R: f32 = 0.12;
/// Where the lane above a group runs, below its top.
const LANE: f32 = 0.035;
/// The trigger and bumper, of the group's width and height.
const TRIGGER: (f32, f32) = (0.5, 0.82);
const BUMPER: (f32, f32) = (0.76, 0.5);
/// The small round buttons' radius, of their group's height, and their centre's distance from the
/// cluster's inner edge, of their radius.
const ROUND: f32 = 0.2;
const ROUND_IN: f32 = 2.2;

/// A control as drawn: its shape, its target, and where its line in leaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    /// What is drawn (its highlight and focus ring follow it), and the corner radius of that.
    pub shown: Rect,
    pub radius: f32,
    /// What a finger hits; a stick also takes its group's corners.
    pub target: Rect,
    pub corners: Option<[Rect; 4]>,
    /// Where its line starts, and the height of the lane it goes up to first, if it does.
    pub anchor: (f32, f32),
    pub lane: Option<f32>,
    pub side: Side,
}

/// The clusters placed: the area they're in and each one's width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pad {
    area: Rect,
    width: f32,
}

impl Pad {
    pub fn new(area: Rect, width: f32) -> Pad {
        Pad { area, width }
    }

    /// A side's cluster.
    pub fn cluster(&self, side: Side) -> Rect {
        let x = if side == Side::Left { self.area.x } else { self.area.right() - self.width };
        Rect::new(x, self.area.y, self.width, self.area.h)
    }

    /// Where `group` is on `side`, if it is there.
    pub fn group(&self, side: Side, group: Group) -> Option<Rect> {
        let c = self.cluster(side);
        let table = if side == Side::Left { LEFT } else { RIGHT };
        let total: f32 = table.iter().map(|(_, share)| share).sum();
        let mut y = c.y;
        for (g, share) in table {
            let h = c.h * share / total.max(1e-3);
            if g == group {
                return Some(Rect::new(c.x, y, c.w, h));
            }
            y += h;
        }
        None
    }

    /// A stick's well, as a square around it.
    pub fn well(&self, side: Side) -> Option<Rect> {
        let g = self.group(side, Group::Stick)?;
        let s = g.w.min(g.h);
        Some(square(g.center(), WELL * s))
    }

    /// A small round button in group `g` on `side`, near the cluster's inner edge.
    fn round(&self, side: Side, g: Rect) -> Rect {
        let r = (g.h * ROUND).min(g.w * 0.07);
        let x = if side == Side::Left { g.right() - ROUND_IN * r } else { g.x + ROUND_IN * r };
        square((x, g.center().1), r)
    }

    /// Home's or Back's small round button, at the screen's bottom corner (drawn, not remappable).
    pub fn foot(&self, side: Side) -> Option<Rect> {
        let g = self.group(side, Group::Foot)?;
        Some(self.round(side, g))
    }

    /// Where `input` is, if it is in a cluster (the gyro is in the middle; the Thor has no back buttons).
    pub fn part(&self, input: Input) -> Option<Part> {
        let (side, group) = match input {
            Input::Trigger(s) => (s, Group::Trigger),
            Input::Stick(s) | Input::StickDir(s, _) => (s, Group::Stick),
            Input::Dpad(_) => (side_of(Group::Dpad), Group::Dpad),
            Input::Button(b) => match b {
                Button::LB => (Side::Left, Group::Bumper),
                Button::RB => (Side::Right, Group::Bumper),
                Button::LS => (Side::Left, Group::Stick),
                Button::RS => (Side::Right, Group::Stick),
                Button::View => (Side::Left, Group::Top),
                Button::Menu => (Side::Right, Group::Top),
                Button::A | Button::B | Button::X | Button::Y => (side_of(Group::Face), Group::Face),
                // The Thor has no back buttons.
                Button::L4 | Button::L5 | Button::R4 | Button::R5 => return None,
            },
            Input::Gyro => return None,
        };
        // Toward the labels.
        let k = if side == Side::Left { 1.0 } else { -1.0 };
        let g = self.group(side, group)?;
        let (cx, cy) = g.center();
        let s = g.w.min(g.h);
        let lane = g.y + LANE * s;
        let flat = |shown: Rect, radius: f32| Part { shown, radius, target: g, corners: None, anchor: (inner(shown, k), cy), lane: None, side };
        let part = match input {
            Input::Trigger(_) => {
                let shown = g.centered(g.w * TRIGGER.0, g.h * TRIGGER.1);
                flat(shown, shown.w / 2.0)
            }
            Input::Button(Button::LB | Button::RB) => {
                let shown = g.centered(g.w * BUMPER.0, (g.h * BUMPER.1).min(g.w * 0.16));
                flat(shown, shown.h / 2.0)
            }
            Input::Button(Button::View | Button::Menu) => {
                let shown = self.round(side, g);
                Part { shown, radius: shown.w / 2.0, target: g, corners: None, anchor: (inner(shown, k), cy), lane: None, side }
            }
            Input::Stick(_) => {
                // From the cap's edge above the middle, clear of the direction marks.
                let r = CAP * s;
                let (x0, x1, y0, y1) = (cx - r, cx + r, cy - r, cy + r);
                let corners = [
                    Rect::new(g.x, g.y, x0 - g.x, y0 - g.y),
                    Rect::new(x1, g.y, g.right() - x1, y0 - g.y),
                    Rect::new(g.x, y1, x0 - g.x, g.bottom() - y1),
                    Rect::new(x1, y1, g.right() - x1, g.bottom() - y1),
                ];
                Part { shown: square((cx, cy), r), radius: r, target: square((cx, cy), r), corners: Some(corners), anchor: (cx + k * 0.8 * r, cy - 0.6 * r), lane: None, side }
            }
            Input::Button(Button::LS | Button::RS) => {
                let (r, cap) = (DIMPLE * s, CAP * s);
                let target = Rect::new(cx - cap, cy - CLICK * s, 2.0 * cap, 2.0 * CLICK * s);
                Part { shown: square((cx, cy), r), radius: r, target, corners: None, anchor: (cx + k * 0.7 * r, cy + 0.7 * r), lane: None, side }
            }
            Input::StickDir(_, d) => {
                let (dx, dy) = dir(d);
                let (well, cap) = (WELL * s, CAP * s);
                let mid = (well + cap) / 2.0;
                let half = (well - cap) / 2.0;
                let at = (cx + dx * mid, cy + dy * mid);
                let shown = square(at, half);
                let target = match d {
                    Dir::Up => Rect::new(cx - cap, g.y, 2.0 * cap, cy - cap - g.y),
                    Dir::Down => Rect::new(cx - cap, cy + cap, 2.0 * cap, g.bottom() - cy - cap),
                    Dir::Left => Rect::new(g.x, cy - cap, cx - cap - g.x, 2.0 * cap),
                    Dir::Right => Rect::new(cx + cap, cy - cap, g.right() - cx - cap, 2.0 * cap),
                };
                let toward = dx * k;
                let (anchor, lane) = if toward > 0.0 {
                    ((at.0 + k * half, at.1), None)
                } else if toward < 0.0 {
                    // Away from the labels: up from the mark, through the well, to the lane.
                    ((at.0, at.1 - half), Some(lane))
                } else {
                    ((at.0 + k * half, at.1), None)
                };
                Part { shown, radius: half, target, corners: None, anchor, lane, side }
            }
            Input::Dpad(d) => {
                let (dx, dy) = dir(d);
                let (l, w, from) = (ARM_L * s, ARM_HALF * s, ARM_FROM * s);
                let shown = if dx == 0.0 {
                    let (y0, y1) = (cy + dy * from, cy + dy * l);
                    Rect::new(cx - w, y0.min(y1), 2.0 * w, (y1 - y0).abs())
                } else {
                    let (x0, x1) = (cx + dx * from, cx + dx * l);
                    Rect::new(x0.min(x1), cy - w, (x1 - x0).abs(), 2.0 * w)
                };
                let band = w * 1.7;
                let target = match d {
                    Dir::Up => Rect::new(g.x, g.y, g.w, cy - band - g.y),
                    Dir::Down => Rect::new(g.x, cy + band, g.w, g.bottom() - cy - band),
                    Dir::Left => Rect::new(g.x, cy - band, cx - g.x, 2.0 * band),
                    Dir::Right => Rect::new(cx, cy - band, g.right() - cx, 2.0 * band),
                };
                let (anchor, lane) = if dx * k < 0.0 { ((shown.center().0, shown.y), Some(lane)) } else { ((inner(shown, k), shown.center().1), None) };
                Part { shown, radius: 0.02 * s, target, corners: None, anchor, lane, side }
            }
            Input::Button(b) => {
                let (dx, dy) = match b {
                    Button::A => (0.0, 1.0),
                    Button::B => (1.0, 0.0),
                    Button::X => (-1.0, 0.0),
                    _ => (0.0, -1.0),
                };
                let (o, r) = (FACE_OFF * s, FACE_R * s);
                let at = (cx + dx * o, cy + dy * o);
                let shown = square(at, r);
                // X and B take the middle band, down to where Y and A start.
                let band = o - r;
                let target = if dy < 0.0 {
                    Rect::new(g.x, g.y, g.w, cy - band - g.y)
                } else if dy > 0.0 {
                    Rect::new(g.x, cy + band, g.w, g.bottom() - cy - band)
                } else if dx < 0.0 {
                    Rect::new(g.x, cy - band, cx - g.x, 2.0 * band)
                } else {
                    Rect::new(cx, cy - band, g.right() - cx, 2.0 * band)
                };
                let (anchor, lane) = if dx * k < 0.0 { ((at.0, at.1 - r), Some(lane)) } else { ((at.0 + k * r, at.1), None) };
                Part { shown, radius: r, target, corners: None, anchor, lane, side }
            }
            Input::Gyro => return None,
        };
        Some(part)
    }
}

/// Which side's table has `group` (the D-pad and the face buttons may be on either).
fn side_of(group: Group) -> Side {
    if LEFT.iter().any(|(g, _)| *g == group) { Side::Left } else { Side::Right }
}

/// The side of `r` toward the labels (`k` 1: right).
fn inner(r: Rect, k: f32) -> f32 {
    if k > 0.0 { r.right() } else { r.x }
}

fn square((x, y): (f32, f32), half: f32) -> Rect {
    Rect::new(x - half, y - half, 2.0 * half, 2.0 * half)
}

fn dir(d: Dir) -> (f32, f32) {
    match d {
        Dir::Up => (0.0, -1.0),
        Dir::Down => (0.0, 1.0),
        Dir::Left => (-1.0, 0.0),
        Dir::Right => (1.0, 0.0),
    }
}
