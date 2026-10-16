use super::*;

/// A keymaps file's text after its format's line (which is checked).
fn body(text: &str) -> &str {
    text.strip_prefix(KEYMAPS_FORMAT).and_then(|t| t.strip_prefix('\n')).unwrap()
}

fn chord(text: &str) -> Chord {
    Chord::parse(text).unwrap()
}

#[test]
fn chords_as_text() {
    let mut text = String::new();
    chord("x+ctrl").write(&mut text).unwrap();
    assert_eq!(text, "ctrl+x", "modifiers first");
    for t in ["shift", "space", "f5", "alt+tab", "ctrl+shift+=", "pagedown", "super+d", "/"] {
        let mut back = String::new();
        chord(t).write(&mut back).unwrap();
        assert_eq!(back, t);
    }
    assert_eq!(Chord::parse("ctrl+x+y"), None, "two keys");
    assert_eq!(Chord::parse("f13"), None);
    assert_eq!(Chord::parse("é"), None, "no key types it");
    assert_eq!(Chord::parse(""), None);
}

#[test]
fn only_fitting_targets_bind() {
    let mut k = Keymap::new(1, "k");
    assert!(k.set(Input::Button(Button::A), Some(Binding::to(Output::Key(chord("ctrl+x"))))));
    assert!(!k.set(Input::Button(Button::A), Some(Binding::to(Output::Pointer))), "a button can't move the pointer");
    assert!(!k.set(Input::Stick(Side::Right), Some(Binding::to(Output::Mouse(MouseButton::Left)))));
    assert!(k.set(Input::Stick(Side::Right), Some(Binding::to(Output::Pointer))));
    assert!(k.set(Input::Trigger(Side::Left), Some(Binding::to(Output::Key(chord("shift"))))), "a trigger past its threshold");
    assert!(!k.set(Input::Trigger(Side::Left), Some(Binding::to(Output::Dpad))));
    assert!(!k.set(Input::Gyro, Some(Binding::to(Output::Keys([chord("w"); 4])))));
    assert!(k.set(Input::Gyro, Some(Binding::to(Output::Stick(Side::Right)))));
    for kind in [Kind::Digital, Kind::Stick, Kind::Trigger, Kind::Gyro] {
        assert!(Output::Nothing.fits(kind));
    }
    assert!(k.set(Input::Button(Button::A), None));
    assert!(!k.set(Input::Button(Button::A), None), "already as on the pad");
    assert_eq!(k.get(Input::Button(Button::A)), None);
}

#[test]
fn keymaps_round_trip_as_text() {
    let mut maps = Keymaps::initial();
    let fps = maps.add("FPS");
    let k = maps.get_mut(fps).unwrap();
    k.set(Input::Button(Button::A), Some(Binding::to(Output::Key(chord("ctrl+x")))));
    k.set(Input::Stick(Side::Right), Some(Binding { speed: 75, deadzone: 20, full: 90, curve: 40, invert_y: true, ..Binding::to(Output::Pointer) }));
    k.set(Input::Trigger(Side::Right), Some(Binding { deadzone: 5, ..Binding::to(Output::Trigger(Side::Right)) }));
    k.set(Input::Stick(Side::Left), Some(Binding::to(Output::Keys([chord("w"), chord("a"), chord("s"), chord("d")]))));
    k.set(Input::Trigger(Side::Left), Some(Binding { threshold: 25, ..Binding::to(Output::Mouse(MouseButton::Right)) }));
    k.set(Input::StickDir(Side::Right, Dir::Up), Some(Binding::to(Output::Wheel(Dir::Up))));
    k.set(Input::Dpad(Dir::Down), Some(Binding::to(Output::Nothing)));
    k.set(Input::Button(Button::L4), Some(Binding::to(Output::Pad(PadOut::Dpad(Dir::Left)))));
    k.set(Input::Gyro, Some(Binding { speed: 37, hold: Some(Input::Trigger(Side::Left)), ..Binding::to(Output::Pointer) }));
    let text = maps.text();
    let text = body(&text);
    assert!(text.starts_with("keymap 1 Nintendo layout\na = pad b\nb = pad a\nx = pad y\ny = pad x\nkeymap 2 FPS\na = key ctrl+x\nl4 = pad left\n"), "{text}");
    assert!(
        text.contains("\nlstick = keys w a s d\n")
            && text.contains("\nrstick = pointer speed=75 deadzone=20 full=90 curve=40 invert=y\n")
            && text.contains("\ngyro = pointer speed=37 hold=lt\n"),
        "{text}"
    );
    assert!(
        text.contains("\nlt = mouse right threshold=25\n") && text.contains("\nrt = trigger right deadzone=5\n") && text.contains("\nrstick-up = wheel up\n"),
        "{text}"
    );
    assert_eq!(Keymaps::load(&maps.text()), maps);
}

