//! The keymaps file, keymaps.txt (see the module's doc): written, and read back.

use super::*;

/// A layer's bindings, a line each, a press's, then a hold's and a double's (see the module's doc).
pub(super) fn write_bindings(bindings: &Bindings, text: &mut String) {
    for act in Actuation::ALL {
        for (input, b) in bindings.of(act) {
            let _ = input.name(text);
            if act != Actuation::Press {
                text.push(' ');
                text.push_str(act.name());
            }
            text.push_str(" = ");
            let _ = write_output(&b.output, text);
            for p in params_of(*input, b.output, act).filter(|p| !p.switch()) {
                let v = p.get(b);
                if v != p.default_for(*input, b.output) {
                    let _ = write!(text, " {}={v}", p.name());
                }
            }
            if b.invert_x || b.invert_y {
                let _ = write!(text, " invert={}{}", if b.invert_x { "x" } else { "" }, if b.invert_y { "y" } else { "" });
            }
            if let Some(hold) = b.hold {
                text.push_str(" hold=");
                let _ = hold.name(text);
            }
            for (on, word) in [(b.toggle, "toggle"), (b.twist, "twist"), (b.ratchet, "ratchet")] {
                if on {
                    text.push(' ');
                    text.push_str(word);
                }
            }
            text.push('\n');
        }
    }
}

pub(super) fn write_output(o: &Output, out: &mut String) -> fmt::Result {
    match o {
        Output::Nothing => out.write_str("nothing"),
        Output::Pad(p) => {
            out.write_str("pad ")?;
            p.input().name(out)
        }
        Output::Key(c) => {
            out.write_str("key ")?;
            c.write(out)
        }
        Output::Mouse(m) => write!(out, "mouse {}", m.name()),
        Output::Wheel(d) => write!(out, "wheel {}", d.name()),
        Output::Stick(s) => write!(out, "stick {}", s.name()),
        Output::Pointer => out.write_str("pointer"),
        Output::Keys(keys) => {
            out.write_str("keys")?;
            for k in keys {
                out.write_char(' ')?;
                k.write(out)?;
            }
            Ok(())
        }
        Output::Dpad => out.write_str("dpad"),
        Output::Trigger(s) => write!(out, "trigger {}", s.name()),
        Output::Layer(l) => write!(out, "layer {}", l.0),
    }
}

/// `INPUT = OUTPUT [option=value ...]`.
pub(super) fn parse_binding(line: &str) -> Option<(Input, Actuation, Binding)> {
    let (input, rest) = line.split_once('=')?;
    let mut left = input.split_whitespace();
    let input = Input::parse(left.next()?)?;
    let act = match left.next() {
        None => Actuation::Press,
        Some(w) => Actuation::from_name(w).filter(|a| *a != Actuation::Press)?,
    };
    let mut words = rest.split_whitespace().peekable();
    let output = match (words.next()?, words.peek().copied()) {
        ("nothing", _) => Output::Nothing,
        ("pointer", _) => Output::Pointer,
        ("dpad", _) => Output::Dpad,
        ("pad", Some(w)) => Output::Pad(PadOut::of(Input::parse(w)?)?),
        ("key", Some(w)) => Output::Key(Chord::parse(w)?),
        ("mouse", Some(w)) => Output::Mouse(MouseButton::from_name(w)?),
        ("wheel", Some(w)) => Output::Wheel(Dir::from_name(w)?),
        ("stick", Some(w)) => Output::Stick(Side::from_name(w)?),
        ("trigger", Some(w)) => Output::Trigger(Side::from_name(w)?),
        ("layer", Some(w)) => Output::Layer(Layer(w.parse().ok().filter(|n| *n > 0)?)),
        ("keys", Some(_)) => {
            let mut keys = [Chord::default(); 4];
            for k in &mut keys {
                *k = Chord::parse(words.next()?)?;
            }
            Output::Keys(keys)
        }
        _ => return None,
    };
    if !matches!(output, Output::Nothing | Output::Pointer | Output::Dpad | Output::Keys(_)) {
        words.next();
    }
    let mut binding = Binding::of(input, output);
    for word in words {
        match word.split_once('=') {
            Some((name @ ("speed" | "threshold" | "deadzone" | "full" | "curve" | "turbo" | "holdtime" | "accel" | "accelat" | "tighten" | "stickmin"), v)) => {
                if let Some(p) = Param::from_name(name)
                    && let Some(v) = v.parse().ok().filter(|v| p.range().contains(v))
                {
                    p.set(&mut binding, v);
                }
            }
            Some(("invert", v)) => {
                binding.invert_x = v.contains('x');
                binding.invert_y = v.contains('y');
            }
            Some(("hold", v)) => binding.hold = Input::parse(v).filter(|h| gyro_holds().any(|g| g == *h)),
            None if word == "toggle" => binding.toggle = true,
            None if word == "twist" => binding.twist = true,
            None if word == "ratchet" => binding.ratchet = true,
            // (Options of later features.)
            _ => {}
        }
    }
    (input.kind() == Kind::Gyro || binding.hold.is_none()).then_some((input, act, binding))
}

impl Keymaps {
    /// As text (see the module's doc).
    pub fn text(&self) -> String {
        let mut text = format!("{KEYMAPS_FORMAT}\n");
        for k in &self.list {
            let _ = writeln!(text, "keymap {} {}", k.id, k.name);
            for l in &k.layers {
                if l.id != Layer::BASE {
                    let _ = writeln!(text, "layer {} {}", l.id.0, l.name);
                    if l.once {
                        text.push_str("once\n");
                    }
                }
                write_bindings(&l.bindings, &mut text);
            }
        }
        text
    }

    /// Reads [`Keymaps::text`] back; lines it doesn't understand are skipped, and the bindings of a
    /// layer it doesn't know.
    pub fn load(text: &str) -> Keymaps {
        let mut keymaps = Keymaps::default();
        // The layer the lines are in (none: one not understood).
        let mut layer = Some(Layer::BASE);
        for line in text.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("keymap ") {
                let (id, name) = rest.split_once(' ').unwrap_or((rest, ""));
                if let Ok(id) = id.parse::<u16>()
                    && id > 0
                    && keymaps.get(id).is_none()
                {
                    keymaps.list.push(Keymap::new(id, name.trim()));
                }
                layer = Some(Layer::BASE);
                continue;
            }
            if let Some(rest) = line.strip_prefix("layer ") {
                let Some(keymap) = keymaps.list.last_mut() else {
                    continue;
                };
                let (id, name) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
                layer = match id.parse::<u8>() {
                    Ok(0) => Some(Layer::BASE),
                    Ok(n) => {
                        keymap.ensure_layer(Layer(n), (!name.trim().is_empty()).then_some(name));
                        Some(Layer(n))
                    }
                    Err(_) => None,
                };
                continue;
            }
            if line == "once" {
                if let (Some(keymap), Some(layer)) = (keymaps.list.last_mut(), layer) {
                    keymap.set_layer_once(layer, true);
                }
                continue;
            }
            let (Some(keymap), Some(layer), Some((input, act, binding))) = (keymaps.list.last_mut(), layer, parse_binding(line)) else {
                continue;
            };
            if let Output::Layer(to) = binding.output {
                keymap.ensure_layer(to, None);
            }
            keymap.set_of(layer, input, act, Some(binding));
        }
        keymaps
    }
}
