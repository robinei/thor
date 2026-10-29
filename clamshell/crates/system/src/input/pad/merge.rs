//! Several controllers as one player's controller. Each controller's events are first put in the virtual
//! pad's terms ([`Normalise`]): its sticks and triggers in the virtual pad's ranges ([`RANGES`]), and its
//! D-pad buttons, where it has them, as the hat. Then each player's controllers are merged into one
//! controller ([`Merge`]):
//!
//! - a button is down while it is down on any of them;
//! - each stick axis, trigger and hat axis has the value of whichever controller has it furthest from
//!   its rest (every axis rests at 0 in the virtual pad's terms; the first controller wins a tie).
//!
//! What comes out is one stream of events per player, as from one controller, with a report after each
//! change. It goes to that player's router as a real controller's events did. A controller that comes,
//! goes, or changes player, or whose events the kernel dropped, changes its player's controller all at
//! once: what differs comes out as events, Home's press first and its release last (see
//! `PadRouter`'s Home).
//!
//! Nothing here allocates once the buffers have grown.

use clamshell_state::device_view::controllers::Player;
use evdev::{AbsoluteAxisCode, KeyCode};

use super::route::Ranges;
use crate::os::evdev::{Press, Range, Raw};

/// The virtual pad's axes, as an Xbox 360 pad's with xpad: the sticks, the triggers and the hat.
pub(crate) const PAD_AXES: [AbsoluteAxisCode; 8] = [
    AbsoluteAxisCode::ABS_X,
    AbsoluteAxisCode::ABS_Y,
    AbsoluteAxisCode::ABS_Z,
    AbsoluteAxisCode::ABS_RX,
    AbsoluteAxisCode::ABS_RY,
    AbsoluteAxisCode::ABS_RZ,
    AbsoluteAxisCode::ABS_HAT0X,
    AbsoluteAxisCode::ABS_HAT0Y,
];

/// The virtual pad's sticks' and triggers' ranges, an Xbox 360 pad's.
pub(crate) const RANGES: Ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 255 } };

/// The hat's range.
pub(crate) const HAT: Range = Range { min: -1, max: 1 };

const HAT_X: usize = 6;
const HAT_Y: usize = 7;

/// Key codes go up to KEY_MAX, 0x2ff.
const KEY_WORDS: usize = 12;

/// The place of `code` in [`PAD_AXES`].
pub(crate) fn axis_index(code: AbsoluteAxisCode) -> Option<usize> {
    PAD_AXES.iter().position(|&c| c == code)
}

/// The virtual pad's range for axis `i` of [`PAD_AXES`].
pub(crate) fn pad_range(i: usize) -> Range {
    match PAD_AXES.get(i) {
        Some(&code) if super::codes::is_trigger(code) => RANGES.trigger,
        Some(&code) if super::codes::HAT.contains(&code) => HAT,
        _ => RANGES.stick,
    }
}

/// A controller's state in the virtual pad's terms: the keys held and the axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Inputs {
    keys: [u64; KEY_WORDS],
    axes: [i32; PAD_AXES.len()],
}

impl Inputs {
    pub(crate) fn held(&self, code: u16) -> bool {
        let (word, bit) = (usize::from(code / 64), code % 64);
        self.keys.get(word).is_some_and(|w| w & (1 << bit) != 0)
    }

    fn set(&mut self, code: u16, down: bool) {
        let (word, bit) = (usize::from(code / 64), code % 64);
        if let Some(w) = self.keys.get_mut(word) {
            if down {
                *w |= 1 << bit;
            } else {
                *w &= !(1 << bit);
            }
        }
    }

    /// Holds `code` down, as for a key that isn't the controller's own.
    pub(crate) fn press(&mut self, code: u16) {
        self.set(code, true);
    }

    pub(crate) fn axis(&self, i: usize) -> i32 {
        self.axes.get(i).copied().unwrap_or(0)
    }

    fn set_axis(&mut self, i: usize, value: i32) {
        if let Some(a) = self.axes.get_mut(i) {
            *a = value;
        }
    }