#[test]
fn lines_not_understood_are_skipped() {
    let text = "# a comment\nkeymap 3 Turbo\na = key x turbo=10\nb = warp 9\nzz = pad a\nlstick = key x\ngyro = pointer hold=up\nx = pad x hold=a\nlayer x\ny = pad a\nlayer 0\nlt = trigger right\nkeymap 0 bad\nkeymap x bad\nkeymap 3 again\n";
    let maps = Keymaps::load(text);
    assert_eq!(maps.list().len(), 1);
    let k = maps.get(3).unwrap();
    assert_eq!(k.name, "Turbo");
    assert_eq!(k.get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Key(chord("x"))), "an unknown option is skipped");
    assert_eq!(k.get(Input::Gyro).map(|b| b.hold), Some(None), "a D-pad direction doesn't hold the gyro");
    assert_eq!(k.get(Input::Button(Button::Y)), None, "a layer not known is skipped");
    assert_eq!(k.get(Input::Trigger(Side::Left)).map(|b| b.output), Some(Output::Trigger(Side::Right)), "Base again");
    assert_eq!(k.bindings().len(), 3, "{k:?}");
    assert_eq!(k.layers().count(), 1, "Base only");
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
    assert!(k.set_in(aim, a, Some(Binding::to(Output::Key(chord("y"))))));
    assert_eq!(k.lookup(a, &[aim]).map(|b| b.output), Some(Output::Key(chord("y"))));
    assert_eq!(k.lookup(a, &[]), None, "Base: as on the pad");
    let maps = Keymaps { list: vec![k.clone()] };
    let text = maps.text();
    assert_eq!(body(&text), "keymap 5 Layers\nlb = layer 1\nlayer 1 Aim\na = key y\n");
    assert_eq!(Keymaps::load(&text), maps);
    // Removed: its bindings, and those to it, go.
    assert!(k.remove_layer(aim));
    assert_eq!((k.layers().count(), k.get(Input::Button(Button::LB))), (1, None));
    // Back on the pad games see: nothing as on the pad; not a pad output.
    assert_eq!(as_on_the_pad(back), Some(Output::Nothing));
    assert_eq!(PadOut::of(back), None);
}

#[test]
fn parameters_load_in_range_and_apply_by_target() {
    let text = "keymap 1 K\nrstick = pointer speed=75 deadzone=60\nlt = key x threshold=75\nrt = key y threshold=0\n";
    let maps = Keymaps::load(text);
    let get = |i: Input| *maps.get(1).unwrap().get(i).unwrap();
    assert_eq!(get(Input::Stick(Side::Right)).speed, 75);
    assert_eq!(get(Input::Stick(Side::Right)).deadzone, POINTER_DEADZONE, "out of range: its default");
    assert_eq!(get(Input::Trigger(Side::Left)).threshold, 75);
    assert_eq!(get(Input::Trigger(Side::Right)).threshold, DEFAULT_THRESHOLD, "out of range");
    assert!(maps.text().contains("rstick = pointer speed=75\n"), "{}", maps.text());
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
    let maps = Keymaps { list: vec![k] };
    assert!(maps.text().contains("gyro = stick right accel=40 accelat=90 tighten=25 stickmin=15 hold=lb twist ratchet\n"), "{}", maps.text());
    assert_eq!(Keymaps::load(&maps.text()), maps);
    assert_eq!(ps(Input::Gyro, Output::Nothing), vec![], "off");
    assert_eq!(ps(Input::Trigger(Side::Left), Output::Key(chord("x"))), vec![Param::Deadzone, Param::Full, Param::Threshold, Param::Turbo, Param::Toggle]);
    assert_eq!(ps(Input::Trigger(Side::Left), Output::Trigger(Side::Right)), vec![Param::Deadzone, Param::Full]);
    assert_eq!(ps(Input::Button(Button::A), Output::Key(chord("x"))), vec![Param::Turbo, Param::Toggle]);
    assert_eq!(ps(Input::Button(Button::A), Output::Nothing), vec![], "nothing has no turbo");
    let a = Input::Button(Button::A);
    assert_eq!(params_of(a, Output::Key(chord("x")), Actuation::Hold).collect::<Vec<_>>(), vec![Param::HoldTime, Param::Turbo, Param::Toggle]);
}

