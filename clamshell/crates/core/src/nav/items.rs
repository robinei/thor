//! List items by what they are, not where they are. Taps, long presses and level rows name list items by
//! position, so drawing needs no allocation. But a list can change under a finger: a scan re-sorts the
//! networks, a device connects and moves up, an app stops playing, a game ends. So a press remembers
//! what its item is when it comes down ([`crate::kit::list::ItemKey`]), and acts on that item wherever it is by then, or
//! not at all once it is gone.

#[cfg(test)]
mod tests {
    use crate::levels::{Audio, Level, Stream};
    use crate::radios::bluetooth::{Bluetooth, Command, Device, Kind};
    use crate::testkit::*;
    use crate::*;

    fn device(name: &str, paired: bool, connected: bool) -> Device {
        Device { address: format!("00:{name}"), name: name.into(), kind: Kind::Other, paired, connected }
    }

    #[test]
    fn a_long_press_forgets_the_device_it_came_down_on() {
        let mut s = state();
        let bt = |devices| Reading::Bluetooth(Bluetooth { present: true, on: true, devices, ..Bluetooth::default() });
        let pad = ListId::Bluetooth.row(1);
        read(&mut s, bt(vec![device("buds", true, false), device("pad", true, false)]));
        s.apply(PointerEvent::Down(pad.clone()).into());
        // The buds connect and the list changes under the finger.
        read(&mut s, bt(vec![device("pad", true, false), device("buds", true, true)]));
        assert_eq!(long(&mut s, pad.clone()), vec![Effect::Bluetooth(Command::Forget("00:pad".into()))]);
        s.apply(PointerEvent::Up(pad).into());
        // If it is gone in the meantime, nothing happens.
        let first = ListId::Bluetooth.row(0);
        s.apply(PointerEvent::Down(first.clone()).into());
        read(&mut s, bt(vec![device("buds", true, true)]));
        assert_eq!(long(&mut s, first), vec![]);
    }

    #[test]
    fn a_level_drag_stays_on_its_app() {
        let mut s = state();
        let stream = |id, app: &str| Stream { id, app: app.into(), title: String::new(), volume: 50, muted: false };
        read(&mut s, Reading::Audio(Audio { outputs: vec![], streams: vec![stream(1, "a"), stream(2, "b")] }));
        s.apply(PointerEvent::Down(Level::Stream(0).id()).into());
        s.apply(LevelEvent::Set(Level::Stream(0), 60).into());
        // A new app sorts first.
        read(&mut s, Reading::Audio(Audio { outputs: vec![], streams: vec![stream(0, "new"), stream(1, "a"), stream(2, "b")] }));
        let fx = s.apply(LevelEvent::Set(Level::Stream(0), 70).into()).1;
        assert_eq!(fx, vec![Effect::Sound(Sound::StreamVolume(1, 70))]);
    }
}
