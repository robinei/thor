//! Splits the touchscreen's multitouch stream (protocol B) between the UI and the virtual touchpad. Each
//! finger is decided at touch-down: one that starts on a region the UI keeps (the bar) is the UI's,
//! wherever it moves; any other is the touchpad's, its events copied in touchpad coordinates. Pure: raw
//! events in, what they become out.

use clamshell_core::Id;
use clamshell_core::touch::TouchLayout;
use evdev::{AbsoluteAxisCode, EventType, KeyCode};

use crate::input::{Press, Range, Raw, Rect};

/// Slots (fingers) tracked.
pub const SLOTS: usize = 10;
/// BTN_TOOL_FINGER, _DOUBLETAP, _TRIPLETAP, _QUADTAP: how many fingers are on the touchpad.
const TOOLS: [KeyCode; 4] = [KeyCode::BTN_TOOL_FINGER, KeyCode::BTN_TOOL_DOUBLETAP, KeyCode::BTN_TOOL_TRIPLETAP, KeyCode::BTN_TOOL_QUADTAP];

/// A finger on a UI target.
#[derive(Debug, Clone, PartialEq)]
pub enum UiTouch {
    /// It went down on the target, at this horizontal screen position.
    Down { id: Id, x: f32 },
    /// It moved to this horizontal screen position.
    Move { x: f32 },
    /// It lifted.
    Up { id: Id },
}

/// What a frame of touchscreen events becomes.
#[derive(Debug, Default, PartialEq)]
pub struct Routed {
    /// Events for the virtual touchpad (positions in touchpad coordinates).
    pub pad: Vec<Raw>,
    pub ui: Vec<UiTouch>,
}

/// The touchscreen's raw coordinate ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axes {
    pub x: Range,
    pub y: Range,
}

#[derive(Debug, Clone, PartialEq)]
enum Owner {
    Pad,
    Ui(Id),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase {
    /// No finger in this slot.
    #[default]
    Up,
    /// A finger came down in this frame: its owner is decided at the frame's end.
    Began,
    Down,
    /// It lifted in this frame.
    Lifted,
}

#[derive(Debug, Clone, Default)]
struct Touch {
    phase: Phase,
    owner: Option<Owner>,
    x: i32,
    y: i32,
    /// Its events in the frame being read.
    frame: Vec<Raw>,
}

impl Touch {
    fn moved(&self) -> bool {
        self.frame.iter().any(|e| e.as_abs().is_some_and(|(axis, _)| axis == AbsoluteAxisCode::ABS_MT_POSITION_X))
    }
}

#[derive(Debug)]
pub struct Router {
    axes: Axes,
    slot: usize,
    touches: Vec<Touch>,
    /// The slots with events in the frame being read, in the order they came.
    order: Vec<usize>,
    fingers: usize,
    first: Option<(i32, i32)>,
}

impl Router {
    pub fn new(axes: Axes) -> Self {
        Router { axes, slot: 0, touches: vec![Touch::default(); SLOTS], order: Vec::with_capacity(SLOTS), fingers: 0, first: None }
    }

    /// Whether a finger is on the touchpad part.
    pub fn pad_fingers(&self) -> bool {
        self.fingers > 0
    }

    fn to_screen(&self, x: i32, y: i32, layout: &TouchLayout) -> (f32, f32) {
        (self.axes.x.fraction(x) * layout.screen.0, self.axes.y.fraction(y) * layout.screen.1)
    }

    /// A raw position as touchpad coordinates: screen pixels from the pad area's corner, inside it.
    fn to_pad(&self, x: i32, y: i32, layout: &TouchLayout) -> (i32, i32) {
        let (sx, sy) = self.to_screen(x, y, layout);
        let (px, py) = Rect::from_corners(layout.pad).inside(sx, sy);
        (px.round() as i32, py.round() as i32)
    }

    /// Feeds one raw event; a SYN_REPORT completes the frame and adds what it becomes to `out`.
    pub fn feed(&mut self, raw: Raw, layout: &TouchLayout, out: &mut Routed) {
        if raw.is_report() {
            self.flush(layout, out);
            return;
        }
        // Touch keys and the single-touch axes are made anew for the fingers we forward.
        if raw.kind == EventType::KEY || raw.kind == EventType::SYNCHRONIZATION {
            return;
        }
        let axis = raw.as_abs().map(|(axis, _)| axis);
        if matches!(axis, Some(AbsoluteAxisCode::ABS_X | AbsoluteAxisCode::ABS_Y)) {
            return;
        }
        if axis == Some(AbsoluteAxisCode::ABS_MT_SLOT) {
            self.slot = usize::try_from(raw.value).unwrap_or(0);
            return;
        }
        let slot = self.slot;
        let Some(t) = self.touches.get_mut(slot) else {
            return;
        };
        match axis {
            Some(AbsoluteAxisCode::ABS_MT_TRACKING_ID) if raw.value >= 0 => {
                t.phase = Phase::Began;
                t.owner = None;
            }
            Some(AbsoluteAxisCode::ABS_MT_TRACKING_ID) => t.phase = Phase::Lifted,
            Some(AbsoluteAxisCode::ABS_MT_POSITION_X) => t.x = raw.value,
            Some(AbsoluteAxisCode::ABS_MT_POSITION_Y) => t.y = raw.value,
            _ => {}
        }
        t.frame.push(raw);
        if !self.order.contains(&slot) {
            self.order.push(slot);
        }
    }

