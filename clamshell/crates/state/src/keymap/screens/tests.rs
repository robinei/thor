use super::*;
use crate::Compositor;
use crate::bottom::keyboard::{Modifier, find};
use crate::keymap::{Button, Dir, MouseButton, Side};
use crate::levels::slider::Control;
use crate::profiles::picker::Users;
use crate::settings::Setting;
use crate::testkit::*;
use crate::*;

fn key(s: &mut State, k: Key) -> Vec<Effect> {
    tap(s, Id::Key(find(k).unwrap()))
}

fn type_text(s: &mut State, text: &str) {
    for c in text.chars() {
        key(s, Key::Char(c));
    }
}

fn keymap(s: &mut State, id: KeymapId) -> Vec<Effect> {
    tap(s, Id::Keymap(id))
}

fn profile(s: &mut State, id: ProfileId) -> Vec<Effect> {
    tap(s, Id::Profile(ProfileKind::Keymap, id))
}

/// Slides the selected input's parameter to `value`: a finger is held on its row, moved and lifted.
fn slide(s: &mut State, param: Param, value: u8) -> Vec<Effect> {
    s.applied(Slide::Start { control: Control::Param(param), x: 300.0, width: 620.0 }.into());
    let slider = s.keymapping().edit().slider.unwrap();
    assert_eq!(
        slider.start,
        param.get(&s.keymapping().param_base(s.keymapping().screen().and_then(Screen::keymap).unwrap(), s.keymapping().edit().selected.unwrap()).unwrap())
    );
    let x = slider.x + (f32::from(value) - f32::from(slider.start)) / f32::from(slider.hi - slider.lo) * slider.length;
    let mut fx = s.applied(Slide::To { x }.into());
    fx.extend(s.applied(Slide::End.into()));
    fx
}

/// Selects an input on the edit screen, so that the ways in its panel remap it.
fn open(s: &mut State, input: Input) {
    if s.keymapping().edit().selected != Some(input) {
        keymap(s, KeymapId::Input(input));
    }
}

fn row(i: u16) -> Id {
    ListId::Keymaps.row(i)
}

fn picker(s: &mut State) {
    tap(s, Id::Tab(Tab::Settings));
    long(s, Id::Settings(SettingsId::Tile(Setting::Keymap)));
}

fn sent(fx: &[Effect]) -> Option<&Keymap> {
    fx.iter().find_map(|e| if let Effect::Keymap(k) = e { Some(k) } else { None })
}

#[test]
fn the_picker_chooses_a_keymap_for_the_level_edited() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::List)));
    assert_eq!(s.lists().len(ListId::Keymaps), 3, "the game's own (made from it), Default, Nintendo layout");
    tap(&mut s, row(2));
    assert_eq!(s.keymapping().screen(), None, "chosen: the picker closes");
    assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(1));
    assert_eq!(s.prefs().settings().users(Setting::Keymap, 1), Users { all_entries: false, entries: 1 });
    // Once the game runs, the keymap is sent to the pad, and only once.
    let fx = crate::testkit::start(&mut s, "zelda-lttp");
    assert_eq!(sent(&fx).map(|k| k.name.as_str()), Some("Nintendo layout"));
    assert_eq!(sent(&s.applied(Event::tick(5))), None);
    // Choosing Default for the game sends the pad an empty keymap.
    picker(&mut s);
    let fx = tap(&mut s, row(1));
    assert_eq!(sent(&fx).map(|k| (k.id, k.bindings().len())), Some((0, 0)));
    assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(0));
}

#[test]
fn modifiers_armed_on_the_capture_screen_go_when_its_tab_is_left() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    profile(&mut s, ProfileId::New);
    type_text(&mut s, "fps");
    tap(&mut s, Id::Editor(EditorId::Save));
    open(&mut s, Input::Button(Button::A));
    keymap(&mut s, KeymapId::Target(Target::Key));
    assert!(matches!(s.keymapping().screen(), Some(Screen::Capture { .. })));
    key(&mut s, Key::Mod(Modifier::Shift));
    assert!(s.keyboard().latch(Modifier::Shift).on());
    tap(&mut s, Id::Tab(Tab::Keyboard));
    assert!(!s.keyboard().latch(Modifier::Shift).on(), "not typed into an app");
    // Armed on the keyboard tab, they stay when another tab is chosen, the capture screen still open.
    key(&mut s, Key::Mod(Modifier::Shift));
    tap(&mut s, Id::Tab(Tab::Dock));
    assert!(s.keyboard().latch(Modifier::Shift).on());
}

