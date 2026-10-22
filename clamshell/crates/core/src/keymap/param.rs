//! A binding's parameters (Param): their names, ranges, defaults and labels, and which apply to an
//! input mapped to an output.

use super::*;
use crate::names::names;

/// A binding's parameters beside its output: values set with a slider (0 to 100 on its track), and
/// switches ([`Param::switch`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Param {
    Speed,
    Threshold,
    Deadzone,
    Full,
    Curve,
    InvertX,
    InvertY,
    /// A hold's: how long the press must last.
    HoldTime,
    /// Presses a second while on (0: off).
    Turbo,
    Toggle,
    /// The gyro's.
    Accel,
    AccelAt,
    Tighten,
    StickMin,
    Twist,
    Ratchet,
}

// In the panel's order.
names!(Param {
    Speed: "speed", "Speed";
    Accel: "accel", "Acceleration";
    AccelAt: "accelat", "Fastest at";
    Tighten: "tighten", "Tightening";
    StickMin: "stickmin", "Stick from";
    Deadzone: "deadzone", "Deadzone";
    Full: "full", "Full at";
    Curve: "curve", "Curve";
    Threshold: "threshold", "Acts past";
    HoldTime: "holdtime", "Hold for";
    Turbo: "turbo", "Turbo";
    Toggle: "toggle", "Toggle";
    Twist: "twist", "Turn by tilting";
    InvertX: "invertx", "Invert X";
    InvertY: "inverty", "Invert Y";
    Ratchet: "ratchet", "Off while held";
});

impl Param {
    /// On or off (a tap switches it), rather than a value on a slider.
    pub fn switch(self) -> bool {
        matches!(self, Param::InvertX | Param::InvertY | Param::Toggle | Param::Twist | Param::Ratchet)
    }

    /// A value of it as shown: `40 %`, `Off` or `12/s` (turbo), `400 ms` (a hold's time).
    pub fn show(self, v: u8, out: &mut impl Write) -> fmt::Result {
        match self {
            Param::Turbo if v == 0 => out.write_str("Off"),
            Param::Turbo => write!(out, "{v}/s"),
            Param::HoldTime => write!(out, "{} ms", u16::from(v) * 10),
            Param::Accel if v == 0 => out.write_str("Off"),
            Param::AccelAt => write!(out, "{v} °/s"),
            Param::Tighten => write!(out, "{}.{} °/s", v / 10, v % 10),
            _ => write!(out, "{v} %"),
        }
    }

    /// Whether `input` mapped to `output` has it: a stick its deadzone, full point, curve and inversion
    /// (and speed on the pointer); a trigger its deadzone and full point (and threshold on a button);
    /// the gyro on motion its speed and inversion.
    pub fn applies(self, input: Input, output: Output) -> bool {
        let motion = matches!(output, Output::Pointer | Output::Stick(_));
        match (input.kind(), self) {
            (Kind::Stick, Param::Deadzone | Param::Full | Param::Curve | Param::InvertX | Param::InvertY) => true,
            (Kind::Stick, Param::Speed) => output == Output::Pointer,
            (Kind::Trigger, Param::Deadzone | Param::Full) => true,
            (Kind::Trigger, Param::Threshold) => output.digital(),
            (Kind::Gyro, Param::Speed | Param::InvertX | Param::InvertY | Param::Accel | Param::AccelAt | Param::Tighten | Param::Twist | Param::Ratchet) => {
                motion
            }
            (Kind::Gyro, Param::StickMin) => matches!(output, Output::Stick(_)),
            (Kind::Digital | Kind::Trigger, Param::Toggle) => output.digital(),
            // (A layer doesn't beat.)
            (Kind::Digital | Kind::Trigger, Param::Turbo) => output.digital() && !matches!(output, Output::Layer(_)),
            _ => false,
        }
    }

    /// [`Param::applies`] for `input`'s binding of `act`: a hold's also has its time.
    pub fn applies_to(self, input: Input, output: Output, act: Actuation) -> bool {
        match (self, act) {
            (Param::HoldTime, _) => act == Actuation::Hold,
            // (A full pull has its threshold; the trigger's tuning is the pull's.)
            (Param::Deadzone | Param::Full, Actuation::FullPull) => false,
            (p, _) => p.applies(input, output),
        }
    }

