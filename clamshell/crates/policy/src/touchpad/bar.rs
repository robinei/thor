//! Fingers on the bar while the touchscreen is a touchpad, turned into the events the UI would make of
//! them: a press on a target, and on volume or brightness the slider's gesture (a tap opens the tab;
//! held for [`SLIDE_AFTER`] or moved sideways [`SLIDE_MOVE`] it becomes a slide, which doesn't end in a
//! tap). Positions on the slider are in the layout's units. Pure.

use std::time::Instant;

use clamshell_core::slider::{Control, SLIDE_AFTER, SLIDE_MOVE};
use clamshell_core::touch::TouchLayout;
use clamshell_core::{Event, Id, PointerEvent, Slide};

use super::router::UiTouch;

/// A finger on volume or brightness.
#[derive(Debug)]
struct SliderPress {
    control: Control,
    down_x: f32,
    x: f32,
    at: Instant,
    sliding: bool,
}

impl SliderPress {
    /// The slide starting, if it is due (held long enough, or `force`d by a move).
    fn start(&mut self, now: Instant, force: bool, width: f32, out: &mut Vec<Event>) {
        if !self.sliding && (force || now.duration_since(self.at) >= SLIDE_AFTER) {
            self.sliding = true;
            out.push(Slide::Start { control: self.control, x: self.x, width }.into());
        }
    }

    fn moved(&mut self, x: f32, now: Instant, width: f32, out: &mut Vec<Event>) {
        self.x = x;
        let was = self.sliding;
        self.start(now, (x - self.down_x).abs() >= SLIDE_MOVE, width, out);
        if was {
            out.push(Slide::To { x }.into());
        }
    }

    /// Lifted: whether it was a tap.
    fn tap(&self, now: Instant) -> bool {
        !self.sliding && now.duration_since(self.at) < SLIDE_AFTER
    }
}

#[derive(Debug, Default)]
pub struct BarGesture {
    /// The targets fingers are on.
    held: Vec<Id>,
    press: Option<SliderPress>,
}

impl BarGesture {
    /// A finger on a UI target at `now`; `layout`: where the layout is on the screen.
    pub fn on(&mut self, touch: UiTouch, now: Instant, layout: &TouchLayout, out: &mut Vec<Event>) {
        match touch {
            UiTouch::Down { id, x } => {
                log::debug!("touchpad mode: tap on {id:?}");
                out.push(PointerEvent::HideFocus.into());
                out.push(PointerEvent::Down(id.clone()).into());
                if let Id::Control(control) = id {
                    let x = layout.layout_x(x);
                    self.press = Some(SliderPress { control, down_x: x, x, at: now, sliding: false });
                }
                self.held.push(id);
            }
            UiTouch::Move { x } => {
                if let Some(press) = self.press.as_mut() {
                    press.moved(layout.layout_x(x), now, layout.width, out);
                }
            }
            UiTouch::Up { id } => {
                if self.press.take().is_none_or(|p| p.tap(now)) {
                    out.push(PointerEvent::Activate(id.clone()).into());
                }
                if let Some(i) = self.held.iter().position(|h| *h == id) {
                    self.held.remove(i);
                }
                out.push(PointerEvent::Up(id).into());
            }
        }
    }

    /// A finger held still on volume or brightness: the slider comes up (`width`: the layout's).
    pub fn tick(&mut self, now: Instant, width: f32, out: &mut Vec<Event>) {
        if let Some(press) = self.press.as_mut() {
            press.start(now, false, width, out);
        }
    }

    /// Touchpad mode ends: what the fingers were on is let go (no tap), a slide ends.
    pub fn cancel(&mut self, out: &mut Vec<Event>) {
        out.extend(self.held.drain(..).map(|id| PointerEvent::Up(id).into()));
        if self.press.take().is_some_and(|p| p.sliding) {
            out.push(Slide::End.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A layout 620 units wide at scale 2 (screen pixels are twice the units).
    fn layout() -> TouchLayout {
        TouchLayout { scale: 2.0, origin_x: 0.0, width: 620.0, ..TouchLayout::default() }
    }

    fn volume() -> Id {
        Id::Control(Control::Volume)
    }

    #[test]
    fn a_quick_tap_opens_the_tab() {
        let t0 = Instant::now();
        let mut g = BarGesture::default();
        let mut out = Vec::new();
        g.on(UiTouch::Down { id: volume(), x: 560.0 }, t0, &layout(), &mut out);
        g.on(UiTouch::Up { id: volume() }, t0 + Duration::from_millis(120), &layout(), &mut out);
        assert_eq!(
            out,
            vec![
                PointerEvent::HideFocus.into(),
                PointerEvent::Down(volume()).into(),
                PointerEvent::Activate(volume()).into(),
                PointerEvent::Up(volume()).into()
            ]
        );
    }

    #[test]
    fn held_still_the_slider_comes_up_and_lifting_is_no_tap() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut g = BarGesture::default();
        let mut out = Vec::new();
        g.on(UiTouch::Down { id: volume(), x: 560.0 }, t0, &layout(), &mut out);
        out.clear();
        g.tick(ms(100), 620.0, &mut out);
        assert!(out.is_empty(), "not yet");
        g.tick(ms(200), 620.0, &mut out);
        assert_eq!(out, vec![Slide::Start { control: Control::Volume, x: 280.0, width: 620.0 }.into()]);
        out.clear();
        g.on(UiTouch::Up { id: volume() }, ms(250), &layout(), &mut out);
        assert_eq!(out, vec![PointerEvent::Up(volume()).into()]);
    }

    #[test]
    fn slid_sideways_the_slider_follows_the_finger() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let brightness = Id::Control(Control::Brightness);
        let mut g = BarGesture::default();
        let mut out = Vec::new();
        g.on(UiTouch::Down { id: brightness.clone(), x: 660.0 }, t0, &layout(), &mut out);
        out.clear();
        g.on(UiTouch::Move { x: 666.0 }, ms(20), &layout(), &mut out);
        assert!(out.is_empty(), "a small wobble is still a tap");
        g.on(UiTouch::Move { x: 690.0 }, ms(40), &layout(), &mut out);
        g.on(UiTouch::Move { x: 720.0 }, ms(60), &layout(), &mut out);
        assert_eq!(out, vec![Slide::Start { control: Control::Brightness, x: 345.0, width: 620.0 }.into(), Slide::To { x: 360.0 }.into()]);
        out.clear();
        g.on(UiTouch::Up { id: brightness.clone() }, ms(80), &layout(), &mut out);
        assert_eq!(out, vec![PointerEvent::Up(brightness).into()], "a quick slide isn't a tap either");
    }

    #[test]
    fn ending_touchpad_mode_lets_go_without_a_tap() {
        let t0 = Instant::now();
        let mut g = BarGesture::default();
        let mut out = Vec::new();
        let tab = Id::Tab(clamshell_core::Tab::Touchpad);
        g.on(UiTouch::Down { id: tab.clone(), x: 10.0 }, t0, &layout(), &mut out);
        g.on(UiTouch::Down { id: volume(), x: 560.0 }, t0, &layout(), &mut out);
        g.tick(t0 + SLIDE_AFTER, 620.0, &mut out);
        out.clear();
        g.cancel(&mut out);
        assert_eq!(out, vec![PointerEvent::Up(tab).into(), PointerEvent::Up(volume()).into(), Slide::End.into()]);
        out.clear();
        g.cancel(&mut out);
        assert!(out.is_empty(), "once");
    }
}