#[test]
fn a_new_keymap_is_named_edited_and_mapped() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    profile(&mut s, ProfileId::New);
    assert!(matches!(s.bottom().editor(), Some(Editor { purpose: Purpose::ProfileName(ProfileKind::Keymap, ProfileName::New), .. })));
    type_text(&mut s, "fps");
    let fx = tap(&mut s, Id::Editor(EditorId::Save));
    assert!(matches!(fx.first(), Some(Effect::Save(Save::Keymaps(k))) if k.get(2).is_some_and(|k| k.name == "fps")), "{fx:?}");
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(2)));
    assert!(s.listening(), "the pad listens on the edit screen");
    assert_eq!(s.pad().route(), PadRoute::Listen);
    // Pressing A on the pad selects it. Then the keyboard opens and a key chord is typed: Ctrl, then x.
    s.applied(Event::Listened(Input::Button(Button::A)));
    assert_eq!((s.keymapping().screen(), s.keymapping().edit().selected), (Some(Screen::Edit(2)), Some(Input::Button(Button::A))));
    s.applied(Event::Listened(Input::Button(Button::A)));
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(2)), "pressed again, nothing to reopen yet");
    keymap(&mut s, KeymapId::Target(Target::Key));
    assert_eq!(s.keymapping().screen(), Some(Screen::Capture { id: 2, input: Input::Button(Button::A), keys: None }));
    assert_eq!(s.pad().route(), PadRoute::Ui, "the pad moves the focus there");
    key(&mut s, Key::Mod(Modifier::Ctrl));
    let fx = key(&mut s, Key::Char('x'));
    assert!(fx.iter().all(|e| !matches!(e, Effect::Compositor(Compositor::Key(_)))), "not typed into an app");
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(2)), "done: back to the inputs");
    let a = s.keymapping().keymaps().get(2).unwrap().get(Input::Button(Button::A)).copied();
    assert_eq!(a.map(|b| b.output), Some(Output::Key(Chord { ctrl: true, ..Chord::of(Key::Char('x')) })));
    // The right stick is mapped to the pointer, faster.
    open(&mut s, Input::Stick(Side::Right));
    keymap(&mut s, KeymapId::Target(Target::To(Output::Pointer)));

    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(2)), "chosen: its parameters are in the panel");
    slide(&mut s, Param::Speed, 80);
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Stick(Side::Right)).map(|b| (b.output, b.speed)), Some((Output::Pointer, 80)));
    // The left stick is mapped to four keys, typed in turn.
    open(&mut s, Input::Stick(Side::Left));
    keymap(&mut s, KeymapId::Target(Target::Keys));
    type_text(&mut s, "wasd");
    let w = |c| Chord::of(Key::Char(c));
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Stick(Side::Left)).map(|b| b.output), Some(Output::Keys([w('w'), w('a'), w('s'), w('d')])));
    // The gyro is mapped to the pointer while LT is held. Then LB gets Shift alone, by pressing Done with
    // only a modifier.
    open(&mut s, Input::Gyro);
    keymap(&mut s, KeymapId::Target(Target::To(Output::Pointer)));
    keymap(&mut s, KeymapId::Holds);
    assert_eq!(s.keymapping().screen(), Some(Screen::Holds { id: 2, input: Input::Gyro }));
    assert_eq!(s.pad().route(), PadRoute::Listen, "picked on the drawing: the pad listens");
    keymap(&mut s, KeymapId::Input(Input::Stick(Side::Right)));
    assert!(matches!(s.keymapping().screen(), Some(Screen::Holds { .. })), "a stick can't hold it: still picking");
    keymap(&mut s, KeymapId::Input(Input::Trigger(Side::Left)));
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Gyro).and_then(|b| b.hold), Some(Input::Trigger(Side::Left)));
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(2)));
    // The hold can be pressed on the pad instead, and then set back to always on.
    keymap(&mut s, KeymapId::Holds);
    s.applied(Event::Listened(Input::Button(Button::RB)));
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Gyro).and_then(|b| b.hold), Some(Input::Button(Button::RB)));
    keymap(&mut s, KeymapId::Holds);
    keymap(&mut s, KeymapId::Hold(None));
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Gyro).and_then(|b| b.hold), None);
    keymap(&mut s, KeymapId::Holds);
    keymap(&mut s, KeymapId::Hold(Some(Input::Trigger(Side::Left))));
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Target(Target::Key));
    key(&mut s, Key::Mod(Modifier::Shift));
    keymap(&mut s, KeymapId::Done);
    assert_eq!(
        s.keymapping().keymaps().get(2).unwrap().get(Input::Button(Button::LB)).map(|b| b.output),
        Some(Output::Key(Chord { shift: true, ..Chord::default() }))
    );
    assert!(!s.keyboard().latch(crate::bottom::keyboard::Modifier::Shift).on(), "used up");
    // Selecting A again opens the keyboard again for its key. Back gives up and changes nothing.
    keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
    keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
    assert!(matches!(s.keymapping().screen(), Some(Screen::Capture { .. })));
    profile(&mut s, ProfileId::Back);
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(2)));
    assert!(s.keymapping().keymaps().get(2).unwrap().get(Input::Button(Button::A)).is_some());
    // B is mapped to a pad button pressed on the pad (Y), then to one tapped on the drawing (X).
    open(&mut s, Input::Button(Button::B));
    keymap(&mut s, KeymapId::Target(Target::Pick));
    assert_eq!(s.keymapping().screen(), Some(Screen::Pick { id: 2, input: Input::Button(Button::B) }));
    assert_eq!(s.pad().route(), PadRoute::Listen);
    s.applied(Event::Listened(Input::Stick(Side::Left)));
    assert!(matches!(s.keymapping().screen(), Some(Screen::Pick { .. })), "a stick isn't a button: still picking");
    s.applied(Event::Listened(Input::Button(Button::Y)));
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Button(Button::B)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::Y))));
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(2)), "chosen: closed");
    keymap(&mut s, KeymapId::Target(Target::Pick));
    keymap(&mut s, KeymapId::Input(Input::Button(Button::X)));
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Button(Button::B)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::X))));
    // The trigger is mapped to a mouse button on the mouse's drawing, acting past 75 %.
    open(&mut s, Input::Trigger(Side::Right));
    keymap(&mut s, KeymapId::Target(Target::MouseParts));
    assert_eq!(s.keymapping().screen(), Some(Screen::Mouse { id: 2, input: Input::Trigger(Side::Right) }));
    keymap(&mut s, KeymapId::Target(Target::To(Output::Mouse(MouseButton::Left))));
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(2)));
    slide(&mut s, Param::Threshold, 75);
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Trigger(Side::Right)).map(|b| b.threshold), Some(75));
    // A long press on an input puts it back as on the pad. Home leaves the edit screen.
    long(&mut s, Id::Keymap(KeymapId::Input(Input::Button(Button::B))));
    assert_eq!(s.keymapping().keymaps().get(2).unwrap().get(Input::Button(Button::B)), None);
    s.applied(Event::Home);
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::List)));
    assert_eq!(s.pad().route(), PadRoute::Ui);
}

