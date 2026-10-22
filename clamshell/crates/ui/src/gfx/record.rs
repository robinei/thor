//! What a frame draws, as a list of commands. The canvas records them ([`super::Canvas`]) and a player
//! turns them into pixels ([`super::paint::play`]). Two recordings of a screen are compared to find what
//! changed: only a changed screen is drawn, and only the part that changed.
//!
//! Commands hold what the canvas's methods were given, in logical pixels, so the player computes the same
//! pixels from them as drawing directly would. Floats are kept as their bits: layout is deterministic, so
//! the same bits mean the same drawing.

use super::Rect;
use super::icons::Icon;
use super::images::Image;

/// A float compared and hashed by its bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Px(u32);

impl Px {
    pub fn new(v: f32) -> Px {
        Px(v.to_bits())
    }

    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }

    pub(super) fn rect(r: Rect) -> [Px; 4] {
        [Px::new(r.x), Px::new(r.y), Px::new(r.w), Px::new(r.h)]
    }

    pub(super) fn to_rect([x, y, w, h]: [Px; 4]) -> Rect {
        Rect::new(x.get(), y.get(), w.get(), h.get())
    }
}

/// A colour as 8-bit RGBA, not premultiplied. Every colour the UI uses is made from 8-bit channels, so
/// it turns back into the same `tiny_skia::Color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba8([u8; 4]);

impl From<tiny_skia::Color> for Rgba8 {
    fn from(c: tiny_skia::Color) -> Self {
        let c = c.to_color_u8();
        Rgba8([c.red(), c.green(), c.blue(), c.alpha()])
    }
}

impl From<Rgba8> for tiny_skia::Color {
    fn from(Rgba8([r, g, b, a]): Rgba8) -> Self {
        tiny_skia::Color::from_rgba8(r, g, b, a)
    }
}

/// A rectangle of whole buffer pixels, from (x0, y0) up to but not including (x1, y1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Bounds {
    pub const EMPTY: Bounds = Bounds { x0: 0, y0: 0, x1: 0, y1: 0 };

    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds {
        Bounds { x0, y0, x1, y1 }
    }

    /// The whole of a `w` x `h` buffer.
    pub fn of_size((w, h): (u32, u32)) -> Bounds {
        Bounds::new(0, 0, i32::try_from(w).unwrap_or(i32::MAX), i32::try_from(h).unwrap_or(i32::MAX))
    }

    /// The pixels `(x0, y0, x1, y1)` touches, and one more on each side for antialiasing.
    pub(super) fn around((x0, y0, x1, y1): (f32, f32, f32, f32)) -> Bounds {
        Bounds::new(x0.floor() as i32 - 1, y0.floor() as i32 - 1, x1.ceil() as i32 + 1, y1.ceil() as i32 + 1)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn intersect(&self, o: Bounds) -> Bounds {
        Bounds::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1))
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, o: Bounds) -> Bounds {
        match (self.is_empty(), o.is_empty()) {
            (true, _) => o,
            (_, true) => *self,
            _ => Bounds::new(self.x0.min(o.x0), self.y0.min(o.y0), self.x1.max(o.x1), self.y1.max(o.y1)),
        }
    }

    pub fn width(&self) -> i32 {
        (self.x1 - self.x0).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.y1 - self.y0).max(0)
    }

    pub(super) fn tuple(&self) -> (i32, i32, i32, i32) {
        (self.x0, self.y0, self.x1, self.y1)
    }
}

/// One drawing command, with what the canvas method it records was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// The whole buffer in one colour. Every recording starts with it.
    Clear(Rgba8),
    Fill {
        r: [Px; 4],
        color: Rgba8,
    },
    HLine {
        x0: Px,
        x1: Px,
        y: Px,
        px: u32,
        color: Rgba8,
    },
    VLine {
        x: Px,
        y0: Px,
        y1: Px,
        px: u32,
        color: Rgba8,
    },
    Rounded {
        r: [Px; 4],
        radius: Px,
        color: Rgba8,
    },
    Outline {
        r: [Px; 4],
        radius: Px,
        width: Px,
        color: Rgba8,
    },
    Circle {
        cx: Px,
        cy: Px,
        r: Px,
        color: Rgba8,
    },
    Line {
        a: [Px; 2],
        b: [Px; 2],
        width: Px,
        color: Rgba8,
    },
    /// An image, in grey at `grey`/256 of its brightness if that is set.
    Image {
        image: Image,
        x: Px,
        y: Px,
        grey: Option<u32>,
    },
    Icon {
        icon: Icon,
        r: [Px; 4],
        color: Rgba8,
    },
    /// A line of text, bytes `start..start + len` of the recording's text. `fonts` is the glyph cache's
    /// generation, which changes when a font arrives for characters that were waiting for one.
    Text {
        x: Px,
        y: Px,
        size: Px,
        color: Rgba8,
        start: u32,
        len: u32,
        fonts: u32,
    },
}

/// A command and the buffer pixels it may touch: its shape's, a pixel larger for antialiasing, within
/// the clip in force when it was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cmd {
    pub op: Op,
    pub bounds: Bounds,
}