    /// The values it may take.
    pub fn range(self) -> std::ops::RangeInclusive<u8> {
        match self {
            Param::Speed | Param::Curve => 0..=100,
            Param::Threshold => 1..=100,
            Param::Deadzone => 0..=50,
            Param::Full => 50..=100,
            Param::InvertX | Param::InvertY | Param::Toggle => 0..=1,
            Param::HoldTime => 20..=100,
            Param::Turbo => 0..=30,
            Param::Accel => 0..=100,
            Param::AccelAt => 10..=200,
            Param::Tighten => 0..=50,
            Param::StickMin => 0..=50,
            Param::Twist | Param::Ratchet => 0..=1,
        }
    }

    /// Its value unless set, for `input` mapped to `output`: the same for every pair but a stick on
    /// the pointer, whose deadzone and curve are [`POINTER_DEADZONE`] and [`POINTER_CURVE`].
    pub fn default_for(self, input: Input, output: Output) -> u8 {
        let pointer = input.kind() == Kind::Stick && output == Output::Pointer;
        match self {
            Param::Speed => DEFAULT_SPEED,
            Param::Threshold => DEFAULT_THRESHOLD,
            Param::Full => 100,
            Param::Deadzone if pointer => POINTER_DEADZONE,
            Param::Curve if pointer => POINTER_CURVE,
            Param::Deadzone | Param::Curve | Param::InvertX | Param::InvertY | Param::Turbo | Param::Toggle => 0,
            Param::Accel | Param::StickMin | Param::Twist | Param::Ratchet => 0,
            Param::HoldTime => DEFAULT_HOLD_TIME,
            Param::AccelAt => DEFAULT_ACCEL_AT,
            Param::Tighten => DEFAULT_TIGHTEN,
        }
    }

    pub fn get(self, b: &Binding) -> u8 {
        match self {
            Param::Speed => b.speed,
            Param::Threshold => b.threshold,
            Param::Deadzone => b.deadzone,
            Param::Full => b.full,
            Param::Curve => b.curve,
            Param::InvertX => u8::from(b.invert_x),
            Param::InvertY => u8::from(b.invert_y),
            Param::HoldTime => b.hold_time,
            Param::Turbo => b.turbo,
            Param::Toggle => u8::from(b.toggle),
            Param::Accel => b.accel,
            Param::AccelAt => b.accel_at,
            Param::Tighten => b.tighten,
            Param::StickMin => b.stick_min,
            Param::Twist => u8::from(b.twist),
            Param::Ratchet => u8::from(b.ratchet),
        }
    }

    /// Sets it, kept within [`Param::range`].
    pub fn set(self, b: &mut Binding, value: u8) {
        let value = value.clamp(*self.range().start(), *self.range().end());
        match self {
            Param::Speed => b.speed = value,
            Param::Threshold => b.threshold = value,
            Param::Deadzone => b.deadzone = value,
            Param::Full => b.full = value,
            Param::Curve => b.curve = value,
            Param::InvertX => b.invert_x = value > 0,
            Param::InvertY => b.invert_y = value > 0,
            Param::HoldTime => b.hold_time = value,
            Param::Turbo => b.turbo = value,
            Param::Toggle => b.toggle = value > 0,
            Param::Accel => b.accel = value,
            Param::AccelAt => b.accel_at = value,
            Param::Tighten => b.tighten = value,
            Param::StickMin => b.stick_min = value,
            Param::Twist => b.twist = value > 0,
            Param::Ratchet => b.ratchet = value > 0,
        }
    }
}

/// The parameters `input` mapped to `output` has, in the order shown.
pub fn params(input: Input, output: Output) -> impl Iterator<Item = Param> {
    Param::ALL.into_iter().filter(move |p| p.applies(input, output))
}

/// The parameters of `input`'s binding of `act` mapped to `output` (a hold's has its time too).
pub fn params_of(input: Input, output: Output, act: Actuation) -> impl Iterator<Item = Param> {
    Param::ALL.into_iter().filter(move |p| p.applies_to(input, output, act))
}
