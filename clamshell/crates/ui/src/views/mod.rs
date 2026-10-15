//! The screens. Each is a function of the state and its size that draws everything and lists what can be
//! tapped or focused (immediate mode): shelf on the main screen; the bar and the open tab below it on the
//! second screen (or nothing, while it is dark). One module per tab; what they share is in [`style`]
//! (palette, sizes, looks), [`widgets`] (tracks, switches, rows, tiles, box art) and [`format`] (text
//! without allocation), and [`Frame::button`] gives every tappable thing the same pressed highlight,
//! focus ring and target.
//!
//! Everything here is in layout units: logical pixels from the top left of the area inside the burn-in
//! margin (see `render`). Sizes are fractions of the screen or of the element they sit in, named per
//! module unless they are shared (then in `style`).

mod bar;
mod battery;
mod bluetooth;
mod dark;
mod dock;
mod editor;
pub(crate) mod format;
mod keyboard;
mod keymaps;
mod levels;
mod pad;
mod radio;
pub(crate) mod scroll;
mod settings;
mod shelf;
pub(crate) mod style;
mod sysinfo;
mod touchpad;
mod widgets;
mod wifi;

use std::fmt::Write;

use clamshell_core::settings::Setting;
use clamshell_core::{Id, ListId, State, Tab};

use crate::draw::{Canvas, Rect};
use crate::images::Images;
use format::Fmt;
use style::{BG, Look, PRESSED, RADIUS_S, Tone};

/// Something drawn that can be tapped or focused. While drawing, in layout units; once the frame is done,
/// in the surface's logical pixels (moved by the burn-in offset).
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub rect: Rect,
    pub id: Id,
    /// Where a finger on it sets a value, if it does.
    pub track: Option<Track>,
    /// The scrolled list it is in, if it is in one: a vertical drag on it scrolls the list.
    pub list: Option<scroll::InList>,
}

/// The span along x that a finger on a target sets a value over: a level row's track (0 at `x0`, 100 at
/// `x0 + w`), or for the bar's volume and brightness, the whole screen width that their slider runs
/// along. In the same units as the target's rect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Track {
    pub x0: f32,
    pub w: f32,
}

impl Track {
    /// The value (0-100) a finger at `x` sets.
    pub fn value_at(&self, x: f32) -> u8 {
        let v = (x - self.x0) / self.w.max(1.0) * 100.0;
        v.round().clamp(0.0, 100.0) as u8
    }

    /// `x` relative to where the track starts.
    pub fn along(&self, x: f32) -> f32 {
        x - self.x0
    }
}

/// What a frame tells the rest of the program besides its picture and targets.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameOut {
    /// How many rows shelf's list shows (the main screen): the state keeps the selection within them.
    pub list_rows: Option<usize>,
    /// The area below the bar (the second screen), which is the touchpad in the touchpad tab.
    pub touchpad_area: Option<Rect>,
    /// How many rows each scrolled list drawn shows (by [`ListId::index`]): the state keeps the lists'
    /// offsets and the pad's focus within them.
    pub lists: [Option<f32>; ListId::COUNT],
}

/// Text kept between frames: the clock (formatted once a minute), and the buffer `core`'s setting
/// descriptions are written into.
#[derive(Debug, Default)]
pub struct Scratch {
    clock: Option<(u64, Fmt<8>)>,
    describe: String,
}

impl Scratch {
    /// The local time, "14:05".
    fn clock(&mut self, unix_secs: u64) -> Fmt<8> {
        let minute = unix_secs / 60;
        match self.clock {
            Some((m, text)) if m == minute => text,
            _ => {
                let mut text = Fmt::new();
                if let Some(t) = i64::try_from(unix_secs).ok().and_then(|secs| jiff::Timestamp::from_second(secs).ok()) {
                    let local = t.to_zoned(jiff::tz::TimeZone::system());
                    let _ = write!(text, "{}", local.strftime("%H:%M"));
                }
                self.clock = Some((minute, text));
                text
            }
        }
    }

    /// How `value` of `setting` reads on its tile.
    fn describe(&mut self, setting: Setting, value: u16) -> Fmt<32> {
        setting.describe(value, &mut self.describe);
        Fmt::of(format_args!("{}", self.describe))
    }
}

