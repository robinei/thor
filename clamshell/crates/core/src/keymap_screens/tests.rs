use super::*;
use crate::keyboard::{Modifier, find};
use crate::keymap::{Button, Dir, MouseButton, Side};
use crate::slider::Control;
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

/// The selected input's parameter slid to `value` (a finger held on its row, moved, lifted).
fn slide(s: &mut State, param: Param, value: u8) -> Vec<Effect> {
    s.apply(Slide::Start { control: Control::Param(param), x: 300.0, width: 620.0 }.into());
    let slider = s.keymap_edit().slider.unwrap();
    assert_eq!(slider.start, param.get(&s.param_base(s.keymap_screen().and_then(Screen::keymap).unwrap(), s.keymap_edit().selected.unwrap()).unwrap()));
    let x = slider.x + (f32::from(value) - f32::from(slider.start)) / f32::from(slider.hi - slider.lo) * slider.length;
    let mut fx = s.apply(Slide::To { x }.into()).1;
    fx.extend(s.apply(Slide::End.into()).1);
    fx
}

/// An input selected on the edit screen (its panel's ways then remap it).
fn open(s: &mut State, input: Input) {
    if s.keymap_edit().selected != Some(input) {
        keymap(s, KeymapId::Input(input));
    }
}

fn row(i: u16) -> Id {
    ListId::Keymaps.row(i)
}

fn picker(s: &mut State) {
    tap(s, Id::Tab(Tab::Settings));
    tap(s, Id::Settings(SettingsId::Tile(Setting::Keymap)));
}

fn sent(fx: &[Effect]) -> Option<&Keymap> {
    fx.iter().find_map(|e| if let Effect::Keymap(k) = e { Some(k) } else { None })
}

#[test]
fn the_picker_chooses_a_keymap_for_the_scope_edited() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    assert_eq!(s.keymap_screen(), Some(Screen::List));
    assert_eq!(s.list_len(ListId::Keymaps), 3, "the game's own (made from it), Default, Nintendo layout");
    tap(&mut s, row(2));
    assert_eq!(s.keymap_screen(), None, "chosen: the picker closes");
    assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(1));
    assert_eq!(s.keymap_users(1), Users { all_games: false, games: 1 });
    // Running, it reaches the pad (once).
    let fx = s.apply(LaunchEvent::Started("zelda-lttp".into()).into()).1;
    assert_eq!(sent(&fx).map(|k| k.name.as_str()), Some("Nintendo layout"));
    assert_eq!(sent(&s.apply(Event::Tick { unix_secs: 5 }).1), None);
    // Default for it, and the pad gets an empty keymap.
    picker(&mut s);
    let fx = tap(&mut s, row(1));
    assert_eq!(sent(&fx).map(|k| (k.id, k.bindings().len())), Some((0, 0)));
    assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(0));
}

