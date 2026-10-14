//! Line icons, drawn as paths on a 24x24 grid scaled into a square. No icon font or image files.

use tiny_skia::Color;

use crate::draw::{Canvas, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Gamepad,
    Apps,
    Keyboard,
    Touchpad,
    Sysinfo,
    Shift,
    /// Shift with a bar under it.
    CapsLock,
    Backspace,
    Enter,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
}

/// Draws `icon` centred in `r`, as large as fits.
pub fn draw(c: &mut Canvas, icon: Icon, r: Rect, color: Color) {
    let size = r.w.min(r.h);
    let (cx, cy) = r.center();
    let u = size / 24.0;
    let (x0, y0) = (cx - size / 2.0, cy - size / 2.0);
    let p = |x: f32, y: f32| (x0 + x * u, y0 + y * u);
    let rr = |x: f32, y: f32, w: f32, h: f32| Rect::new(x0 + x * u, y0 + y * u, w * u, h * u);
    let line = 1.8 * u;
    match icon {
        Icon::Gamepad => {
            c.outline(rr(2.0, 7.0, 20.0, 11.0), 5.0 * u, line, color);
            c.polyline(&[p(5.5, 12.5), p(10.5, 12.5)], line, color, false);
            c.polyline(&[p(8.0, 10.0), p(8.0, 15.0)], line, color, false);
            c.circle(x0 + 15.5 * u, y0 + 11.5 * u, 1.2 * u, color);
            c.circle(x0 + 18.0 * u, y0 + 14.0 * u, 1.2 * u, color);
        }
        Icon::Apps => {
            for (x, y) in [(4.0, 4.0), (13.0, 4.0), (4.0, 13.0), (13.0, 13.0)] {
                c.outline(rr(x, y, 7.0, 7.0), 1.5 * u, line, color);
            }
        }
        Icon::Keyboard => {
            c.outline(rr(2.0, 6.0, 20.0, 12.0), 2.0 * u, line, color);
            for y in [9.5, 12.5] {
                for x in [6.0, 9.0, 12.0, 15.0, 18.0] {
                    c.circle(x0 + x * u, y0 + y * u, 0.9 * u, color);
                }
            }
            c.polyline(&[p(8.5, 15.3), p(15.5, 15.3)], line, color, false);
        }
        Icon::Touchpad => {
            c.outline(rr(3.0, 4.0, 18.0, 16.0), 3.0 * u, line, color);
            c.polyline(&[p(3.0, 15.0), p(21.0, 15.0)], line, color, false);
            c.polyline(&[p(12.0, 15.0), p(12.0, 20.0)], line, color, false);
        }
        Icon::Sysinfo => {
            c.polyline(&[p(6.0, 20.0), p(6.0, 13.0)], 2.4 * u, color, false);
            c.polyline(&[p(12.0, 20.0), p(12.0, 5.0)], 2.4 * u, color, false);
            c.polyline(&[p(18.0, 20.0), p(18.0, 10.0)], 2.4 * u, color, false);
        }
        Icon::Shift => {
            let arrow = [p(12.0, 3.0), p(20.0, 11.5), p(15.5, 11.5), p(15.5, 18.5), p(8.5, 18.5), p(8.5, 11.5), p(4.0, 11.5)];
            c.polyline(&arrow, line, color, true);
        }
        Icon::CapsLock => {
            let arrow = [p(12.0, 2.0), p(20.0, 10.0), p(15.5, 10.0), p(15.5, 16.0), p(8.5, 16.0), p(8.5, 10.0), p(4.0, 10.0)];
            c.polyline(&arrow, line, color, true);
            c.polyline(&[p(8.5, 20.5), p(15.5, 20.5)], line, color, false);
        }
        Icon::Backspace => {
            c.polyline(&[p(8.0, 6.0), p(21.0, 6.0), p(21.0, 18.0), p(8.0, 18.0), p(3.0, 12.0)], line, color, true);
            c.polyline(&[p(12.0, 9.5), p(17.0, 14.5)], line, color, false);
            c.polyline(&[p(17.0, 9.5), p(12.0, 14.5)], line, color, false);
        }
        Icon::ArrowUp => {
            c.polyline(&[p(12.0, 19.0), p(12.0, 5.0)], line, color, false);
            c.polyline(&[p(6.0, 11.0), p(12.0, 5.0), p(18.0, 11.0)], line, color, false);
        }
        Icon::ArrowDown => {
            c.polyline(&[p(12.0, 5.0), p(12.0, 19.0)], line, color, false);
            c.polyline(&[p(6.0, 13.0), p(12.0, 19.0), p(18.0, 13.0)], line, color, false);
        }
        Icon::ArrowLeft => {
            c.polyline(&[p(19.0, 12.0), p(5.0, 12.0)], line, color, false);
            c.polyline(&[p(11.0, 6.0), p(5.0, 12.0), p(11.0, 18.0)], line, color, false);
        }
        Icon::ArrowRight => {
            c.polyline(&[p(5.0, 12.0), p(19.0, 12.0)], line, color, false);
            c.polyline(&[p(13.0, 6.0), p(19.0, 12.0), p(13.0, 18.0)], line, color, false);
        }
        Icon::Enter => {
            c.polyline(&[p(19.0, 5.0), p(19.0, 14.0), p(5.0, 14.0)], line, color, false);
            c.polyline(&[p(9.0, 10.0), p(5.0, 14.0), p(9.0, 18.0)], line, color, false);
        }
    }
}
