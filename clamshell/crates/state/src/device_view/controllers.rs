//! The players and the controllers. There are 1 to 8 players (one by default), each with a virtual pad of
//! its own for as long as the count stands, and its own keymap. Every controller the pad thread takes is
//! remembered by its key (`clamshell_system`'s `input::pad::players` makes it from what evdev gives) with
//! its name and its player, and saved. A controller seen for the first time joins player 1, and a notice
//! says so. The Device view shows the count and lists the controllers, the disconnected ones dimmed; a tap
//! on a row, or the pad's left and right on it, steps its value round (a controller's player within the
//! count). Lowering the count puts the controllers of the players gone on player 1.
//!
//! The pad thread decides nothing: it is sent the count and the players by key ([`crate::Effect::Players`])
//! whenever they change, and at start from the same file the state reads. A controller it doesn't know is
//! player 1, as here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bottom::notice::{NoticeKind, NoticeSlot};
use crate::effect::{Effect, Save};
use crate::out::Out;

/// A player, 1 to [`Player::MOST`]. Player 1 drives ClamShell and has the pad mouse; each player has a
/// virtual pad and a keymap of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Player(u8);

impl Player {
    pub const ONE: Player = Player(1);
    /// The most players there can be.
    pub const MOST: usize = 8;
    pub const ALL: [Player; Player::MOST] = [Player(1), Player(2), Player(3), Player(4), Player(5), Player(6), Player(7), Player(8)];

    /// Player `n`, if `n` is 1 to [`Player::MOST`].
    pub fn new(n: u8) -> Option<Player> {
        (1..=Player::MOST as u8).contains(&n).then_some(Player(n))
    }

    /// Its number, from 1.
    pub fn number(self) -> u8 {
        self.0
    }

    /// Its place in [`Player::ALL`], from 0.
    pub fn index(self) -> usize {
        usize::from(self.0 - 1)
    }

    /// The player `by` places on among the first `count`, round from the last to 1 and back.
    pub fn step(self, by: i8, count: Player) -> Player {
        let n = i16::from(count.0);
        Player(((i16::from(self.0) - 1 + i16::from(by)).rem_euclid(n) + 1) as u8)
    }

    /// The players 1 to `self`, when `self` is the count.
    pub fn up_to(self) -> impl Iterator<Item = Player> {
        Player::ALL.into_iter().take(usize::from(self.0))
    }
}

impl TryFrom<u8> for Player {
    type Error = String;

    fn try_from(n: u8) -> Result<Player, String> {
        Player::new(n).ok_or_else(|| format!("player {n}: players are 1 to {}", Player::MOST))
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

/// What is saved: how many players there are (as the last one), and every controller seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub players: Player,
    pub controllers: Vec<Controller>,
}

impl Default for Saved {
    fn default() -> Saved {
        Saved { players: Player::ONE, controllers: Vec::new() }
    }
}

/// What the pad thread is sent: how many players there are (as the last one), and the players by
/// controller key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Players {
    pub count: Player,
    pub of: BTreeMap<String, Player>,
}

impl Default for Players {
    fn default() -> Players {
        Players { count: Player::ONE, of: BTreeMap::new() }
    }
}

impl Players {
    /// The player of the controller with this key: its own, or player 1 if it has none or one past the
    /// count.
    pub fn of(&self, key: &str) -> Player {
        self.of.get(key).copied().filter(|p| *p <= self.count).unwrap_or(Player::ONE)
    }
}

/// What the pad thread says about controllers, and what the pad asks of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerEvent {
    /// A controller was taken, with its key and name.
    Connected { key: String, name: String },
    /// The controller with this key is gone.
    Disconnected(String),
    /// Steps the player of the controller on row `i` by `by` (the pad's left, -1, or right, 1).
    Step(u16, i8),
    /// Steps how many players there are by `by`, round from 8 to 1 and back.
    StepCount(i8),
}

/// The players and every controller seen, in the order first seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controllers {
    count: Player,
    list: Vec<Controller>,
}

impl Default for Controllers {
    fn default() -> Controllers {
        Controllers::new(Saved::default())
    }
}

impl Controllers {
    /// The controllers as saved, none connected yet. One saved with a player past the count is player 1.
    pub(crate) fn new(saved: Saved) -> Controllers {
        let count = saved.players;
        let list = saved.controllers.into_iter().map(|c| Controller { connected: false, player: if c.player <= count { c.player } else { Player::ONE }, ..c });
        Controllers { count, list: list.collect() }
    }

    pub fn list(&self) -> &[Controller] {
        &self.list
    }

    /// How many players there are, as the last one.
    pub fn count(&self) -> Player {
        self.count
    }

    /// The count and the players by key, for the pad thread.
    pub fn players(&self) -> Players {
        Players { count: self.count, of: self.list.iter().map(|c| (c.key.clone(), c.player)).collect() }
    }