#[test]
fn a_new_keymap_is_named_edited_and_mapped() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    keymap(&mut s, KeymapId::New);
    assert!(matches!(s.bottom().editor(), Some(Editor { purpose: Purpose::KeymapName(KeymapName::New), .. })));
    type_text(&mut s, "fps");
    let fx = tap(&mut s, Id::Editor(EditorId::Save));
    assert!(matches!(fx.first(), Some(Effect::Save(Save::Keymaps(k))) if k.get(2).is_some_and(|k| k.name == "fps")), "{fx:?}");
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)));
    assert!(s.listening(), "the pad listens on the edit screen");
    assert_eq!(s.pad().route(), PadRoute::Listen);
    // A pressed on the pad: selected; the keyboard; a key chord typed: Ctrl, then x.
    s.apply(Event::Listened(Input::Button(Button::A)));
    assert_eq!((s.keymap_screen(), s.keymap_edit().selected), (Some(Screen::Edit(2)), Some(Input::Button(Button::A))));
    s.apply(Event::Listened(Input::Button(Button::A)));
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)), "pressed again, nothing to reopen yet");
    keymap(&mut s, KeymapId::Target(Target::Key));
    assert_eq!(s.keymap_screen(), Some(Screen::Capture { id: 2, input: Input::Button(Button::A), keys: None }));
    assert_eq!(s.pad().route(), PadRoute::Ui, "the pad moves the focus there");
    key(&mut s, Key::Mod(Modifier::Ctrl));
    let fx = key(&mut s, Key::Char('x'));
    assert!(fx.iter().all(|e| !matches!(e, Effect::Key(_))), "not typed into an app");
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)), "done: back to the inputs");
    let a = s.keymaps().get(2).unwrap().get(Input::Button(Button::A)).copied();
    assert_eq!(a.map(|b| b.output), Some(Output::Key(Chord { ctrl: true, ..Chord::of(Key::Char('x')) })));
    // The right stick to the pointer, faster.
    open(&mut s, Input::Stick(Side::Right));
    keymap(&mut s, KeymapId::Target(Target::Pointer));

    assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)), "chosen: its parameters are in the panel");
    slide(&mut s, Param::Speed, 80);
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Stick(Side::Right)).map(|b| (b.output, b.speed)), Some((Output::Pointer, 80)));
    // The left stick to four keys, in turn.
    open(&mut s, Input::Stick(Side::Left));
    keymap(&mut s, KeymapId::Target(Target::Keys));
    type_text(&mut s, "wasd");
    let w = |c| Chord::of(Key::Char(c));
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Stick(Side::Left)).map(|b| b.output), Some(Output::Keys([w('w'), w('a'), w('s'), w('d')])));
    // The gyro to the pointer while LT is held; Shift alone on LB (Done with only a modifier).
    open(&mut s, Input::Gyro);
    keymap(&mut s, KeymapId::Target(Target::Pointer));
    keymap(&mut s, KeymapId::Holds);
    assert_eq!(s.keymap_screen(), Some(Screen::Holds { id: 2, input: Input::Gyro }));
    assert_eq!(s.pad().route(), PadRoute::Listen, "picked on the drawing: the pad listens");
    keymap(&mut s, KeymapId::Input(Input::Stick(Side::Right)));
    assert!(matches!(s.keymap_screen(), Some(Screen::Holds { .. })), "a stick can't hold it: still picking");
    keymap(&mut s, KeymapId::Input(Input::Trigger(Side::Left)));
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Gyro).and_then(|b| b.hold), Some(Input::Trigger(Side::Left)));
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)));
    // Pressed on the pad instead; then always on.
    keymap(&mut s, KeymapId::Holds);
    s.apply(Event::Listened(Input::Button(Button::RB)));
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Gyro).and_then(|b| b.hold), Some(Input::Button(Button::RB)));
    keymap(&mut s, KeymapId::Holds);
    keymap(&mut s, KeymapId::Hold(None));
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Gyro).and_then(|b| b.hold), None);
    keymap(&mut s, KeymapId::Holds);
    keymap(&mut s, KeymapId::Hold(Some(Input::Trigger(Side::Left))));
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Target(Target::Key));
    key(&mut s, Key::Mod(Modifier::Shift));
    keymap(&mut s, KeymapId::Done);
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::LB)).map(|b| b.output), Some(Output::Key(Chord { shift: true, ..Chord::default() })));
    assert!(!s.keyboard().shift.on(), "used up");
    // A pressed again: the keyboard again, for the key it has; Back gives up, nothing changed.
    keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
    keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
    assert!(matches!(s.keymap_screen(), Some(Screen::Capture { .. })));
    keymap(&mut s, KeymapId::Back);
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)));
    assert!(s.keymaps().get(2).unwrap().get(Input::Button(Button::A)).is_some());
    // B to a pad button pressed on the pad (Y); then to one tapped on the drawing (X).
    open(&mut s, Input::Button(Button::B));
    keymap(&mut s, KeymapId::Target(Target::Pick));
    assert_eq!(s.keymap_screen(), Some(Screen::Pick { id: 2, input: Input::Button(Button::B) }));
    assert_eq!(s.pad().route(), PadRoute::Listen);
    s.apply(Event::Listened(Input::Stick(Side::Left)));
    assert!(matches!(s.keymap_screen(), Some(Screen::Pick { .. })), "a stick isn't a button: still picking");
    s.apply(Event::Listened(Input::Button(Button::Y)));
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::B)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::Y))));
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)), "chosen: closed");
    keymap(&mut s, KeymapId::Target(Target::Pick));
    keymap(&mut s, KeymapId::Input(Input::Button(Button::X)));
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::B)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::X))));
    // The trigger to a mouse button on the mouse's drawing, past 75 %.
    open(&mut s, Input::Trigger(Side::Right));
    keymap(&mut s, KeymapId::Target(Target::MouseParts));
    assert_eq!(s.keymap_screen(), Some(Screen::Mouse { id: 2, input: Input::Trigger(Side::Right) }));
    keymap(&mut s, KeymapId::Target(Target::Mouse(MouseButton::Left)));
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)));
    slide(&mut s, Param::Threshold, 75);
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Trigger(Side::Right)).map(|b| b.threshold), Some(75));
    // A long press on an input: as on the pad again. Home leaves the edit screen.
    long(&mut s, Id::Keymap(KeymapId::Input(Input::Button(Button::B))));
    assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::B)), None);
    s.apply(Event::Home);
    assert_eq!(s.keymap_screen(), Some(Screen::List));
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
    assert_eq!(keymap(&mut s, KeymapId::Target(Target::Pointer)), vec![], "a button can't move the pointer");
    keymap(&mut s, KeymapId::Target(Target::Wheel(Dir::Down)));
    assert_eq!(s.keymaps().get(1).unwrap().get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Wheel(Dir::Down)));
    open(&mut s, Input::Button(Button::A));
    keymap(&mut s, KeymapId::Target(Target::Default));
    assert_eq!(s.keymaps().get(1).unwrap().get(Input::Button(Button::A)), None);
}