#[test]
fn picking_on_the_drawing_fits_the_input() {
    let (a, ls, rs, lt) = (Input::Button(Button::A), Input::Stick(Side::Left), Input::Stick(Side::Right), Input::Trigger(Side::Left));
    assert_eq!(picked(a, Input::Button(Button::B)), Some(Output::Pad(PadOut::Button(Button::B))));
    assert_eq!(picked(a, Input::Dpad(Dir::Up)), Some(Output::Pad(PadOut::Dpad(Dir::Up))));
    assert_eq!(picked(a, Input::Trigger(Side::Right)), Some(Output::Pad(PadOut::Trigger(Side::Right))));
    assert_eq!(picked(a, rs), None, "a button can't be a stick");
    assert_eq!(picked(lt, Input::Trigger(Side::Right)), Some(Output::Trigger(Side::Right)));
    assert_eq!(picked(ls, Input::StickDir(Side::Right, Dir::Up)), Some(Output::Stick(Side::Right)));
    assert_eq!(picked(ls, Input::Dpad(Dir::Left)), Some(Output::Dpad));
    assert_eq!(picked(ls, a), None);
    assert_eq!(picked(Input::Gyro, rs), Some(Output::Stick(Side::Right)));
    assert_eq!(picked(Input::Gyro, Input::Dpad(Dir::Up)), None);
    assert_eq!(way_of(Some(Output::Wheel(Dir::Up))), Target::MouseParts);
    assert_eq!(way_of(None), Target::Default);
}

#[test]
fn targets_offered_fit_the_input() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(2));
    picker(&mut s);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    open(&mut s, Input::Button(Button::A));
    assert_eq!(keymap(&mut s, KeymapId::Target(Target::To(Output::Pointer))), vec![], "a button can't move the pointer");
    keymap(&mut s, KeymapId::Target(Target::To(Output::Wheel(Dir::Down))));
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Wheel(Dir::Down)));
    open(&mut s, Input::Button(Button::A));
    keymap(&mut s, KeymapId::Target(Target::Default));
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(Input::Button(Button::A)), None);
}

#[test]
fn rename_duplicate_and_delete_with_games_using_it() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(2));
    tap(&mut s, Id::Settings(SettingsId::Scope(SettingsLevel::Global)));
    picker(&mut s);
    tap(&mut s, row(1));
    assert_eq!(s.prefs().settings().users(Setting::Keymap, 1), Users { all_entries: true, entries: 1 });
    // Rename.
    picker(&mut s);
    long(&mut s, row(1));
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::Menu(1))));
    profile(&mut s, ProfileId::Rename);
    key(&mut s, Key::Char('1'));
    tap(&mut s, Id::Editor(EditorId::Save));
    assert_eq!(s.keymapping().name(1), "Nintendo layout1");
    // Duplicate.
    long(&mut s, row(1));
    profile(&mut s, ProfileId::Duplicate);
    tap(&mut s, Id::Editor(EditorId::Save));
    assert_eq!(s.keymapping().keymaps().list().len(), 2);
    assert_eq!(s.keymapping().keymaps().get(2).map(|k| k.bindings().len()), Some(4));
    // A keymap no game uses is deleted at once.
    long(&mut s, row(2));
    profile(&mut s, ProfileId::Delete);
    assert_eq!((s.keymapping().keymaps().list().len(), s.keymapping().screen()), (1, Some(Screen::Picker(PickerScreen::List))));
    // A keymap in use asks first, and then everyone is on Default.
    long(&mut s, row(1));
    profile(&mut s, ProfileId::Delete);
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::Delete(1))));
    let fx = profile(&mut s, ProfileId::ConfirmDelete);
    assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Settings(_)))), "{fx:?}");
    assert!(s.keymapping().keymaps().list().is_empty());
    assert_eq!(s.prefs().settings().global.get(Setting::Keymap), 0);
    assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(0), "Default, its own choice");
    // The Default row has no menu.
    long(&mut s, row(0));
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::List)));
    profile(&mut s, ProfileId::Back);
    assert_eq!(s.keymapping().screen(), None);
}

#[test]
fn the_keymap_screens_stay_with_their_tab_and_listen_only_in_view() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    open(&mut s, Input::Button(Button::LB));
    assert_eq!(s.pad().route(), PadRoute::Listen);
    // On another tab the pad doesn't listen and its Back does nothing here. Back on the settings tab,
    // everything is as it was left.
    let fx = tap(&mut s, Id::Tab(Tab::Dock));
    assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Ui)]);
    assert_eq!(s.pad_back(), None);
    let fx = tap(&mut s, Id::Tab(Tab::Settings));
    assert!(fx.contains(&Effect::PadRoute(PadRoute::Listen)), "{fx:?}");
    assert_eq!((s.keymapping().screen(), s.keymapping().edit().selected), (Some(Screen::Edit(1)), Some(Input::Button(Button::LB))));
    // A game in front keeps the pad.
    crate::testkit::start(&mut s, "zelda-lttp");
    show_window(&mut s, "zelda-lttp", 7);
    assert_eq!(s.pad().route(), PadRoute::Game);
    s.applied(Event::Listened(Input::Button(Button::A)));
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(1)), "not listening: nothing");
}

/// Opens keymap 1's edit screen, with the pad listening.
fn edit_screen() -> State {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    assert_eq!(s.pad().route(), PadRoute::Listen);
    s
}

