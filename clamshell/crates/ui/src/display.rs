//! The screens as the compositor has them, through standard Wayland protocols (no compositor's own IPC):
//! whether it has seen input lately (ext-idle-notify: idle, held off by apps that ask it to keep the
//! screen on), turning the screens off and on (wlr-output-power-management, for idle), and the main
//! screen's refresh rate (wlr-output-management, for the Refresh setting). A compositor without one of
//! them just goes without that part.

use std::sync::Arc;

use clamshell_core::Event;
use clamshell_core::idle::{IdleEvent, NOTIFY_AFTER};
use smithay_client_toolkit::{
    dispatch2::Dispatch2,
    reexports::{
        client::{
            Connection, QueueHandle,
            backend::ObjectData,
            globals::{BindError, GlobalList},
            protocol::{wl_output::WlOutput, wl_seat::WlSeat},
        },
        protocols::ext::idle_notify::v1::client::{
            ext_idle_notification_v1::{self, ExtIdleNotificationV1},
            ext_idle_notifier_v1::{self, ExtIdleNotifierV1},
        },
        protocols_wlr::{
            output_management::v1::client::{
                zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1},
                zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
                zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
                zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
                zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
            },
            output_power_management::v1::client::{
                zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1},
                zwlr_output_power_v1::{self, ZwlrOutputPowerV1},
            },
        },
    },
};

use crate::App;

/// An output's mode as the compositor lists it.
struct Mode {
    mode: ZwlrOutputModeV1,
    size: (i32, i32),
    /// In mHz.
    refresh: i32,
}

/// An output as the compositor lists it.
struct Head {
    head: ZwlrOutputHeadV1,
    name: String,
    enabled: bool,
    modes: Vec<Mode>,
    current: Option<ZwlrOutputModeV1>,
}

#[derive(Default)]
pub(crate) struct Display {
    notifier: Option<ExtIdleNotifierV1>,
    notification: Option<ExtIdleNotificationV1>,
    power: Option<ZwlrOutputPowerManagerV1>,
    /// The outputs turned off (each with its power control), to turn on again.
    off: Vec<ZwlrOutputPowerV1>,
    manager: Option<ZwlrOutputManagerV1>,
    heads: Vec<Head>,
    /// The output configuration's serial (from the manager's `done`).
    serial: Option<u32>,
    /// The second screen's output: the other enabled one is the main screen.
    bottom_output: String,
    /// The main screen's mode as it was at start (the Refresh setting's "as is"), by size and refresh.
    original: Option<((i32, i32), i32)>,
    /// The refresh rate asked for (Hz; 0: as it was) before the outputs were known: set once they are
    /// (once, so that a mode set since by something else stays).
    wanted_refresh: Option<u16>,
}

impl Display {
    pub(crate) fn bind(globals: &GlobalList, qh: &QueueHandle<App>, bottom_output: &str) -> Display {
        fn quiet(what: &'static str) -> impl Fn(BindError) {
            move |e| log::info!("no {what} protocol: {e}")
        }
        Display {
            notifier: globals.bind::<ExtIdleNotifierV1, App, Data>(qh, 1..=1, Data).map_err(quiet("idle notification")).ok(),
            power: globals.bind::<ZwlrOutputPowerManagerV1, App, Data>(qh, 1..=1, Data).map_err(quiet("output power")).ok(),
            manager: globals.bind::<ZwlrOutputManagerV1, App, Data>(qh, 1..=4, Data).map_err(quiet("output management")).ok(),
            bottom_output: bottom_output.to_owned(),
            ..Display::default()
        }
    }

    /// Asks the compositor, for `seat`, to say when it has seen no input for a while (and input again).
    pub(crate) fn seat(&mut self, seat: &WlSeat, qh: &QueueHandle<App>) {
        if let (Some(notifier), None) = (&self.notifier, &self.notification) {
            let timeout = u32::try_from(NOTIFY_AFTER * 1000).unwrap_or(u32::MAX);
            self.notification = Some(notifier.get_idle_notification(timeout, seat, qh, Data));
        }
    }

    /// The second screen's output's name.
    pub(crate) fn bottom_output(&self) -> &str {
        &self.bottom_output
    }