#[test]
fn rename_duplicate_and_delete_with_games_using_it() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(2));
    tap(&mut s, Id::Settings(SettingsId::Scope(Scope::Global)));
    picker(&mut s);
    tap(&mut s, row(1));
    assert_eq!(s.keymap_users(1), Users { all_games: true, games: 1 });
    // Rename.
    picker(&mut s);
    long(&mut s, row(1));
    assert_eq!(s.keymap_screen(), Some(Screen::Menu(1)));
    keymap(&mut s, KeymapId::Rename);
    key(&mut s, Key::Char('1'));
    tap(&mut s, Id::Editor(EditorId::Save));
    assert_eq!(s.keymap_name(1), "Nintendo layout1");
    // Duplicate.
    long(&mut s, row(1));
    keymap(&mut s, KeymapId::Duplicate);
    tap(&mut s, Id::Editor(EditorId::Save));
    assert_eq!(s.keymaps().list().len(), 2);
    assert_eq!(s.keymaps().get(2).map(|k| k.bindings().len()), Some(4));
    // Not used: deleted at once.
    long(&mut s, row(2));
    keymap(&mut s, KeymapId::Delete);
    assert_eq!((s.keymaps().list().len(), s.keymap_screen()), (1, Some(Screen::List)));
    // In use: asked first; then everyone is on Default.
    long(&mut s, row(1));
    keymap(&mut s, KeymapId::Delete);
    assert_eq!(s.keymap_screen(), Some(Screen::Delete(1)));
    let fx = keymap(&mut s, KeymapId::ConfirmDelete);
    assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Settings(_)))), "{fx:?}");
    assert!(s.keymaps().list().is_empty());
    assert_eq!(s.settings().global.get(Setting::Keymap), 0);
    assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), None, "Default, as all games");
    // The Default row has no menu.
    long(&mut s, row(0));
    assert_eq!(s.keymap_screen(), Some(Screen::List));
    keymap(&mut s, KeymapId::Back);
    assert_eq!(s.keymap_screen(), None);
}

#[test]
fn the_keymap_screens_stay_with_their_tab_and_listen_only_in_view() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    open(&mut s, Input::Button(Button::LB));
    assert_eq!(s.pad().route(), PadRoute::Listen);
    // Another tab: not listening, nothing for the pad's Back; back to the settings: as left.
    let fx = tap(&mut s, Id::Tab(Tab::Dock));
    assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Ui)]);
    assert_eq!(s.pad_back(), None);
    let fx = tap(&mut s, Id::Tab(Tab::Settings));
    assert!(fx.contains(&Effect::PadRoute(PadRoute::Listen)), "{fx:?}");
    assert_eq!((s.keymap_screen(), s.keymap_edit().selected), (Some(Screen::Edit(1)), Some(Input::Button(Button::LB))));
    // A game in front keeps the pad.
    s.apply(LaunchEvent::Started("zelda-lttp".into()).into());
    assert_eq!(s.pad().route(), PadRoute::Game);
    s.apply(Event::Listened(Input::Button(Button::A)));
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(1)), "not listening: nothing");
}

/// Keymap 1's edit screen, the pad listening.
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
    assert_eq!(s.keymap_edit(), EditView::default(), "the main layer, nothing selected");
    assert_eq!(keymap(&mut s, KeymapId::Target(Target::Key)), vec![], "nothing selected");
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(1)));
    keymap(&mut s, KeymapId::Input(a));
    assert_eq!((s.keymap_screen(), s.keymap_edit().selected), (Some(Screen::Edit(1)), Some(a)), "a touch selects");
    s.apply(Event::Listened(Input::Button(Button::B)));
    assert_eq!(s.keymap_edit().selected, Some(Input::Button(Button::B)), "so does a press");
    s.apply(Event::Listened(a));
    keymap(&mut s, KeymapId::Target(Target::MouseParts));
    keymap(&mut s, KeymapId::Target(Target::Mouse(MouseButton::Left)));
    assert_eq!((s.keymap_screen(), s.keymap_edit().selected), (Some(Screen::Edit(1)), Some(a)), "back with it still selected");
    assert_eq!(s.keymaps().get(1).unwrap().get(a).map(|b| b.output), Some(Output::Mouse(MouseButton::Left)));
    let fx = keymap(&mut s, KeymapId::Target(Target::Default));
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert_eq!(s.keymaps().get(1).unwrap().get(a), None, "as on the pad");
    assert_eq!(keymap(&mut s, KeymapId::Target(Target::Default)), vec![], "already");
    // Only the ways that fit: a button has no pointer, nor four keys.
    assert_eq!(ways(Kind::Digital).iter().filter(|w| matches!(w, Target::Pointer | Target::Keys)).count(), 0);
    keymap(&mut s, KeymapId::Target(Target::Keys));
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(1)));
}

