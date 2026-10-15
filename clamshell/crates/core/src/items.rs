//! List items by what they are, not where. Taps, long presses and level rows name list items by their
//! position (so drawing needs no allocation), but a list can change under a finger: a scan re-sorts the
//! networks, a device connects and moves up, an app stops playing, a game ends. A press remembers what
//! its item is when it comes down ([`ItemKey`]), and acts on that item wherever it is by then, or not at
//! all once it is gone.

use crate::State;
use crate::id::{AudioId, BtId, DockId, Id, ShelfId, WifiId};
use crate::levels::Level;
use crate::pointer::Press;

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
        let (wifi, bluetooth, audio) = (&self.radios.wifi, &self.radios.bluetooth, self.levels.audio());
        match id {
            Id::Wifi(WifiId::Network(i)) => wifi.networks.get(at(i)).and_then(|n| name(&n.ssid)),
            Id::Bluetooth(BtId::Device(i)) => bluetooth.devices.get(at(i)).and_then(|d| name(&d.address)),
            Id::Dock(DockId::GameTile(i)) => self.session.games().get(at(i)).map(|g| ItemKey::Name(g.instance.name())),
            Id::Shelf(ShelfId::Collection(i)) => self.shelf.library().collections.get(at(i)).and_then(|c| name(&c.name)),
            Id::Audio(AudioId::StreamMute(i)) | Id::Level(Level::Stream(i)) => audio.streams.get(at(i)).map(|s| ItemKey::Number(s.id)),
            Id::Audio(AudioId::Output(i)) => audio.outputs.get(at(i)).map(|o| ItemKey::Number(o.id)),
            Id::Level(Level::Backlight(i)) => self.levels.backlights().get(at(i)).and_then(|b| name(b)),
            _ => None,
        }
    }

    /// Where the item with `key` is now, as an id of the same kind as `id`.
    pub(crate) fn find_item(&self, id: &Id, key: &ItemKey) -> Option<Id> {
        let index = |p: Option<usize>| p.and_then(|p| u16::try_from(p).ok());
        let (wifi, bluetooth, audio) = (&self.radios.wifi, &self.radios.bluetooth, self.levels.audio());
        let stream = |n: &u32| index(audio.streams.iter().position(|x| x.id == *n));
        match (id, key) {
            (Id::Wifi(WifiId::Network(_)), ItemKey::Name(n)) => {
                index(wifi.networks.iter().position(|x| x.ssid == *n)).map(|i| Id::Wifi(WifiId::Network(i)))
            }
            (Id::Bluetooth(BtId::Device(_)), ItemKey::Name(n)) => {
                index(bluetooth.devices.iter().position(|x| x.address == *n)).map(|i| Id::Bluetooth(BtId::Device(i)))
            }
            (Id::Dock(DockId::GameTile(_)), ItemKey::Name(n)) => {
                index(self.session.games().iter().position(|x| x.instance.is(n))).map(|i| Id::Dock(DockId::GameTile(i)))
            }
            (Id::Shelf(ShelfId::Collection(_)), ItemKey::Name(n)) => {
                index(self.shelf.library().collections.iter().position(|x| x.name == *n)).map(|i| Id::Shelf(ShelfId::Collection(i)))
            }
            (Id::Level(Level::Backlight(_)), ItemKey::Name(n)) => {
                index(self.levels.backlights().iter().position(|x| x == n)).map(|i| Id::Level(Level::Backlight(i)))
            }
            (Id::Audio(AudioId::StreamMute(_)), ItemKey::Number(n)) => stream(n).map(|i| Id::Audio(AudioId::StreamMute(i))),
            (Id::Level(Level::Stream(_)), ItemKey::Number(n)) => stream(n).map(|i| Id::Level(Level::Stream(i))),
            (Id::Audio(AudioId::Output(_)), ItemKey::Number(n)) => {
                index(audio.outputs.iter().position(|x| x.id == *n)).map(|i| Id::Audio(AudioId::Output(i)))
            }
            _ => None,
        }
    }

    /// `id` as the press that came down on it meant it: the same item where it is now, `None` once it
    /// is gone. Ids that aren't list items, or weren't pressed, stand as they are.
    pub(crate) fn resolve(&self, id: &Id) -> Option<Id> {
        match &self.pointer.press {
            Some(Press { id: pressed, item }) if pressed == id => self.find_item(id, item),
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
    use crate::testkit::*;
    use crate::*;

    fn device(name: &str, paired: bool, connected: bool) -> Device {
        Device { address: format!("00:{name}"), name: name.into(), kind: Kind::Other, paired, connected }
    }

    #[test]
    fn a_long_press_forgets_the_device_it_came_down_on() {
        let mut s = state();
        let bt = |devices| Reading::Bluetooth(Bluetooth { adapter: true, powered: true, devices, ..Bluetooth::default() });
        let pad = Id::Bluetooth(BtId::Device(1));
        read(&mut s, bt(vec![device("buds", true, false), device("pad", true, false)]));
        s.apply(PointerEvent::Down(pad.clone()).into());
        // The buds connect and the list changes under the finger.
        read(&mut s, bt(vec![device("pad", true, false), device("buds", true, true)]));
        assert_eq!(long(&mut s, pad.clone()), vec![Effect::Bluetooth(Command::Forget("00:pad".into()))]);
        s.apply(PointerEvent::Up(pad).into());
        // Gone meanwhile: nothing.
        let first = Id::Bluetooth(BtId::Device(0));
        s.apply(PointerEvent::Down(first.clone()).into());
        read(&mut s, bt(vec![device("buds", true, true)]));
        assert_eq!(long(&mut s, first), vec![]);
    }

    #[test]
    fn a_level_drag_stays_on_its_app() {
        let mut s = state();
        let stream = |id, app: &str| Stream { id, app: app.into(), title: String::new(), volume: 50, muted: false };
        read(&mut s, Reading::Audio(Audio { outputs: vec![], streams: vec![stream(1, "a"), stream(2, "b")] }));
        s.apply(PointerEvent::Down(Id::Level(Level::Stream(0))).into());
        s.apply(LevelEvent::Set(Level::Stream(0), 60).into());
        // A new app sorts first.
        read(&mut s, Reading::Audio(Audio { outputs: vec![], streams: vec![stream(0, "new"), stream(1, "a"), stream(2, "b")] }));
        let fx = s.apply(LevelEvent::Set(Level::Stream(0), 70).into()).1;
        assert_eq!(fx, vec![Effect::Sound(Sound::StreamVolume(1, 70))]);
    }
}