#[test]
fn the_panel_selects_changes_and_resets() {
    let mut s = edit_screen();
    let a = Input::Button(Button::A);
    assert_eq!(s.keymapping().edit(), EditView::default(), "Base, nothing selected");
    assert_eq!(keymap(&mut s, KeymapId::Target(Target::Key)), vec![], "nothing selected");
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(1)));
    keymap(&mut s, KeymapId::Input(a));
    assert_eq!((s.keymapping().screen(), s.keymapping().edit().selected), (Some(Screen::Edit(1)), Some(a)), "a touch selects");
    s.applied(Event::Listened(Input::Button(Button::B)));
    assert_eq!(s.keymapping().edit().selected, Some(Input::Button(Button::B)), "so does a press");
    s.applied(Event::Listened(a));
    keymap(&mut s, KeymapId::Target(Target::MouseParts));
    keymap(&mut s, KeymapId::Target(Target::To(Output::Mouse(MouseButton::Left))));
    assert_eq!((s.keymapping().screen(), s.keymapping().edit().selected), (Some(Screen::Edit(1)), Some(a)), "back with it still selected");
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(a).map(|b| b.output), Some(Output::Mouse(MouseButton::Left)));
    let fx = keymap(&mut s, KeymapId::Target(Target::Default));
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(a), None, "as on the pad");
    assert_eq!(keymap(&mut s, KeymapId::Target(Target::Default)), vec![], "already");
    // Only the ways that fit are offered: a button has no pointer and no four keys.
    assert_eq!(ways(Kind::Digital).iter().filter(|w| matches!(w, Target::To(Output::Pointer) | Target::Keys)).count(), 0);
    keymap(&mut s, KeymapId::Target(Target::Keys));
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(1)));
}

#[test]
fn layers_are_added_shown_edited_and_fall_through() {
    let mut s = edit_screen();
    let a = Input::Button(Button::A);
    // The + button adds a layer, which is named and then shown.
    let aim = add_layer(&mut s, "aim");
    assert_eq!((aim, s.keymapping().edit().layer), (Layer(1), Layer(1)));
    // A gets a key in the new layer, while Base keeps the Nintendo layout's B.
    open(&mut s, a);
    keymap(&mut s, KeymapId::Target(Target::Key));
    key(&mut s, Key::Char('y'));
    let k = s.keymapping().keymaps().get(1).unwrap();
    assert_eq!(k.get_in(aim, a).map(|b| b.output), Some(Output::Key(Chord::of(Key::Char('y')))));
    assert_eq!(k.get(a).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::B))));
    // X is picked as itself, which overrides Base's Y with X as on the pad. Resetting it falls back to
    // Base again.
    let x = Input::Button(Button::X);
    open(&mut s, x);
    keymap(&mut s, KeymapId::Target(Target::Pick));
    keymap(&mut s, KeymapId::Input(x));
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get_in(aim, x).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::X))));
    keymap(&mut s, KeymapId::Target(Target::Default));
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get_in(aim, x), None);
    // Back, an input like any other, is mapped in Base to the layer, from the list of layers.
    keymap(&mut s, KeymapId::Layer(Layer::BASE));
    open(&mut s, Input::Button(Button::Back));
    keymap(&mut s, KeymapId::Target(Target::Layers));
    assert_eq!(s.keymapping().screen(), Some(Screen::Layers { id: 1, input: Input::Button(Button::Back) }));
    keymap(&mut s, KeymapId::ToLayer(aim));
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(1)), "chosen: closed");
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(Input::Button(Button::Back)).map(|b| b.output), Some(Output::Layer(aim)));
    // A new layer made from the list is named, and the input maps to it at once, here on a hold.
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Actuation(Actuation::Hold));
    keymap(&mut s, KeymapId::Target(Target::Layers));
    keymap(&mut s, KeymapId::NewLayer);
    type_text(&mut s, "menu");
    tap(&mut s, Id::Editor(EditorId::Save));
    let k = s.keymapping().keymaps().get(1).unwrap();
    assert_eq!(k.get_of(Layer::BASE, Input::Button(Button::LB), Actuation::Hold).map(|b| b.output), Some(Output::Layer(Layer(2))));
    assert_eq!(k.layer_name(Layer(2)), Some("menu"));
    // A long press on a layer in the list opens its menu. The layer is renamed, then deleted along with
    // what maps to it.
    keymap(&mut s, KeymapId::LayerPicker);
    long(&mut s, Id::Keymap(KeymapId::Layer(Layer(2))));
    assert_eq!(s.keymapping().screen(), Some(Screen::LayerMenu { id: 1, layer: Layer(2) }));
    keymap(&mut s, KeymapId::RenameLayer);
    key(&mut s, Key::Char('s'));
    tap(&mut s, Id::Editor(EditorId::Save));
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().layer_name(Layer(2)), Some("menus"));
    assert_eq!(s.keymapping().screen(), Some(Screen::LayerPicker(1)), "renamed: the list");
    long(&mut s, Id::Keymap(KeymapId::Layer(Layer(2))));
    keymap(&mut s, KeymapId::DeleteLayer);
    let k = s.keymapping().keymaps().get(1).unwrap();
    assert_eq!((k.layer_name(Layer(2)), k.get_of(Layer::BASE, Input::Button(Button::LB), Actuation::Hold)), (None, None));
    // While a control is being picked, the pad's Back gives up.
    let mut s = edit_screen();
    open(&mut s, a);
    keymap(&mut s, KeymapId::Target(Target::Pick));
    assert_eq!(s.pad_back(), Some(Id::Profile(ProfileKind::Keymap, ProfileId::Back)), "picking: back");
}

