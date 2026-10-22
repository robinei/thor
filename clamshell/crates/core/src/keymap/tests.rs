use super::*;

/// Saves the keymaps, each as JSON as they are kept, and reads them back.
fn round_trip(maps: &Keymaps) -> Keymaps {
    let saved: Vec<String> = maps.list().iter().map(|k| serde_json::to_string(k).unwrap()).collect();
    Keymaps::from_list(saved.iter().map(|k| serde_json::from_str(k).unwrap()).collect())
}

fn ctrl(c: char) -> Chord {
    Chord { ctrl: true, ..Chord::of(Key::Char(c)) }
}

#[test]
fn only_fitting_targets_bind() {
    let mut k = Keymap::new(1, "k");
    assert!(k.set(Input::Button(Button::A), Some(Binding::to(Output::Key(ctrl('x'))))));
    assert!(!k.set(Input::Button(Button::A), Some(Binding::to(Output::Pointer))), "a button can't move the pointer");
    assert!(!k.set(Input::Stick(Side::Right), Some(Binding::to(Output::Mouse(MouseButton::Left)))));
    assert!(k.set(Input::Stick(Side::Right), Some(Binding::to(Output::Pointer))));
    assert!(k.set(Input::Trigger(Side::Left), Some(Binding::to(Output::Key(Chord { shift: true, ..Chord::default() })))), "a trigger past its threshold");
    assert!(!k.set(Input::Trigger(Side::Left), Some(Binding::to(Output::Dpad))));
    assert!(!k.set(Input::Gyro, Some(Binding::to(Output::Keys([Chord::of(Key::Char('w')); 4])))));
    assert!(k.set(Input::Gyro, Some(Binding::to(Output::Stick(Side::Right)))));
    for kind in [Kind::Digital, Kind::Stick, Kind::Trigger, Kind::Gyro] {
        assert!(Output::Nothing.fits(kind));
    }
    assert!(k.set(Input::Button(Button::A), None));
    assert!(!k.set(Input::Button(Button::A), None), "already as on the pad");
    assert_eq!(k.get(Input::Button(Button::A)), None);
}

#[test]
fn keymaps_round_trip() {
    let mut maps = Keymaps::initial();
    let fps = maps.add("FPS").unwrap();
    let k = maps.get_mut(fps).unwrap();
    k.set(Input::Button(Button::A), Some(Binding::to(Output::Key(ctrl('x')))));
    k.set(Input::Stick(Side::Right), Some(Binding { speed: 75, deadzone: 20, full: 90, curve: 40, invert_y: true, ..Binding::to(Output::Pointer) }));
    k.set(Input::Trigger(Side::Right), Some(Binding { deadzone: 5, ..Binding::to(Output::Trigger(Side::Right)) }));
    k.set(
        Input::Stick(Side::Left),
        Some(Binding::to(Output::Keys([Chord::of(Key::Char('w')), Chord::of(Key::Char('a')), Chord::of(Key::Char('s')), Chord::of(Key::Char('d'))]))),
    );
    k.set(Input::Trigger(Side::Left), Some(Binding { threshold: 25, ..Binding::to(Output::Mouse(MouseButton::Right)) }));
    k.set(Input::StickDir(Side::Right, Dir::Up), Some(Binding::to(Output::Wheel(Dir::Up))));
    k.set(Input::Dpad(Dir::Down), Some(Binding::to(Output::Nothing)));
    k.set(Input::Button(Button::L4), Some(Binding::to(Output::Pad(PadOut::Dpad(Dir::Left)))));
    k.set(Input::Gyro, Some(Binding { speed: 37, hold: Some(Input::Trigger(Side::Left)), ..Binding::to(Output::Pointer) }));
    assert_eq!(round_trip(&maps), maps);
}

#[test]
fn a_games_own_keymap_round_trips() {
    let mut maps = Keymaps::initial();
    let game: std::sync::Arc<str> = "zelda-lttp".into();
    let own = maps.make_own(1, &game).unwrap();
    assert_eq!(maps.get(own).map(|k| (k.name.as_str(), k.bindings().len())), Some(("", 4)), "a copy of Nintendo layout");
    assert_eq!(round_trip(&maps), maps);
    assert_eq!(round_trip(&maps).get(own).and_then(|k| k.owner.clone()), Some(game));
    assert_eq!(maps.named().count(), 1, "not among the named");
    assert_eq!(maps.own_of("zelda-lttp").map(|k| k.id), Some(own));
}