#[test]
fn layers_are_added_shown_edited_and_fall_through() {
    let mut s = edit_screen();
    let a = Input::Button(Button::A);
    // The +: named, then shown.
    let aim = add_layer(&mut s, "aim");
    assert_eq!((aim, s.keymap_edit().layer), (Layer(1), Layer(1)));
    // A in it: a key; Base keeps the Nintendo layout's B.
    open(&mut s, a);
    keymap(&mut s, KeymapId::Target(Target::Key));
    key(&mut s, Key::Char('y'));
    let k = s.keymaps().get(1).unwrap();
    assert_eq!(k.get_in(aim, a).map(|b| b.output), Some(Output::Key(Chord::of(Key::Char('y')))));
    assert_eq!(k.get(a).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::B))));
    // X as on the pad (picked: itself), over Base's Y; reset: as Base again.
    let x = Input::Button(Button::X);
    open(&mut s, x);
    keymap(&mut s, KeymapId::Target(Target::Pick));
    keymap(&mut s, KeymapId::Input(x));
    assert_eq!(s.keymaps().get(1).unwrap().get_in(aim, x).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::X))));
    keymap(&mut s, KeymapId::Target(Target::Default));
    assert_eq!(s.keymaps().get(1).unwrap().get_in(aim, x), None);
    // Back (an input like any other) on Base to the layer, from the layers' list.
    keymap(&mut s, KeymapId::Layer(Layer::BASE));
    open(&mut s, Input::Button(Button::Back));
    keymap(&mut s, KeymapId::Target(Target::Layers));
    assert_eq!(s.keymap_screen(), Some(Screen::Layers { id: 1, input: Input::Button(Button::Back) }));
    keymap(&mut s, KeymapId::ToLayer(aim));
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(1)), "chosen: closed");
    assert_eq!(s.keymaps().get(1).unwrap().get(Input::Button(Button::Back)).map(|b| b.output), Some(Output::Layer(aim)));
    // A new layer from the list: named, and the input maps to it at once (held long here).
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Actuation(Actuation::Hold));
    keymap(&mut s, KeymapId::Target(Target::Layers));
    keymap(&mut s, KeymapId::NewLayer);
    type_text(&mut s, "menu");
    tap(&mut s, Id::Editor(EditorId::Save));
    let k = s.keymaps().get(1).unwrap();
    assert_eq!(k.get_of(Layer::BASE, Input::Button(Button::LB), Actuation::Hold).map(|b| b.output), Some(Output::Layer(Layer(2))));
    assert_eq!(k.layer_name(Layer(2)), Some("menu"));
    // A long press on a layer in the list: its menu; renamed, then deleted (with what maps to it).
    keymap(&mut s, KeymapId::LayerPicker);
    long(&mut s, Id::Keymap(KeymapId::Layer(Layer(2))));
    assert_eq!(s.keymap_screen(), Some(Screen::LayerMenu { id: 1, layer: Layer(2) }));
    keymap(&mut s, KeymapId::RenameLayer);
    key(&mut s, Key::Char('s'));
    tap(&mut s, Id::Editor(EditorId::Save));
    assert_eq!(s.keymaps().get(1).unwrap().layer_name(Layer(2)), Some("menus"));
    assert_eq!(s.keymap_screen(), Some(Screen::LayerPicker(1)), "renamed: the list");
    long(&mut s, Id::Keymap(KeymapId::Layer(Layer(2))));
    keymap(&mut s, KeymapId::DeleteLayer);
    let k = s.keymaps().get(1).unwrap();
    assert_eq!((k.layer_name(Layer(2)), k.get_of(Layer::BASE, Input::Button(Button::LB), Actuation::Hold)), (None, None));
    // Picking a control, the pad's Back gives up.
    let mut s = edit_screen();
    open(&mut s, a);
    keymap(&mut s, KeymapId::Target(Target::Pick));
    assert_eq!(s.pad_back(), Some(Id::Keymap(KeymapId::Back)), "picking: back");
}