/// Adds a layer named `name` with the edit screen's + button. The new layer is shown.
fn add_layer(s: &mut State, name: &str) -> Layer {
    keymap(s, KeymapId::LayerPicker);
    keymap(s, KeymapId::NewLayer);
    type_text(s, name);
    tap(s, Id::Editor(EditorId::Save));
    s.keymapping().edit().layer
}

#[test]
fn the_layer_lists_rows_show_choose_and_add_layers() {
    let mut s = edit_screen();
    let aim = add_layer(&mut s, "aim");
    let row = |i: u16, part| Id::Row { list: ListId::Layers, i, part };
    // The list for showing a layer has Base, aim and New layer. On another screen it is empty.
    profile(&mut s, ProfileId::Back);
    assert_eq!(s.lists().len(ListId::Layers), 0);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    keymap(&mut s, KeymapId::LayerPicker);
    assert_eq!((s.lists().len(ListId::Layers), s.keymapping().layer_row(1), s.keymapping().layer_row(2)), (3, Some(aim), None));
    tap(&mut s, row(1, RowPart::Main));
    assert_eq!((s.keymapping().screen(), s.keymapping().edit().layer), (Some(Screen::Edit(1)), aim));
    keymap(&mut s, KeymapId::LayerPicker);
    tap(&mut s, row(1, RowPart::Edit));
    assert_eq!(s.keymapping().screen(), Some(Screen::LayerMenu { id: 1, layer: aim }));
    profile(&mut s, ProfileId::Back);
    tap(&mut s, row(2, RowPart::Main));
    assert!(matches!(s.bottom().editor(), Some(Editor { purpose: Purpose::KeymapName(KeymapName::NewLayer(1, None)), .. })), "New layer");
    tap(&mut s, Id::Editor(EditorId::Cancel));
    // As a target, the list has aim and New layer but no Base, and a row chooses its layer.
    let mut s = edit_screen();
    let aim = add_layer(&mut s, "aim");
    keymap(&mut s, KeymapId::LayerPicker);
    tap(&mut s, row(0, RowPart::Main));
    open(&mut s, Input::Button(Button::Back));
    keymap(&mut s, KeymapId::Target(Target::Layers));
    assert_eq!((s.lists().len(ListId::Layers), s.keymapping().layer_row(0), s.keymapping().layer_row(1)), (2, Some(aim), None));
    tap(&mut s, row(0, RowPart::Main));
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(Input::Button(Button::Back)).map(|b| b.output), Some(Output::Layer(aim)));
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(1)), "chosen: closed");
}

#[test]
fn undo_goes_back_over_the_changes_since_the_edit_screen_opened() {
    let mut s = edit_screen();
    let start = s.keymapping().keymaps().get(1).unwrap().clone();
    assert!(!s.keymapping().can_undo());
    assert_eq!(keymap(&mut s, KeymapId::Undo), vec![], "nothing to undo");
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Target(Target::To(Output::Nothing)));
    let after_lb = s.keymapping().keymaps().get(1).unwrap().clone();
    let aim = add_layer(&mut s, "aim");
    assert_eq!(s.keymapping().edit().layer, aim);
    assert!(s.keymapping().can_undo());
    // Undoing the new layer removes it and shows Base. The next undo reverts LB's change.
    let fx = keymap(&mut s, KeymapId::Undo);
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert_eq!(s.keymapping().keymaps().get(1), Some(&after_lb));
    assert_eq!(s.keymapping().edit().layer, Layer::BASE);
    keymap(&mut s, KeymapId::Undo);
    assert_eq!(s.keymapping().keymaps().get(1), Some(&start));
    assert!(!s.keymapping().can_undo());
    // Opening the edit screen again starts Undo afresh.
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Target(Target::To(Output::Nothing)));
    profile(&mut s, ProfileId::Back);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    assert!(!s.keymapping().can_undo());
}

#[test]
fn a_layer_is_cleared_from_its_menu_once_confirmed() {
    let mut s = edit_screen();
    let aim = add_layer(&mut s, "aim");
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Target(Target::To(Output::Nothing)));
    // The layer button opens the list, and a long press on a layer opens its menu.
    keymap(&mut s, KeymapId::LayerPicker);
    assert_eq!(s.keymapping().screen(), Some(Screen::LayerPicker(1)));
    long(&mut s, Id::Keymap(KeymapId::Layer(aim)));
    assert_eq!(s.keymapping().screen(), Some(Screen::LayerMenu { id: 1, layer: aim }));
    // Once is switched on and off, and saved each time.
    let fx = keymap(&mut s, KeymapId::LayerOnce);
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert!(s.keymapping().keymaps().get(1).unwrap().layer_once(aim));
    keymap(&mut s, KeymapId::LayerOnce);
    assert!(!s.keymapping().keymaps().get(1).unwrap().layer_once(aim));
    keymap(&mut s, KeymapId::Clear);
    assert!(s.keymapping().edit().clearing, "asked first");
    // Back instead leaves the layer uncleared, with its menu still open.
    profile(&mut s, ProfileId::Back);
    assert_eq!((s.keymapping().screen(), s.keymapping().edit().clearing), (Some(Screen::LayerMenu { id: 1, layer: aim }), false));
    keymap(&mut s, KeymapId::Clear);
    let fx = keymap(&mut s, KeymapId::ConfirmClear);
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert_eq!(s.keymapping().screen(), Some(Screen::LayerPicker(1)), "back to the list");
    let k = s.keymapping().keymaps().get(1).unwrap();
    assert!(k.layer(aim).is_empty());
    assert_eq!(k.bindings().len(), 4, "Base kept");
    // Base can be cleared too, though its menu has no delete. The keymap is kept and behaves as Default.
    long(&mut s, Id::Keymap(KeymapId::Layer(Layer::BASE)));
    assert_eq!(keymap(&mut s, KeymapId::DeleteLayer), vec![], "Base stays");
    keymap(&mut s, KeymapId::Clear);
    keymap(&mut s, KeymapId::ConfirmClear);
    assert!(s.keymapping().keymaps().get(1).unwrap().bindings().is_empty());
    // Tapping a layer in the list shows it. A fresh edit screen starts on Base.
    tap(&mut s, Id::Keymap(KeymapId::Layer(aim)));
    assert_eq!((s.keymapping().screen(), s.keymapping().edit().layer), (Some(Screen::Edit(1)), aim));
    profile(&mut s, ProfileId::Back);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    assert_eq!(s.keymapping().edit(), EditView::default());
}

