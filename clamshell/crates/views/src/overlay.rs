//! What the compositor shows over the window on the top screen: the HUD, when the program's HUD setting
//! asks, the window arrows while Home is held, and the overview's cells. The frame starts clear, so the
//! window shows through.

use clamshell_state::session::overview::OverviewEvent;
use clamshell_state::settings::Hud;
use clamshell_state::{Event, Id, PadInput};

use clamshell_ui::gfx::icons::Icon;
use clamshell_ui::gfx::{Rect, Rgb};
use clamshell_ui::toolkit::Frame;
use clamshell_ui::toolkit::format::Fmt;
use clamshell_ui::toolkit::style::{Look, Tone};

/// The HUD's text size, as a fraction of the screen's height.
const HUD_TEXT: f32 = 0.028;
/// The HUD box's background.
const HUD_BACKGROUND: Rgb = Rgb(0x000000);
/// How far an arrow's target reaches in from the screen's edge, how tall it is, and its icon's side, as
/// fractions of the screen's height (80, 100 and 48 pixels at 1080).
const ARROW_REACH: f32 = 0.074;
const ARROW_HEIGHT: f32 = 0.093;
const ARROW_ICON: f32 = 0.044;

pub fn draw(f: &mut Frame, screen: Rect) {
    if f.s.session().overview().is_some() {
        f.mode(overview_input);
        return overview(f);
    }
    hud(f, screen);
    arrows(f, screen);
}

/// What the pad does while the overview of the program's windows is open: the D-pad moves its choice, A
/// brings the chosen window in front and B or Back goes back. Anything else does nothing, rather than
/// act on what is under the overview.
pub(super) fn overview_input(input: PadInput) -> Option<Event> {
    use clamshell_state::keymap::Dir as Step;
    Some(Event::Overview(match input {
        PadInput::Up => OverviewEvent::Step(Step::Up),
        PadInput::Down => OverviewEvent::Step(Step::Down),
        PadInput::Left => OverviewEvent::Step(Step::Left),
        PadInput::Right => OverviewEvent::Step(Step::Right),
        PadInput::A(true) => OverviewEvent::Choose,
        PadInput::B | PadInput::Back => OverviewEvent::Close,
        _ => return None,
    }))
}