/// One screen's drawing pass: the canvas, the state it shows, images and kept text, the targets it
/// registers and what it reports.
pub struct Frame<'f, 'c> {
    pub c: &'f mut Canvas<'c>,
    pub s: &'f State,
    pub images: &'f mut Images,
    scratch: &'f mut Scratch,
    targets: &'f mut Vec<Target>,
    out: FrameOut,
}

impl<'f, 'c> Frame<'f, 'c> {
    pub fn new(c: &'f mut Canvas<'c>, s: &'f State, images: &'f mut Images, scratch: &'f mut Scratch, targets: &'f mut Vec<Target>) -> Self {
        targets.clear();
        Frame { c, s, images, scratch, targets, out: FrameOut::default() }
    }

    /// A tappable, focusable area: its background (lit while pressed), `content`, then the focus ring if
    /// focused; `rect` becomes its target.
    fn button(&mut self, id: Id, rect: Rect, look: Look, content: impl FnOnce(&mut Self)) {
        self.button_in(id, rect, rect, look, content);
    }

    /// [`Frame::button`] for something drawn smaller than its target: the background and the focus ring
    /// follow `shown`, the target is `rect`.
    fn button_in(&mut self, id: Id, rect: Rect, shown: Rect, look: Look, content: impl FnOnce(&mut Self)) {
        let pressed = self.s.pointer().pressed() == Some(&id);
        let background = if pressed && look.press_fill { Some(PRESSED) } else { look.fill };
        if let Some(color) = background {
            let r = shown.inset(look.inset);
            if look.radius > 0.0 {
                self.c.rounded(r, look.radius, color);
            } else {
                self.c.fill(r, color);
            }
        }
        content(self);
        if self.s.pointer().focused(&id) {
            self.c.outline(shown.inset(look.ring), look.radius.max(RADIUS_S), 2.0, Tone::Accent);
        }
        self.targets.push(Target { rect, id, track: None, list: None });
    }

    /// Whether `id` is under a finger (or the pad's A) now.
    fn pressed(&self, id: &Id) -> bool {
        self.s.pointer().pressed() == Some(id)
    }

    /// A text action (see [`Look::TEXT`]): `label` in `r` at `size` (centred, or from the left with
    /// [`crate::draw::Align::Start`]), in `tone` (blue for an action, see [`Tone::Fn`]), white while
    /// pressed. The whole of `r` is its target.
    fn text_button(&mut self, id: Id, r: Rect, size: f32, tone: Tone, label: &str, align: crate::draw::Align) {
        let tone = if self.pressed(&id) { Tone::High } else { tone };
        self.button(id, r, Look::TEXT, |f| {
            let w = f.c.text_width(size, label).min(r.w);
            let x = if align == crate::draw::Align::Start { r.x } else { r.x + (r.w - w) / 2.0 };
            f.c.text_fit(Rect::new(x, r.y, w + 1.0, r.h), size, tone, label, crate::draw::Keep::Start);
        });
    }

    /// Gives the last target a track (whatever drew the track knows where it is).
    fn track(&mut self, track: Track) {
        if let Some(t) = self.targets.last_mut() {
            t.track = Some(track);
        }
    }

    pub fn finish(self) -> FrameOut {
        self.out
    }
}

/// The main screen: shelf, over all of `screen`.
pub fn top(f: &mut Frame, screen: Rect) {
    f.c.fill(screen, BG);
    f.out.list_rows = Some(shelf::draw(f, screen));
}

