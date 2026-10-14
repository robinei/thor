//! Line icons, drawn as paths on a 24x24 grid scaled into a square. No icon font or image files.

use tiny_skia::Color;

use crate::draw::{Canvas, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Apps,
    /// The pad mouse toggle: a mouse.
    Mouse,
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
    /// Charge in tenths (0-10) and whether it is charging.
    Battery { tenths: u8, charging: bool },
    /// Lit arcs (0-3); `None`: not connected.
    Wifi { bars: Option<u8> },
    /// Sound waves (0-3) and mute.
    Volume { waves: u8, muted: bool },
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
    // Levels show as thick lit strokes; the rest of the scale stays visible, thin.
    let thin = 0.8 * u;
    let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> [(f32, f32); 7] {
        let mut points = [(0.0, 0.0); 7];
        for (i, point) in points.iter_mut().enumerate() {
            let a = (from + (to - from) * i as f32 / 6.0).to_radians();
            *point = p(cx + r * a.cos(), cy - r * a.sin());
        }
        points
    };
    match icon {
        Icon::Mouse => {
            c.outline(rr(6.0, 3.0, 12.0, 18.0), 6.0 * u, line, color);
            c.polyline(&[p(12.0, 3.5), p(12.0, 9.5)], line, color, false);
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
        Icon::Battery { tenths, charging } => {
            c.outline(rr(1.0, 7.5, 15.5, 9.0), 2.0 * u, 1.5 * u, color);
            c.rounded(rr(17.0, 10.0, 1.6, 4.0), 0.6 * u, color);
            let level = f32::from(tenths.min(10)) / 10.0;
            if level > 0.0 {
                c.rounded(rr(3.0, 9.5, 11.5 * level, 5.0), 0.8 * u, color);
            }
            if charging {
                c.polyline(&[p(22.5, 5.0), p(19.8, 12.6), p(22.8, 12.6), p(20.2, 19.5)], 1.6 * u, color, false);
            }
        }
        Icon::Wifi { bars } => {
            c.circle(x0 + 12.0 * u, y0 + 19.0 * u, 1.6 * u, color);
            let lit = bars.unwrap_or(0);
            for (i, r) in [5.5, 10.0, 14.5].into_iter().enumerate() {
                let width = if (i as u8) < lit { 2.2 * u } else { thin };
                c.polyline(&arc(12.0, 19.0, r, 45.0, 135.0), width, color, false);
            }
            if bars.is_none() {
                c.polyline(&[p(4.0, 4.0), p(20.0, 20.0)], line, color, false);
            }
        }
        Icon::Volume { waves, muted } => {
            let speaker = [p(2.5, 9.0), p(6.5, 9.0), p(11.5, 4.5), p(11.5, 19.5), p(6.5, 15.0), p(2.5, 15.0)];
            c.polyline(&speaker, 1.6 * u, color, true);
            if muted {
                c.polyline(&[p(15.5, 9.0), p(21.5, 15.0)], line, color, false);
                c.polyline(&[p(21.5, 9.0), p(15.5, 15.0)], line, color, false);
            } else {
                for (i, r) in [4.0, 7.0, 10.0].into_iter().enumerate() {
                    let width = if (i as u8) < waves { 2.0 * u } else { thin };
                    c.polyline(&arc(11.5, 12.0, r, -50.0, 50.0), width, color, false);
                }
            }
        }
        Icon::Enter => {
            c.polyline(&[p(19.0, 5.0), p(19.0, 14.0), p(5.0, 14.0)], line, color, false);
            c.polyline(&[p(9.0, 10.0), p(5.0, 14.0), p(9.0, 18.0)], line, color, false);
        }
    }
}
