//! The screens, drawn in immediate mode. Each is a function of the state and the screen's size that
//! draws everything and lists what can be tapped or focused. The main screen shows the shelf. The second
//! screen shows the bar with the open tab below it, or nothing while it is dark. There is one module per
//! tab. They are drawn with the toolkit (`crate::toolkit`): the frame, style, widgets, text formatting and
//! scrolled lists.
//!
//! Everything here is in layout units: logical pixels from the top left of the area inside the burn-in
//! margin (see `render`). Sizes are fractions of the screen or of the element they sit in. Sizes that
//! define a layout, such as a row's height, a column's width or a gap, are named constants in their
//! module, or in `style` when shared. Small adjustments, such as half a padding or slightly larger text,
//! are written where they are used.

pub(crate) mod bar;
mod battery;
mod bluetooth;
mod curves;
mod dark;
mod device;
mod dock;
mod editor;
mod keyboard;
mod keymaps;
mod levels;
mod profiles;
mod radio;
mod settings;
mod shelf;
mod sysinfo;
mod time;
mod touchpad;
mod wifi;

use clamshell_state::Tab;
use clamshell_state::bottom::editor::Purpose;
use clamshell_state::shelf::files::FileName;

use crate::gfx::Rect;
use crate::toolkit::Frame;

/// How much the black layer over the area below the bar hides it while that area is a touchpad.
const POINTING_FADE: f32 = 0.7;
/// How much the black layer over the second screen hides it while it is dimmed. A third is left, as with
/// the screens' brightness when idle dims them.
const BOTTOM_DIM: f32 = 2.0 / 3.0;

/// The main screen, over all of `screen`: the shelf, or the Device view shown in its place. The frame
/// starts black (see `render`).
pub fn top(f: &mut Frame, screen: Rect) {
    f.body = screen;
    match f.s.device().top_view() {
        clamshell_state::device::TopView::Shelf => f.out.list_rows = Some(shelf::draw(f, screen)),
        clamshell_state::device::TopView::Device => device::draw(f, screen),
    }
}

/// The windows' tab: the window moved down, which the compositor shows below the bar, or with none
/// there, how to move one there.
fn windows(f: &mut Frame, body: Rect) {
    if f.s.bottom().shows_windows(f.s.session()) {
        return;
    }
    let size = f.text(crate::toolkit::style::Text::Body);
    let line = size * 1.6;
    let mid = body.y + body.h / 2.0;
    let at = |y: f32| Rect::new(body.x, y, body.w, line);
    f.c.text_in(at(mid - line), size, crate::toolkit::style::Tone::Normal, "Nothing here", crate::gfx::Align::Center);
    f.c.text_in(at(mid), size * 0.85, crate::toolkit::style::Tone::Dim, "Home with the right stick down moves a window here", crate::gfx::Align::Center);
}