/// The second screen: the bar, the open tab below it, and the slider over both while one moves; or,
/// while a game wants it dark, nothing.
pub fn bottom(f: &mut Frame, screen: Rect) {
    f.c.fill(screen, BG);
    if f.s.bottom_dark() {
        dark::draw(f, screen);
        f.out.touchpad_area = Some(screen);
        return;
    }
    let body = bar::draw(f, screen);
    match f.s.bottom().tab() {
        Tab::Settings => settings::draw(f, body),
        Tab::Keyboard => keyboard::draw(f, body),
        Tab::Dock => dock::draw(f, body),
        Tab::Touchpad => touchpad::draw(f, body),
        Tab::Sysinfo => sysinfo::draw(f, body),
        Tab::Wifi => wifi::draw(f, body),
        Tab::Battery => battery::draw(f, body),
        Tab::Bluetooth => bluetooth::draw(f, body),
        Tab::Volume => levels::draw_volume(f, body),
        Tab::Brightness => levels::draw_brightness(f, body),
    }
    bar::draw_slider_over(f, screen, body.y);
    f.out.touchpad_area = Some(body);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::draw::DrawCache;
    use clamshell_core::{Event, Init, Reading, Status};
    use tiny_skia::Pixmap;

    fn demo() -> State {
        let mut state = State::new(Init { library: clamshell_library::demo_library(), ..Init::default() });
        state.apply(Event::Tick { unix_secs: 1_700_000_000 });
        state.apply(Event::Reading(Reading::Status(Status { battery: Some(50), volume: Some(50), brightness: Some(50), wifi: Some(50), ..Status::default() })));
        state
    }

    /// Two targets may overlap only when one is drawn over the other on purpose: the dock's "+" over its
    /// app's tile.
    fn stacked(a: &Id, b: &Id) -> bool {
        use clamshell_core::DockId;
        // A list's area lies under its rows.
        if matches!(a, Id::List(_)) || matches!(b, Id::List(_)) {
            return true;
        }
        use clamshell_core::KeymapId::Input as K;
        use clamshell_core::keymap::{Button, Input, Side};
        // A stick's click sits on its cap.
        let click = |a: &Id, b: &Id| {
            matches!((a, b), (Id::Keymap(K(Input::Stick(Side::Left))), Id::Keymap(K(Input::Button(Button::LS)))) | (Id::Keymap(K(Input::Stick(Side::Right))), Id::Keymap(K(Input::Button(Button::RS)))))
        };
        click(a, b)
            || click(b, a)
            || matches!((a, b), (Id::Dock(DockId::App(i)), Id::Dock(DockId::NewInstance(j))) | (Id::Dock(DockId::NewInstance(j)), Id::Dock(DockId::App(i))) if i == j)
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        let apart = a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5;
        !apart
    }

    /// The keymaps' screens one after another (as a user goes through them) at the desktop's window, the
    /// Thor's screen and a narrow one: targets within the area, none overlapping but on purpose.
    #[test]
    fn the_keymap_screens_lay_out() {
        use clamshell_core::keymap::{Button, Input, Keymaps, Layer, Param, Side};
        use clamshell_core::{KeymapId, ListId, PointerEvent, SettingsId, Target};
        use clamshell_core::settings::Setting;
        let tap = |id| Event::Pointer(PointerEvent::Activate(id));
        let km = |id| tap(Id::Keymap(id));
        let pick = |input| vec![km(KeymapId::Input(input)), km(KeymapId::Input(input))];
        let slide = |param, lift: bool| {
            let control = clamshell_core::slider::Control::Param(param);
            let mut v = vec![Event::Slide(clamshell_core::Slide::Start { control, x: 100.0, width: 300.0 }), Event::Slide(clamshell_core::Slide::To { x: 160.0 })];
            if lift {
                v.push(Event::Slide(clamshell_core::Slide::End));
            }
            v
        };
        let screens: Vec<(&str, Vec<Event>)> = vec![
            ("picker", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(Setting::Keymap)))]),
            ("menu", vec![Event::Pointer(PointerEvent::LongPress(ListId::Keymaps.row(1)))]),
            ("name", vec![km(KeymapId::Rename)]),
            ("delete", vec![tap(Id::Editor(clamshell_core::EditorId::Cancel)), km(KeymapId::Back), tap(ListId::Keymaps.row(1)), tap(Id::Settings(SettingsId::Tile(Setting::Keymap))), Event::Pointer(PointerEvent::LongPress(ListId::Keymaps.row(1))), km(KeymapId::Delete)]),
            ("edit", vec![km(KeymapId::Back), tap(Id::Row { list: ListId::Keymaps, i: 1, part: clamshell_core::RowPart::Edit })]),
            ("edit, a button selected", vec![km(KeymapId::Input(Input::Button(Button::A)))]),
            ("map to, a button", vec![km(KeymapId::Change)]),
            ("map to, a trigger", [vec![km(KeymapId::Back)], pick(Input::Trigger(Side::Left)), vec![km(KeymapId::Target(Target::Key))]].concat()),
            ("capture", vec![]),
            ("map to, the gyro", [vec![km(KeymapId::Back), km(KeymapId::Back)], pick(Input::Gyro), vec![km(KeymapId::Target(Target::Pointer)), km(KeymapId::Input(Input::Gyro))]].concat()),
            ("edit, the gyro's panel", vec![km(KeymapId::Back)]),
            ("the gyro's hold", vec![km(KeymapId::Holds)]),
            ("map to, a stick", [vec![km(KeymapId::Back)], pick(Input::Stick(Side::Right)), vec![km(KeymapId::Target(Target::Pointer))]].concat()),
            ("edit, a stick's panel", vec![]),
            ("edit, a slider up", slide(Param::Speed, false)),
            ("edit, the Back layer", [slide(Param::Speed, true), vec![km(KeymapId::Layer(Layer::Back)), km(KeymapId::Input(Input::Trigger(Side::Right)))]].concat()),
            ("edit, clearing", vec![km(KeymapId::Clear)]),
            ("press", [vec![km(KeymapId::Back), km(KeymapId::Layer(Layer::Main))], pick(Input::Button(Button::B)), vec![km(KeymapId::Target(Target::PressPad))]].concat()),
        ];
        let mut cache = DrawCache::load();
        for (w, h) in [(620u32, 540u32), (1240, 1080), (300, 540)] {
            let mut state = State::new(Init { library: clamshell_library::demo_library(), keymaps: Keymaps::initial(), ..Init::default() });
            for (name, events) in &screens {
                for e in events {
                    state.apply(e.clone());
                }
                assert!(state.keymap_screen().is_some() || state.bottom().editor().is_some(), "{name}: a keymap screen");
                let mut pixmap = Pixmap::new(w, h).expect("pixmap");
                let mut targets = Vec::new();
                let mut canvas = Canvas::new(&mut pixmap, 1.0, &mut cache);
                let (mut images, mut scratch) = (Images::synchronous(), Scratch::default());
                let mut f = Frame::new(&mut canvas, &state, &mut images, &mut scratch, &mut targets);
                let screen = Rect::new(0.0, 0.0, w as f32, h as f32);
                bottom(&mut f, screen);
                let body = f.finish().touchpad_area.expect("the area below the bar");
                let case = format!("{name} at {w}x{h}");
                assert!(targets.iter().any(|t| matches!(t.id, Id::Keymap(_) | Id::Row { .. } | Id::Editor(_))), "{case}: its buttons");
                if name.starts_with("edit") {
                    // The panel, its buttons and rows, between the label columns; the label columns
                    // clear of the clusters.
                    let l = keymaps::edit_layout(body);
                    for t in targets.iter().filter(|t| matches!(t.id, Id::Control(clamshell_core::slider::Control::Param(_)) | Id::Keymap(KeymapId::Change | KeymapId::Reset | KeymapId::Toggle(_) | KeymapId::Holds))) {
                        assert!(t.rect.x >= l.strip.x - 0.5 && t.rect.right() <= l.strip.right() + 0.5, "{case}: {:?} at {:?} is outside the panel's strip {:?}", t.id, t.rect, l.strip);
                    }
                    assert!(l.strip.w > 0.0, "{case}: no room for the panel");
                    if w >= 620 {
                        assert!(l.strip.w >= w as f32 * 0.2, "{case}: the panel is {} wide", l.strip.w);
                    }
                }
                if *name == "edit" {
                    // Every input the Thor has can be touched; on its screen, with room for a finger.
                    // (The Thor has no back buttons.)
                    for input in Input::all().filter(|i| !matches!(i, Input::Button(b) if b.is_back())) {
                        let id = Id::Keymap(KeymapId::Input(input));
                        let biggest = targets.iter().filter(|t| t.id == id).map(|t| t.rect.w.min(t.rect.h)).fold(0.0, f32::max);
                        assert!(biggest > 0.0, "{case}: {input:?} has no target");
                        assert!(w != 1240 || biggest >= 60.0, "{case}: {input:?}'s target is {biggest} across");
                    }
                }
                for (i, a) in targets.iter().enumerate() {
                    assert!(a.rect.w > 0.0 && a.rect.h > 0.0, "{case}: {:?} has no size", a.id);
                    if a.rect.y + a.rect.h > body.y + 0.5 {
                        let inside = a.rect.x >= body.x - 0.5 && a.rect.y >= body.y - 0.5 && a.rect.right() <= body.right() + 0.5 && a.rect.bottom() <= body.bottom() + 0.5;
                        assert!(inside, "{case}: {:?} at {:?} is outside {body:?}", a.id, a.rect);
                    }
                    for b in targets.iter().skip(i + 1) {
                        assert!(!overlap(a.rect, b.rect) || stacked(&a.id, &b.id), "{case}: {:?} and {:?} overlap", a.id, b.id);
                    }
                }
            }
        }
    }

    /// Long lists (more networks, devices, apps playing and backlights than fit) at the desktop's window,
    /// the Thor's screen and a narrow one, at the top and scrolled part way: rows are cut to the list's
    /// area, every target stays within the area below the bar and none overlap but on purpose.
    #[test]
    fn long_lists_scroll_within_their_area() {
        use clamshell_core::bluetooth::{Bluetooth, Device, Kind};
        use clamshell_core::levels::{Audio, Stream};
        use clamshell_core::wifi::{Network, Wifi};
        use clamshell_core::{ListEvent, ListId};
        let mut cache = DrawCache::load();
        let readings = [
            Reading::Wifi(Wifi { device: true, enabled: true, networks: (0..30).map(|i| Network { ssid: format!("net {i}"), ..Network::default() }).collect(), ..Wifi::default() }),
            Reading::Bluetooth(Bluetooth {
                adapter: true,
                powered: true,
                devices: (0..30).map(|i| Device { address: format!("{i}"), name: format!("dev {i}"), kind: Kind::Other, paired: true, connected: false }).collect(),
                ..Bluetooth::default()
            }),
            Reading::Audio(Audio { outputs: vec![], streams: (0..30).map(|i| Stream { id: i, app: format!("app {i}"), title: String::new(), volume: 50, muted: false }).collect() }),
            Reading::Backlights((0..30).map(|i| format!("panel {i}")).collect()),
        ];
        for (tab, list) in [(Tab::Wifi, ListId::Wifi), (Tab::Bluetooth, ListId::Bluetooth), (Tab::Volume, ListId::Streams), (Tab::Brightness, ListId::Backlights)] {
            for (w, h) in [(620u32, 540u32), (1240, 1080), (300, 540)] {
                for scrolled in [0.0, 2.5] {
                    let mut state = demo();
                    for r in &readings {
                        state.apply(Event::Reading(r.clone()));
                    }
                    state.apply(Event::Pointer(clamshell_core::PointerEvent::Activate(Id::Tab(tab))));
                    let mut pixmap = Pixmap::new(w, h).expect("pixmap");
                    let screen = Rect::new(0.0, 0.0, w as f32, h as f32);
                    let mut targets = Vec::new();
                    let draw = |state: &State, cache: &mut DrawCache, pixmap: &mut Pixmap, targets: &mut Vec<Target>| {
                        let mut canvas = Canvas::new(pixmap, 1.0, cache);
                        let (mut images, mut scratch) = (Images::synchronous(), Scratch::default());
                        let mut f = Frame::new(&mut canvas, state, &mut images, &mut scratch, targets);
                        bottom(&mut f, screen);
                        f.finish()
                    };
                    let out = draw(&state, &mut cache, &mut pixmap, &mut targets);
                    let visible = out.lists[list.index()].expect("the list reports its rows");
                    state.apply(ListEvent::Visible(list, visible).into());
                    state.apply(ListEvent::Scroll(list, scrolled).into());
                    let body = draw(&state, &mut cache, &mut pixmap, &mut targets).touchpad_area.expect("the area below the bar");
                    let case = format!("{tab:?} at {w}x{h} scrolled {scrolled}");
                    let area = targets.iter().find(|t| t.id == Id::List(list)).map(|t| t.rect).expect("the list's area");
                    assert!(visible > 1.0 && (visible - area.h / targets.iter().find_map(|t| t.list).map_or(1.0, |l| l.row_h)).abs() < 1e-3, "{case}: {visible}");
                    let rows: Vec<&Target> = targets.iter().filter(|t| clamshell_core::ListId::of(&t.id).is_some_and(|(l, _)| l == list)).collect();
                    assert!(!rows.is_empty(), "{case}: rows drawn");
                    let first = rows.iter().filter_map(|t| clamshell_core::ListId::of(&t.id)).map(|(_, i)| i).min();
                    assert_eq!(first, Some(scrolled as usize), "{case}: the first row shown");
                    for (i, a) in targets.iter().enumerate() {
                        assert!(a.rect.w > 0.0 && a.rect.h > 0.0, "{case}: {:?} has no size", a.id);
                        if a.list.is_some() {
                            assert!(a.rect.y >= area.y - 0.01 && a.rect.bottom() <= area.bottom() + 0.01, "{case}: {:?} outside its list", a.id);
                        }
                        let in_bar = a.rect.y + a.rect.h <= body.y + 0.5;
                        if !in_bar {
                            assert!(a.rect.y >= body.y - 0.5 && a.rect.bottom() <= body.bottom() + 0.5, "{case}: {:?} at {:?} is outside", a.id, a.rect);
                        }
                        for b in targets.iter().skip(i + 1) {
                            assert!(!overlap(a.rect, b.rect) || stacked(&a.id, &b.id), "{case}: {:?} and {:?} overlap", a.id, b.id);
                        }
                    }
                }
            }
        }
    }

    /// Every tab at the desktop's window, the Thor's screen and a window dragged narrow, empty and with
    /// readings: targets have a size, stay on the screen (the tab's within its area below the bar), and
    /// don't overlap unless stacked on purpose; the bar shows all its buttons.
    #[test]
    fn every_tab_lays_out() {
        let mut cache = DrawCache::load();
        for with_data in [false, true] {
            for tab in Tab::ALL {
                for (w, h) in [(620u32, 540u32), (1240, 1080), (300, 540)] {
                    let mut state = if with_data { demo() } else { State::new(Init::default()) };
                    state.apply(Event::Pointer(clamshell_core::PointerEvent::Activate(Id::Tab(tab))));
                    let mut pixmap = Pixmap::new(w, h).expect("pixmap");
                    let mut targets = Vec::new();
                    let mut canvas = Canvas::new(&mut pixmap, 1.0, &mut cache);
                    let (mut images, mut scratch) = (Images::synchronous(), Scratch::default());
                    let mut f = Frame::new(&mut canvas, &state, &mut images, &mut scratch, &mut targets);
                    let screen = Rect::new(0.0, 0.0, w as f32, h as f32);
                    bottom(&mut f, screen);
                    let body = f.finish().touchpad_area.expect("the area below the bar");
                    let case = format!("{tab:?} at {w}x{h}{}", if with_data { ", with data" } else { "" });
                    let bar = targets.iter().filter(|t| t.rect.y + t.rect.h <= body.y + 0.5).count();
                    assert!(bar >= 11, "{case}: {bar} bar targets");
                    for (i, a) in targets.iter().enumerate() {
                        assert!(a.rect.w > 0.0 && a.rect.h > 0.0, "{case}: {:?} has no size", a.id);
                        let inside = |r: Rect, area: Rect| {
                            r.x >= area.x - 0.5 && r.y >= area.y - 0.5 && r.right() <= area.right() + 0.5 && r.bottom() <= area.bottom() + 0.5
                        };
                        let in_bar = a.rect.y + a.rect.h <= body.y + 0.5;
                        assert!(inside(a.rect, if in_bar { screen } else { body }), "{case}: {:?} at {:?} is outside", a.id, a.rect);
                        for b in targets.iter().skip(i + 1) {
                            assert!(!overlap(a.rect, b.rect) || stacked(&a.id, &b.id), "{case}: {:?} and {:?} overlap", a.id, b.id);
                        }
                    }
                }
            }
        }
    }
}