    fn flush(&mut self, layout: &TouchLayout, out: &mut Routed) {
        let forwarded = out.pad.len();
        // A new finger belongs to the UI target under it, else to the touchpad (its position comes in
        // the same frame as its tracking id).
        let mut began = Vec::new();
        for i in 0..self.touches.len() {
            let Some(t) = self.touches.get(i).filter(|t| t.phase == Phase::Began) else {
                continue;
            };
            let (sx, sy) = self.to_screen(t.x, t.y, layout);
            let owner = match layout.region_at(sx, sy) {
                Some(id) => {
                    out.ui.push(UiTouch::Down { id: id.clone(), x: sx });
                    Owner::Ui(id.clone())
                }
                None => Owner::Pad,
            };
            if let Some(t) = self.touches.get_mut(i) {
                t.owner = Some(owner);
                t.phase = Phase::Down;
                began.push(i);
            }
        }
        // UI fingers that moved (not new ones: their position is where they went down).
        for (i, t) in self.touches.iter().enumerate() {
            if matches!(t.owner, Some(Owner::Ui(_))) && t.phase == Phase::Down && !began.contains(&i) && t.moved() {
                out.ui.push(UiTouch::Move { x: self.axes.x.fraction(t.x) * layout.screen.0 });
            }
        }
        // The touchpad fingers' events, slot by slot, in touchpad coordinates.
        for &slot in &self.order {
            let Some(t) = self.touches.get(slot).filter(|t| t.owner == Some(Owner::Pad)) else {
                continue;
            };
            out.pad.push(Raw::abs(AbsoluteAxisCode::ABS_MT_SLOT, slot as i32));
            for &e in &t.frame {
                out.pad.push(match e.as_abs() {
                    Some((AbsoluteAxisCode::ABS_MT_POSITION_X, v)) => Raw::abs(AbsoluteAxisCode::ABS_MT_POSITION_X, self.to_pad(v, t.y, layout).0),
                    Some((AbsoluteAxisCode::ABS_MT_POSITION_Y, v)) => Raw::abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, self.to_pad(t.x, v, layout).1),
                    _ => e,
                });
            }
        }
        self.order.clear();
        // Lifted fingers.
        for t in &mut self.touches {
            t.frame.clear();
            if t.phase == Phase::Lifted {
                if let Some(Owner::Ui(id)) = t.owner.take() {
                    out.ui.push(UiTouch::Up { id });
                }
                *t = Touch::default();
            }
        }
        // BTN_TOUCH and the tool key for the number of touchpad fingers; ABS_X/Y follow the first one.
        let fingers = self.touches.iter().filter(|t| t.owner == Some(Owner::Pad)).count();
        if fingers != self.fingers {
            let press = |down: bool| if down { Press::Down } else { Press::Up };
            if (fingers > 0) != (self.fingers > 0) {
                out.pad.push(Raw::key(KeyCode::BTN_TOUCH, press(fingers > 0)));
            }
            let tool = |n: usize| n.checked_sub(1).and_then(|i| TOOLS.get(i.min(TOOLS.len() - 1)).copied());
            if let Some(code) = tool(self.fingers) {
                out.pad.push(Raw::key(code, Press::Up));
            }
            if let Some(code) = tool(fingers) {
                out.pad.push(Raw::key(code, Press::Down));
            }
            self.fingers = fingers;
        }
        let first = self.touches.iter().find(|t| t.owner == Some(Owner::Pad)).map(|t| self.to_pad(t.x, t.y, layout));
        if let Some((x, y)) = first
            && self.first != first
        {
            out.pad.push(Raw::abs(AbsoluteAxisCode::ABS_X, x));
            out.pad.push(Raw::abs(AbsoluteAxisCode::ABS_Y, y));
        }
        self.first = first;
        if out.pad.len() > forwarded {
            out.pad.push(Raw::REPORT);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_core::Tab;
    use clamshell_core::touch::Region;

    const SLOT: AbsoluteAxisCode = AbsoluteAxisCode::ABS_MT_SLOT;
    const ID: AbsoluteAxisCode = AbsoluteAxisCode::ABS_MT_TRACKING_ID;
    const X: AbsoluteAxisCode = AbsoluteAxisCode::ABS_MT_POSITION_X;
    const Y: AbsoluteAxisCode = AbsoluteAxisCode::ABS_MT_POSITION_Y;

    /// A 1000x1000 touchscreen on a 1000x800 screen: a bar region along the top, the pad below it.
    fn setup() -> (Router, TouchLayout) {
        let layout = TouchLayout {
            screen: (1000.0, 800.0),
            pad: (0.0, 100.0, 1000.0, 800.0),
            regions: vec![Region { x0: 0.0, y0: 0.0, x1: 100.0, y1: 100.0, id: Id::Tab(Tab::Touchpad) }],
            ..TouchLayout::default()
        };
        (Router::new(Axes { x: Range { min: 0, max: 1000 }, y: Range { min: 0, max: 1000 } }), layout)
    }

    fn frame(r: &mut Router, l: &TouchLayout, events: &[(AbsoluteAxisCode, i32)]) -> Routed {
        let mut out = Routed::default();
        for &(axis, v) in events {
            r.feed(Raw::abs(axis, v), l, &mut out);
        }
        r.feed(Raw::key(KeyCode::BTN_TOUCH, Press::Down), l, &mut out);
        r.feed(Raw::REPORT, l, &mut out);
        out
    }

    fn key(code: KeyCode, down: bool) -> Raw {
        Raw::key(code, if down { Press::Down } else { Press::Up })
    }

    #[test]
    fn a_touch_on_the_bar_is_the_uis_not_forwarded() {
        let (mut r, l) = setup();
        // Raw (50, 50) is screen (50, 40): inside the bar region.
        let down = frame(&mut r, &l, &[(SLOT, 0), (ID, 7), (X, 50), (Y, 50)]);
        assert_eq!(down, Routed { ui: vec![UiTouch::Down { id: Id::Tab(Tab::Touchpad), x: 50.0 }], pad: vec![] });
        let moved = frame(&mut r, &l, &[(X, 400)]);
        assert_eq!(moved.ui, vec![UiTouch::Move { x: 400.0 }], "a finger that started on the bar stays the bar's (a slider's)");
        let up = frame(&mut r, &l, &[(ID, -1)]);
        assert_eq!(up, Routed { ui: vec![UiTouch::Up { id: Id::Tab(Tab::Touchpad) }], pad: vec![] });
    }

    #[test]
    fn a_touch_on_the_pad_is_forwarded_in_pad_coordinates() {
        let (mut r, l) = setup();
        // Raw (500, 500) is screen (500, 400), pad (500, 300).
        let down = frame(&mut r, &l, &[(SLOT, 0), (ID, 7), (X, 500), (Y, 500)]);
        assert_eq!(
            down.pad,
            vec![
                Raw::abs(SLOT, 0),
                Raw::abs(ID, 7),
                Raw::abs(X, 500),
                Raw::abs(Y, 300),
                key(KeyCode::BTN_TOUCH, true),
                key(TOOLS[0], true),
                Raw::abs(AbsoluteAxisCode::ABS_X, 500),
                Raw::abs(AbsoluteAxisCode::ABS_Y, 300),
                Raw::REPORT,
            ]
        );
        let up = frame(&mut r, &l, &[(ID, -1)]);
        assert_eq!(up.pad, vec![Raw::abs(SLOT, 0), Raw::abs(ID, -1), key(KeyCode::BTN_TOUCH, false), key(TOOLS[0], false), Raw::REPORT]);
    }

    #[test]
    fn two_pad_fingers_are_a_doubletap_and_a_bar_finger_does_not_count() {
        let (mut r, l) = setup();
        frame(&mut r, &l, &[(SLOT, 0), (ID, 1), (X, 500), (Y, 500)]);
        let second = frame(&mut r, &l, &[(SLOT, 1), (ID, 2), (X, 600), (Y, 600)]);
        assert!(second.pad.contains(&key(TOOLS[0], false)) && second.pad.contains(&key(TOOLS[1], true)));
        let bar = frame(&mut r, &l, &[(SLOT, 2), (ID, 3), (X, 10), (Y, 10)]);
        assert!(bar.pad.is_empty() && matches!(bar.ui.as_slice(), [UiTouch::Down { id: Id::Tab(Tab::Touchpad), .. }]), "{bar:?}");
    }

    #[test]
    fn fingers_in_one_frame_are_forwarded_slot_by_slot() {
        let (mut r, l) = setup();
        frame(&mut r, &l, &[(SLOT, 0), (ID, 1), (X, 500), (Y, 500), (SLOT, 1), (ID, 2), (X, 600), (Y, 600)]);
        let both = frame(&mut r, &l, &[(SLOT, 0), (X, 510), (SLOT, 1), (Y, 610)]);
        assert_eq!(both.pad, vec![Raw::abs(SLOT, 0), Raw::abs(X, 510), Raw::abs(SLOT, 1), Raw::abs(Y, 388), Raw::abs(AbsoluteAxisCode::ABS_X, 510), Raw::abs(AbsoluteAxisCode::ABS_Y, 300), Raw::REPORT]);
    }
}
