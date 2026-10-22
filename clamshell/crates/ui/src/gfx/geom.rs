//! Layout geometry in logical pixels: rectangles cut into rows, columns and cells, a column filled from
//! the top, the span of a line of text, and alignment.

use super::text::LINE;

/// A rectangle in logical pixels: its top left corner and size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Smaller by `d` on every side, or larger if `d` is negative.
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }

    /// The part of this inside `other`. If there is none, the result has no size.
    pub fn intersect(&self, other: Rect) -> Rect {
        let (x0, y0) = (self.x.max(other.x), self.y.max(other.y));
        let (x1, y1) = (self.right().min(other.right()), self.bottom().min(other.bottom()));
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    /// Wider by `dx` on each side. A list's rows use it to reach into the margins when drawn pressed.
    pub fn widened(&self, dx: f32) -> Rect {
        Rect::new(self.x - dx, self.y, self.w + 2.0 * dx, self.h)
    }

    /// Moved by `dx`, `dy`.
    pub fn shifted(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// A `w` by `h` rectangle centred in this one.
    pub fn centered(&self, w: f32, h: f32) -> Rect {
        Rect::new(self.x + (self.w - w) / 2.0, self.y + (self.h - h) / 2.0, w, h)
    }

    /// A square of `side` centred in this one, as for an icon.
    pub fn square_center(&self, side: f32) -> Rect {
        self.centered(side, side)
    }

    /// The first line for text of `size` in this rectangle: at its top, its full width and one [`LINE`]
    /// high.
    pub fn top_line(&self, size: f32) -> Rect {
        Rect::new(self.x, self.y, self.w, size * LINE)
    }

    /// Cuts the top `h` off this and returns it.
    pub fn take_top(&mut self, h: f32) -> Rect {
        let top = Rect::new(self.x, self.y, self.w, h);
        self.y += h;
        self.h -= h;
        top
    }

    /// Cuts the bottom `h` off this and returns it.
    pub fn take_bottom(&mut self, h: f32) -> Rect {
        self.h -= h;
        Rect::new(self.x, self.y + self.h, self.w, h)
    }

    /// Cuts the left `w` off this and returns it.
    pub fn take_left(&mut self, w: f32) -> Rect {
        let left = Rect::new(self.x, self.y, w, self.h);
        self.x += w;
        self.w -= w;
        left
    }

    /// Cuts the right `w` off this and returns it.
    pub fn take_right(&mut self, w: f32) -> Rect {
        self.w -= w;
        Rect::new(self.x + self.w, self.y, w, self.h)
    }

    /// Cell `i` of a grid of `cols` by `rows` equal cells that fills this rectangle, with `gap` between
    /// the cells. Cells are counted row by row. A row of buttons is a grid with one row.
    pub fn cell(&self, cols: usize, rows: usize, gap: f32, i: usize) -> Rect {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let w = (self.w - (cols - 1) as f32 * gap) / cols as f32;
        let h = (self.h - (rows - 1) as f32 * gap) / rows as f32;
        let (col, row) = ((i % cols) as f32, (i / cols) as f32);
        Rect::new(self.x + col * (w + gap), self.y + row * (h + gap), w, h)
    }
}

/// Lines stacked down an area. Each [`Column::take`] takes the next `h` below the last line.
#[derive(Debug, Clone, Copy)]
pub struct Column {
    area: Rect,
    y: f32,
}

impl Column {
    pub fn new(area: Rect) -> Self {
        Column { area, y: area.y }
    }

    /// Where the next line would start.
    pub fn y(&self) -> f32 {
        self.y
    }

    /// The bottom of the area.
    pub fn end(&self) -> f32 {
        self.area.y + self.area.h
    }

    /// Whether `h` more fits.
    pub fn fits(&self, h: f32) -> bool {
        self.y + h <= self.end()
    }

    /// The next `h`, whether it fits or not.
    pub fn take(&mut self, h: f32) -> Rect {
        let r = Rect::new(self.area.x, self.y, self.area.w, h);
        self.y += h;
        r
    }

    /// The next `h`, if it fits. Use it to take rows while there is room.
    pub fn next(&mut self, h: f32) -> Option<Rect> {
        if self.y + h > self.end() + 0.01 {
            return None;
        }
        Some(self.take(h))
    }

    /// Leaves `h` empty.
    pub fn skip(&mut self, h: f32) {
        self.y += h;
    }

    /// What is left below the lines taken.
    pub fn rest(&self) -> Rect {
        Rect::new(self.area.x, self.y, self.area.w, self.end() - self.y)
    }
}

/// Where a line of text starts and how much room it has to the right. It is used for text that runs on
/// over several lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub x: f32,
    pub y: f32,
    pub w: f32,
}

/// Where text or a picture sits in the room it has, along one axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn cells_fill_the_rect_with_gaps_between() {
        let r = Rect::new(10.0, 20.0, 310.0, 100.0);
        assert_eq!(r.cell(3, 2, 5.0, 0), Rect::new(10.0, 20.0, 100.0, 47.5));
        assert_eq!(r.cell(3, 2, 5.0, 5), Rect::new(220.0, 72.5, 100.0, 47.5));
        let mut rest = r;
        let top = rest.take_top(30.0);
        assert_eq!((top.h, rest.y, rest.h), (30.0, 50.0, 70.0));
        let right = rest.take_right(10.0);
        assert_eq!((right.x, rest.w), (310.0, 300.0));
    }
}
