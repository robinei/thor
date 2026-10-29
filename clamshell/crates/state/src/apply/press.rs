//! Touches and the pad's presses: the focus moved, a target pressed, and what a tap or long press on a
//! target or a list's row does.

use crate::bottom::editor::Done;
use crate::notifications::NotificationEvent;
use crate::out::LogLevel;
use crate::out::Out;
use crate::profiles::picker::{Kind as ProfileKind, Owner};
use crate::session::overview::OverviewEvent;
use crate::{BtId, Compositor, Control, EditorId, Effect, Event, HardwareKey, Id, ListId, PointerEvent, RowPart, State, Tab, WifiId};

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
        if let PointerEvent::Act(id) = event {
            return Self::act(id, out);
        }
        let used = match &event {
            PointerEvent::Focus(id) | PointerEvent::Activate(id) | PointerEvent::LongPress(id) | PointerEvent::Down(id, _) => !id.on_top(),
            _ => false,
        };
        if used {
            self.bottom.used(&self.clock);
        }
        // Input that reached ClamShell itself, such as the pad driving the screens.
        if !matches!(event, PointerEvent::Start(_)) {
            self.idle.own_input(self.clock.mono());
        }
        match event {
            PointerEvent::Start(id) => self.pointer.focus_on(id),
            PointerEvent::Focus(id) => {
                // A row of the Library's edit mode is selected as the focus comes to it.
                if let Id::LibraryView(crate::LibraryViewId::Edit(crate::library_view::edit::EditId::Row(i))) = &id {
                    self.library_view.edit_select(usize::from(*i));
                }
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
                if let Id::Key(pos) = id {
                    self.keyboard.up(pos);
                }
            }
            PointerEvent::Act(_) => {}
        }
    }

    /// An overlay's target pressed (see [`PointerEvent::Act`]): the event it stands for is queued.
    fn act(id: Id, out: &mut Out) {
        let event = match id {
            Id::WindowArrow { next } => Event::HardwareKey(if next { HardwareKey::WindowNext } else { HardwareKey::WindowPrevious }),
            Id::OverviewWindow(window) => Event::Overview(OverviewEvent::Pick(window)),
            Id::Notification { id, close: false } => NotificationEvent::Tapped(id).into(),
            Id::Notification { id, close: true } => NotificationEvent::Dismissed(id).into(),
            id => return out.log(LogLevel::Warn, format!("a press acted on {id:?}, which isn't an overlay's: nothing done")),
        };
        out.events.push(event);
    }

    /// A tap or long press on `id`, resolved to what the press landed on. It gets the focus, and its
    /// subsystem acts on it.
    fn activate(&mut self, id: &Id, press: Press, out: &mut Out) {
        let long = press == Press::Long;
        // The list item the press came down on is gone.
        let Some(id) = self.lists().resolve(id) else {
            return;
        };
        // The Library entry that isn't in the list shown takes nothing, not even the focus.
        if !self.library_view.has_target(&id) {
            return;
        }
        self.pointer.focus_on(id.clone());
        match id {
            Id::LibraryView(id) => {
                self.library_view.on_target(id, long, &mut self.pointer, out);
            }
            Id::Dock(id) => {
                self.dock.on(id, long, &self.session, out);
            }
            Id::Settings(id) => {
                self.on_settings(id, long, out);
            }
            Id::Editor(EditorId::Insert(i)) => self.bottom.insert_snippet(usize::from(i)),
            Id::Editor(id) => {
                let done = if id == EditorId::Save { Done::Save } else { Done::Cancel };
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
            Id::Lights(id) => {
                self.on_lights(id, out);
            }
            Id::Tab(tab) => {
                self.choose_tab(tab);
            }
            // A tap opens the tab. Slides are handled by the UI and don't end in a tap.
            Id::Control(control) if !long => {
                let tab = match control {
                    Control::Volume => Tab::Volume,
                    Control::Brightness => Tab::Brightness,
                    // A keymap's, a curve's or a light's parameter, or a setting's slider, is held or slid,
                    // not tapped.
                    Control::Param(_) | Control::Curve(_) | Control::Light(_) | Control::Setting(_) => return,
                };
                self.choose_tab(tab);
            }
            Id::Control(_) | Id::Level(_) | Id::List(_) | Id::Blank => {}
            Id::Key(pos) => {
                let stroke = self.keyboard.press(pos, long);
                // The key goes to the editor while it is open, to the app of ClamShell's in front as it says
                // (see `crate::builtin_apps`), and otherwise to the focused window.
                match stroke {
                    Some(stroke) if self.bottom.editor().is_some() => {
                        if let Some(done) = self.bottom.edit(stroke) {
                            self.editor_done(done, out);
                        }
                    }
                    Some(stroke) if self.keymapping.capturing(self.bottom.tab(), self.bottom.editor().is_some()) => {
                        self.keymapping.captured(crate::keymap::screens::chord(stroke), out);
                    }
                    Some(stroke) => match self.top() {
                        crate::Top::App { app, .. } => (crate::builtin_apps::definition(app).typed)(self, stroke, out),
                        crate::Top::Window { .. } => out.effects.push(Effect::Compositor(Compositor::Key(stroke))),
                    },
                    None => {}
                }
            }
            Id::Time(id) => self.sys.on_time(id, self.scrolls.get_mut(ListId::Time), out),
            Id::Device(id) => self.device_view.tap(id, self.setup.account(), self.clock.mono(), out),
            Id::Setup(id) => self.setup.wizard_tap(id, out),
            Id::Battery(id) => {
                self.sys.on_battery(id, &self.session, out);
            }
            Id::PadMouse => {
                self.pad.toggle_mouse();
            }
            Id::Wake => {
                self.bottom.wake();
            }
            // An overlay's targets act through `PointerEvent::Act`.
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
            list if let Some(kind) = ProfileKind::of_list(list)
                && self.panel().is_profile_reset(kind, i) =>
            {
                if !long {
                    let setting = self.panel().setting_of(kind);
                    self.change(setting, None, out);
                }
            }
            ListId::Keymaps => self.on_keymap_row(i, part, long, out),
            ListId::Curves => {
                let cx = self.picker_cx(ProfileKind::Curve);
                self.curving.on_row(i, part, long, &cx, self.bottom.editors_mut(), out);
            }
            ListId::Lights => {
                let cx = self.picker_cx(ProfileKind::Lights);
                self.lighting.on_row(i, part, long, &cx, self.bottom.editors_mut(), out);
            }
            ListId::Values if !long => self.on_menu_row(i, out),
            ListId::Values => {}
            ListId::Layers => self.on_layer_row(i, part, long, out),
            ListId::Time => self.sys.on_time_row(i, self.scrolls.get_mut(ListId::Time), out),
            // Their tiles, and the edit mode's rows, are targets of their own.
            ListId::Dock | ListId::Settings | ListId::LibraryEdit => {}
            // The Library game's row is its game's target, and a backlight's row is only its level.
            ListId::LibraryView | ListId::Backlights => {}
        }
    }
}