/// A layer added with the edit screen's +, named `name`; it is shown.
fn add_layer(s: &mut State, name: &str) -> Layer {
    keymap(s, KeymapId::LayerPicker);
    keymap(s, KeymapId::NewLayer);
    type_text(s, name);
    tap(s, Id::Editor(EditorId::Save));
    s.keymap_edit().layer
}

#[test]
fn the_layer_lists_rows_show_choose_and_add_layers() {
    let mut s = edit_screen();
    let aim = add_layer(&mut s, "aim");
    let row = |i: u16, part| Id::Row { list: ListId::Layers, i, part };
    // The list to show one: Base, aim, New layer; nothing on another screen.
    keymap(&mut s, KeymapId::Back);
    assert_eq!(s.list_len(ListId::Layers), 0);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    keymap(&mut s, KeymapId::LayerPicker);
    assert_eq!((s.list_len(ListId::Layers), s.layer_row(1), s.layer_row(2)), (3, Some(aim), None));
    tap(&mut s, row(1, RowPart::Main));
    assert_eq!((s.keymap_screen(), s.keymap_edit().layer), (Some(Screen::Edit(1)), aim));
    keymap(&mut s, KeymapId::LayerPicker);
    tap(&mut s, row(1, RowPart::Edit));
    assert_eq!(s.keymap_screen(), Some(Screen::LayerMenu { id: 1, layer: aim }));
    keymap(&mut s, KeymapId::Back);
    tap(&mut s, row(2, RowPart::Main));
    assert!(matches!(s.bottom().editor(), Some(Editor { purpose: Purpose::KeymapName(KeymapName::NewLayer(1, None)), .. })), "New layer");
    tap(&mut s, Id::Editor(EditorId::Cancel));
    // As a target: aim, New layer (no Base); a row chooses it.
    let mut s = edit_screen();
    let aim = add_layer(&mut s, "aim");
    keymap(&mut s, KeymapId::LayerPicker);
    tap(&mut s, row(0, RowPart::Main));
    open(&mut s, Input::Button(Button::Back));
    keymap(&mut s, KeymapId::Target(Target::Layers));
    assert_eq!((s.list_len(ListId::Layers), s.layer_row(0), s.layer_row(1)), (2, Some(aim), None));
    tap(&mut s, row(0, RowPart::Main));
    assert_eq!(s.keymaps().get(1).unwrap().get(Input::Button(Button::Back)).map(|b| b.output), Some(Output::Layer(aim)));
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(1)), "chosen: closed");
}

#[test]
fn undo_goes_back_over_the_changes_since_the_edit_screen_opened() {
    let mut s = edit_screen();
    let start = s.keymaps().get(1).unwrap().clone();
    assert!(!s.can_undo());
    assert_eq!(keymap(&mut s, KeymapId::Undo), vec![], "nothing to undo");
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Target(Target::Nothing));
    let after_lb = s.keymaps().get(1).unwrap().clone();
    let aim = add_layer(&mut s, "aim");
    assert_eq!(s.keymap_edit().layer, aim);
    assert!(s.can_undo());
    // The layer's adding undone: gone, Base shown; then LB's.
    let fx = keymap(&mut s, KeymapId::Undo);
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert_eq!(s.keymaps().get(1), Some(&after_lb));
    assert_eq!(s.keymap_edit().layer, Layer::BASE);
    keymap(&mut s, KeymapId::Undo);
    assert_eq!(s.keymaps().get(1), Some(&start));
    assert!(!s.can_undo());
    // Opened again: a fresh start.
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Target(Target::Nothing));
    keymap(&mut s, KeymapId::Back);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    assert!(!s.can_undo());
}

