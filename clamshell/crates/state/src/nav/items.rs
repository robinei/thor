//! List items by what they are, not where they are. Taps, long presses and level rows name list items by
//! position, so drawing needs no allocation. But a list can change under a finger: a scan re-sorts the
//! networks, a device connects and moves up, an app stops playing, a game ends. So a press remembers
//! what its item is ([`crate::kit::list::ItemKey`]), and acts on that item wherever it is by then, or
//! not at all once it is gone. The UI records each target's item as it draws it, and the press carries
//! it, so the item is the one that was drawn where the finger came down even if the list changed
//! between the frame and the press. A press without one, such as the pad's A, takes the item at its
//! position when it comes down.

#[cfg(test)]
mod tests {
    use crate::levels::{Audio, AudioOutput, Level, Stream};
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
        s.applied(PointerEvent::Down(pad.clone(), None).into());
        // The buds connect and the list changes under the finger.
        read(&mut s, bt(vec![device("pad", true, false), device("buds", true, true)]));
        assert_eq!(long(&mut s, pad.clone()), vec![Effect::Bluetooth(Command::Forget("00:pad".into()))]);
        s.applied(PointerEvent::Up(pad).into());
        // If it is gone in the meantime, nothing happens.
        let first = ListId::Bluetooth.row(0);
        s.applied(PointerEvent::Down(first.clone(), None).into());
        read(&mut s, bt(vec![device("buds", true, true)]));
        assert_eq!(long(&mut s, first), vec![]);
    }

    #[test]
    fn a_press_acts_on_the_item_that_was_drawn() {
        let mut s = state();
        let bt = |devices| Reading::Bluetooth(Bluetooth { present: true, on: true, devices, ..Bluetooth::default() });
        let first = ListId::Bluetooth.row(0);
        read(&mut s, bt(vec![device("buds", true, false), device("pad", true, false)]));
        // The frame shows the buds first.
        let drawn = s.lists().item_key(&first);
        // The list re-sorts before the finger comes down there.
        read(&mut s, bt(vec![device("pad", true, true), device("buds", true, false)]));
        s.applied(PointerEvent::Down(first.clone(), drawn).into());
        assert_eq!(long(&mut s, first), vec![Effect::Bluetooth(Command::Forget("00:buds".into()))]);
    }

    /// A dock tile pressed acts on its entry wherever it is by then: here the one before it ended and left
    /// the dock. Once its own entry is gone, the press does nothing.
    #[test]
    fn a_dock_press_acts_on_the_entry_it_came_down_on() {
        let mut s = state();
        s.applied(Event::tick(1000));
        let zelda = launch(&mut s, "zelda-lttp");
        let ff6 = launch(&mut s, "ff6");
        let windows = |zelda_open: bool| {
            let mut w = vec![win_of(4, "ff6", &ff6)];
            if zelda_open {
                w.push(win_of(3, "zelda3", &zelda));
            }
            w
        };
        s.observed(windows(true));
        // After the Library's tile, which is first.
        let tile = Id::Dock(DockId::Entry(2));
        assert_eq!(s.dock().items().get(2).map(|i| &*i.id), Some("ff6"));
        s.applied(PointerEvent::Down(tile.clone(), None).into());
        s.observed(windows(false));
        end(&mut s, "zelda-lttp");
        assert_eq!(s.dock().items().get(1).map(|i| &*i.id), Some("ff6"), "moved up");
        tap(&mut s, tile.clone());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(4));
        s.applied(PointerEvent::Up(tile.clone()).into());
        let first = Id::Dock(DockId::Entry(1));
        s.applied(PointerEvent::Down(first.clone(), None).into());
        s.observed(vec![]);
        end(&mut s, "ff6");
        assert_eq!(s.dock().items().len(), 1, "the Library's alone");
        assert_eq!(tap(&mut s, first), [], "gone");
    }

    /// A collection pressed is the one chosen, though a library read moved it.
    #[test]
    fn a_collection_press_acts_on_the_collection_it_came_down_on() {
        let mut s = state();
        let more = Id::LibraryView(LibraryViewId::Collection(1));
        assert_eq!(s.library_view().library().collections.get(1).map(|c| c.name.as_str()), Some("More"));
        s.applied(PointerEvent::Down(more.clone(), None).into());
        let mut library = clamshell_library::demo::library();
        library.collections.reverse();
        s.applied(Event::Library(Box::new(library)));
        tap(&mut s, more.clone());
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("More"));
        s.applied(PointerEvent::Up(more.clone()).into());
        // Gone by the time it is let go: nothing, though another collection is at its place.
        let files = Id::LibraryView(LibraryViewId::Collection(0));
        s.applied(PointerEvent::Down(files.clone(), None).into());
        let mut library = clamshell_library::demo::library();
        library.collections.reverse();
        library.collections.retain(|c| c.name != "Files");
        s.applied(Event::Library(Box::new(library)));
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("More"));
        tap(&mut s, files);
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("More"), "not Tools, now first");
    }

    /// An audio output pressed is the one chosen, though a reading re-sorted the outputs.
    #[test]
    fn an_output_press_chooses_the_output_it_came_down_on() {
        let mut s = state();
        let output = |id, name: &str, default| AudioOutput { id, name: name.into(), default };
        read(&mut s, Reading::Audio(Audio { outputs: vec![output(1, "speakers", true), output(2, "buds", false)], streams: vec![], seen: 0 }));
        let second = Id::Audio(AudioId::Output(1));
        s.applied(PointerEvent::Down(second.clone(), None).into());
        read(&mut s, Reading::Audio(Audio { outputs: vec![output(2, "buds", false), output(1, "speakers", true)], streams: vec![], seen: 0 }));
        assert_eq!(sounds(&tap(&mut s, second.clone())), [Sound::Output(2)]);
        s.applied(PointerEvent::Up(second.clone()).into());
        s.applied(PointerEvent::Down(second.clone(), None).into());
        read(&mut s, Reading::Audio(Audio { outputs: vec![output(2, "buds", true)], streams: vec![], seen: 0 }));
        assert_eq!(tap(&mut s, second), [], "gone");
    }

    /// A row of the battery tab, which re-ranks at every reading, opens the menu of the program it showed
    /// when the finger came down, and nothing once that program is gone.
    #[test]
    fn a_battery_row_press_opens_the_program_it_came_down_on() {
        use crate::id::BatteryId;
        use crate::status::battery::{Battery, Group, Spender};
        let mut s = state();
        let spender = |name: &str, cpu| Spender { group: Group::Process(name.into()), cpu, gpu: 0.0, memory: 0, pids: vec![1], endable: true };
        let reading = |spenders| Reading::Battery(Battery { spenders, ..Battery::default() });
        read(&mut s, reading(vec![spender("firefox", 0.5), spender("foot", 0.1)]));
        let first = Id::Battery(BatteryId::Spender(0));
        let drawn = s.lists().item_key(&first);
        // Busiest first: foot goes up before the finger comes down where firefox was drawn.
        read(&mut s, reading(vec![spender("foot", 0.9), spender("firefox", 0.5)]));
        s.applied(PointerEvent::Down(first.clone(), drawn).into());
        tap(&mut s, first.clone());
        assert_eq!(s.sys().spender_menu(), Some(&Group::Process("firefox".into())));
        s.applied(PointerEvent::Activate(Id::Battery(BatteryId::Back)).into());
        let drawn = s.lists().item_key(&first);
        read(&mut s, reading(vec![spender("firefox", 0.5)]));
        s.applied(PointerEvent::Down(first.clone(), drawn).into());
        tap(&mut s, first);
        assert_eq!(s.sys().spender_menu(), None, "foot gone");
    }

    #[test]
    fn a_level_drag_stays_on_its_app() {
        let mut s = state();
        let stream = |id, app: &str| Stream { id, app: app.into(), title: String::new(), volume: 50, muted: false };
        read(&mut s, Reading::Audio(Audio { outputs: vec![], streams: vec![stream(1, "a"), stream(2, "b")], seen: 0 }));
        s.applied(PointerEvent::Down(Level::Stream(0).id(), None).into());
        s.applied(LevelEvent::Set(Level::Stream(0), 60).into());
        // A new app sorts first.
        read(&mut s, Reading::Audio(Audio { outputs: vec![], streams: vec![stream(0, "new"), stream(1, "a"), stream(2, "b")], seen: 0 }));
        let fx = s.applied(LevelEvent::Set(Level::Stream(0), 70).into());
        assert_eq!(sounds(&fx), [Sound::StreamVolume(1, 70)]);
    }
}