/// The second screen: the bar, the open tab below it, and the slider over both while one is moving.
/// While a game wants the screen dark, nothing is drawn. The frame starts black (see `render`).
pub fn bottom(f: &mut Frame, screen: Rect) {
    f.body = screen;
    if f.s.bottom_dark() {
        dark::draw(f, screen);
        f.out.touchpad_area = Some(screen);
        return;
    }
    let body = bar::draw(f, screen);
    f.body = body;
    match f.s.bottom().tab() {
        Tab::Settings => settings::draw(f),
        // While a browsed file's name or the account password is being typed, the editor replaces the
        // keyboard.
        Tab::Keyboard => match f.s.bottom().editor().map(|e| &e.purpose) {
            Some(Purpose::FileName(FileName::Rename(_))) => editor::draw(f, "New name", "Rename"),
            Some(Purpose::FileName(FileName::NewFolder(_))) => editor::draw(f, "New folder's name", "Make"),
            Some(Purpose::FileName(FileName::Extract(_))) => editor::draw(f, "Extract to", "Extract"),
            Some(Purpose::AccountPassword(_)) => device::draw_password(f),
            _ => keyboard::draw(f, body),
        },
        Tab::Dock => dock::draw(f),
        Tab::Touchpad => touchpad::draw(f),
        Tab::Sysinfo => sysinfo::draw(f),
        Tab::Wifi => wifi::draw(f),
        Tab::Battery => battery::draw(f),
        Tab::Bluetooth => bluetooth::draw(f),
        Tab::Volume => levels::draw_volume(f),
        Tab::Brightness => levels::draw_brightness(f),
        Tab::Time => time::draw(f),
        Tab::Windows => windows(f, body),
    }
    // While pointing, the area below the bar is a touchpad and its contents are faded. The touchpad tab
    // has nothing to fade.
    if f.s.bottom().pointing() && f.s.bottom().tab() != Tab::Touchpad {
        f.c.fill(body, crate::gfx::Rgb(0).alpha(POINTING_FADE));
    }
    bar::draw_slider_over(f, screen, body.y);
    f.out.touchpad_area = Some(body);
    // Dimmed when untouched for a while and the pad is in use elsewhere, as the screens are before they
    // go off.
    if f.s.bottom().dimmed() {
        f.c.fill(screen, crate::gfx::Rgb(0).alpha(BOTTOM_DIM));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::gfx::images::Images;
    use crate::gfx::record::Recording;
    use crate::gfx::{Canvas, DrawCache};
    use crate::toolkit::{Scratch, Target};
    use clamshell_state::profiles::picker::{Kind, ProfileId};
    use clamshell_state::{Event, Id, Init, Reading, State, Status};

    /// Draws the second screen for `state` at `w` x `h`. Returns what can be tapped in it and what the
    /// frame reports.
    fn draw_bottom(state: &State, (w, h): (u32, u32), cache: &mut DrawCache) -> (Vec<Target>, crate::toolkit::FrameOut) {
        let mut rec = Recording::default();
        let mut targets = Vec::new();
        let mut canvas = Canvas::new(&mut rec, (w, h), 1.0, cache);
        let (mut images, mut scratch) = (Images::synchronous(), Scratch::default());
        let mut f = Frame::new(&mut canvas, state, &mut images, &mut scratch, &mut targets);
        bottom(&mut f, Rect::new(0.0, 0.0, w as f32, h as f32));
        let out = f.finish();
        (targets, out)
    }

    fn demo() -> State {
        let mut state = State::new(Init { library: clamshell_library::demo::library(), ..Init::default() });
        state.apply(Event::tick(1_700_000_000), &mut Vec::new());
        state.apply(
            Event::Reading(Reading::Status(Status { battery: Some(50), volume: Some(50), brightness: Some(50), wifi: Some(50), ..Status::default() })),
            &mut Vec::new(),
        );
        state
    }

    /// Whether two targets may overlap because one is drawn over the other on purpose: a list's area
    /// under its rows, a stick's click on its cap, and the mouse's wheel on its buttons.
    fn stacked(a: &Id, b: &Id) -> bool {
        // A list's area lies under its rows.
        if matches!(a, Id::List(_)) || matches!(b, Id::List(_)) {
            return true;
        }
        use clamshell_state::KeymapId::Input as K;
        use clamshell_state::keymap::{Button, Input, Side};
        // A stick's click sits on its cap.
        let click = |a: &Id, b: &Id| {
            matches!(
                (a, b),
                (Id::Keymap(K(Input::Stick(Side::Left))), Id::Keymap(K(Input::Button(Button::LS))))
                    | (Id::Keymap(K(Input::Stick(Side::Right))), Id::Keymap(K(Input::Button(Button::RS))))
            )
        };
        // The mouse's wheel and its turns sit on its buttons.
        use clamshell_state::Target;
        use clamshell_state::keymap::{MouseButton, Output};
        let button = |id: &Id| matches!(id, Id::Keymap(clamshell_state::KeymapId::Target(Target::To(Output::Mouse(MouseButton::Left | MouseButton::Right)))));
        let wheel = |id: &Id| {
            matches!(id, Id::Keymap(clamshell_state::KeymapId::Target(Target::To(Output::Mouse(MouseButton::Middle)) | Target::To(Output::Wheel(_)))))
        };
        click(a, b) || (button(a) && wheel(b)) || (wheel(a) && button(b)) || click(b, a)
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        let apart = a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5;
        !apart
    }

    /// The keymap screens one after another, as a user goes through them, at the desktop's window size,
    /// the Thor's screen and a narrow one. Targets stay within the area and none overlap except on purpose.
    #[test]
    fn the_keymap_screens_lay_out() {
        use clamshell_state::keymap::{Button, Input, Keymaps, Layer, Output, Param, Side};
        use clamshell_state::settings::Setting;
        use clamshell_state::{KeymapId, ListId, PointerEvent, SettingsId, Target};
        let tap = |id| Event::Pointer(PointerEvent::Activate(id));
        let km = |id| tap(Id::Keymap(id));
        let pick = |input| vec![km(KeymapId::Input(input))];
        let slide = |param, lift: bool| {
            let control = clamshell_state::Control::Param(param);
            let mut v =
                vec![Event::Slide(clamshell_state::Slide::Start { control, x: 100.0, width: 300.0 }), Event::Slide(clamshell_state::Slide::To { x: 160.0 })];
            if lift {
                v.push(Event::Slide(clamshell_state::Slide::End));
            }
            v
        };
        let screens: Vec<(&str, Vec<Event>)> = vec![
            ("picker", vec![tap(Id::Tab(Tab::Settings)), PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into()]),
            ("menu", vec![Event::Pointer(PointerEvent::LongPress(ListId::Keymaps.row(2)))]),
            ("name", vec![tap(Id::Profile(clamshell_state::profiles::picker::Kind::Keymap, clamshell_state::profiles::picker::ProfileId::Rename))]),
            (
                "delete",
                vec![
                    tap(Id::Editor(clamshell_state::EditorId::Cancel)),
                    tap(Id::Profile(Kind::Keymap, ProfileId::Back)),
                    tap(ListId::Keymaps.row(2)),
                    PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into(),
                    Event::Pointer(PointerEvent::LongPress(ListId::Keymaps.row(2))),
                    tap(Id::Profile(clamshell_state::profiles::picker::Kind::Keymap, clamshell_state::profiles::picker::ProfileId::Delete)),
                ],
            ),
            ("edit", vec![tap(Id::Profile(Kind::Keymap, ProfileId::Back)), tap(Id::Row { list: ListId::Keymaps, i: 2, part: clamshell_state::RowPart::Edit })]),
            ("edit, a button selected", vec![km(KeymapId::Input(Input::Button(Button::A)))]),
            ("the mouse", vec![km(KeymapId::Target(Target::MouseParts))]),
            (
                "capture",
                [vec![tap(Id::Profile(Kind::Keymap, ProfileId::Back))], pick(Input::Trigger(Side::Left)), vec![km(KeymapId::Target(Target::Key))]].concat(),
            ),
            (
                "edit, the gyro's panel",
                [vec![tap(Id::Profile(Kind::Keymap, ProfileId::Back))], pick(Input::Gyro), vec![km(KeymapId::Target(Target::To(Output::Pointer)))]].concat(),
            ),
            ("the gyro's hold", vec![km(KeymapId::Holds)]),
            (
                "edit, a stick's panel",
                [
                    vec![tap(Id::Profile(Kind::Keymap, ProfileId::Back))],
                    pick(Input::Stick(Side::Right)),
                    vec![km(KeymapId::Target(Target::To(Output::Pointer)))],
                ]
                .concat(),
            ),
            ("edit, a slider up", slide(Param::Speed, false)),
            (
                "edit, another layer",
                [slide(Param::Speed, true), vec![km(KeymapId::Layer(Layer(1))), km(KeymapId::Input(Input::Trigger(Side::Right)))]].concat(),
            ),
            ("edit, clearing", vec![km(KeymapId::Clear)]),
            (
                "edit, picking a control",
                [
                    vec![tap(Id::Profile(Kind::Keymap, ProfileId::Back)), km(KeymapId::Layer(Layer::BASE))],
                    pick(Input::Button(Button::B)),
                    vec![km(KeymapId::Target(Target::Pick))],
                ]
                .concat(),
            ),
        ];
        let mut cache = DrawCache::load();
        for (w, h) in [(620u32, 540u32), (1240, 1080), (300, 540)] {
            // The Nintendo layout with a layer added.
            let mut keymaps = Keymaps::initial();
            keymaps.get_mut(1).and_then(|k| k.add_layer("aim"));
            let mut state = State::new(Init { library: clamshell_library::demo::library(), keymaps, ..Init::default() });
            for (name, events) in &screens {
                for e in events {
                    state.apply(e.clone(), &mut Vec::new());
                }
                assert!(state.keymapping().screen().is_some() || state.bottom().editor().is_some(), "{name}: a keymap screen");
                let (targets, out) = draw_bottom(&state, (w, h), &mut cache);
                let body = out.touchpad_area.expect("the area below the bar");
                let case = format!("{name} at {w}x{h}");
                assert!(targets.iter().any(|t| matches!(t.id, Id::Keymap(_) | Id::Profile(..) | Id::Row { .. } | Id::Editor(_))), "{case}: its buttons");
                if name.starts_with("edit") {
                    // The panel, its buttons and rows lie between the label columns, and the label
                    // columns are clear of the clusters.
                    let l = keymaps::edit_layout(body);
                    for t in targets.iter().filter(|t| {
                        matches!(
                            t.id,
                            Id::Control(clamshell_state::Control::Param(_)) | Id::Keymap(KeymapId::Target(_) | KeymapId::Toggle(_) | KeymapId::Holds)
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
                    // Every input the Thor has can be touched, and on its screen there is room for a
                    // finger. The Thor has no back buttons.
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

    /// Lists longer than fit (networks, devices, apps playing and backlights) at the desktop's window
    /// size, the Thor's screen and a narrow one, at the top and scrolled part way. Rows are cut to the
    /// list's area, every target stays within the area below the bar, and none overlap except on purpose.
    #[test]
    fn long_lists_scroll_within_their_area() {
        use clamshell_state::levels::{Audio, Stream};
        use clamshell_state::radios::bluetooth::{Bluetooth, Device, Kind};
        use clamshell_state::radios::wifi::{Network, Wifi};
        use clamshell_state::{ListEvent, ListId};
        let mut cache = DrawCache::load();
        let readings = [
            Reading::Wifi(Wifi {
                present: true,
                on: true,
                networks: (0..30).map(|i| Network { ssid: format!("net {i}"), ..Network::default() }).collect(),
                ..Wifi::default()
            }),
            Reading::Bluetooth(Bluetooth {
                present: true,
                on: true,
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
                        state.apply(Event::Reading(r.clone()), &mut Vec::new());
                    }
                    state.apply(Event::Pointer(clamshell_state::PointerEvent::Activate(Id::Tab(tab))), &mut Vec::new());
                    let (_, out) = draw_bottom(&state, (w, h), &mut cache);
                    let visible = out.lists[list.index()].expect("the list reports its rows");
                    state.apply(ListEvent::Visible(list, visible).into(), &mut Vec::new());
                    state.apply(ListEvent::Scroll(list, scrolled).into(), &mut Vec::new());
                    let (targets, out) = draw_bottom(&state, (w, h), &mut cache);
                    let body = out.touchpad_area.expect("the area below the bar");
                    let case = format!("{tab:?} at {w}x{h} scrolled {scrolled}");
                    let area = targets.iter().find(|t| t.id == Id::List(list)).map(|t| t.rect).expect("the list's area");
                    assert!(
                        visible > 1.0 && (visible - area.h / targets.iter().find_map(|t| t.list).map_or(1.0, |l| l.row_h)).abs() < 1e-3,
                        "{case}: {visible}"
                    );
                    let rows: Vec<&Target> = targets.iter().filter(|t| clamshell_state::ListId::of(&t.id).is_some_and(|(l, _)| l == list)).collect();
                    assert!(!rows.is_empty(), "{case}: rows drawn");
                    let first = rows.iter().filter_map(|t| clamshell_state::ListId::of(&t.id)).map(|(_, i)| i).min();
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

    /// Every tab at the desktop's window size, the Thor's screen and a window dragged narrow, both empty
    /// and with readings. Targets have a size, stay on the screen (a tab's within its area below the
    /// bar), and don't overlap unless stacked on purpose. The bar shows all its buttons.
    #[test]
    fn every_tab_lays_out() {
        let mut cache = DrawCache::load();
        for with_data in [false, true] {
            for tab in Tab::ALL {
                for (w, h) in [(620u32, 540u32), (1240, 1080), (300, 540)] {
                    let mut state = if with_data { demo() } else { State::new(Init::default()) };
                    state.apply(Event::Pointer(clamshell_state::PointerEvent::Activate(Id::Tab(tab))), &mut Vec::new());
                    let (targets, out) = draw_bottom(&state, (w, h), &mut cache);
                    let screen = Rect::new(0.0, 0.0, w as f32, h as f32);
                    let body = out.touchpad_area.expect("the area below the bar");
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

    /// Pressing down from a tab's icon in the bar reaches what is right below it in the tab, including a
    /// control at the header's end (Wi-Fi's switch, the volume's slider), rather than the first list row.
    #[test]
    fn down_from_the_bar_reaches_the_tabs_first_line() {
        use crate::input::nav::{Dir, NavTarget, step};
        use clamshell_state::levels::{Audio, Level, Stream};
        use clamshell_state::radios::wifi::{Network, Wifi};
        use clamshell_state::{ListId, WifiId};
        let mut cache = DrawCache::load();
        let (w, h) = (1240u32, 1080u32);
        for (tab, first, second) in [
            (Tab::Wifi, Id::Wifi(WifiId::Power), ListId::Wifi.row(0)),
            (Tab::Volume, Id::Level(Level::Volume), Id::Row { list: ListId::Streams, i: 0, part: clamshell_state::nav::lists::RowPart::Level }),
        ] {
            let mut state = demo();
            let networks = (0..4).map(|i| Network { ssid: format!("net {i}"), ..Network::default() }).collect();
            state.apply(Event::Reading(Reading::Wifi(Wifi { present: true, on: true, networks, ..Wifi::default() })), &mut Vec::new());
            let streams = (0..3).map(|i| Stream { id: i, app: format!("app {i}"), title: String::new(), volume: 50, muted: false }).collect();
            state.apply(Event::Reading(Reading::Audio(Audio { outputs: vec![], streams })), &mut Vec::new());
            state.apply(Event::Pointer(clamshell_state::PointerEvent::Activate(Id::Tab(tab))), &mut Vec::new());
            let (targets, _) = draw_bottom(&state, (w, h), &mut cache);
            let all: Vec<NavTarget> = targets
                .iter()
                .filter(|t| !matches!(t.id, Id::List(_)))
                .map(|t| NavTarget {
                    screen: crate::screens::Screen::Bottom,
                    rect: Rect::new(t.rect.x / w as f32, 1.0 + t.rect.y / h as f32, t.rect.w / w as f32, t.rect.h / h as f32),
                    id: t.id.clone(),
                })
                .collect();
            let down = step(&all, Some(&bar::tab_target(tab)), Dir::Down);
            assert_eq!(down.as_ref(), Some(&first), "{tab:?}");
            assert_eq!(step(&all, down.as_ref(), Dir::Down).as_ref(), Some(&second), "{tab:?}: then the list");
        }
    }

    /// A press keys its list item from the frame it landed on, so a list that re-sorts between the frame
    /// and the press acts on the item that was drawn there.
    #[test]
    fn a_tap_acts_on_the_item_drawn_under_it() {
        use crate::input::{End, Input, Source};
        use clamshell_state::radios::bluetooth::{Bluetooth, Command, Device, Kind};
        use clamshell_state::{Effect, ListId, Tab};
        let device = |name: &str, connected| Device { address: format!("00:{name}"), name: name.into(), kind: Kind::Other, paired: true, connected };
        let bt = |devices| Event::Reading(Reading::Bluetooth(Bluetooth { present: true, on: true, devices, ..Bluetooth::default() }));
        let mut state = demo();
        state.apply(bt(vec![device("buds", false), device("pad", false)]), &mut Vec::new());
        state.apply(Event::Pointer(clamshell_state::PointerEvent::Activate(Id::Tab(Tab::Bluetooth))), &mut Vec::new());
        let (targets, _) = draw_bottom(&state, (1240, 1080), &mut DrawCache::load());
        let first = targets.iter().rev().find(|t| t.id == ListId::Bluetooth.row(0)).expect("the first device");
        // The pad connects and sorts first before the finger comes down where the buds were drawn.
        state.apply(bt(vec![device("pad", true), device("buds", false)]), &mut Vec::new());
        let (mut input, now) = (Input::default(), std::time::Instant::now());
        let mut actions = input.touch(Source::Touch(1), first, (first.rect.x + 1.0, first.rect.y + 1.0), now);
        actions.extend(input.up(Source::Touch(1), End::Release, &state, now));
        let mut effects = Vec::new();
        for action in actions {
            if let crate::input::Action::Emit(e) = action {
                state.apply(e, &mut effects);
            }
        }
        assert!(effects.contains(&Effect::Bluetooth(Command::Connect("00:buds".into()))), "{effects:?}");
        assert!(!effects.iter().any(|e| matches!(e, Effect::Bluetooth(Command::Disconnect(_)))), "{effects:?}");
    }
}