    /// The screens on or off: on, those it turned off.
    pub(crate) fn power(&mut self, on: bool, outputs: impl Iterator<Item = WlOutput>, qh: &QueueHandle<App>) {
        log::info!("screens {}", if on { "on" } else { "off" });
        if on {
            for power in self.off.drain(..) {
                power.set_mode(zwlr_output_power_v1::Mode::On);
                power.destroy();
            }
            return;
        }
        let Some(manager) = &self.power else {
            return;
        };
        for output in outputs {
            let power = manager.get_output_power(&output, qh, Data);
            power.set_mode(zwlr_output_power_v1::Mode::Off);
            self.off.push(power);
        }
    }

    /// Every screen on, at start: a ClamShell before this one may have left them off (idle), and nothing
    /// else turns them on again.
    pub(crate) fn all_on(&self, outputs: impl Iterator<Item = WlOutput>, qh: &QueueHandle<App>) {
        let Some(manager) = &self.power else {
            return;
        };
        for output in outputs {
            let power = manager.get_output_power(&output, qh, Data);
            power.set_mode(zwlr_output_power_v1::Mode::On);
            power.destroy();
        }
    }

    /// The main screen's refresh rate to `hz` (0: as it was at start), the nearest its mode list has.
    pub(crate) fn refresh(&mut self, hz: u16, qh: &QueueHandle<App>) {
        let (Some(manager), Some(serial)) = (&self.manager, self.serial) else {
            self.wanted_refresh = Some(hz);
            return;
        };
        let Some(main) = self.heads.iter().find(|h| h.enabled && h.name != self.bottom_output) else {
            self.wanted_refresh = Some(hz);
            return;
        };
        let current = main.current.as_ref().and_then(|c| main.modes.iter().find(|m| m.mode == *c));
        let Some(current) = current else {
            return;
        };
        let target = match (hz, self.original) {
            (0, Some((size, refresh))) => main.modes.iter().find(|m| m.size == size && m.refresh == refresh),
            (0, None) => None,
            (hz, _) => main.modes.iter().filter(|m| m.size == current.size).min_by_key(|m| (m.refresh - i32::from(hz) * 1000).abs()),
        };
        let Some(target) = target.filter(|t| t.mode != current.mode) else {
            return;
        };
        log::info!("{}: {}x{} at {:.0} Hz", main.name, target.size.0, target.size.1, f64::from(target.refresh) / 1000.0);
        // A configuration names every output: the others as they are.
        let config = manager.create_configuration(serial, qh, Data);
        for head in &self.heads {
            if head.enabled {
                let configured = config.enable_head(&head.head, qh, Data);
                if head.head == main.head {
                    configured.set_mode(&target.mode);
                }
            } else {
                config.disable_head(&head.head);
            }
        }
        config.apply();
    }

    fn head_mut(&mut self, head: &ZwlrOutputHeadV1) -> Option<&mut Head> {
        self.heads.iter_mut().find(|h| h.head == *head)
    }

    fn mode_mut(&mut self, mode: &ZwlrOutputModeV1) -> Option<&mut Mode> {
        self.heads.iter_mut().flat_map(|h| h.modes.iter_mut()).find(|m| m.mode == *mode)
    }

    /// The outputs are known (a `done`): the main screen's mode at start is remembered, and a refresh
    /// rate asked for before is set.
    fn listed(&mut self, serial: u32, qh: &QueueHandle<App>) {
        self.serial = Some(serial);
        if self.original.is_none()
            && let Some(main) = self.heads.iter().find(|h| h.enabled && h.name != self.bottom_output)
            && let Some(current) = main.current.as_ref().and_then(|c| main.modes.iter().find(|m| m.mode == *c))
        {
            self.original = Some((current.size, current.refresh));
        }
        if let Some(hz) = self.wanted_refresh.take() {
            self.refresh(hz, qh);
        }
    }
}

/// The objects' user data (they carry nothing; what they describe is kept in [`Display`]).
pub(crate) struct Data;

impl Dispatch2<ExtIdleNotifierV1, App> for Data {
    fn event(&self, _: &mut App, _: &ExtIdleNotifierV1, _: ext_idle_notifier_v1::Event, _: &Connection, _: &QueueHandle<App>) {}
}

impl Dispatch2<ExtIdleNotificationV1, App> for Data {
    fn event(&self, app: &mut App, _: &ExtIdleNotificationV1, event: ext_idle_notification_v1::Event, _: &Connection, _: &QueueHandle<App>) {
        let heard = match event {
            ext_idle_notification_v1::Event::Idled => IdleEvent::CompositorQuiet,
            ext_idle_notification_v1::Event::Resumed => IdleEvent::CompositorInput,
            _ => return,
        };
        app.handle(Event::Idle(heard));
    }
}