    /// The keys held, by code.
    fn keys(&self) -> impl Iterator<Item = u16> + '_ {
        (0..(KEY_WORDS * 64) as u16).filter(|&c| self.held(c))
    }

    /// A controller as its keys held and its axes say, in its own terms (`normalise`'s), with its D-pad
    /// buttons as the hat.
    pub(crate) fn of(normalise: &Normalise, keys: &[KeyCode], axes: &[(AbsoluteAxisCode, i32)]) -> Inputs {
        let mut inputs = Inputs::default();
        for k in keys {
            inputs.set(k.0, true);
        }
        for &(code, value) in axes {
            if let Some(Raw { code, value, .. }) = normalise.raw(Raw::abs(code, value))
                && let Some(i) = axis_index(AbsoluteAxisCode(code))
            {
                inputs.set_axis(i, value);
            }
        }
        if normalise.dpad {
            inputs.dpad_to_hat();
        }
        inputs
    }

    /// The hat from the D-pad's buttons, for a controller that has them (and so no hat of its own).
    /// Returns whether the hat changed.
    fn dpad_to_hat(&mut self) -> bool {
        let dir = |minus: KeyCode, plus: KeyCode| i32::from(self.held(plus.0)) - i32::from(self.held(minus.0));
        let (x, y) = (dir(KeyCode::BTN_DPAD_LEFT, KeyCode::BTN_DPAD_RIGHT), dir(KeyCode::BTN_DPAD_UP, KeyCode::BTN_DPAD_DOWN));
        let changed = (x, y) != (self.axis(HAT_X), self.axis(HAT_Y));
        self.set_axis(HAT_X, x);
        self.set_axis(HAT_Y, y);
        changed
    }
}

/// Whether `code` is one of the D-pad's buttons.
fn is_dpad(code: u16) -> bool {
    [KeyCode::BTN_DPAD_UP, KeyCode::BTN_DPAD_DOWN, KeyCode::BTN_DPAD_LEFT, KeyCode::BTN_DPAD_RIGHT].iter().any(|k| k.0 == code)
}

/// How a controller's axes become the virtual pad's: each of [`PAD_AXES`]' range on the controller, if
/// it has it, and whether its D-pad is buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Normalise {
    ranges: [Option<Range>; PAD_AXES.len()],
    dpad: bool,
}

impl Normalise {
    /// From the controller's axes and their ranges, and whether it has the D-pad as buttons. Axes the
    /// virtual pad doesn't have are left out.
    pub(crate) fn new(axes: impl IntoIterator<Item = (AbsoluteAxisCode, Range)>, dpad: bool) -> Normalise {
        let mut n = Normalise { dpad, ..Normalise::default() };
        for (code, range) in axes {
            if let Some(slot) = axis_index(code).and_then(|i| n.ranges.get_mut(i)) {
                *slot = Some(range);
            }
        }
        n
    }

    /// An event of the controller's in the virtual pad's terms: a stick's or trigger's value in its range,
    /// a hat's as -1, 0 or 1, a key as it is. An axis the virtual pad doesn't have gives none.
    pub(crate) fn raw(&self, raw: Raw) -> Option<Raw> {
        let Some((code, value)) = raw.as_abs() else { return Some(raw) };
        let i = axis_index(code)?;
        let from = self.ranges.get(i).copied().flatten()?;
        Some(Raw::abs(code, scale(value, from, pad_range(i), super::codes::is_trigger(code))))
    }
}

/// `value` in range `from` mapped to range `to`. A trigger maps bottom to bottom and top to top. A stick or
/// the hat maps its rest, the middle of its range, to the middle of `to`, and each end to its end.
fn scale(value: i32, from: Range, to: Range, trigger: bool) -> i32 {
    let [v, fmin, fmax, tmin, tmax] = [value, from.min, from.max, to.min, to.max].map(i64::from);
    let scaled = if trigger {
        if fmax <= fmin {
            return to.min;
        }
        tmin + (v - fmin) * (tmax - tmin) / (fmax - fmin)
    } else {
        let (fmid, tmid) = (i64::from(from.middle()), i64::from(to.middle()));
        let (span, out) = if v >= fmid { (fmax - fmid, tmax - tmid) } else { (fmid - fmin, tmid - tmin) };
        if span <= 0 {
            return to.middle();
        }
        tmid + (v - fmid) * out / span
    };
    i32::try_from(scaled.clamp(tmin, tmax)).unwrap_or(0)
}

/// One controller in the merge, by the slot the pad thread gave it.
#[derive(Debug, Clone, Copy)]
struct Pad {
    slot: u32,
    player: Player,
    inputs: Inputs,
}

/// Each player's events from the merge, for its router.
#[derive(Debug, Default)]
pub(crate) struct Merged {
    pub(crate) events: [Vec<Raw>; Player::MOST],
}