/// A frame's commands, the bytes of its text, and where it goes: the buffer's size, the scale and the
/// burn-in offset. The vectors are reused from frame to frame, so recording allocates nothing once warm.
#[derive(Debug, Default)]
pub struct Recording {
    pub(super) size: (u32, u32),
    pub(super) scale: Px,
    pub(super) offset: (i32, i32),
    pub(super) cmds: Vec<Cmd>,
    pub(super) text: Vec<u8>,
}

impl Recording {
    /// Empties it for a frame of `size` buffer pixels at `scale`, moved by `offset`.
    pub(super) fn start(&mut self, size: (u32, u32), scale: f32, offset: (i32, i32)) {
        self.size = size;
        self.scale = Px::new(scale);
        self.offset = offset;
        self.cmds.clear();
        self.text.clear();
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The bytes of a text command's text; none for any other command.
    pub(super) fn text_of(&self, op: &Op) -> &[u8] {
        match *op {
            Op::Text { start, len, .. } => self.text.get(start as usize..start as usize + len as usize).unwrap_or(&[]),
            _ => &[],
        }
    }

    /// Whether this recording's command `a` draws the same as `other`'s `b`. Text is compared by its
    /// bytes, not by where they are kept.
    fn same(&self, a: &Cmd, other: &Recording, b: &Cmd) -> bool {
        let unplaced = |c: &Cmd| match c.op {
            Op::Text { x, y, size, color, len, fonts, .. } => Cmd { op: Op::Text { x, y, size, color, start: 0, len, fonts }, bounds: c.bounds },
            _ => *c,
        };
        unplaced(a) == unplaced(b) && self.text_of(&a.op) == other.text_of(&b.op)
    }

    /// What changed since `before`: the buffer pixels the commands that differ may touch, in either
    /// recording. The commands the two start and end with alike are left out, and the rest is one
    /// rectangle around them all. `None` if nothing changed.
    pub fn damage(&self, before: &Recording) -> Option<Bounds> {
        if (self.size, self.scale, self.offset) != (before.size, before.scale, before.offset) {
            return Some(Bounds::of_size(self.size));
        }
        let (new, old) = (&self.cmds, &before.cmds);
        let prefix = new.iter().zip(old).take_while(|(a, b)| self.same(a, before, b)).count();
        let rest = new.len().min(old.len()) - prefix;
        let suffix = new.iter().rev().zip(old.iter().rev()).take(rest).take_while(|(a, b)| self.same(a, before, b)).count();
        let changed = |cmds: &[Cmd]| cmds.get(prefix..cmds.len() - suffix).unwrap_or(&[]).iter().fold(Bounds::EMPTY, |d, c| d.union(c.bounds));
        let damage = changed(new).union(changed(old)).intersect(Bounds::of_size(self.size));
        (!damage.is_empty()).then_some(damage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(cmds: &[(u8, i32)], text: &str) -> Recording {
        let mut r = Recording::default();
        r.start((100, 100), 1.0, (0, 0));
        r.text.extend_from_slice(text.as_bytes());
        for &(c, x) in cmds {
            let color = Rgba8([c, 0, 0, 255]);
            r.cmds.push(Cmd { op: Op::Clear(color), bounds: Bounds::new(x, 0, x + 10, 10) });
        }
        r
    }

    #[test]
    fn damage_is_around_the_commands_between_the_first_and_last_difference() {
        let old = rec(&[(0, 0), (1, 10), (2, 20), (3, 30)], "");
        assert_eq!(old.damage(&old), None);
        let new = rec(&[(0, 0), (9, 10), (2, 20), (3, 30)], "");
        assert_eq!(new.damage(&old), Some(Bounds::new(10, 0, 20, 10)));
        // One command fewer: the one dropped is damaged.
        let fewer = rec(&[(0, 0), (1, 10), (3, 30)], "");
        assert_eq!(fewer.damage(&old), Some(Bounds::new(20, 0, 30, 10)));
        // Two apart: everything between them too.
        let two = rec(&[(0, 0), (9, 10), (2, 20), (9, 50)], "");
        assert_eq!(two.damage(&old), Some(Bounds::new(10, 0, 60, 10)));
        // Moved by the burn-in drift: all of it.
        let mut moved = rec(&[(0, 0)], "");
        moved.offset = (1, 0);
        assert_eq!(moved.damage(&rec(&[(0, 0)], "")), Some(Bounds::new(0, 0, 100, 100)));
    }

    #[test]
    fn text_is_compared_by_its_bytes() {
        let text = |r: &mut Recording, start: u32, len: u32| {
            r.cmds.push(Cmd {
                op: Op::Text { x: Px::new(0.0), y: Px::new(0.0), size: Px::new(10.0), color: Rgba8([0; 4]), start, len, fonts: 0 },
                bounds: Bounds::new(0, 0, 5, 5),
            })
        };
        let (mut a, mut b) = (rec(&[], "abcab"), rec(&[], "xxab"));
        text(&mut a, 3, 2);
        text(&mut b, 2, 2);
        assert_eq!(a.damage(&b), None);
        text(&mut a, 0, 1);
        text(&mut b, 0, 1);
        assert_eq!(a.damage(&b), Some(Bounds::new(0, 0, 5, 5)));
    }
}
