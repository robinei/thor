//! The controllers and their players. Every controller the pad thread takes is remembered by its key
//! (`clamshell_system`'s `input::pad::players` makes it from what evdev gives) with its name and its
//! player, 1 to 4, and saved. A controller seen for the first time joins player 1, and a notice says so.
//! The Device view lists them all, the disconnected ones dimmed, and a tap on a row, or the pad's left and
//! right on it, steps its player round (1, 2, 3, 4, 1).
//!
//! The pad thread decides nothing: it is sent the players by key ([`crate::Effect::Players`]) whenever
//! one changes, and at start from the same file the state reads. One it doesn't know is player 1, as
//! here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bottom::notice::{NoticeKind, NoticeSlot};
use crate::effect::{Effect, Save};
use crate::out::Out;

/// A player, 1 to 4. Player 1 drives ClamShell and has the keymaps and the pad mouse; players 2 to 4 each
/// get a virtual pad of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Player(u8);

impl Player {
    pub const ONE: Player = Player(1);
    pub const ALL: [Player; 4] = [Player(1), Player(2), Player(3), Player(4)];

    /// Player `n`, if `n` is 1 to 4.
    pub fn new(n: u8) -> Option<Player> {
        (1..=4).contains(&n).then_some(Player(n))
    }

    /// Its number, 1 to 4.
    pub fn number(self) -> u8 {
        self.0
    }

    /// Its place in [`Player::ALL`], 0 to 3.
    pub fn index(self) -> usize {
        usize::from(self.0 - 1)
    }

    /// The player `by` places on, round from 4 to 1 and from 1 to 4.
    pub fn step(self, by: i8) -> Player {
        Player((i16::from(self.0) - 1 + i16::from(by)).rem_euclid(4) as u8 + 1)
    }
}

impl TryFrom<u8> for Player {
    type Error = String;

    fn try_from(n: u8) -> Result<Player, String> {
        Player::new(n).ok_or_else(|| format!("player {n}: players are 1 to 4"))
    }
}

impl From<Player> for u8 {
    fn from(p: Player) -> u8 {
        p.0
    }
}

/// A controller remembered: its key, its name as it gives it, its player, and whether it is connected
/// now (not saved).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Controller {
    pub key: String,
    pub name: String,
    pub player: Player,
    #[serde(skip)]
    pub connected: bool,
}

/// The players by controller key, as the pad thread is sent them.
pub type Players = BTreeMap<String, Player>;

/// What the pad thread says about controllers, and what the pad asks of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerEvent {
    /// A controller was taken, with its key and name.
    Connected { key: String, name: String },
    /// The controller with this key is gone.
    Disconnected(String),
    /// Steps the player of the controller on row `i` by `by` (the pad's left, -1, or right, 1).
    Step(u16, i8),
}

/// Every controller seen, in the order first seen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Controllers {
    list: Vec<Controller>,
}

impl Controllers {
    /// The controllers as saved, none connected yet.
    pub(crate) fn new(saved: Vec<Controller>) -> Controllers {
        Controllers { list: saved.into_iter().map(|c| Controller { connected: false, ..c }).collect() }
    }

    pub fn list(&self) -> &[Controller] {
        &self.list
    }

    /// The players by key, for the pad thread.
    pub fn players(&self) -> Players {
        self.list.iter().map(|c| (c.key.clone(), c.player)).collect()
    }

    /// What is saved: every controller, with its player.
    pub(crate) fn save_effect(&self) -> Effect {
        Effect::Save(Save::Controllers(self.list.clone()))
    }

    /// Handles what the pad thread says, or a row stepped, at `now`. A controller seen for the first time
    /// joins player 1 with a notice, and is saved. A step changes a player: saved, and sent to the pad
    /// thread. A row that isn't there, or a key not known, does nothing.
    pub(crate) fn event(&mut self, event: ControllerEvent, notice: &mut NoticeSlot, now: u64, out: &mut Out) {
        match event {
            ControllerEvent::Connected { key, name } => match self.list.iter_mut().find(|c| c.key == key) {
                Some(c) => {
                    c.connected = true;
                    if c.name != name {
                        c.name = name;
                        out.effects.push(self.save_effect());
                    }
                }
                None => {
                    notice.show(NoticeKind::Controller(name.clone(), Player::ONE), now);
                    self.list.push(Controller { key, name, player: Player::ONE, connected: true });
                    out.effects.push(self.save_effect());
                }
            },
            ControllerEvent::Disconnected(key) => {
                if let Some(c) = self.list.iter_mut().find(|c| c.key == key) {
                    c.connected = false;
                }
            }
            ControllerEvent::Step(i, by) => self.step(usize::from(i), by, out),
        }
    }