impl Merged {
    fn push(&mut self, player: Player, raw: Raw) {
        if let Some(events) = self.events.get_mut(player.index()) {
            events.push(raw);
        }
    }
}

/// Every controller's state, and each player's controller made of them.
#[derive(Debug, Default)]
pub(crate) struct Merge {
    pads: Vec<Pad>,
    merged: [Inputs; Player::MOST],
    /// The player's controller changed since its last report.
    changed: [bool; Player::MOST],
}

impl Merge {
    /// Player `player`'s controller as the merge has it.
    pub(crate) fn of(&self, player: Player) -> Inputs {
        self.merged.get(player.index()).copied().unwrap_or_default()
    }

    /// The player of the controller in `slot`, if it is in the merge.
    pub(crate) fn player(&self, slot: u32) -> Option<Player> {
        self.pads.iter().find(|p| p.slot == slot).map(|p| p.player)
    }

    /// Whether the controller in `slot` holds `code`.
    pub(crate) fn held(&self, slot: u32, code: u16) -> bool {
        self.pads.iter().any(|p| p.slot == slot && p.inputs.held(code))
    }

    /// A controller joins `player`'s, as `inputs` says it is.
    pub(crate) fn add(&mut self, slot: u32, player: Player, inputs: Inputs, out: &mut Merged) {
        self.pads.push(Pad { slot, player, inputs });
        self.remerge(player, out);
    }

    /// A controller is gone. Returns the player it was.
    pub(crate) fn remove(&mut self, slot: u32, out: &mut Merged) -> Option<Player> {
        let i = self.pads.iter().position(|p| p.slot == slot)?;
        let pad = self.pads.remove(i);
        self.remerge(pad.player, out);
        Some(pad.player)
    }

    /// A controller is now `player`'s: what it holds is let go on its old player's controller and held on
    /// the new one's.
    pub(crate) fn set_player(&mut self, slot: u32, player: Player, out: &mut Merged) {
        let Some(pad) = self.pads.iter_mut().find(|p| p.slot == slot) else { return };
        let old = std::mem::replace(&mut pad.player, player);
        if old != player {
            self.remerge(old, out);
            self.remerge(player, out);
        }
    }

    /// A controller is as `inputs` says, after the kernel dropped some of its events.
    pub(crate) fn resync(&mut self, slot: u32, inputs: Inputs, out: &mut Merged) {
        let Some(pad) = self.pads.iter_mut().find(|p| p.slot == slot) else { return };
        pad.inputs = inputs;
        let player = pad.player;
        self.remerge(player, out);
    }

    /// One event of the controller in `slot`, in the virtual pad's terms ([`Normalise::raw`]). What it
    /// changes on its player's controller comes out, and a report once one comes after a change. A key's
    /// repeat is dropped.
    pub(crate) fn event(&mut self, slot: u32, raw: Raw, out: &mut Merged) {
        let Some(pad) = self.pads.iter_mut().find(|p| p.slot == slot) else { return };
        let player = pad.player;
        if raw.is_report() {
            if let Some(changed) = self.changed.get_mut(player.index())
                && std::mem::take(changed)
            {
                out.push(player, Raw::REPORT);
            }
            return;
        }
        if let Some((key, press)) = raw.as_key() {
            if press == Press::Repeat {
                return;
            }
            pad.inputs.set(key.0, press.held());
            if is_dpad(key.0) {
                if pad.inputs.dpad_to_hat() {
                    self.merge_axis(player, HAT_X, out);
                    self.merge_axis(player, HAT_Y, out);
                }
                return;
            }
            self.merge_key(player, key.0, out);
        } else if let Some((code, value)) = raw.as_abs()
            && let Some(i) = axis_index(code)
        {
            pad.inputs.set_axis(i, value);
            self.merge_axis(player, i, out);
        }
    }

    fn merge_key(&mut self, player: Player, code: u16, out: &mut Merged) {
        let down = self.pads.iter().any(|p| p.player == player && p.inputs.held(code));
        let (Some(merged), Some(changed)) = (self.merged.get_mut(player.index()), self.changed.get_mut(player.index())) else { return };
        if merged.held(code) != down {
            merged.set(code, down);
            *changed = true;
            out.push(player, Raw::key(KeyCode(code), if down { Press::Down } else { Press::Up }));
        }
    }

