//! Home-held window-switch arrows. Drawing and hit testing share the same geometry.

use clamshell_state::{State, device::buttons::DeviceKey};
use tiny_skia::{Color, Paint, PixmapMut, Rect, Transform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowArrows {
    pub visible: bool,
    pub size: (u32, u32),
}
impl WindowArrows {
    pub fn new(state: &State, size: (u32, u32)) -> Self {
        Self { visible: state.session().home_held() && state.session().has_other_top_window(), size }
    }
    pub fn hit(&self, (x, y): (f64, f64)) -> Option<DeviceKey> {
        if !self.visible || (y - self.size.1 as f64 / 2.0).abs() > 50.0 {
            return None;
        }
        if (0.0..80.0).contains(&x) {
            Some(DeviceKey::WindowPrevious)
        } else if (self.size.0 as f64 - 80.0..self.size.0 as f64).contains(&x) {
            Some(DeviceKey::WindowNext)
        } else {
            None
        }
    }
    pub fn draw(&self, mut pixmap: PixmapMut) {
        pixmap.fill(Color::TRANSPARENT);
        if !self.visible {
            return;
        }
        let mut paint = Paint::default();
        paint.set_color_rgba8(160, 160, 168, 255);
        paint.anti_alias = true;
        let y = self.size.1 as f32 / 2.0;
        for (x, angles) in [(32.0, [-45.0, 45.0]), (self.size.0 as f32 - 32.0, [135.0, 225.0])] {
            if let Some(line) = Rect::from_xywh(x, y - 1.5, 24.0, 3.0) {
                for angle in angles {
                    pixmap.fill_rect(line, &paint, Transform::from_rotate_at(angle, x, y), None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invisible_arrows_and_the_space_between_them_do_not_take_input() {
        let mut arrows = WindowArrows { visible: true, size: (960, 540) };
        assert_eq!(arrows.hit((32.0, 270.0)), Some(DeviceKey::WindowPrevious));
        assert_eq!(arrows.hit((928.0, 270.0)), Some(DeviceKey::WindowNext));
        assert_eq!(arrows.hit((480.0, 270.0)), None);
        assert_eq!(arrows.hit((32.0, 30.0)), None);
        arrows.visible = false;
        assert_eq!(arrows.hit((32.0, 270.0)), None);
    }
}