#[test]
fn layers_are_named_targets() {
    let a = Input::Button(Button::A);
    let back = Input::Button(Button::Back);
    let mut k = Keymap::new(5, "Layers");
    let aim = k.add_layer("Aim").unwrap();
    assert_eq!(aim, Layer(1));
    assert!(k.set(Input::Button(Button::LB), Some(Binding::to(Output::Layer(aim)))));
    assert!(!k.set(Input::Button(Button::RB), Some(Binding::to(Output::Layer(Layer(9))))), "not there");
    assert!(k.set_in(aim, a, Some(Binding::to(Output::Key(Chord::of(Key::Char('y')))))));
    assert_eq!(k.lookup(a, &[aim]).map(|b| b.output), Some(Output::Key(Chord::of(Key::Char('y')))));
    assert_eq!(k.lookup(a, &[]), None, "Base: as on the pad");
    let maps = Keymaps::from_list(vec![k.clone()]);
    assert_eq!(round_trip(&maps), maps);
    // Removing a layer drops its bindings and the bindings to it.
    assert!(k.remove_layer(aim));
    assert_eq!((k.layers().count(), k.get(Input::Button(Button::LB))), (1, None));
    // Back isn't on the pad that games see: on the pad it does nothing, and it isn't a pad output.
    assert_eq!(as_on_the_pad(back), Some(Output::Nothing));
    assert_eq!(PadOut::of(back), None);
}

#[test]
fn parameters_stay_in_range_and_apply_by_target() {
    let mut b = Binding::to(Output::Pointer);
    Param::Full.set(&mut b, 10);
    assert_eq!(b.full, 50);
    Param::Deadzone.set(&mut b, 90);
    Param::Threshold.set(&mut b, 0);
    assert_eq!((b.deadzone, b.threshold), (50, 1), "kept in range");
    let ps = |i, o| params(i, o).collect::<Vec<_>>();
    let stick = vec![Param::Deadzone, Param::Full, Param::Curve, Param::InvertX, Param::InvertY];
    assert_eq!(ps(Input::Stick(Side::Left), Output::Pointer), [vec![Param::Speed], stick.clone()].concat());
    assert_eq!(ps(Input::Stick(Side::Left), Output::Stick(Side::Left)), stick, "as on the pad: tuned all the same");
    let gyro = |stick: bool| {
        let mut v = vec![Param::Speed, Param::Accel, Param::AccelAt, Param::Tighten];
        if stick {
            v.push(Param::StickMin);
        }
        v.extend([Param::Twist, Param::InvertX, Param::InvertY, Param::Ratchet]);
        v
    };
    assert_eq!(ps(Input::Gyro, Output::Stick(Side::Right)), gyro(true));
    assert_eq!(ps(Input::Gyro, Output::Pointer), gyro(false));
    // Written and read back.
    let b = Binding {
        accel: 40,
        accel_at: 90,
        tighten: 25,
        stick_min: 15,
        twist: true,
        ratchet: true,
        hold: Some(Input::Button(Button::LB)),
        ..Binding::to(Output::Stick(Side::Right))
    };
    let mut k = Keymap::new(1, "g");
    k.set(Input::Gyro, Some(b));
    let maps = Keymaps::from_list(vec![k]);
    assert_eq!(round_trip(&maps), maps);
    assert_eq!(ps(Input::Gyro, Output::Nothing), vec![], "off");
    assert_eq!(
        ps(Input::Trigger(Side::Left), Output::Key(Chord::of(Key::Char('x')))),
        vec![Param::Deadzone, Param::Full, Param::Threshold, Param::Turbo, Param::Toggle]
    );
    assert_eq!(ps(Input::Trigger(Side::Left), Output::Trigger(Side::Right)), vec![Param::Deadzone, Param::Full]);
    assert_eq!(ps(Input::Button(Button::A), Output::Key(Chord::of(Key::Char('x')))), vec![Param::Turbo, Param::Toggle]);
    assert_eq!(ps(Input::Button(Button::A), Output::Nothing), vec![], "nothing has no turbo");
    let a = Input::Button(Button::A);
    assert_eq!(params_of(a, Output::Key(Chord::of(Key::Char('x'))), Actuation::Hold).collect::<Vec<_>>(), vec![Param::HoldTime, Param::Turbo, Param::Toggle]);
}