    fn merge_axis(&mut self, player: Player, i: usize, out: &mut Merged) {
        let value = furthest(self.pads.iter().filter(|p| p.player == player).map(|p| p.inputs.axis(i)));
        let (Some(merged), Some(changed)) = (self.merged.get_mut(player.index()), self.changed.get_mut(player.index())) else { return };
        if merged.axis(i) != value {
            merged.set_axis(i, value);
            *changed = true;
            if let Some(&code) = PAD_AXES.get(i) {
                out.push(player, Raw::abs(code, value));
            }
        }
    }

    /// Works `player`'s controller out again from its controllers, and sends what changed: Home's press
    /// first, the other keys let go, the other keys pressed, the axes, Home's release last, then a report.
    fn remerge(&mut self, player: Player, out: &mut Merged) {
        let mut now = Inputs::default();
        for p in self.pads.iter().filter(|p| p.player == player) {
            for (w, k) in now.keys.iter_mut().zip(p.inputs.keys) {
                *w |= k;
            }
        }
        // The D-pad's buttons are the hat on the virtual pad.
        for k in [KeyCode::BTN_DPAD_UP, KeyCode::BTN_DPAD_DOWN, KeyCode::BTN_DPAD_LEFT, KeyCode::BTN_DPAD_RIGHT] {
            now.set(k.0, false);
        }
        for (i, axis) in now.axes.iter_mut().enumerate() {
            *axis = furthest(self.pads.iter().filter(|p| p.player == player).map(|p| p.inputs.axis(i)));
        }
        let before = self.of(player);
        if before == now {
            return;
        }
        let home = KeyCode::BTN_MODE.0;
        if now.held(home) && !before.held(home) {
            out.push(player, Raw::key(KeyCode::BTN_MODE, Press::Down));
        }
        for k in before.keys().filter(|&k| k != home && !now.held(k)) {
            out.push(player, Raw::key(KeyCode(k), Press::Up));
        }
        for k in now.keys().filter(|&k| k != home && !before.held(k)) {
            out.push(player, Raw::key(KeyCode(k), Press::Down));
        }
        for (i, code) in PAD_AXES.iter().enumerate() {
            if now.axis(i) != before.axis(i) {
                out.push(player, Raw::abs(*code, now.axis(i)));
            }
        }
        if before.held(home) && !now.held(home) {
            out.push(player, Raw::key(KeyCode::BTN_MODE, Press::Up));
        }
        out.push(player, Raw::REPORT);
        if let Some(m) = self.merged.get_mut(player.index()) {
            *m = now;
        }
        if let Some(c) = self.changed.get_mut(player.index()) {
            *c = false;
        }
    }
}

