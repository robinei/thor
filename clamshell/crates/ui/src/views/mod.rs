//! The screens. Each is a function of the state and its size that draws everything and lists what can be
//! tapped or focused (immediate mode): shelf on the main screen; the bar and the open tab below it on the
//! second screen (or nothing, while it is dark). One module per tab; what they're drawn with is the
//! toolkit (`crate::toolkit`: the frame, style, widgets, text formatting, scrolled lists).
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
mod keyboard;
mod keymaps;
mod levels;
mod radio;
mod settings;
mod shelf;
mod sysinfo;
mod time;
mod touchpad;
mod wifi;

use clamshell_core::Tab;

use crate::draw::Rect;
use crate::toolkit::Frame;
use crate::toolkit::style::BG;

/// The main screen: shelf, over all of `screen`.
pub fn top(f: &mut Frame, screen: Rect) {
    f.body = screen;
    f.c.fill(screen, BG);
    f.out.list_rows = Some(shelf::draw(f, screen));
}

/// The second screen: the bar, the open tab below it, and the slider over both while one moves; or,
/// while a game wants it dark, nothing.
pub fn bottom(f: &mut Frame, screen: Rect) {
    f.body = screen;
    f.c.fill(screen, BG);
    if f.s.bottom_dark() {
        dark::draw(f, screen);
        f.out.touchpad_area = Some(screen);
        return;
    }
    let body = bar::draw(f, screen);
    f.body = body;
    match f.s.bottom().tab() {
        Tab::Settings => settings::draw(f),
        Tab::Keyboard => keyboard::draw(f, body),
        Tab::Dock => dock::draw(f),
        Tab::Touchpad => touchpad::draw(f),
        Tab::Sysinfo => sysinfo::draw(f),
        Tab::Wifi => wifi::draw(f),
        Tab::Battery => battery::draw(f),
        Tab::Bluetooth => bluetooth::draw(f),
        Tab::Volume => levels::draw_volume(f),
        Tab::Brightness => levels::draw_brightness(f),
        Tab::Time => time::draw(f),
    }
    bar::draw_slider_over(f, screen, body.y);
    f.out.touchpad_area = Some(body);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::draw::{Canvas, DrawCache};
    use crate::images::Images;
    use crate::toolkit::{Scratch, Target};
    use clamshell_core::{Event, Id, Init, Reading, State, Status};
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
        // A list's area lies under its rows.
        if matches!(a, Id::List(_)) || matches!(b, Id::List(_)) {
            return true;
        }
        use clamshell_core::KeymapId::Input as K;
        use clamshell_core::keymap::{Button, Input, Side};
        // A stick's click sits on its cap.
        let click = |a: &Id, b: &Id| {
            matches!(
                (a, b),
                (Id::Keymap(K(Input::Stick(Side::Left))), Id::Keymap(K(Input::Button(Button::LS))))
                    | (Id::Keymap(K(Input::Stick(Side::Right))), Id::Keymap(K(Input::Button(Button::RS))))
            )
        };
        // The mouse's wheel and its turns sit on its buttons.
        use clamshell_core::Target;
        use clamshell_core::keymap::MouseButton;
        let button = |id: &Id| matches!(id, Id::Keymap(clamshell_core::KeymapId::Target(Target::Mouse(MouseButton::Left | MouseButton::Right))));
        let wheel = |id: &Id| matches!(id, Id::Keymap(clamshell_core::KeymapId::Target(Target::Mouse(MouseButton::Middle) | Target::Wheel(_))));
        click(a, b) || (button(a) && wheel(b)) || (wheel(a) && button(b)) || click(b, a)
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
        use clamshell_core::settings::Setting;
        use clamshell_core::{KeymapId, ListId, PointerEvent, SettingsId, Target};
        let tap = |id| Event::Pointer(PointerEvent::Activate(id));
        let km = |id| tap(Id::Keymap(id));
        let pick = |input| vec![km(KeymapId::Input(input))];
        let slide = |param, lift: bool| {
            let control = clamshell_core::slider::Control::Param(param);
            let mut v =
                vec![Event::Slide(clamshell_core::Slide::Start { control, x: 100.0, width: 300.0 }), Event::Slide(clamshell_core::Slide::To { x: 160.0 })];
            if lift {
                v.push(Event::Slide(clamshell_core::Slide::End));
            }
            v
        };
        let screens: Vec<(&str, Vec<Event>)> = vec![
            ("picker", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(Setting::Keymap)))]),
            ("menu", vec![Event::Pointer(PointerEvent::LongPress(ListId::Keymaps.row(2)))]),
            ("name", vec![km(KeymapId::Rename)]),
            (
                "delete",
                vec![
                    tap(Id::Editor(clamshell_core::EditorId::Cancel)),
                    km(KeymapId::Back),
                    tap(ListId::Keymaps.row(2)),
                    tap(Id::Settings(SettingsId::Tile(Setting::Keymap))),
                    Event::Pointer(PointerEvent::LongPress(ListId::Keymaps.row(2))),
                    km(KeymapId::Delete),
                ],
            ),
            ("edit", vec![km(KeymapId::Back), tap(Id::Row { list: ListId::Keymaps, i: 2, part: clamshell_core::RowPart::Edit })]),
            ("edit, a button selected", vec![km(KeymapId::Input(Input::Button(Button::A)))]),
            ("the mouse", vec![km(KeymapId::Target(Target::MouseParts))]),
            ("capture", [vec![km(KeymapId::Back)], pick(Input::Trigger(Side::Left)), vec![km(KeymapId::Target(Target::Key))]].concat()),
            ("edit, the gyro's panel", [vec![km(KeymapId::Back)], pick(Input::Gyro), vec![km(KeymapId::Target(Target::Pointer))]].concat()),
            ("the gyro's hold", vec![km(KeymapId::Holds)]),
            ("edit, a stick's panel", [vec![km(KeymapId::Back)], pick(Input::Stick(Side::Right)), vec![km(KeymapId::Target(Target::Pointer))]].concat()),
            ("edit, a slider up", slide(Param::Speed, false)),
            (
                "edit, the Back layer",
                [slide(Param::Speed, true), vec![km(KeymapId::Layer(Layer(1))), km(KeymapId::Input(Input::Trigger(Side::Right)))]].concat(),
            ),
            ("edit, clearing", vec![km(KeymapId::Clear)]),
            (
                "edit, picking a control",
                [vec![km(KeymapId::Back), km(KeymapId::Layer(Layer::BASE))], pick(Input::Button(Button::B)), vec![km(KeymapId::Target(Target::Pick))]].concat(),
            ),
        ];
        let mut cache = DrawCache::load();
        for (w, h) in [(620u32, 540u32), (1240, 1080), (300, 540)] {
            // (The Nintendo layout with a layer added.)
            let mut keymaps = Keymaps::initial();
            keymaps.get_mut(1).and_then(|k| k.add_layer("aim"));
            let mut state = State::new(Init { library: clamshell_library::demo_library(), keymaps, ..Init::default() });
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
                    for t in targets.iter().filter(|t| {
                        matches!(
                            t.id,
                            Id::Control(clamshell_core::slider::Control::Param(_)) | Id::Keymap(KeymapId::Target(_) | KeymapId::Toggle(_) | KeymapId::Holds)
                        )
                    }) {
                        assert!(
                            t.rect.x >= l.strip.x - 0.5 && t.rect.right() <= l.strip.right() + 0.5,
                            "{case}: {:?} at {:?} is outside the panel's strip {:?}",
                            t.id,
                            t.rect,
                            l.strip
                        );
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
                        let inside = a.rect.x >= body.x - 0.5
                            && a.rect.y >= body.y - 0.5
                            && a.rect.right() <= body.right() + 0.5
                            && a.rect.bottom() <= body.bottom() + 0.5;
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
            Reading::Wifi(Wifi {
                device: true,
                enabled: true,
                networks: (0..30).map(|i| Network { ssid: format!("net {i}"), ..Network::default() }).collect(),
                ..Wifi::default()
            }),
            Reading::Bluetooth(Bluetooth {
                adapter: true,
                powered: true,
                devices: (0..30)
                    .map(|i| Device { address: format!("{i}"), name: format!("dev {i}"), kind: Kind::Other, paired: true, connected: false })
                    .collect(),
                ..Bluetooth::default()
            }),
            Reading::Audio(Audio {
                outputs: vec![],
                streams: (0..30).map(|i| Stream { id: i, app: format!("app {i}"), title: String::new(), volume: 50, muted: false }).collect(),
            }),
            Reading::Backlights((0..30).map(|i| format!("panel {i}")).collect()),
        ];
        for (tab, list) in
            [(Tab::Wifi, ListId::Wifi), (Tab::Bluetooth, ListId::Bluetooth), (Tab::Volume, ListId::Streams), (Tab::Brightness, ListId::Backlights)]
        {
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
                    assert!(
                        visible > 1.0 && (visible - area.h / targets.iter().find_map(|t| t.list).map_or(1.0, |l| l.row_h)).abs() < 1e-3,
                        "{case}: {visible}"
                    );
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