#[test]
fn holds_doubles_turbo_and_toggle_are_kept() {
    let a = Input::Button(Button::A);
    let mut k = Keymap::new(4, "Combo");
    assert!(k.set(a, Some(Binding { turbo: 12, ..Binding::to(Output::Key(Chord::of(Key::Char('x')))) })));
    assert!(k.set_of(Layer::BASE, a, Actuation::Hold, Some(Binding { hold_time: 60, ..Binding::to(Output::Key(Chord::of(Key::Named(Named::Esc)))) })));
    assert!(k.set_of(Layer::BASE, a, Actuation::Double, Some(Binding { toggle: true, ..Binding::to(Output::Mouse(MouseButton::Left)) })));
    assert!(!k.set_of(Layer::BASE, Input::Stick(Side::Left), Actuation::Hold, Some(Binding::to(Output::Pointer))), "a stick has no hold");
    let aim = k.add_layer("Aim").unwrap();
    assert!(k.set_of(aim, a, Actuation::Hold, Some(Binding::to(Output::Key(Chord::of(Key::Named(Named::Tab)))))));
    assert_eq!(k.lookup_of(a, Actuation::Hold, &[aim]).map(|b| b.output), Some(Output::Key(Chord::of(Key::Named(Named::Tab)))));
    assert_eq!(k.lookup_of(a, Actuation::Double, &[aim]).map(|b| b.toggle), Some(true), "falls through");
    let maps = Keymaps::from_list(vec![k]);
    assert_eq!(round_trip(&maps), maps);
    assert!(!maps.list.first().unwrap().is_empty(aim));
    // How values are shown: turbo off or a rate, and a hold's time.
    let show = |p: Param, v| {
        let mut s = String::new();
        p.show(v, &mut s).unwrap();
        s
    };
    assert_eq!(
        (show(Param::Turbo, 0), show(Param::Turbo, 12), show(Param::HoldTime, 40), show(Param::Curve, 30)),
        ("Off".into(), "12/s".into(), "400 ms".into(), "30 %".into())
    );
}

#[test]
fn a_stick_on_the_pointer_has_defaults_of_its_own() {
    let rs = Input::Stick(Side::Right);
    let b = Binding::of(rs, Output::Pointer);
    assert_eq!((b.deadzone, b.curve), (POINTER_DEADZONE, POINTER_CURVE));
    assert!(!b.tuned(rs));
    // Mapped back to the stick, the pointer's defaults go and a value that was set stays.
    let own = b.retarget(rs, Output::Stick(Side::Right));
    assert_eq!((own.deadzone, own.curve), (0, 0));
    let set = Binding { deadzone: 20, ..b }.retarget(rs, Output::Stick(Side::Right));
    assert_eq!((set.deadzone, set.curve), (20, 0));
    let mut maps = Keymaps::initial();
    maps.get_mut(1).unwrap().set(rs, Some(Binding { deadzone: 0, ..b }));
    assert_eq!(round_trip(&maps), maps);
}

#[test]
fn an_input_only_tuned_counts_as_changed_but_not_remapped() {
    let ls = Input::Stick(Side::Left);
    let own = Binding::to(Output::Stick(Side::Left));
    assert_eq!(change(ls, None), Change::None);
    assert_eq!(change(ls, Some(&own)), Change::None, "as on the pad, said");
    assert_eq!(change(ls, Some(&Binding { deadzone: 12, ..own })), Change::Tuned);
    assert_eq!(change(ls, Some(&Binding { deadzone: 12, ..Binding::to(Output::Pointer) })), Change::Mapped);
    assert_eq!(change(Input::Button(Button::A), Some(&Binding::to(Output::Pad(PadOut::Button(Button::B))))), Change::Mapped);
    let tuned = Binding { deadzone: 12, invert_x: true, invert_y: true, ..own };
    let mut k = Keymap::new(1, "T");
    k.set(ls, Some(tuned));
    assert_eq!(k.get(ls), Some(&tuned));
}

#[test]
fn layers_round_trip() {
    let mut maps = Keymaps::initial();
    let k = maps.get_mut(1).unwrap();
    let aim = k.add_layer("Aim").unwrap();
    assert!(k.set_in(aim, Input::Button(Button::A), Some(Binding::to(Output::Key(ctrl('y'))))));
    assert!(k.set_in(aim, Input::Gyro, Some(Binding { hold: Some(Input::Button(Button::RB)), ..Binding::to(Output::Pointer) })));
    assert!(k.set_in(aim, Input::Button(Button::B), as_on_the_pad(Input::Button(Button::B)).map(Binding::to)));
    assert!(!k.set_layer_once(Layer::BASE, true), "Base is never Once");
    assert!(k.set_layer_once(aim, true) && k.layer_once(aim));
    let back = round_trip(&maps);
    assert_eq!(back, maps);
    assert!(back.get(1).unwrap().layer_once(aim));
    // A copy has its layers; clearing one leaves the others.
    let copy = maps.duplicate(1, "Copy").unwrap();
    let k = maps.get_mut(copy).unwrap();
    assert_eq!(k.layer(aim).len(), 3);
    assert!(k.clear(aim) && !k.clear(aim));
    assert_eq!(k.bindings().len(), 4);
}