/// The value furthest from rest (0) of an axis' values, the first of those as far; 0 for none.
fn furthest(values: impl Iterator<Item = i32>) -> i32 {
    values.fold(0, |best, v| if v.unsigned_abs() > best.unsigned_abs() { v } else { best })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const X: AbsoluteAxisCode = AbsoluteAxisCode::ABS_X;
    const RZ: AbsoluteAxisCode = AbsoluteAxisCode::ABS_RZ;

    fn p(n: u8) -> Player {
        Player::new(n).unwrap()
    }

    fn key(code: KeyCode, down: bool) -> Raw {
        Raw::key(code, if down { Press::Down } else { Press::Up })
    }

    /// A merge with two controllers on player 1, slots 1 and 2, at rest.
    fn two() -> (Merge, Merged) {
        let (mut m, mut out) = (Merge::default(), Merged::default());
        m.add(1, Player::ONE, Inputs::default(), &mut out);
        m.add(2, Player::ONE, Inputs::default(), &mut out);
        (m, out)
    }

    fn events(out: &mut Merged, player: Player) -> Vec<Raw> {
        out.events.get_mut(player.index()).map(std::mem::take).unwrap_or_default()
    }

    /// Sticks and the hat rest in the middle and reach the ends whatever the controller's range; a
    /// trigger goes from its bottom to its top. Each in the virtual pad's ranges.
    /// `value` of axis `code` on a controller normalised by `n`, in the virtual pad's terms.
    fn pad_value(n: &Normalise, code: AbsoluteAxisCode, value: i32) -> Option<i32> {
        let raw = n.raw(Raw::abs(code, value))?;
        Some(raw.value)
    }

    #[test]
    fn axes_are_put_in_the_virtual_pads_ranges_from_each_controllers_rest() {
        let y = AbsoluteAxisCode::ABS_Y;
        let z = AbsoluteAxisCode::ABS_Z;
        let axes = vec![
            (X, Range { min: 0, max: 255 }),
            (y, Range { min: -1024, max: 1024 }),
            (AbsoluteAxisCode::ABS_RX, Range::STICK),
            (z, Range { min: 0, max: 1023 }),
            (RZ, Range { min: 0, max: 255 }),
        ];
        let n = Normalise::new(axes, false);
        // The DualSense's 0..255 stick rests at 128.
        assert_eq!(pad_value(&n, X, 128), Some(0));
        assert_eq!(pad_value(&n, X, 0), Some(-32768));
        assert_eq!(pad_value(&n, X, 255), Some(32767));
        assert_eq!(pad_value(&n, y, 0), Some(0));
        assert_eq!(pad_value(&n, y, -1024), Some(-32768));
        assert_eq!(pad_value(&n, y, 512), Some(16383));
        assert_eq!(pad_value(&n, AbsoluteAxisCode::ABS_RX, -1234), Some(-1234), "already in range");
        // A 0..1023 trigger, and a 0..255 one as it is.
        assert_eq!(pad_value(&n, z, 0), Some(0));
        assert_eq!(pad_value(&n, z, 1023), Some(255));
        assert_eq!(pad_value(&n, RZ, 255), Some(255));
        assert_eq!(pad_value(&n, AbsoluteAxisCode::ABS_HAT0X, 1), None, "no such axis on the controller");
        assert_eq!(n.raw(Raw::abs(AbsoluteAxisCode::ABS_MISC, 3)), None, "not the virtual pad's");
        assert_eq!(n.raw(key(KeyCode::BTN_SOUTH, true)), Some(key(KeyCode::BTN_SOUTH, true)));
    }

    /// A button is down while it is down on any of the player's controllers.
    #[test]
    fn a_button_is_down_while_any_controller_holds_it() {
        let (mut m, mut out) = two();
        events(&mut out, Player::ONE);
        m.event(1, key(KeyCode::BTN_SOUTH, true), &mut out);
        m.event(1, Raw::REPORT, &mut out);
        assert_eq!(events(&mut out, Player::ONE), [key(KeyCode::BTN_SOUTH, true), Raw::REPORT]);
        m.event(2, key(KeyCode::BTN_SOUTH, true), &mut out);
        m.event(2, Raw::REPORT, &mut out);
        assert_eq!(events(&mut out, Player::ONE), [], "already down: nothing, not even a report");
        m.event(1, key(KeyCode::BTN_SOUTH, false), &mut out);
        m.event(1, Raw::REPORT, &mut out);
        assert_eq!(events(&mut out, Player::ONE), [], "still held on the other");
        m.event(2, key(KeyCode::BTN_SOUTH, false), &mut out);
        m.event(2, Raw::REPORT, &mut out);
        assert_eq!(events(&mut out, Player::ONE), [key(KeyCode::BTN_SOUTH, false), Raw::REPORT]);
        m.event(1, Raw { kind: evdev::EventType::KEY, code: KeyCode::BTN_EAST.0, value: 2 }, &mut out);
        assert_eq!(events(&mut out, Player::ONE), [], "a repeat is dropped");
    }

    /// Each axis has the value furthest from rest among the controllers, the first one on a tie.
    #[test]
    fn each_axis_is_the_controller_furthest_from_rest() {
        let (mut m, mut out) = two();
        m.event(1, Raw::abs(X, 1000), &mut out);
        m.event(2, Raw::abs(X, -3000), &mut out);
        assert_eq!(m.of(Player::ONE).axis(0), -3000);
        m.event(1, Raw::abs(X, 3000), &mut out);
        assert_eq!(m.of(Player::ONE).axis(0), 3000, "as far: the first controller's");
        m.event(2, Raw::abs(X, 10), &mut out);
        assert_eq!(m.of(Player::ONE).axis(0), 3000, "unchanged: no event");
        m.event(1, Raw::abs(RZ, 40), &mut out);
        m.event(2, Raw::abs(RZ, 200), &mut out);
        m.event(2, Raw::REPORT, &mut out);
        assert_eq!(events(&mut out, Player::ONE), [Raw::abs(X, 1000), Raw::abs(X, -3000), Raw::abs(X, 3000), Raw::abs(RZ, 40), Raw::abs(RZ, 200), Raw::REPORT]);
    }

    /// A controller with the D-pad as buttons gives the hat, merged per axis with another's hat.
    #[test]
    fn dpad_buttons_are_the_hat_merged_per_axis() {
        let (mut m, mut out) = two();
        events(&mut out, Player::ONE);
        m.event(1, key(KeyCode::BTN_DPAD_LEFT, true), &mut out);
        m.event(2, Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1), &mut out);
        m.event(2, Raw::REPORT, &mut out);
        assert_eq!(events(&mut out, Player::ONE), [Raw::abs(AbsoluteAxisCode::ABS_HAT0X, -1), Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1), Raw::REPORT]);
        m.event(1, key(KeyCode::BTN_DPAD_LEFT, false), &mut out);
        assert_eq!(events(&mut out, Player::ONE), [Raw::abs(AbsoluteAxisCode::ABS_HAT0X, 0)]);
        assert!(!m.of(Player::ONE).held(KeyCode::BTN_DPAD_LEFT.0), "never a button on the virtual pad");
    }

    /// A controller that goes lets go of what only it held; what another holds stays.
    #[test]
    fn a_controller_gone_lets_go_of_what_only_it_held() {
        let (mut m, mut out) = two();
        m.event(1, key(KeyCode::BTN_SOUTH, true), &mut out);
        m.event(1, key(KeyCode::BTN_EAST, true), &mut out);
        m.event(1, Raw::abs(X, 9000), &mut out);
        m.event(2, key(KeyCode::BTN_EAST, true), &mut out);
        m.event(2, Raw::REPORT, &mut out);
        events(&mut out, Player::ONE);
        assert_eq!(m.remove(1, &mut out), Some(Player::ONE));
        assert_eq!(events(&mut out, Player::ONE), [key(KeyCode::BTN_SOUTH, false), Raw::abs(X, 0), Raw::REPORT]);
        assert_eq!(m.remove(1, &mut out), None, "gone already");
        let held = Inputs::of(&Normalise::new([], true), &[KeyCode::BTN_NORTH, KeyCode::BTN_DPAD_UP], &[]);
        m.add(3, Player::ONE, held, &mut out);
        assert_eq!(
            events(&mut out, Player::ONE),
            [key(KeyCode::BTN_NORTH, true), Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1), Raw::REPORT],
            "one that comes holding a button and the D-pad's up"
        );
    }

    /// Players' controllers are apart: one player's controller's events are only its.
    #[test]
    fn each_player_has_its_own_controller() {
        let (mut m, mut out) = two();
        m.add(3, p(2), Inputs::default(), &mut out);
        m.event(3, key(KeyCode::BTN_SOUTH, true), &mut out);
        m.event(3, Raw::REPORT, &mut out);
        assert_eq!(events(&mut out, Player::ONE), []);
        assert_eq!(events(&mut out, p(2)), [key(KeyCode::BTN_SOUTH, true), Raw::REPORT]);
        // Moved to player 1, it holds A there and lets it go on player 2.
        m.set_player(3, Player::ONE, &mut out);
        assert_eq!(events(&mut out, p(2)), [key(KeyCode::BTN_SOUTH, false), Raw::REPORT]);
        assert_eq!(events(&mut out, Player::ONE), [key(KeyCode::BTN_SOUTH, true), Raw::REPORT]);
        assert_eq!(m.player(3), Some(Player::ONE));
    }

    /// After dropped events, a controller is as the kernel says: Home's press comes before the other
    /// buttons and its release after them, so a button Home took is let go while still Home's.
    #[test]
    fn a_resync_presses_home_first_and_lets_it_go_last() {
        let (mut m, mut out) = two();
        m.event(1, key(KeyCode::BTN_SOUTH, true), &mut out);
        m.event(1, Raw::REPORT, &mut out);
        events(&mut out, Player::ONE);
        let n = Normalise::new([(RZ, Range { min: 0, max: 1023 })], false);
        m.resync(1, Inputs::of(&n, &[KeyCode::BTN_EAST, KeyCode::BTN_MODE], &[(RZ, 1023)]), &mut out);
        assert_eq!(
            events(&mut out, Player::ONE),
            [key(KeyCode::BTN_MODE, true), key(KeyCode::BTN_SOUTH, false), key(KeyCode::BTN_EAST, true), Raw::abs(RZ, 255), Raw::REPORT]
        );
        m.resync(1, Inputs::of(&n, &[KeyCode::BTN_EAST, KeyCode::BTN_MODE], &[(RZ, 1023)]), &mut out);
        assert_eq!(events(&mut out, Player::ONE), [], "in step");
        m.resync(1, Inputs::default(), &mut out);
        assert_eq!(events(&mut out, Player::ONE), [key(KeyCode::BTN_EAST, false), Raw::abs(RZ, 0), key(KeyCode::BTN_MODE, false), Raw::REPORT]);
    }
}