#[test]
fn holds_doubles_turbo_and_toggle_are_kept_and_written() {
    let a = Input::Button(Button::A);
    let mut k = Keymap::new(4, "Combo");
    assert!(k.set(a, Some(Binding { turbo: 12, ..Binding::to(Output::Key(chord("x"))) })));
    assert!(k.set_of(Layer::BASE, a, Actuation::Hold, Some(Binding { hold_time: 60, ..Binding::to(Output::Key(chord("esc"))) })));
    assert!(k.set_of(Layer::BASE, a, Actuation::Double, Some(Binding { toggle: true, ..Binding::to(Output::Mouse(MouseButton::Left)) })));
    assert!(!k.set_of(Layer::BASE, Input::Stick(Side::Left), Actuation::Hold, Some(Binding::to(Output::Pointer))), "a stick has no hold");
    let aim = k.add_layer("Aim").unwrap();
    assert!(k.set_of(aim, a, Actuation::Hold, Some(Binding::to(Output::Key(chord("tab"))))));
    assert_eq!(k.lookup_of(a, Actuation::Hold, &[aim]).map(|b| b.output), Some(Output::Key(chord("tab"))));
    assert_eq!(k.lookup_of(a, Actuation::Double, &[aim]).map(|b| b.toggle), Some(true), "falls through");
    let maps = Keymaps { list: vec![k] };
    let text = maps.text();
    assert_eq!(body(&text), "keymap 4 Combo\na = key x turbo=12\na hold = key esc holdtime=60\na double = mouse left toggle\nlayer 1 Aim\na hold = key tab\n");
    assert_eq!(Keymaps::load(&text), maps);
    assert!(!maps.list.first().unwrap().is_empty(aim));
    // Shown: turbo off or a rate, a hold's time.
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
    // Back to the stick: those defaults go with it; a value set stays.
    let own = b.retarget(rs, Output::Stick(Side::Right));
    assert_eq!((own.deadzone, own.curve), (0, 0));
    let set = Binding { deadzone: 20, ..b }.retarget(rs, Output::Stick(Side::Right));
    assert_eq!((set.deadzone, set.curve), (20, 0));
    // Written only where it differs from them; read back with them.
    let mut maps = Keymaps::initial();
    maps.get_mut(1).unwrap().set(rs, Some(Binding { deadzone: 0, ..b }));
    assert!(maps.text().contains("rstick = pointer deadzone=0\n"), "{}", maps.text());
    assert_eq!(Keymaps::load(&maps.text()), maps);
    assert_eq!(Keymaps::load("keymap 1 K\nrstick = pointer\n").get(1).unwrap().get(rs), Some(&b));
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
    let text = "keymap 1 T\nlstick = stick left deadzone=12 invert=xy\n";
    let maps = Keymaps::load(text);
    assert_eq!(maps.get(1).unwrap().get(ls), Some(&Binding { deadzone: 12, invert_x: true, invert_y: true, ..own }));
    assert_eq!(body(&maps.text()), text, "written with its format's line first");
}

#[test]
fn layers_round_trip() {
    let base = "keymap 1 Nintendo layout\na = pad b\nb = pad a\nx = pad y\ny = pad x\n";
    let mut maps = Keymaps::initial();
    let k = maps.get_mut(1).unwrap();
    let aim = k.add_layer("Aim").unwrap();
    assert!(k.set_in(aim, Input::Button(Button::A), Some(Binding::to(Output::Key(chord("ctrl+y"))))));
    assert!(k.set_in(aim, Input::Gyro, Some(Binding { hold: Some(Input::Button(Button::RB)), ..Binding::to(Output::Pointer) })));
    assert!(k.set_in(aim, Input::Button(Button::B), as_on_the_pad(Input::Button(Button::B)).map(Binding::to)));
    assert!(!k.set_layer_once(Layer::BASE, true), "Base is never Once");
    assert!(k.set_layer_once(aim, true) && k.layer_once(aim));
    let text = maps.text();
    assert_eq!(body(&text), format!("{base}layer 1 Aim\nonce\na = key ctrl+y\nb = pad b\ngyro = pointer hold=rb\n"));
    assert_eq!(Keymaps::load(&text), maps);
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
    let a = maps.add("Racing");
    assert_eq!(maps.add("Racing"), 3);
    assert_eq!(maps.get(3).map(|k| k.name.as_str()), Some("Racing 2"), "names stay apart");
    maps.get_mut(a).unwrap().set(Input::Button(Button::A), Some(Binding::to(Output::Pad(PadOut::Trigger(Side::Right)))));
    let copy = maps.duplicate(a, "Racing").unwrap();
    assert_eq!(maps.get(copy).map(|k| (k.name.as_str(), k.bindings().len())), Some(("Racing 3", 1)));
    assert!(maps.rename(a, "Rally"));
    assert!(maps.rename(a, "Rally"), "its own name again");
    assert_eq!(maps.get(a).map(|k| k.name.as_str()), Some("Rally"));
    assert!(maps.remove(copy) && !maps.remove(copy));
    assert_eq!(maps.add("New"), 4, "ids are never taken twice while the highest stays");
    assert_eq!(nintendo(1).get(Input::Button(Button::X)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::Y))));
}

#[test]
fn inputs_by_name() {
    for input in Input::all() {
        let mut name = String::new();
        input.name(&mut name).unwrap();
        assert_eq!(Input::parse(&name), Some(input), "{name}");
    }
    assert_eq!(Input::all().count(), 15 + 4 + 10 + 2 + 1);
    assert_eq!(PadOut::all().count(), 16);
}