impl Dispatch2<ZwlrOutputPowerManagerV1, App> for Data {
    fn event(&self, _: &mut App, _: &ZwlrOutputPowerManagerV1, _: zwlr_output_power_manager_v1::Event, _: &Connection, _: &QueueHandle<App>) {}
}

impl Dispatch2<ZwlrOutputPowerV1, App> for Data {
    fn event(&self, app: &mut App, power: &ZwlrOutputPowerV1, event: zwlr_output_power_v1::Event, _: &Connection, _: &QueueHandle<App>) {
        // The output went away, or another client controls its power: let go of it.
        if let zwlr_output_power_v1::Event::Failed = event {
            app.display.off.retain(|p| p != power);
            power.destroy();
        }
    }
}

impl Dispatch2<ZwlrOutputManagerV1, App> for Data {
    fn event(&self, app: &mut App, _: &ZwlrOutputManagerV1, event: zwlr_output_manager_v1::Event, _: &Connection, qh: &QueueHandle<App>) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                app.display.heads.push(Head { head, name: String::new(), enabled: false, modes: Vec::new(), current: None });
            }
            zwlr_output_manager_v1::Event::Done { serial } => app.display.listed(serial, qh),
            _ => {}
        }
    }

    fn event_created_child(_: u16, qh: &QueueHandle<App>) -> Arc<dyn ObjectData> {
        qh.make_data::<ZwlrOutputHeadV1, Data>(Data)
    }
}

impl Dispatch2<ZwlrOutputHeadV1, App> for Data {
    fn event(&self, app: &mut App, head: &ZwlrOutputHeadV1, event: zwlr_output_head_v1::Event, _: &Connection, _: &QueueHandle<App>) {
        let display = &mut app.display;
        match event {
            zwlr_output_head_v1::Event::Mode { mode } => {
                if let Some(h) = display.head_mut(head) {
                    h.modes.push(Mode { mode, size: (0, 0), refresh: 0 });
                }
            }
            zwlr_output_head_v1::Event::Finished => display.heads.retain(|h| h.head != *head),
            event => {
                let Some(h) = display.head_mut(head) else {
                    return;
                };
                match event {
                    zwlr_output_head_v1::Event::Name { name } => h.name = name,
                    zwlr_output_head_v1::Event::Enabled { enabled } => h.enabled = enabled != 0,
                    zwlr_output_head_v1::Event::CurrentMode { mode } => h.current = Some(mode),
                    _ => {}
                }
            }
        }
    }

    fn event_created_child(_: u16, qh: &QueueHandle<App>) -> Arc<dyn ObjectData> {
        qh.make_data::<ZwlrOutputModeV1, Data>(Data)
    }
}

impl Dispatch2<ZwlrOutputModeV1, App> for Data {
    fn event(&self, app: &mut App, mode: &ZwlrOutputModeV1, event: zwlr_output_mode_v1::Event, _: &Connection, _: &QueueHandle<App>) {
        let display = &mut app.display;
        match event {
            zwlr_output_mode_v1::Event::Finished => {
                for head in &mut display.heads {
                    head.modes.retain(|m| m.mode != *mode);
                }
            }
            event => {
                let Some(m) = display.mode_mut(mode) else {
                    return;
                };
                match event {
                    zwlr_output_mode_v1::Event::Size { width, height } => m.size = (width, height),
                    zwlr_output_mode_v1::Event::Refresh { refresh } => m.refresh = refresh,
                    _ => {}
                }
            }
        }
    }
}

impl Dispatch2<ZwlrOutputConfigurationV1, App> for Data {
    fn event(&self, _: &mut App, config: &ZwlrOutputConfigurationV1, event: zwlr_output_configuration_v1::Event, _: &Connection, _: &QueueHandle<App>) {
        match event {
            zwlr_output_configuration_v1::Event::Succeeded => config.destroy(),
            zwlr_output_configuration_v1::Event::Failed | zwlr_output_configuration_v1::Event::Cancelled => {
                log::warn!("the refresh rate wasn't changed: the compositor refused ({event:?})");
                config.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch2<ZwlrOutputConfigurationHeadV1, App> for Data {
    fn event(&self, _: &mut App, _: &ZwlrOutputConfigurationHeadV1, _: zwlr_output_configuration_head_v1::Event, _: &Connection, _: &QueueHandle<App>) {}
}
