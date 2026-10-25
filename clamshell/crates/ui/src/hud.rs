//! The HUD the compositor draws over the window on the top screen when its program's HUD setting asks:
//! the frame rate, and with the full HUD the frame time too, from the compositor's own measurement
//! ([`clamshell_state::Reading::Frames`]). A small box in the top left corner; nothing while the window
//! draws nothing.

use clamshell_state::State;
use clamshell_state::settings::Hud;
use tiny_skia::PixmapMut;

use crate::gfx::canvas::DrawCache;
use crate::gfx::color::Rgb;
use crate::gfx::geom::Rect;
use crate::gfx::paint::Painter;
use crate::toolkit::style::Tone;

/// The text's size, as a fraction of the screen's height.
const TEXT: f32 = 0.028;
/// The box's background.
const BACKGROUND: Rgb = Rgb(0x000000);

/// What the HUD shows: its line of text, at a size for a screen `height` pixels high.
#[derive(Debug, Clone, PartialEq)]
pub struct HudLine {
    pub text: String,
    pub height: u32,
}

impl HudLine {
    /// The line for the top screen's window, if its program's HUD setting is on and it draws.
    pub fn of(state: &State, height: u32) -> Option<HudLine> {
        let window = state.session().top_window()?;
        let hud = state.window_hud(window.id);
        let metrics = state.sys().metrics();
        let fps = metrics.fps?;
        let text = match (hud, metrics.frame_ms) {
            (Hud::Off, _) => return None,
            (Hud::Full, Some(ms)) => format!("{fps:.0} fps  {ms:.1} ms"),
            (Hud::Fps | Hud::Full, _) => format!("{fps:.0} fps"),
        };
        Some(HudLine { text, height })
    }

    /// The pixmap it needs: room for the box and its margin from the screen's corner.
    pub fn size(height: u32) -> (u32, u32) {
        let size = TEXT * height as f32;
        ((size * 12.0).ceil() as u32, (size * 3.0).ceil() as u32)
    }

    /// Draws the box with its text into `pixmap`, of [`HudLine::size`], clearing the rest.
    pub fn draw(&self, mut pixmap: PixmapMut, cache: &mut DrawCache) {
        let size = TEXT * self.height as f32;
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        let mut p = Painter::new(pixmap, 1.0, (0, 0), cache);
        let pad = size * 0.5;
        // About half the size per character, as Noto Sans's digits are.
        let width = self.text.chars().count() as f32 * size * 0.56 + pad * 2.0;
        p.rounded(Rect::new(pad, pad, width, size * 1.7), size * 0.4, BACKGROUND.alpha(0.6));
        p.text(pad * 2.0, pad + size * 0.35, size, Tone::High.rgb(), &self.text);
    }
}