#[test]
fn parameters_slide_and_switch_whatever_the_mapping() {
    let mut s = edit_screen();
    let ls = Input::Stick(Side::Left);
    keymap(&mut s, KeymapId::Input(ls));
    // An input as on the pad can still be tuned. The slider changes it as it moves, and it is saved when
    // the finger lifts.
    s.applied(Slide::Start { control: Control::Param(Param::Deadzone), x: 300.0, width: 620.0 }.into());
    let slider = s.keymapping().edit().slider.unwrap();
    assert_eq!((slider.start, slider.control), (0, Control::Param(Param::Deadzone)));
    // Deadzone's range is 0-50, so about a quarter of the track is 12.
    assert_eq!((slider.lo, slider.hi), (0, 50));
    let fx = s.applied(Slide::To { x: 300.0 + 0.24 * slider.length }.into());
    assert!(fx.iter().all(|e| !matches!(e, Effect::Save(_))), "not saved while sliding: {fx:?}");
    let k = s.keymapping().keymaps().get(1).unwrap().get(ls).copied();
    assert_eq!(k.map(|b| (b.output, b.deadzone)), Some((Output::Stick(Side::Left), 12)));
    assert_eq!(s.keymapping().edit().slider.map(|s| s.value), Some(12));
    let fx = s.applied(Slide::End.into());
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert_eq!(s.keymapping().edit().slider, None);
    // The value is kept within its range, and a switch flips.
    slide(&mut s, Param::Deadzone, 90);
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(ls).map(|b| b.deadzone), Some(50));
    keymap(&mut s, KeymapId::Toggle(Param::InvertY));
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(ls).map(|b| b.invert_y), Some(true));
    // With everything back at its default, no binding is left in Base.
    keymap(&mut s, KeymapId::Toggle(Param::InvertY));
    slide(&mut s, Param::Deadzone, 0);
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(ls), None);
    // In another layer it starts from Base's binding and becomes that layer's own.
    slide(&mut s, Param::Full, 80);
    let aim = add_layer(&mut s, "aim");
    slide(&mut s, Param::Curve, 30);
    let k = s.keymapping().keymaps().get(1).unwrap();
    assert_eq!(k.get_in(aim, ls).map(|b| (b.full, b.curve)), Some((80, 30)));
    assert_eq!(k.get(ls).map(|b| (b.full, b.curve)), Some((80, 0)));
    // A parameter the input doesn't have gets no slider. Buttons have no parameters here.
    assert_eq!(slide_start(&mut s, Param::Threshold), None);
    keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
    assert_eq!(slide_start(&mut s, Param::Speed), None);
    // The bar's sliders are unchanged by it.
    s.applied(Slide::Start { control: Control::Volume, x: 300.0, width: 620.0 }.into());
    assert!(s.levels().slider().is_some() && s.keymapping().edit().slider.is_none());
    s.applied(Slide::End.into());
}

#[test]
fn a_trigger_full_pull_beside_its_pull() {
    let mut s = edit_screen();
    let rt = Input::Trigger(Side::Right);
    keymap(&mut s, KeymapId::Input(rt));
    keymap(&mut s, KeymapId::Actuation(Actuation::Hold));
    assert_eq!(s.keymapping().edit().act, Actuation::Press, "a trigger has no hold");
    keymap(&mut s, KeymapId::Actuation(Actuation::FullPull));
    assert_eq!(s.keymapping().edit().act, Actuation::FullPull);
    keymap(&mut s, KeymapId::Target(Target::MouseParts));
    keymap(&mut s, KeymapId::Target(Target::To(Output::Mouse(MouseButton::Left))));
    let k = s.keymapping().keymaps().get(1).unwrap();
    assert_eq!(k.get_of(Layer::BASE, rt, Actuation::FullPull).map(|b| (b.output, b.threshold)), Some((Output::Mouse(MouseButton::Left), 95)));
    assert_eq!(k.get(rt), None, "the pull as on the pad (analog)");
    // Picking the other trigger maps to that trigger pulled all the way.
    keymap(&mut s, KeymapId::Target(Target::Pick));
    keymap(&mut s, KeymapId::Input(Input::Trigger(Side::Left)));
    let full = s.keymapping().keymaps().get(1).unwrap().get_of(Layer::BASE, rt, Actuation::FullPull).map(|b| b.output);
    assert_eq!(full, Some(Output::Pad(PadOut::Trigger(Side::Left))));
}