    /// Steps the player of the controller on row `i` by `by`.
    pub(crate) fn step(&mut self, i: usize, by: i8, out: &mut Out) {
        let Some(c) = self.list.get_mut(i) else { return };
        c.player = c.player.step(by);
        out.effects.extend([self.save_effect(), Effect::Players(self.players())]);
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{DeviceId, Event, Id, PointerEvent, State};

    fn connected(key: &str, name: &str) -> Event {
        Event::Controller(ControllerEvent::Connected { key: key.into(), name: name.into() })
    }

    fn saved(fx: &[Effect]) -> Option<Vec<(String, u8, bool)>> {
        fx.iter().rev().find_map(|e| match e {
            Effect::Save(Save::Controllers(list)) => Some(list.iter().map(|c| (c.key.clone(), c.player.number(), c.connected)).collect()),
            _ => None,
        })
    }

    fn players(s: &State) -> Vec<(String, bool, u8)> {
        s.device_view().controllers().list().iter().map(|c| (c.name.clone(), c.connected, c.player.number())).collect()
    }

    #[test]
    fn players_step_round_both_ways() {
        let p = |n| Player::new(n).unwrap_or(Player::ONE);
        assert_eq!([1, 2, 3, 4].map(|n| p(n).step(1).number()), [2, 3, 4, 1]);
        assert_eq!([1, 2, 3, 4].map(|n| p(n).step(-1).number()), [4, 1, 2, 3]);
        assert_eq!(Player::new(0), None);
        assert_eq!(Player::new(5), None);
        assert_eq!(serde_json::from_str::<Player>("5").ok(), None, "a file's player out of range isn't read");
        assert_eq!(serde_json::from_str::<Player>("3").ok(), Player::new(3));
    }

    /// A controller seen for the first time joins player 1: a notice says so and it is saved. Seen again
    /// after it went, it is the same controller, with no notice and nothing saved.
    #[test]
    fn a_new_controller_joins_player_1_with_a_notice_and_is_remembered() {
        let mut s = State::new(init());
        s.applied(Event::tick(100));
        let fx = s.applied(connected("045e:0b13:aa", "Xbox Wireless Controller"));
        assert_eq!(notice(&s), Some(NoticeKind::Controller("Xbox Wireless Controller".into(), Player::ONE)));
        assert_eq!(saved(&fx), Some(vec![("045e:0b13:aa".into(), 1, true)]));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Players(_))), "the pad thread takes a new one as player 1 itself");
        s.applied(Event::tick(103));
        s.applied(Event::Controller(ControllerEvent::Disconnected("045e:0b13:aa".into())));
        assert_eq!(players(&s), [("Xbox Wireless Controller".into(), false, 1)], "remembered, disconnected");
        let fx = s.applied(connected("045e:0b13:aa", "Xbox Wireless Controller"));
        assert_eq!(notice(&s), None, "known: no notice");
        assert_eq!(saved(&fx), None);
        assert_eq!(players(&s), [("Xbox Wireless Controller".into(), true, 1)]);
    }

    /// A tap on a row steps its player on, round from 4 to 1; the pad's left steps it back. Each change is
    /// saved and sent to the pad thread with every controller's player.
    #[test]
    fn a_row_tapped_or_stepped_cycles_its_player_saved_and_sent() {
        let mut s = State::new(init());
        s.applied(connected("thor", "AYN Thor controller"));
        s.applied(connected("pad", "8BitDo Pro 2"));
        let tap = |s: &mut State| s.applied(PointerEvent::Activate(Id::Device(DeviceId::Controller(1))).into());
        let fx = tap(&mut s);
        let sent = Players::from([("thor".into(), Player::ONE), ("pad".into(), Player(2))]);
        assert_eq!(fx, vec![Effect::Save(Save::Controllers(s.device_view().controllers().list().to_vec())), Effect::Players(sent)]);
        tap(&mut s);
        tap(&mut s);
        assert_eq!(players(&s)[1].2, 4);
        tap(&mut s);
        assert_eq!(players(&s)[1].2, 1, "round to 1");
        let fx = s.applied(Event::Controller(ControllerEvent::Step(0, -1)));
        assert_eq!(players(&s)[0].2, 4, "the built-in one too, back from 1 to 4");
        assert!(fx.iter().any(|e| matches!(e, Effect::Players(p) if p.get("thor") == Some(&Player(4)))), "{fx:?}");
        assert!(s.applied(Event::Controller(ControllerEvent::Step(7, 1))).is_empty(), "no such row: nothing");
    }

    /// A remembered controller keeps its player while away, and can be given another while away.
    #[test]
    fn a_disconnected_controller_keeps_its_player_and_can_be_given_another() {
        let saved = vec![Controller { key: "pad".into(), name: "8BitDo Pro 2".into(), player: Player(3), connected: true }];
        let mut s = State::new(crate::Init { controllers: saved, ..init() });
        assert_eq!(players(&s), [("8BitDo Pro 2".into(), false, 3)], "none connected at start");
        s.applied(Event::Controller(ControllerEvent::Step(0, 1)));
        let fx = s.applied(connected("pad", "8BitDo Pro 2 (renamed)"));
        assert_eq!(players(&s), [("8BitDo Pro 2 (renamed)".into(), true, 4)]);
        assert_eq!(saved_names(&fx), Some(vec!["8BitDo Pro 2 (renamed)".into()]), "a new name is saved");
        assert_eq!(notice(&s), None);
    }

    fn saved_names(fx: &[Effect]) -> Option<Vec<String>> {
        fx.iter().find_map(|e| match e {
            Effect::Save(Save::Controllers(list)) => Some(list.iter().map(|c| c.name.clone()).collect()),
            _ => None,
        })
    }

    /// The file holds each controller's key, name and player, never whether it was connected.
    #[test]
    fn the_saved_form_has_no_connection() {
        let c = Controller { key: "k".into(), name: "n".into(), player: Player(2), connected: true };
        let json = serde_json::to_string(&c).unwrap_or_default();
        assert_eq!(json, r#"{"key":"k","name":"n","player":2}"#);
    }
}