#[test]
fn a_layer_is_cleared_from_its_menu_once_confirmed() {
    let mut s = edit_screen();
    let aim = add_layer(&mut s, "aim");
    open(&mut s, Input::Button(Button::LB));
    keymap(&mut s, KeymapId::Target(Target::Nothing));
    // The layer button: the list; a long press on a layer: its menu.
    keymap(&mut s, KeymapId::LayerPicker);
    assert_eq!(s.keymap_screen(), Some(Screen::LayerPicker(1)));
    long(&mut s, Id::Keymap(KeymapId::Layer(aim)));
    assert_eq!(s.keymap_screen(), Some(Screen::LayerMenu { id: 1, layer: aim }));
    // Once, switched on and off (saved).
    let fx = keymap(&mut s, KeymapId::LayerOnce);
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert!(s.keymaps().get(1).unwrap().layer_once(aim));
    keymap(&mut s, KeymapId::LayerOnce);
    assert!(!s.keymaps().get(1).unwrap().layer_once(aim));
    keymap(&mut s, KeymapId::Clear);
    assert!(s.keymap_edit().clearing, "asked first");
    // Back instead: not cleared, still its menu.
    keymap(&mut s, KeymapId::Back);
    assert_eq!((s.keymap_screen(), s.keymap_edit().clearing), (Some(Screen::LayerMenu { id: 1, layer: aim }), false));
    keymap(&mut s, KeymapId::Clear);
    let fx = keymap(&mut s, KeymapId::ConfirmClear);
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert_eq!(s.keymap_screen(), Some(Screen::LayerPicker(1)), "back to the list");
    let k = s.keymaps().get(1).unwrap();
    assert!(k.layer(aim).is_empty());
    assert_eq!(k.bindings().len(), 4, "Base kept");
    // Base cleared too (its menu has no delete): Default's behaviour, the keymap kept.
    long(&mut s, Id::Keymap(KeymapId::Layer(Layer::BASE)));
    assert_eq!(keymap(&mut s, KeymapId::DeleteLayer), vec![], "Base stays");
    keymap(&mut s, KeymapId::Clear);
    keymap(&mut s, KeymapId::ConfirmClear);
    assert!(s.keymaps().get(1).unwrap().bindings().is_empty());
    // A layer tapped in the list: shown. A fresh edit screen starts on Base.
    tap(&mut s, Id::Keymap(KeymapId::Layer(aim)));
    assert_eq!((s.keymap_screen(), s.keymap_edit().layer), (Some(Screen::Edit(1)), aim));
    keymap(&mut s, KeymapId::Back);
    tap(&mut s, Id::Row { list: ListId::Keymaps, i: 2, part: RowPart::Edit });
    assert_eq!(s.keymap_edit(), EditView::default());
}

#[test]
fn parameters_slide_and_switch_whatever_the_mapping() {
    let mut s = edit_screen();
    let ls = Input::Stick(Side::Left);
    keymap(&mut s, KeymapId::Input(ls));
    // As on the pad: tuned all the same; the slider changes it as it goes, saved on lifting.
    s.apply(Slide::Start { control: Control::Param(Param::Deadzone), x: 300.0, width: 620.0 }.into());
    let slider = s.keymap_edit().slider.unwrap();
    assert_eq!((slider.start, slider.control), (0, Control::Param(Param::Deadzone)));
    // (Deadzone's range is 0-50: a quarter of the track is 12.)
    assert_eq!((slider.lo, slider.hi), (0, 50));
    let (_, fx) = s.apply(Slide::To { x: 300.0 + 0.24 * slider.length }.into());
    assert!(fx.iter().all(|e| !matches!(e, Effect::Save(_))), "not saved while sliding: {fx:?}");
    let k = s.keymaps().get(1).unwrap().get(ls).copied();
    assert_eq!(k.map(|b| (b.output, b.deadzone)), Some((Output::Stick(Side::Left), 12)));
    assert_eq!(s.keymap_edit().slider.map(|s| s.value), Some(12));
    let (_, fx) = s.apply(Slide::End.into());
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
    assert_eq!(s.keymap_edit().slider, None);
    // Kept within its range; a switch flips.
    slide(&mut s, Param::Deadzone, 90);
    assert_eq!(s.keymaps().get(1).unwrap().get(ls).map(|b| b.deadzone), Some(50));
    keymap(&mut s, KeymapId::Toggle(Param::InvertY));
    assert_eq!(s.keymaps().get(1).unwrap().get(ls).map(|b| b.invert_y), Some(true));
    // Back to the defaults: no binding left in the main layer.
    keymap(&mut s, KeymapId::Toggle(Param::InvertY));
    slide(&mut s, Param::Deadzone, 0);
    assert_eq!(s.keymaps().get(1).unwrap().get(ls), None);
    // In the Back layer it starts from the main layer's and is the Back layer's own.
    slide(&mut s, Param::Full, 80);
    let aim = add_layer(&mut s, "aim");
    slide(&mut s, Param::Curve, 30);
    let k = s.keymaps().get(1).unwrap();
    assert_eq!(k.get_in(aim, ls).map(|b| (b.full, b.curve)), Some((80, 30)));
    assert_eq!(k.get(ls).map(|b| (b.full, b.curve)), Some((80, 0)));
    // A parameter the input hasn't: no slider. Buttons have none.
    assert_eq!(slide_start(&mut s, Param::Threshold), None);
    keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
    assert_eq!(slide_start(&mut s, Param::Speed), None);
    // The bar's sliders are unchanged by it.
    s.apply(Slide::Start { control: Control::Volume, x: 300.0, width: 620.0 }.into());
    assert!(s.levels().slider().is_some() && s.keymap_edit().slider.is_none());
    s.apply(Slide::End.into());
}