#[test]
fn the_hold_and_double_tabs_edit_their_own_bindings() {
    let mut s = edit_screen();
    let a = Input::Button(Button::A);
    keymap(&mut s, KeymapId::Input(a));
    keymap(&mut s, KeymapId::Actuation(Actuation::Hold));
    assert_eq!(s.keymapping().edit().act, Actuation::Hold);
    assert_eq!(s.keymapping().param_base(1, a), None, "no hold yet: no parameters");
    keymap(&mut s, KeymapId::Target(Target::Key));
    key(&mut s, Key::Char('e'));
    let k = s.keymapping().keymaps().get(1).unwrap();
    assert_eq!(k.get_of(Layer::BASE, a, Actuation::Hold).map(|b| b.output), Some(Output::Key(Chord::of(Key::Char('e')))));
    assert_eq!(k.get(a).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::B))), "the press as it was");
    assert!(s.keymapping().press_waits(1, a));
    // The hold's time and turbo are set by slider, and toggle by switch.
    slide(&mut s, Param::HoldTime, 60);
    slide(&mut s, Param::Turbo, 12);
    keymap(&mut s, KeymapId::Toggle(Param::Toggle));
    let b = s.keymapping().keymaps().get(1).unwrap().get_of(Layer::BASE, a, Actuation::Hold).copied().unwrap();
    assert_eq!((b.hold_time, b.turbo, b.toggle), (60, 12, true));
    // Resetting on the Hold tab removes the hold and leaves the press untouched.
    keymap(&mut s, KeymapId::Target(Target::Default));
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get_of(Layer::BASE, a, Actuation::Hold), None);
    assert!(!s.keymapping().press_waits(1, a));
    // A stick has no tabs, and selecting another input starts on Press.
    keymap(&mut s, KeymapId::Actuation(Actuation::Double));
    keymap(&mut s, KeymapId::Input(Input::Stick(Side::Left)));
    assert_eq!(s.keymapping().edit().act, Actuation::Press);
    keymap(&mut s, KeymapId::Actuation(Actuation::Hold));
    assert_eq!(s.keymapping().edit().act, Actuation::Press, "not for a stick");
}

fn slide_start(s: &mut State, param: Param) -> Option<Slider> {
    s.applied(Slide::Start { control: Control::Param(param), x: 300.0, width: 620.0 }.into());
    let slider = s.keymapping().edit().slider;
    s.applied(Slide::End.into());
    slider
}

/// Making a game's own keymap saves the settings that choose it, then the keymaps.
#[test]
fn making_a_games_own_keymap_saves_the_settings_then_the_keymaps() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    let fx = tap(&mut s, row(0));
    let saves: Vec<&str> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::Save(Save::Settings(_)) => Some("settings"),
            Effect::Save(Save::Keymaps(_)) => Some("keymaps"),
            _ => None,
        })
        .collect();
    assert_eq!(saves, ["settings", "keymaps"], "{fx:?}");
}

#[test]
fn a_games_own_keymap_is_made_and_kept_while_another_is_chosen() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(2));
    // The game's own keymap is made from Nintendo layout, which is in force for it. It is chosen and its
    // edit screen opens.
    picker(&mut s);
    assert_eq!(s.panel().keymap_row(0), Some(PickerRow::Own(None)));
    let fx = tap(&mut s, row(0));
    assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))), "{fx:?}");
    let own = s.keymapping().keymaps().own_of("zelda-lttp").unwrap().id;
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(own)));
    assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(own));
    assert_eq!((s.keymapping().name(own), s.keymapping().keymaps().get(own).unwrap().bindings().len()), ("Own keymap", 4));
    assert_eq!(s.prefs().settings().users(Setting::Keymap, 1), Users::default(), "Nintendo layout no longer used");
    // With Default chosen, the game's own keymap stays and is still offered.
    profile(&mut s, ProfileId::Back);
    tap(&mut s, row(1));
    assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(0));
    picker(&mut s);
    assert_eq!(s.panel().keymap_row(0).map(|r| matches!(r, PickerRow::Own(Some(k)) if k == own)), Some(true));
    tap(&mut s, row(0));
    assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(own), "chosen again");
    // It isn't deleted: its menu resets it instead, loading Default into it on its edit screen, where
    // Undo puts it back.
    picker(&mut s);
    long(&mut s, row(0));
    profile(&mut s, ProfileId::Delete);
    assert!(s.keymapping().keymaps().own_of("zelda-lttp").is_some(), "never deleted");
    let before = s.keymapping().keymaps().own_of("zelda-lttp").map(|k| k.body.clone());
    profile(&mut s, ProfileId::Reset);
    assert_eq!(s.keymapping().keymaps().own_of("zelda-lttp").map(|k| k.body.clone()), Some(Default::default()), "Default's");
    assert_ne!(before, Some(Default::default()), "it differed from Default before");
    assert_eq!(s.panel().keymap_row(0), Some(PickerRow::Own(Some(own))));
}

#[test]
fn a_games_own_keymap_is_saved_as_a_new_named_one_or_over_one() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(2));
    picker(&mut s);
    tap(&mut s, row(0));
    let own = s.keymapping().keymaps().own_of("zelda-lttp").unwrap().id;
    keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
    keymap(&mut s, KeymapId::Target(Target::To(Output::Nothing)));
    profile(&mut s, ProfileId::Back);
    // Saving as a new keymap makes a named copy, and the game keeps its own.
    long(&mut s, row(0));
    profile(&mut s, ProfileId::SaveAs);
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::SaveAs(own))));
    assert_eq!((s.panel().keymap_row(0), s.lists().len(ListId::Keymaps)), (Some(PickerRow::New), 2), "a new one, Nintendo layout");
    tap(&mut s, row(0));
    type_text(&mut s, "zelda");
    tap(&mut s, Id::Editor(EditorId::Save));
    let copy = s.keymapping().keymaps().named().find(|k| k.name == "zelda").unwrap().id;
    assert_eq!(s.keymapping().keymaps().get(copy).unwrap().get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Nothing));
    assert_eq!(s.keymapping().keymaps().own_of("zelda-lttp").map(|k| k.id), Some(own), "kept");
    assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(own), "still chosen");
    // Saving over Nintendo layout asks first. Then Nintendo layout has the game's own bindings.
    long(&mut s, row(0));
    profile(&mut s, ProfileId::SaveAs);
    tap(&mut s, row(1));
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::Overwrite { own, to: 1 })));
    profile(&mut s, ProfileId::Back);
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::SaveAs(own))), "not after all");
    tap(&mut s, row(1));
    let fx = profile(&mut s, ProfileId::ConfirmSave);
    assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))), "{fx:?}");
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Nothing));
    assert_eq!(s.keymapping().name(1), "Nintendo layout", "its name kept");
    // Make own, on a shared keymap's edit screen, is only for a game without its own keymap.
    assert!(!s.panel().keymap_can_make_own());
}

