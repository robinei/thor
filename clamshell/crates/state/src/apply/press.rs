//! Touches and the pad's presses: the focus moved, a target pressed, and what a tap or long press on a
//! target or a list's row does.

use crate::bottom::editor::Done;
use crate::out::Out;
use crate::profiles::picker::Kind as ProfileKind;
use crate::{BtId, Compositor, Control, EditorId, Effect, Id, Level, ListId, PointerEvent, RowPart, State, Tab, WifiId};

/// How a target was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    Tap,
    Long,
}

impl State {
    /// A touch or a pad focus change. Focusing or pressing a target on the second screen counts as using
    /// that screen.
    pub(crate) fn on_pointer(&mut self, event: PointerEvent, out: &mut Out) {
        let used = match &event {
            PointerEvent::Focus(id) | PointerEvent::Activate(id) | PointerEvent::LongPress(id) | PointerEvent::Down(id, _) => !id.on_top(),
            _ => false,
        };
        if used {
            self.bottom.used(&self.session, &self.clock);
        }
        // Input that reached ClamShell itself, such as the pad driving the screens.
        self.idle.own_input(self.clock.mono());
        match event {
            PointerEvent::Focus(id) => {
                self.show_focused(&id);
                self.pointer.focus_on(id);
                self.pointer.show();
            }
            PointerEvent::HideFocus => {
                self.pointer.hide_focus();
            }
            // A press that woke the screens does nothing else.
            PointerEvent::Activate(_) | PointerEvent::LongPress(_) if self.idle.swallows_press(self.clock.mono()) => {}
            PointerEvent::Activate(id) => self.activate(&id, Press::Tap, out),
            PointerEvent::LongPress(id) => self.activate(&id, Press::Long, out),
            PointerEvent::Down(id, item) => {
                if let Id::Key(pos) = id {
                    self.keyboard.down(pos);
                }
                let item = item.or_else(|| self.lists().item_key(&id));
                self.pointer.down(id, item);
            }
            PointerEvent::Up(id) => {
                self.pointer.up(&id);
                if Level::of(&id).is_some() {
                    self.levels.release();
                }
                if let Id::Key(pos) = id {
                    self.keyboard.up(pos);
                }
            }
        }
    }

    /// A tap or long press on `id`, resolved to what the press landed on. It gets the focus, and its
    /// subsystem acts on it.
    fn activate(&mut self, id: &Id, press: Press, out: &mut Out) {
        let long = press == Press::Long;
        // The list item the press came down on is gone.
        let Some(id) = self.lists().resolve(id) else {
            return;
        };
        // A shelf entry that isn't in the list shown takes nothing, not even the focus.
        if !self.shelf.has_target(&id) {
            return;
        }
        self.pointer.focus_on(id.clone());
        match id {
            Id::Shelf(id) => {
                self.shelf.on_target(id, long, &mut self.pointer, out);
            }
            Id::Dock(id) => {
                self.dock.on(id, long, &self.session, out);
            }
            Id::Settings(id) => {
                self.on_settings(id, long, out);
            }
            Id::Editor(id) => {
                let done = match id {
                    EditorId::Save => Done::Save,
                    EditorId::Cancel => Done::Cancel,
                };
                self.editor_done(done, out);
            }
            Id::Wifi(WifiId::Power) => {
                self.radios.power_wifi(out);
            }
            Id::Bluetooth(BtId::Power) => {
                self.radios.power_bluetooth(out);
            }
            Id::Audio(id) => {
                self.levels.on_audio(id, out);
            }
            Id::Row { list, i, part } => {
                self.on_row(list, usize::from(i), part, long, out);
            }
            Id::Keymap(id) => {
                self.on_keymap(id, long, out);
            }
            Id::Profile(kind, id) => {
                self.on_profile(kind, id, out);
            }
            Id::Tab(tab) => {
                self.choose_tab(tab);
            }
            // A tap opens the tab. Slides are handled by the UI and don't end in a tap.
            Id::Control(control) if !long => {
                let tab = match control {
                    Control::Volume => Tab::Volume,
                    Control::Brightness => Tab::Brightness,
                    // A keymap's or a curve's parameter, or a setting's slider, is held or slid, not tapped.
                    Control::Param(_) | Control::Curve(_) | Control::Setting(_) => return,
                };
                self.choose_tab(tab);
            }
            Id::Control(_) | Id::Level(_) | Id::List(_) | Id::Blank => {}
            Id::Key(pos) => {
                let stroke = self.keyboard.press(pos, long);
                // The key goes to the editor while it is open, to finding an entry while shelf is in
                // front, and otherwise to the focused app.
                match stroke {
                    Some(stroke) if self.bottom.editor().is_some() => {
                        if let Some(done) = self.bottom.edit(stroke) {
                            self.editor_done(done, out);
                        }
                    }
                    Some(stroke) if self.keymapping.capturing(self.bottom.tab(), self.bottom.editor().is_some()) => {
                        self.keymapping.captured(crate::keymap::screens::chord(stroke), out);
                    }
                    Some(stroke) if self.session.front() == crate::session::running::Front::Shelf => {
                        self.shelf.find(stroke, &mut self.pointer, out);
                    }
                    Some(stroke) => out.effects.push(Effect::Compositor(Compositor::Key(stroke))),
                    None => {}
                }
            }
            Id::Time(id) => self.sys.on_time(id, self.scrolls.get_mut(ListId::Time), out),
            Id::Device(id) => {
                self.setup.tap(id, self.clock.mono(), &mut self.pointer, self.shelf.selected(), out);
            }
            Id::Battery(id) => {
                self.sys.on_battery(id, &self.session, out);
            }
            Id::PadMouse => {
                self.pad.toggle_mouse();
            }
            Id::Wake => {
                self.bottom.wake();
            }
            Id::WindowArrow { .. } | Id::OverviewWindow(_) | Id::Notification { .. } => {}
        }
    }

    /// A tap or long press on a row, handled by the list's feature.
    fn on_row(&mut self, list: ListId, i: usize, part: RowPart, long: bool, out: &mut Out) {
        match list {
            ListId::Wifi => self.radios.tap_network(i, long, self.bottom.editors_mut(), out),
            ListId::Bluetooth => self.radios.tap_device(i, long, out),
            ListId::Streams => self.levels.on_stream(i, part, out),
            // A picker's Reset row is the settings': the level edited goes back to the one below's choice.
            ListId::Keymaps | ListId::Curves if self.panel().is_profile_reset(ProfileKind::of_list(list), i) => {
                if !long {
                    self.change(ProfileKind::of_list(list).setting(), None, out);
                }
            }
            ListId::Keymaps => self.on_keymap_row(i, part, long, out),
            ListId::Curves => {
                let cx = self.picker_cx(ProfileKind::Curve);
                self.curving.on_row(i, part, long, &cx, self.bottom.editors_mut(), out);
            }
            ListId::Values if !long => self.on_menu_row(i, out),
            ListId::Values => {}
            ListId::Layers => self.on_layer_row(i, part, long, out),
            ListId::Time => self.sys.on_time_row(i, self.scrolls.get_mut(ListId::Time), out),
            // Their tiles are targets of their own.
            ListId::Dock | ListId::Settings => {}
            // A shelf game's row is its game's target, and a backlight's row is only its level.
            ListId::Shelf | ListId::Backlights => {}
        }
    }
}
