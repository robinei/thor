//! What the compositor shows over the window on the top screen: the HUD, when the program's HUD setting
//! asks, and the window arrows while Home is held. The frame starts clear, so the window shows through.

use clamshell_state::Id;
use clamshell_state::settings::Hud;

use crate::gfx::icons::Icon;
use crate::gfx::{Rect, Rgb};
use crate::toolkit::Frame;
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Look, Tone};

/// The HUD's text size, as a fraction of the screen's height.
const HUD_TEXT: f32 = 0.028;
/// The HUD box's background.
const HUD_BACKGROUND: Rgb = Rgb(0x000000);
/// How far an arrow's target reaches in from the screen's edge, and how tall it is.
const ARROW_REACH: f32 = 80.0;
const ARROW_HEIGHT: f32 = 100.0;
/// The arrow's icon's side.
const ARROW_ICON: f32 = 48.0;

pub fn draw(f: &mut Frame, screen: Rect) {
    hud(f, screen);
    arrows(f, screen);
}

/// The frame rate of the window on the top screen, and with the full HUD the frame time too, from the
/// compositor's own measurement (`clamshell_state::Reading::Frames`): a small box in the top left corner,
/// nothing while the window draws nothing.
fn hud(f: &mut Frame, screen: Rect) {
    let Some(window) = f.s.session().top_window() else { return };
    let metrics = f.s.sys().metrics();
    let Some(fps) = metrics.fps else { return };
    let text: Fmt<32> = match (f.s.window_hud(window.id), metrics.frame_ms) {
        (Hud::Off, _) => return,
        (Hud::Full, Some(ms)) => Fmt::of(format_args!("{fps:.0} fps  {ms:.1} ms")),
        (Hud::Fps | Hud::Full, _) => Fmt::of(format_args!("{fps:.0} fps")),
    };
    let size = HUD_TEXT * screen.h;
    let pad = size * 0.5;
    // About half the size per character, as Noto Sans's digits are.
    let width = text.chars().count() as f32 * size * 0.56 + pad * 2.0;
    f.c.rounded(Rect::new(screen.x + pad, screen.y + pad, width, size * 1.7), size * 0.4, HUD_BACKGROUND.alpha(0.6));
    f.c.text(screen.x + pad * 2.0, screen.y + pad + size * 0.35, size, Tone::High, &text);
}

/// The arrows at the screen's edges while Home is held and the program shown has another window: the
/// previous window's on the left, the next's on the right. They act as they are pressed
/// (`Core::press_overlay`).
fn arrows(f: &mut Frame, screen: Rect) {
    let session = f.s.session();
    if !session.home_held() || !session.has_other_top_window() {
        return;
    }
    let y = screen.y + (screen.h - ARROW_HEIGHT) / 2.0;
    for (next, x, icon) in [(false, screen.x, Icon::ChevronBack), (true, screen.x + screen.w - ARROW_REACH, Icon::Chevron)] {
        let rect = Rect::new(x, y, ARROW_REACH, ARROW_HEIGHT);
        f.button(Id::WindowArrow { next }, rect, Look::PLAIN, |f| {
            let (cx, cy) = rect.center();
            f.c.icon(icon, Rect::new(cx - ARROW_ICON / 2.0, cy - ARROW_ICON / 2.0, ARROW_ICON, ARROW_ICON), Tone::Normal);
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use clamshell_state::session::windows::WindowFacts;
    use clamshell_state::{Event, Id, Init, State, WindowEvent};

    use crate::Layer;
    use crate::gfx::DrawCache;
    use crate::gfx::images::Images;
    use crate::render::{Placement, Retained, record};

    /// Two windows of one launch, the first in front, with Home held or not.
    fn state(home: bool) -> State {
        let mut s = State::new(Init::default());
        let mut fx = Vec::new();
        s.apply(Event::Start, &mut fx);
        for id in [1, 2] {
            let unit = Some("clamshell-run-test-1-0.service".into());
            s.apply(WindowEvent::Opened(WindowFacts { id, app_id: "test".into(), unit, ..WindowFacts::default() }).into(), &mut fx);
        }
        s.apply(WindowEvent::Activate(1).into(), &mut fx);
        s.apply(Event::HomeHeld(home), &mut fx);
        s
    }

    fn overlay(s: &State) -> Retained {
        let mut retained = Retained::default();
        let at = Placement { size_px: (960, 540), scale: 1, drift: (0, 0) };
        record(&mut retained, at, Layer::Overlay(crate::Screen::Top), s, &mut DrawCache::load(), &mut Images::synchronous());
        retained
    }

    #[test]
    fn the_window_arrows_are_targets_at_the_edges_while_home_is_held() {
        let shown = overlay(&state(true));
        let ids: Vec<_> = shown.targets.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, [Id::WindowArrow { next: false }, Id::WindowArrow { next: true }]);
        let (left, right) = (shown.targets[0].rect, shown.targets[1].rect);
        assert!(left.contains(32.0, 270.0) && !left.contains(480.0, 270.0) && !left.contains(32.0, 30.0));
        assert!(right.contains(928.0, 270.0) && !right.contains(480.0, 270.0));
        assert!(!shown.next.ink().is_empty(), "drawn");
        let hidden = overlay(&state(false));
        assert!(hidden.targets.is_empty());
        assert!(hidden.next.ink().is_empty(), "nothing drawn, so nothing is shown over the window");
    }
}