#[test]
fn a_named_keymap_is_loaded_into_a_games_own_and_undo_goes_back() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(1));
    picker(&mut s);
    tap(&mut s, row(0));
    let own = s.keymapping().keymaps().own_of("zelda-lttp").unwrap().id;
    assert_eq!(s.keymapping().keymaps().get(own).unwrap().bindings().len(), 0, "from Default: empty");
    profile(&mut s, ProfileId::Back);
    long(&mut s, row(0));
    profile(&mut s, ProfileId::LoadFrom);
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::Load(own))));
    assert_eq!((s.panel().keymap_row(0), s.lists().len(ListId::Keymaps)), (Some(PickerRow::Default), 2), "Default, Nintendo layout");
    let fx = tap(&mut s, row(1));
    assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))), "{fx:?}");
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(own)));
    assert_eq!(s.keymapping().keymaps().get(own).unwrap().bindings().len(), 4, "Nintendo layout's");
    assert_eq!(s.keymapping().keymaps().get(1).unwrap().bindings().len(), 4, "left as it was");
    keymap(&mut s, KeymapId::Undo);
    assert_eq!(s.keymapping().keymaps().get(own).unwrap().bindings().len(), 0, "undone");
}

/// Calibrate in the gyro's panel calibrates it, with the notice to hold still.
#[test]
fn calibrate_on_the_edit_screen_calibrates_the_gyro() {
    let mut s = edit_screen();
    let fx = keymap(&mut s, KeymapId::Calibrate);
    assert!(fx.contains(&Effect::CalibrateGyro), "{fx:?}");
    assert_eq!(notice(&s), Some(crate::bottom::notice::NoticeKind::Gyro(None)));
}

/// Make own on a shared keymap's edit screen makes the game's own keymap from it, chosen for the game and
/// open for editing, the settings saved before the keymaps. On the game's own keymap it does nothing.
#[test]
fn make_own_on_a_shared_keymaps_edit_screen() {
    let mut s = edit_screen();
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(1)));
    let fx = keymap(&mut s, KeymapId::MakeOwn);
    let own = s.keymapping().keymaps().own_of("zelda-lttp").unwrap().id;
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(own)));
    assert_eq!(s.keymapping().keymaps().get(own).unwrap().bindings().len(), 4, "Nintendo layout's bindings");
    assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(own));
    let saves: Vec<&str> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::Save(Save::Settings(_)) => Some("settings"),
            Effect::Save(Save::Keymaps(_)) => Some("keymaps"),
            _ => None,
        })
        .collect();
    assert_eq!(saves, ["settings", "keymaps"], "{fx:?}");
    let count = s.keymapping().keymaps().list().len();
    assert_eq!(keymap(&mut s, KeymapId::MakeOwn), vec![]);
    assert_eq!(s.keymapping().keymaps().list().len(), count, "no second own keymap");
}

/// Back from a game's own keymap's Load goes to its menu, and from the pickers of a way to map an input
/// to the edit screen.
#[test]
fn back_goes_up_from_load_and_the_pickers() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(0));
    let own = s.keymapping().keymaps().own_of("zelda-lttp").unwrap().id;
    profile(&mut s, ProfileId::Back);
    long(&mut s, row(0));
    profile(&mut s, ProfileId::LoadFrom);
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::Load(own))));
    profile(&mut s, ProfileId::Back);
    assert_eq!(s.keymapping().screen(), Some(Screen::Picker(PickerScreen::Menu(own))));
    profile(&mut s, ProfileId::Edit);
    for (input, target) in [(Input::Button(Button::A), Target::Pick), (Input::Button(Button::A), Target::MouseParts)] {
        open(&mut s, input);
        keymap(&mut s, KeymapId::Target(target));
        assert_ne!(s.keymapping().screen(), Some(Screen::Edit(own)), "{target:?} opens its picker");
        profile(&mut s, ProfileId::Back);
        assert_eq!(s.keymapping().screen(), Some(Screen::Edit(own)), "back from {target:?}");
    }
    open(&mut s, Input::Gyro);
    keymap(&mut s, KeymapId::Target(Target::To(Output::Pointer)));
    keymap(&mut s, KeymapId::Holds);
    assert_eq!(s.keymapping().screen(), Some(Screen::Holds { id: own, input: Input::Gyro }));
    profile(&mut s, ProfileId::Back);
    assert_eq!(s.keymapping().screen(), Some(Screen::Edit(own)));
}

/// An empty name, or one of spaces, makes nothing and saves nothing; the editor closes on the screen it
/// came from.
#[test]
fn an_empty_name_makes_nothing() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    let count = s.keymapping().keymaps().list().len();
    profile(&mut s, ProfileId::New);
    type_text(&mut s, "  ");
    let fx = tap(&mut s, Id::Editor(EditorId::Save));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Save(_))), "{fx:?}");
    assert_eq!(s.keymapping().keymaps().list().len(), count);
    assert_eq!((s.keymapping().screen(), s.bottom().editor()), (Some(Screen::Picker(PickerScreen::List)), None));
}