#[test]
fn layers_fall_through_to_those_beneath() {
    let mut k = nintendo(1);
    let a = Input::Button(Button::A);
    let x = Input::Button(Button::X);
    let (one, two) = (k.add_layer("One").unwrap(), k.add_layer("Two").unwrap());
    k.set_in(one, a, Some(Binding::to(Output::Mouse(MouseButton::Left))));
    k.set_in(one, x, as_on_the_pad(x).map(Binding::to));
    k.set_in(two, a, Some(Binding::to(Output::Mouse(MouseButton::Right))));
    let out = |input, on: &[Layer]| k.lookup(input, on).map(|b| b.output);
    assert_eq!(out(a, &[]), Some(Output::Pad(PadOut::Button(Button::B))), "Base");
    assert_eq!(out(a, &[one]), Some(Output::Mouse(MouseButton::Left)), "the layer's own");
    assert_eq!(out(a, &[one, two]), Some(Output::Mouse(MouseButton::Right)), "the newest's");
    assert_eq!(out(x, &[one, two]), Some(Output::Pad(PadOut::Button(Button::X))), "not in the newest: the one beneath");
    assert_eq!(out(Input::Button(Button::B), &[one]), Some(Output::Pad(PadOut::Button(Button::A))), "in neither: Base's");
    assert_eq!(out(Input::Button(Button::LB), &[one]), None, "nowhere: as on the pad");
    assert_eq!(as_on_the_pad(Input::StickDir(Side::Left, Dir::Up)), None);
    assert!(Input::all().filter_map(|i| as_on_the_pad(i).map(|o| (i, o))).all(|(i, o)| o.fits(i.kind())));
}

#[test]
fn adding_renaming_duplicating_removing() {
    let mut maps = Keymaps::initial();
    let a = maps.add("Racing").unwrap();
    assert_eq!(maps.add("Racing"), Some(3));
    assert_eq!(maps.get(3).map(|k| k.name.as_str()), Some("Racing 2"), "names stay apart");
    maps.get_mut(a).unwrap().set(Input::Button(Button::A), Some(Binding::to(Output::Pad(PadOut::Trigger(Side::Right)))));
    let copy = maps.duplicate(a, "Racing").unwrap();
    assert_eq!(maps.get(copy).map(|k| (k.name.as_str(), k.bindings().len())), Some(("Racing 3", 1)));
    assert!(maps.rename(a, "Rally"));
    assert!(maps.rename(a, "Rally"), "its own name again");
    assert_eq!(maps.get(a).map(|k| k.name.as_str()), Some("Rally"));
    let slash = maps.add(" A/B ").unwrap();
    assert_eq!(maps.get(slash).map(|k| k.name.as_str()), Some("A-B"), "a name that makes a file name");
    maps.remove(slash);
    assert!(maps.remove(copy) && !maps.remove(copy));
    assert_eq!(maps.add("New"), Some(4), "ids are never taken twice while the highest stays");
    let mut full = Keymaps::from_list(vec![Keymap::new(u16::MAX, "Last")]);
    assert_eq!(full.add("More"), None, "no id left: none made");
    assert_eq!(full.make_own(0, &"zelda".into()), None);
    assert_eq!(nintendo(1).get(Input::Button(Button::X)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::Y))));
}

#[test]
fn inputs_by_name() {
    for input in Input::all() {
        let mut name = String::new();
        input.name(&mut name).unwrap();
        assert_eq!(Input::parse(&name), Some(input), "{name}");
    }
    assert_eq!(Input::all().count(), 16 + 4 + 10 + 2 + 1);
    assert_eq!(PadOut::all().count(), 17);
}

#[test]
fn a_keymaps_version_moves_only_when_it_changes() {
    let mut maps = Keymaps::initial();
    assert_eq!(maps.version(1), 0, "as read");
    let a = Input::Button(Button::A);
    assert!(!maps.edit(1, |k| k.set(a, k.get(a).copied())), "set to what it was");
    assert_eq!(maps.version(1), 0);
    let two = maps.add("Two").unwrap();
    let after_add = maps.version(two);
    assert!(maps.edit(1, |k| k.set(a, None)));
    let one = maps.version(1);
    assert!(one > after_add);
    assert!(maps.rename(two, "Two"));
    assert!(maps.copy_into(two, 0));
    assert_eq!((maps.version(1), maps.version(two)), (one, after_add), "renamed to its name, and emptied while empty");
    let before = maps.get(1).cloned().unwrap();
    maps.replace(before.clone());
    assert_eq!(maps.version(1), one, "put back as it is");
    assert_eq!(round_trip(&maps), maps, "versions aren't compared");
}