/// The overview of the program's windows (`clamshell_state::session::overview`), which the compositor draws
/// in their cells: each cell a target, acted on as it is pressed (`Core::press_overlay`), and the ring
/// around the one the pad has chosen, as a tile's, all the picture's rectangle. The cells are the compositor's, on the whole screen in
/// its pixels, so they don't move with the burn-in drift.
fn overview(f: &mut Frame) {
    let session = f.s.session();
    let Some(chosen) = session.overview().map(|o| o.chosen()) else { return };
    let n = session.overview_windows().count();
    let (screen_w, screen_h) = f.c.buffer_size();
    for (i, window) in session.overview_windows().enumerate() {
        let (x, y, w, h) = clamshell_state::session::overview::cell(i, n, (screen_w as f32, screen_h as f32));
        let rect = f.c.pinned(Rect::new(x, y, w, h));
        f.button(Id::OverviewWindow(window.id), rect, Look::TILE, |_| {});
        if window.id == chosen {
            f.ring(rect, Look::TILE);
        }
    }
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
    let width = f.c.text_width(size, &text) + pad * 2.0;
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
    let (reach, height, side) = (ARROW_REACH * screen.h, ARROW_HEIGHT * screen.h, ARROW_ICON * screen.h);
    let y = screen.y + (screen.h - height) / 2.0;
    for (next, x, icon) in [(false, screen.x, Icon::ChevronBack), (true, screen.x + screen.w - reach, Icon::Chevron)] {
        let rect = Rect::new(x, y, reach, height);
        f.button(Id::WindowArrow { next }, rect, Look::PLAIN, |f| {
            f.c.icon(icon, rect.square_center(side), Tone::Normal);
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::{OverviewEvent, PadInput, overview_input};
    use clamshell_state::keymap::Dir as Step;

    #[test]
    fn the_overview_takes_the_d_pad_a_b_and_back_and_swallows_the_rest() {
        let overview = |e| Some(Event::Overview(e));
        assert_eq!(overview_input(PadInput::Left), overview(OverviewEvent::Step(Step::Left)));
        assert_eq!(overview_input(PadInput::A(true)), overview(OverviewEvent::Choose));
        assert_eq!(overview_input(PadInput::B), overview(OverviewEvent::Close));
        assert_eq!(overview_input(PadInput::Back), overview(OverviewEvent::Close));
        assert_eq!(overview_input(PadInput::A(false)), None);
        assert_eq!(overview_input(PadInput::Y), None);
    }

    use clamshell_state::session::windows::WindowFacts;
    use clamshell_state::{Event, Id, Init, State, WindowEvent};

    use clamshell_ui::Layer;
    use clamshell_ui::gfx::DrawCache;
    use clamshell_ui::gfx::images::Images;
    use clamshell_ui::render::{Placement, Retained, record};

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
        overlay_at(s, (0, 0))
    }

    fn overlay_at(s: &State, drift: (i32, i32)) -> Retained {
        let mut retained = Retained::default();
        let at = Placement { size_px: (960, 540), scale: 1, drift };
        record(&mut retained, at, Layer::Overlay(clamshell_ui::Screen::Top), s, &mut DrawCache::load(), &mut Images::synchronous(), &crate::VIEWS);
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

    /// The HUD's box is as wide as its text, measured, with the same margin on both sides.
    #[test]
    fn the_huds_box_fits_its_text() {
        let mut settings = clamshell_state::settings::Settings::default();
        settings.global.set(clamshell_state::settings::Setting::Hud, 2);
        let mut s = State::new(Init { library: Some(clamshell_library::demo::library()), settings, ..Init::default() });
        let mut fx = Vec::new();
        s.apply(Event::tick(1_760_000_000), &mut fx);
        let launch = clamshell_state::LaunchUnit { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 };
        s.apply(Event::Units(vec![launch.clone()]), &mut fx);
        s.apply(WindowEvent::Opened(WindowFacts { id: 1, app_id: "game".into(), unit: Some(launch.service()), ..WindowFacts::default() }).into(), &mut fx);
        s.apply(WindowEvent::Activate(1).into(), &mut fx);
        s.apply(Event::Reading(clamshell_state::Reading::Frames { fps: Some(120.0), frame_ms: Some(16.7) }), &mut fx);
        let (w, h) = (1920, 1080);
        let mut retained = Retained::default();
        let mut cache = DrawCache::load();
        record(
            &mut retained,
            Placement { size_px: (w, h), scale: 1, drift: (0, 0) },
            Layer::Overlay(clamshell_ui::Screen::Top),
            &s,
            &mut cache,
            &mut Images::synchronous(),
            &crate::VIEWS,
        );
        let ink = retained.next.ink();
        assert!(!ink.is_empty(), "the HUD drawn");
        let size = super::HUD_TEXT * h as f32;
        let mut scratch = clamshell_ui::gfx::record::Recording::default();
        let text_w = clamshell_ui::gfx::Canvas::new(&mut scratch, (w, h), 1.0, &mut cache).text_width(size, "120 fps  16.7 ms");
        // The box from half the text size in, with that margin on either side of the text.
        let right = size * 0.5 + text_w + size;
        assert!((ink.x1 as f32 - right).abs() <= 6.0, "the box ends at {} for text ending at {}", ink.x1, size + text_w);
    }

    /// The overview's cells are its targets, side by side for two windows, and take the arrows' place.
    #[test]
    fn the_overviews_cells_are_targets_in_place_of_the_arrows() {
        let mut s = state(true);
        s.apply(Event::Overview(clamshell_state::session::overview::OverviewEvent::Toggle), &mut Vec::new());
        let shown = overlay(&s);
        let ids: Vec<_> = shown.targets.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, [Id::OverviewWindow(1), Id::OverviewWindow(2)]);
        let (left, right) = (shown.targets[0].rect, shown.targets[1].rect);
        assert!(left.contains(240.0, 270.0) && right.contains(720.0, 270.0) && !left.contains(720.0, 270.0));
        assert!(!shown.next.ink().is_empty(), "the chosen one's ring");
        // The cells are the compositor's, in the screen's pixels, whatever the burn-in drift.
        let drifted = overlay_at(&s, (3, -2));
        for (i, target) in drifted.targets.iter().enumerate() {
            let (x, y, w, h) = clamshell_state::session::overview::cell(i, 2, (960.0, 540.0));
            let r = target.rect;
            assert!((r.x - x).abs() < 0.01 && (r.y - y).abs() < 0.01 && (r.w - w).abs() < 0.01 && (r.h - h).abs() < 0.01, "{r:?}");
        }
    }
}