#[test]
fn a_trigger_full_pull_beside_its_pull() {
    let mut s = edit_screen();
    let rt = Input::Trigger(Side::Right);
    keymap(&mut s, KeymapId::Input(rt));
    keymap(&mut s, KeymapId::Actuation(Actuation::Hold));
    assert_eq!(s.keymap_edit().act, Actuation::Press, "a trigger has no hold");
    keymap(&mut s, KeymapId::Actuation(Actuation::FullPull));
    assert_eq!(s.keymap_edit().act, Actuation::FullPull);
    keymap(&mut s, KeymapId::Target(Target::MouseParts));
    keymap(&mut s, KeymapId::Target(Target::Mouse(MouseButton::Left)));
    let k = s.keymaps().get(1).unwrap();
    assert_eq!(k.get_of(Layer::BASE, rt, Actuation::FullPull).map(|b| (b.output, b.threshold)), Some((Output::Mouse(MouseButton::Left), 95)));
    assert_eq!(k.get(rt), None, "the pull as on the pad (analog)");
    // Picking the other trigger: it, pulled all the way.
    keymap(&mut s, KeymapId::Target(Target::Pick));
    keymap(&mut s, KeymapId::Input(Input::Trigger(Side::Left)));
    let full = s.keymaps().get(1).unwrap().get_of(Layer::BASE, rt, Actuation::FullPull).map(|b| b.output);
    assert_eq!(full, Some(Output::Pad(PadOut::Trigger(Side::Left))));
}

#[test]
fn the_hold_and_double_tabs_edit_their_own_bindings() {
    let mut s = edit_screen();
    let a = Input::Button(Button::A);
    keymap(&mut s, KeymapId::Input(a));
    keymap(&mut s, KeymapId::Actuation(Actuation::Hold));
    assert_eq!(s.keymap_edit().act, Actuation::Hold);
    assert_eq!(s.param_base(1, a), None, "no hold yet: no parameters");
    keymap(&mut s, KeymapId::Target(Target::Key));
    key(&mut s, Key::Char('e'));
    let k = s.keymaps().get(1).unwrap();
    assert_eq!(k.get_of(Layer::BASE, a, Actuation::Hold).map(|b| b.output), Some(Output::Key(Chord::of(Key::Char('e')))));
    assert_eq!(k.get(a).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::B))), "the press as it was");
    assert!(s.press_waits(1, a));
    // Its time, and turbo, by slider; toggle by switch.
    slide(&mut s, Param::HoldTime, 60);
    slide(&mut s, Param::Turbo, 12);
    keymap(&mut s, KeymapId::Toggle(Param::Toggle));
    let b = s.keymaps().get(1).unwrap().get_of(Layer::BASE, a, Actuation::Hold).copied().unwrap();
    assert_eq!((b.hold_time, b.turbo, b.toggle), (60, 12, true));
    // Reset on the Hold tab: no hold again; the press untouched.
    keymap(&mut s, KeymapId::Target(Target::Default));
    assert_eq!(s.keymaps().get(1).unwrap().get_of(Layer::BASE, a, Actuation::Hold), None);
    assert!(!s.press_waits(1, a));
    // A stick has no tabs; another input selected starts on Press.
    keymap(&mut s, KeymapId::Actuation(Actuation::Double));
    keymap(&mut s, KeymapId::Input(Input::Stick(Side::Left)));
    assert_eq!(s.keymap_edit().act, Actuation::Press);
    keymap(&mut s, KeymapId::Actuation(Actuation::Hold));
    assert_eq!(s.keymap_edit().act, Actuation::Press, "not for a stick");
}

fn slide_start(s: &mut State, param: Param) -> Option<Slider> {
    s.apply(Slide::Start { control: Control::Param(param), x: 300.0, width: 620.0 }.into());
    let slider = s.keymap_edit().slider;
    s.apply(Slide::End.into());
    slider
}

