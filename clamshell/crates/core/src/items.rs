//! List items by what they are, not where. Taps, long presses and level rows name list items by their
//! position (so drawing needs no allocation), but a list can change under a finger: a scan re-sorts the
//! networks, a device connects and moves up, an app stops playing, a game ends. A press remembers what
//! its item is when it comes down ([`ItemKey`]), and acts on that item wherever it is by then, or not at
//! all once it is gone.

use crate::levels::Level;
use crate::{Id, State};

/// What a list item is: a network's SSID, a device's address, a game's workspace, a collection's name,
/// a backlight's name; a PipeWire object's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ItemKey {
    Name(String),
    Number(u32),
}

impl State {
    /// What the item `id` points at is, if `id` is a list item.
    pub(crate) fn item_key(&self, id: &Id) -> Option<ItemKey> {
        let at = |i: &u16| usize::from(*i);
        let name = |s: &str| Some(ItemKey::Name(s.to_owned()));
        match id {
            Id::Network(i) => self.wifi.networks.get(at(i)).and_then(|n| name(&n.ssid)),
            Id::Device(i) => self.bluetooth.devices.get(at(i)).and_then(|d| name(&d.address)),
            Id::GameTile(i) => self.games.get(at(i)).and_then(|g| name(&g.workspace)),
            Id::Collection(i) => self.library.collections.get(at(i)).and_then(|c| name(&c.name)),
            Id::StreamMute(i) | Id::Level(Level::Stream(i)) => self.audio.streams.get(at(i)).map(|s| ItemKey::Number(s.id)),
            Id::Output(i) => self.audio.outputs.get(at(i)).map(|o| ItemKey::Number(o.id)),
            Id::Level(Level::Backlight(i)) => self.backlights.get(at(i)).and_then(|b| name(b)),
            _ => None,
        }
    }

    /// Where the item with `key` is now, as an id of the same kind as `id`.
    fn find_item(&self, id: &Id, key: &ItemKey) -> Option<Id> {
        let index = |p: Option<usize>| p.and_then(|p| u16::try_from(p).ok());
        match (id, key) {
            (Id::Network(_), ItemKey::Name(n)) => index(self.wifi.networks.iter().position(|x| x.ssid == *n)).map(Id::Network),
            (Id::Device(_), ItemKey::Name(n)) => index(self.bluetooth.devices.iter().position(|x| x.address == *n)).map(Id::Device),
            (Id::GameTile(_), ItemKey::Name(n)) => index(self.games.iter().position(|x| x.workspace == *n)).map(Id::GameTile),
            (Id::Collection(_), ItemKey::Name(n)) => index(self.library.collections.iter().position(|x| x.name == *n)).map(Id::Collection),
            (Id::Level(Level::Backlight(_)), ItemKey::Name(n)) => {
                index(self.backlights.iter().position(|x| x == n)).map(|i| Id::Level(Level::Backlight(i)))
            }
            (Id::StreamMute(_), ItemKey::Number(n)) => index(self.audio.streams.iter().position(|x| x.id == *n)).map(Id::StreamMute),
            (Id::Level(Level::Stream(_)), ItemKey::Number(n)) => {
                index(self.audio.streams.iter().position(|x| x.id == *n)).map(|i| Id::Level(Level::Stream(i)))
            }
            (Id::Output(_), ItemKey::Number(n)) => index(self.audio.outputs.iter().position(|x| x.id == *n)).map(Id::Output),
            _ => None,
        }
    }

    /// `id` as the press that came down on it meant it: the same item where it is now, `None` once it
    /// is gone. Ids that aren't list items, or weren't pressed, stand as they are.
    pub(crate) fn resolve(&self, id: &Id) -> Option<Id> {
        match &self.press_key {
            Some((pressed, key)) if pressed == id => self.find_item(id, key),
            _ => Some(id.clone()),
        }
    }

    /// A level row as the finger on it meant it (see [`State::resolve`]).
    pub(crate) fn resolve_level(&self, level: Level) -> Option<Level> {
        match self.resolve(&Id::Level(level)) {
            Some(Id::Level(level)) => Some(level),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bluetooth::{Bluetooth, Command, Device, Kind};
    use crate::levels::{Audio, Level, Stream};
    use crate::tests::state;
    use crate::{Effect, Event, Id};

    fn device(name: &str, paired: bool, connected: bool) -> Device {
        Device { address: format!("00:{name}"), name: name.into(), kind: Kind::Other, paired, connected }
    }

    #[test]
    fn a_long_press_forgets_the_device_it_came_down_on() {
        let mut s = state();
        let bt = |devices| Event::Bluetooth(Bluetooth { adapter: true, powered: true, devices, ..Bluetooth::default() });
        s.apply(bt(vec![device("buds", true, false), device("pad", true, false)]));
        s.apply(Event::Down(Id::Device(1)));
        // The buds connect and the list changes under the finger.
        s.apply(bt(vec![device("pad", true, false), device("buds", true, true)]));
        assert_eq!(s.apply(Event::LongPress(Id::Device(1))).1, vec![Effect::Bluetooth(Command::Forget("00:pad".into()))]);
        s.apply(Event::Up(Id::Device(1)));
        // Gone meanwhile: nothing.
        s.apply(Event::Down(Id::Device(0)));
        s.apply(bt(vec![device("buds", true, true)]));
        assert_eq!(s.apply(Event::LongPress(Id::Device(0))).1, vec![]);
    }

    #[test]
    fn a_level_drag_stays_on_its_app() {
        let mut s = state();
        let stream = |id, app: &str| Stream { id, app: app.into(), title: String::new(), volume: 50, muted: false };
        s.apply(Event::Audio(Audio { outputs: vec![], streams: vec![stream(1, "a"), stream(2, "b")] }));
        s.apply(Event::Down(Id::Level(Level::Stream(0))));
        s.apply(Event::SetLevel(Level::Stream(0), 60));
        // A new app sorts first.
        s.apply(Event::Audio(Audio { outputs: vec![], streams: vec![stream(0, "new"), stream(1, "a"), stream(2, "b")] }));
        let fx = s.apply(Event::SetLevel(Level::Stream(0), 70)).1;
        assert_eq!(fx, vec![Effect::Audio(crate::levels::Command::StreamVolume(1, 70))]);
    }
}