    /// What is saved: the count, and every controller with its player.
    pub(crate) fn save_effect(&self) -> Effect {
        Effect::Save(Save::Controllers(Saved { players: self.count, controllers: self.list.clone() }))
    }

    /// Handles what the pad thread says, or a row stepped, at `now`. A controller seen for the first time
    /// joins player 1 with a notice, and is saved. A step changes a player or the count: saved, and sent to
    /// the pad thread. A row that isn't there, or a key not known, does nothing.
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
            ControllerEvent::StepCount(by) => self.step_count(by, out),
        }
    }

    /// Steps the player of the controller on row `i` by `by`, within the count. With one player there is
    /// nothing to step to.
    pub(crate) fn step(&mut self, i: usize, by: i8, out: &mut Out) {
        let count = self.count;
        let Some(c) = self.list.get_mut(i) else { return };
        let to = c.player.step(by, count);
        if to != c.player {
            c.player = to;
            self.changed(out);
        }
    }

    /// Steps how many players there are by `by`, round from [`Player::MOST`] to 1 and back. The
    /// controllers of players gone go to player 1.
    pub(crate) fn step_count(&mut self, by: i8, out: &mut Out) {
        let all = Player::ALL.last().copied().unwrap_or(Player::ONE);
        self.count = self.count.step(by, all);
        let count = self.count;
        for c in self.list.iter_mut().filter(|c| c.player > count) {
            c.player = Player::ONE;
        }
        self.changed(out);
    }

    /// Saves the players and sends them to the pad thread.
    fn changed(&self, out: &mut Out) {
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

    fn saved(fx: &[Effect]) -> Option<Saved> {
        fx.iter().rev().find_map(|e| match e {
            Effect::Save(Save::Controllers(saved)) => Some(saved.clone()),
            _ => None,
        })
    }

    fn players(s: &State) -> Vec<(String, bool, u8)> {
        s.device_view().controllers().list().iter().map(|c| (c.name.clone(), c.connected, c.player.number())).collect()
    }

    fn p(n: u8) -> Player {
        Player::new(n).unwrap_or(Player::ONE)
    }

    fn tap(s: &mut State, id: DeviceId) -> Vec<Effect> {
        s.applied(PointerEvent::Activate(Id::Device(id)).into())
    }

    /// Two players, two controllers, the second on player 2.
    fn two_players() -> State {
        let mut s = State::new(init());
        s.applied(connected("thor", "AYN Thor controller"));
        s.applied(connected("pad", "8BitDo Pro 2"));
        tap(&mut s, DeviceId::Players);
        tap(&mut s, DeviceId::Controller(1));
        s
    }

    #[test]
    fn players_step_round_within_the_count() {
        assert_eq!([1, 2, 3, 4].map(|n| p(n).step(1, p(4)).number()), [2, 3, 4, 1]);
        assert_eq!([1, 2, 3, 4].map(|n| p(n).step(-1, p(4)).number()), [4, 1, 2, 3]);
        assert_eq!(p(1).step(1, p(1)), p(1), "one player: nowhere to go");
        assert_eq!(p(8).step(1, p(8)), p(1));
        assert_eq!(Player::new(0), None);
        assert_eq!(Player::new(9), None);
        assert_eq!(serde_json::from_str::<Player>("9").ok(), None, "a file's player out of range isn't read");
        assert_eq!(serde_json::from_str::<Player>("3").ok(), Player::new(3));
        assert_eq!(p(3).up_to().map(Player::number).collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// A controller seen for the first time joins player 1: a notice says so and it is saved. Seen again
    /// after it went, it is the same controller, with no notice and nothing saved.
    #[test]
    fn a_new_controller_joins_player_1_with_a_notice_and_is_remembered() {
        let mut s = State::new(init());
        s.applied(Event::tick(100));
        let fx = s.applied(connected("045e:0b13:aa", "Xbox Wireless Controller"));
        assert_eq!(notice(&s), Some(NoticeKind::Controller("Xbox Wireless Controller".into(), Player::ONE)));
        let file = saved(&fx).map(|f| f.controllers.iter().map(|c| (c.key.clone(), c.player.number())).collect::<Vec<_>>());
        assert_eq!(file, Some(vec![("045e:0b13:aa".into(), 1)]));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Players(_))), "the pad thread takes a new one as player 1 itself");
        s.applied(Event::tick(103));
        s.applied(Event::Controller(ControllerEvent::Disconnected("045e:0b13:aa".into())));
        assert_eq!(players(&s), [("Xbox Wireless Controller".into(), false, 1)], "remembered, disconnected");
        let fx = s.applied(connected("045e:0b13:aa", "Xbox Wireless Controller"));
        assert_eq!(notice(&s), None, "known: no notice");
        assert_eq!(saved(&fx), None);
        assert_eq!(players(&s), [("Xbox Wireless Controller".into(), true, 1)]);
    }

    /// With one player a controller's row has nowhere to step to. The count's row steps the count, and
    /// then a row steps its controller round the players there are; the pad's left steps back. Each change
    /// is saved and sent to the pad thread with the count and every controller's player.
    #[test]
    fn a_row_tapped_or_stepped_cycles_its_player_within_the_count() {
        let mut s = State::new(init());
        s.applied(connected("thor", "AYN Thor controller"));
        s.applied(connected("pad", "8BitDo Pro 2"));
        assert!(tap(&mut s, DeviceId::Controller(1)).is_empty(), "one player");
        let fx = tap(&mut s, DeviceId::Players);
        assert_eq!(s.device_view().controllers().count(), p(2));
        assert!(fx.iter().any(|e| matches!(e, Effect::Players(Players { count, .. }) if *count == p(2))), "{fx:?}");
        let fx = tap(&mut s, DeviceId::Controller(1));
        let sent = Players { count: p(2), of: BTreeMap::from([("thor".into(), Player::ONE), ("pad".into(), p(2))]) };
        assert_eq!(fx, vec![s.device_view().controllers().save_effect(), Effect::Players(sent)]);
        assert_eq!(saved(&fx).map(|f| f.players), Some(p(2)));
        tap(&mut s, DeviceId::Controller(1));
        assert_eq!(players(&s)[1].2, 1, "round to 1");
        let fx = s.applied(Event::Controller(ControllerEvent::Step(0, -1)));
        assert_eq!(players(&s)[0].2, 2, "the built-in one too, back from 1 to the last");
        assert!(fx.iter().any(|e| matches!(e, Effect::Players(p) if p.of("thor") == Player::new(2).unwrap_or(Player::ONE))), "{fx:?}");
        assert!(s.applied(Event::Controller(ControllerEvent::Step(7, 1))).is_empty(), "no such row: nothing");
    }

    /// The count steps 1 to 8 and round; lowering it puts the controllers of the players gone on player 1.
    #[test]
    fn lowering_the_count_moves_controllers_of_players_gone_to_player_1() {
        let mut s = two_players();
        assert_eq!(players(&s)[1].2, 2);
        for _ in 0..6 {
            tap(&mut s, DeviceId::Players);
        }
        assert_eq!(s.device_view().controllers().count(), p(8));
        assert_eq!(players(&s)[1].2, 2, "still there");
        let fx = tap(&mut s, DeviceId::Players);
        assert_eq!(s.device_view().controllers().count(), p(1), "round to 1");
        assert_eq!(players(&s)[1].2, 1, "player 2 is gone: on player 1");
        assert!(fx.iter().any(|e| matches!(e, Effect::Players(p) if p.count == Player::ONE && p.of("pad") == Player::ONE)), "{fx:?}");
        s.applied(Event::Controller(ControllerEvent::StepCount(-1)));
        assert_eq!(s.device_view().controllers().count(), p(8), "back round to 8");
    }

    /// A remembered controller keeps its player while away, and can be given another while away. One
    /// saved past the count is player 1.
    #[test]
    fn a_disconnected_controller_keeps_its_player_and_can_be_given_another() {
        let controllers = vec![
            Controller { key: "pad".into(), name: "8BitDo Pro 2".into(), player: p(3), connected: true },
            Controller { key: "old".into(), name: "Old".into(), player: p(5), connected: false },
        ];
        let mut s = State::new(crate::Init { controllers: Saved { players: p(4), controllers }, ..init() });
        assert_eq!(players(&s), [("8BitDo Pro 2".into(), false, 3), ("Old".into(), false, 1)], "none connected at start");
        s.applied(Event::Controller(ControllerEvent::Step(0, 1)));
        let fx = s.applied(connected("pad", "8BitDo Pro 2 (renamed)"));
        assert_eq!(players(&s)[0], ("8BitDo Pro 2 (renamed)".into(), true, 4));
        assert_eq!(saved(&fx).map(|f| f.controllers[0].name.clone()), Some("8BitDo Pro 2 (renamed)".into()), "a new name is saved");
        assert_eq!(notice(&s), None);
    }

    /// The file holds the count and each controller's key, name and player, never whether it was
    /// connected.
    #[test]
    fn the_saved_form_has_no_connection() {
        let c = Controller { key: "k".into(), name: "n".into(), player: p(2), connected: true };
        let json = serde_json::to_string(&Saved { players: p(2), controllers: vec![c] }).unwrap_or_default();
        assert_eq!(json, r#"{"players":2,"controllers":[{"key":"k","name":"n","player":2}]}"#);
    }
}
