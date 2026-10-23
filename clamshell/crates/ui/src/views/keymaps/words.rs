//! Bindings in words: what an input maps to, a label's summary, the button names printed on the Thor, a
//! parameter's value, and what turns a layer on.

use std::fmt::Write;

use clamshell_state::keymap::{Actuation, Binding, Button, Change, Input, Keymap, Layer, Output, PadOut, Param, Side, change};

use crate::toolkit::format::Fmt;
use crate::toolkit::style::Tone;

/// What a binding maps to, in words. The pad's buttons go by the names printed on the Thor, as inputs
/// do. Games get them by position, so the printed B is the Xbox pad's A.
pub(super) fn mapping(b: Binding, keymap: Option<&Keymap>) -> Fmt<48> {
    match b.output {
        // A layer goes by its name in the keymap.
        Output::Layer(l) => Fmt::of(format_args!("Layer: {}", keymap.and_then(|k| k.layer_name(l)).unwrap_or("?"))),
        out => Fmt::of(format_args!("{}", OutputLabel(out))),
    }
}

/// What an input's binding in a layer says, and its tone. The tone is blue where it maps the input to
/// something else, and grey where the input acts as on the pad, either tuned or set that way in a layer
/// above Base.
pub(super) fn described(input: Input, b: &Binding, keymap: Option<&Keymap>) -> (Fmt<48>, Tone) {
    match change(input, Some(b)) {
        Change::Mapped => (summary(input, b, keymap), Tone::Fn),
        _ if input == Input::Gyro => (Fmt::of(format_args!("Off")), Tone::Normal),
        Change::Tuned => (summary(input, b, keymap), Tone::Normal),
        Change::None => (Fmt::of(format_args!("As on pad")), Tone::Normal),
    }
}

/// A binding in a few words: what it maps to, with its turbo and toggle (`Ctrl+X · 12/s`). A binding
/// that only repeats or toggles the input as on the pad says just that (`Turbo 12/s`). One that is only
/// tuned says `Tuned`.
pub(super) fn summary(input: Input, b: &Binding, keymap: Option<&Keymap>) -> Fmt<48> {
    let mut text = Fmt::<48>::new();
    let mapped = Some(b.output) != clamshell_state::keymap::as_on_the_pad(input);
    match (mapped, b.turbo, b.toggle) {
        (true, ..) => {
            let _ = write!(text, "{}", mapping(*b, keymap).as_str());
            if b.turbo > 0 {
                let _ = write!(text, " · {}/s", b.turbo);
            }
            if b.toggle {
                let _ = write!(text, " · toggle");
            }
        }
        (false, 0, false) => {
            let _ = write!(text, "Tuned");
        }
        (false, t, toggle) => {
            if t > 0 {
                let _ = write!(text, "Turbo {t}/s");
            }
            if toggle {
                let _ = write!(text, "{}Toggle", if t > 0 { " · " } else { "" });
            }
        }
    }
    text
}

/// "As on the pad" in words for an input. For the gyro it is "Off".
pub(super) fn as_on_pad(input: Input) -> Fmt<48> {
    Fmt::of(format_args!("{}", if input == Input::Gyro { "Off" } else { "As on the pad" }))
}

/// A parameter's value in words (`40 %`, `Off`, `12/s`, `400 ms`).
pub(super) fn shown(p: Param, v: u8) -> Fmt<16> {
    let mut text = Fmt::<16>::new();
    let _ = p.show(v, &mut text);
    text
}

/// A button's name as printed on the Thor. The face buttons are lettered Nintendo-style by position: the
/// bottom one (Xbox A, BTN_SOUTH) is B, the right one A, the top one X and the left one Y.
pub(super) fn thor(b: Button) -> &'static str {
    match b {
        Button::A => "B",
        Button::B => "A",
        Button::X => "Y",
        Button::Y => "X",
        Button::LB => "L1",
        Button::RB => "R1",
        Button::LS => "L3",
        Button::RS => "R3",
        Button::View => "Select",
        Button::Menu => "Start",
        b => b.label(),
    }
}

/// An input's name on screen, as printed on the Thor.
pub(super) struct Label(pub(super) Input);

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Input::Button(b) => f.write_str(thor(b)),
            Input::Trigger(s) => f.write_str(if s == Side::Left { "L2" } else { "R2" }),
            input => input.label(f),
        }
    }
}

pub(super) struct OutputLabel(pub(super) Output);

impl std::fmt::Display for OutputLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Output::Pad(PadOut::Button(b)) => f.write_str(thor(b)),
            Output::Pad(PadOut::Trigger(s)) | Output::Trigger(s) => f.write_str(if s == Side::Left { "L2" } else { "R2" }),
            out => out.label(f),
        }
    }
}

/// What turns layer `l` on, in words: each binding to it, such as `Back`, `LB held` or `L1 twice`.
/// Toggles say so, and a Once layer says so first. Base is described as beneath the others.
pub(super) fn turned_on_by(k: &Keymap, l: Layer) -> Fmt<96> {
    let mut out = Fmt::new();
    if l == Layer::BASE {
        let _ = write!(out, "Beneath the others");
        return out;
    }
    if k.layer_once(l) {
        let _ = write!(out, "Once · ");
    }
    let mut any = false;
    for (from, _) in k.layers() {
        for act in Actuation::ALL {
            for (input, b) in k.layer_of(from, act) {
                if b.output != Output::Layer(l) {
                    continue;
                }
                let how = match (act, b.toggle) {
                    (_, true) => " toggles",
                    (Actuation::Press, _) => "",
                    (Actuation::Hold, _) => " held",
                    (Actuation::Double, _) => " twice",
                    (Actuation::FullPull, _) => " full",
                };
                let _ = write!(out, "{}{}{how}", if any { ", " } else { "" }, Label(*input));
                any = true;
            }
        }
    }
    if !any {
        let _ = write!(out, "Nothing turns it on");
    }
    out
}