#[test]
fn a_games_own_keymap_is_made_and_kept_while_another_is_chosen() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(2));
    // Its own, from Nintendo layout (in force for it): chosen, and its edit screen opens.
    picker(&mut s);
    assert_eq!(s.keymap_row(0), Some(PickerRow::Own(None)));
    let fx = tap(&mut s, row(0));
    assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))), "{fx:?}");
    let own = s.keymaps().own_of("zelda-lttp").unwrap().id;
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(own)));
    assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(own));
    assert_eq!((s.keymap_name(own), s.keymaps().get(own).unwrap().bindings().len()), ("Own keymap", 4));
    assert_eq!(s.keymap_users(1), Users::default(), "Nintendo layout no longer used");
    // Default chosen: the own one stays, offered still.
    keymap(&mut s, KeymapId::Back);
    tap(&mut s, row(1));
    assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(0));
    picker(&mut s);
    assert_eq!(s.keymap_row(0).map(|r| matches!(r, PickerRow::Own(Some(k)) if k.id == own)), Some(true));
    tap(&mut s, row(0));
    assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(own), "chosen again");
    // Deleted from its menu (in use: asked first); the game is back on Default.
    picker(&mut s);
    long(&mut s, row(0));
    keymap(&mut s, KeymapId::Delete);
    assert_eq!(s.keymap_screen(), Some(Screen::Delete(own)));
    keymap(&mut s, KeymapId::ConfirmDelete);
    assert!(s.keymaps().own_of("zelda-lttp").is_none());
    assert_eq!(s.keymap_row(0), Some(PickerRow::Own(None)));
}

#[test]
fn a_games_own_keymap_is_saved_as_a_new_named_one_or_over_one() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(2));
    picker(&mut s);
    tap(&mut s, row(0));
    let own = s.keymaps().own_of("zelda-lttp").unwrap().id;
    keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
    keymap(&mut s, KeymapId::Target(Target::Nothing));
    keymap(&mut s, KeymapId::Back);
    // As a new one: a copy, named; the game keeps its own.
    long(&mut s, row(0));
    keymap(&mut s, KeymapId::SaveAs);
    assert_eq!(s.keymap_screen(), Some(Screen::SaveAs(own)));
    assert_eq!((s.keymap_row(0), s.list_len(ListId::Keymaps)), (Some(PickerRow::New), 2), "a new one, Nintendo layout");
    tap(&mut s, row(0));
    type_text(&mut s, "zelda");
    tap(&mut s, Id::Editor(EditorId::Save));
    let copy = s.keymaps().named().find(|k| k.name == "zelda").unwrap().id;
    assert_eq!(s.keymaps().get(copy).unwrap().get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Nothing));
    assert_eq!(s.keymaps().own_of("zelda-lttp").map(|k| k.id), Some(own), "kept");
    assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(own), "still chosen");
    // Over Nintendo layout: asked first; then it has the own one's bindings.
    long(&mut s, row(0));
    keymap(&mut s, KeymapId::SaveAs);
    tap(&mut s, row(1));
    assert_eq!(s.keymap_screen(), Some(Screen::Overwrite { own, to: 1 }));
    keymap(&mut s, KeymapId::Back);
    assert_eq!(s.keymap_screen(), Some(Screen::SaveAs(own)), "not after all");
    tap(&mut s, row(1));
    let fx = keymap(&mut s, KeymapId::ConfirmSave);
    assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))), "{fx:?}");
    assert_eq!(s.keymaps().get(1).unwrap().get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Nothing));
    assert_eq!(s.keymap_name(1), "Nintendo layout", "its name kept");
    // From a shared keymap's edit screen, Make own: only for a game without one.
    assert!(!s.keymap_can_make_own());
}

#[test]
fn a_named_keymap_is_loaded_into_a_games_own_and_undo_goes_back() {
    let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
    picker(&mut s);
    tap(&mut s, row(1));
    picker(&mut s);
    tap(&mut s, row(0));
    let own = s.keymaps().own_of("zelda-lttp").unwrap().id;
    assert_eq!(s.keymaps().get(own).unwrap().bindings().len(), 0, "from Default: empty");
    keymap(&mut s, KeymapId::Back);
    long(&mut s, row(0));
    keymap(&mut s, KeymapId::LoadFrom);
    assert_eq!(s.keymap_screen(), Some(Screen::Load(own)));
    assert_eq!((s.keymap_row(0), s.list_len(ListId::Keymaps)), (Some(PickerRow::Default), 2), "Default, Nintendo layout");
    let fx = tap(&mut s, row(1));
    assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))), "{fx:?}");
    assert_eq!(s.keymap_screen(), Some(Screen::Edit(own)));
    assert_eq!(s.keymaps().get(own).unwrap().bindings().len(), 4, "Nintendo layout's");
    assert_eq!(s.keymaps().get(1).unwrap().bindings().len(), 4, "left as it was");
    keymap(&mut s, KeymapId::Undo);
    assert_eq!(s.keymaps().get(own).unwrap().bindings().len(), 0, "undone");
}
