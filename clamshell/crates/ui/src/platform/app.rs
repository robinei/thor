//! The host's own work. It places the views on the outputs, runs the core after each turn of the loop,
//! draws the views that are due when their frames can be shown, and carries out the core's effects.

use clamshell_state::Effect;
use smithay_client_toolkit::{
    output::{OutputInfo, OutputState},
    reexports::client::{
        QueueHandle,
        globals::GlobalList,
        protocol::{wl_output, wl_surface::WlSurface},
    },
    registry::RegistryState,
    seat::SeatState,
    shm::{Shm, slot::SlotPool},
};

use super::view::View;
use crate::core::Core;
use crate::screens::Screens;
use crate::{Config, Error, Screen};

use super::{App, BottomKind, Shells, display, text_input};

impl App {
    pub(super) fn new(globals: &GlobalList, qh: &QueueHandle<App>, core: Core, config: &Config, on_effect: Box<dyn FnMut(&Effect)>) -> Result<App, Error> {
        let shm = Shm::bind(globals, qh)?;
        let pool = SlotPool::new(4 << 20, &shm)?;
        Ok(App {
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            bottom_output: None,
            shm,
            pool,
            qh: qh.clone(),
            views: Screens::default(),
            core,
            keyboard: None,
            pointer: None,
            cursor_shapes: smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager::bind(globals, qh)
                .inspect_err(|e| log::info!("no cursor shape protocol: {e}"))
                .ok(),
            cursor: None,
            touch: None,
            bottom_kind: BottomKind::Window,
            text_input: text_input::TextInput::bind(globals, qh),
            display: display::Display::bind(globals, qh, &config.bottom_output),
            on_effect,
            outputs: (true, true),
            exit: false,
        })
    }

    /// Places the top view as a window, and the bottom one as `BottomKind::choose` says.
    pub(super) fn place_views(&mut self, shells: &Shells, qh: &QueueHandle<App>, config: &Config) {
        self.views.top = Some(View::window(&shells.compositor, &shells.xdg, qh, Screen::Top, config.top_size));
        self.display.all_on(self.output_state.outputs(), qh);
        let output = self.output_state.outputs().find(|o| self.output_state.info(o).and_then(|i| i.name).as_deref() == Some(config.bottom_output.as_str()));
        let kind = BottomKind::choose(output.is_some(), shells.layer.is_some(), config.bottom_panel);
        self.bottom_kind = kind;
        self.views.bottom = Some(match (kind, &shells.layer) {
            (BottomKind::Output, Some(layer_shell)) => {
                log::info!("bottom view: layer surface on {}", config.bottom_output);
                View::layer(&shells.compositor, layer_shell, qh, output.as_ref(), None)
            }
            (BottomKind::Panel, Some(layer_shell)) => {
                log::info!("bottom view: a panel in the corner (it never takes keyboard focus)");
                View::layer(&shells.compositor, layer_shell, qh, None, Some(config.bottom_size))
            }
            _ => {
                log::info!("bottom view: a window (no output {})", config.bottom_output);
                View::window(&shells.compositor, &shells.xdg, qh, Screen::Bottom, config.bottom_size)
            }
        });
        self.bottom_output = output;
        self.update_bottom_density();
        // Until the compositor configures them, the screens are the views' default sizes.
        for screen in Screen::BOTH {
            if let Some(view) = &self.views[screen] {
                self.core.resize(screen, view.default_size, 1);
            }
        }
    }

    pub(super) fn update_bottom_density(&mut self) {
        let density = self.bottom_output.as_ref().and_then(|output| self.output_state.info(output)).and_then(|info| {
            let profile = self.core.state().profile();
            let hint = (info.name.as_deref() == Some(profile.bottom.connector)).then(|| profile.bottom_pixels_per_mm_hint()).flatten();
            output_density(&info, hint)
        });
        self.core.set_bottom_density(density);
    }

    /// The screen `surface` shows, if it is one of ours. Input is routed by it.
    pub(super) fn screen_of(&self, surface: &WlSurface) -> Option<Screen> {
        Screen::BOTH.into_iter().find(|&s| self.views[s].as_ref().is_some_and(|v| v.shell.surface() == surface))
    }

    pub(super) fn view_mut(&mut self, surface: &WlSurface) -> Option<&mut View> {
        self.views.iter_mut().filter_map(|(_, v)| v.as_mut()).find(|v| v.shell.surface() == surface)
    }

    /// After each turn of the loop: runs what is due in the core, draws the views that are due, and
    /// carries out the effects, those of the events handled and then those of the frames drawn.
    pub(super) fn turn(&mut self) {
        self.core.run(std::time::Instant::now());
        self.carry_out();
        self.redraw();
        self.carry_out();
    }

    /// Carries out the core's effects in order: quitting, the screens' power and the refresh rate here,
    /// the rest through the system crate.
    pub(super) fn carry_out(&mut self) {
        while let Some(effect) = self.core.next_effect() {
            match effect {
                Effect::Quit => self.exit = true,
                Effect::Outputs { top, bottom } => self.outputs(top, bottom),
                // The main screen's refresh rate is set through Wayland.
                Effect::Refresh(hz) => self.display.refresh(hz, &self.qh),
                other => (self.on_effect)(&other),
            }
        }
    }

    /// Sets the screens' power (see `Effect::Outputs`). The compositor turns them off and on for idle,
    /// asked through Wayland; on coming back, it turns on those it turned off. The bottom screen alone
    /// goes through sway, whose `output power` the system crate sends, when it changes while they are on
    /// or is to be set again.
    fn outputs(&mut self, top: bool, bottom: bool) {
        let was = std::mem::replace(&mut self.outputs, (top, bottom));
        if top != was.0 {
            let bottom_name = self.display.bottom_output().to_owned();
            let outputs: Vec<_> = self
                .output_state
                .outputs()
                .map(|o| {
                    let is_bottom = self.output_state.info(&o).and_then(|i| i.name).as_deref() == Some(bottom_name.as_str());
                    (o, is_bottom)
                })
                .collect();
            self.display.power(top, bottom, outputs.into_iter(), &self.qh);
        }
        if (top && bottom != was.1) || (top, bottom) == was {
            (self.on_effect)(&Effect::Outputs { top, bottom });
        }
    }

    /// Draws the views that are due. What the drawing changes, such as rows and lists, may make another
    /// view due. That view is drawn too rather than left until the next event, for a few passes at most.
    fn redraw(&mut self) {
        for _ in 0..3 {
            if !self.draw_due() || !Screen::BOTH.into_iter().any(|s| self.due(s)) {
                break;
            }
        }
    }

    /// Whether `screen`'s view is to be drawn: it may have changed, or the compositor waits for a frame.
    fn due(&self, screen: Screen) -> bool {
        self.views[screen].as_ref().is_some_and(|v| v.due(self.core.stale(screen)))
    }

    /// Draws the views that are due. Returns whether it recorded any.
    fn draw_due(&mut self) -> bool {
        let mut drew = false;
        // While a window is shown on the bottom screen, ClamShell's bottom view goes behind it.
        let state = self.core.state();
        let behind = state.bottom().shows_windows(state.session());
        let hidden = super::hidden_views(self.bottom_kind, state);
        if let Some(view) = self.views.bottom.as_mut() {
            view.set_behind(behind);
        }
        for (screen, view) in self.views.iter_mut() {
            if let Some(view) = view.as_mut() {
                view.visible = !hidden[screen];
            }
        }
        for screen in Screen::BOTH {
            if !self.due(screen) {
                continue;
            }
            let Some(view) = self.views[screen].as_mut() else {
                continue;
            };
            match view.draw(&mut self.core, &mut self.pool, &self.qh) {
                Ok(recorded) => drew |= recorded,
                Err(e) => {
                    log::warn!("drawing {screen:?}: {e}");
                    self.core.skip(screen);
                    view.reconfigured = false;
                }
            }
        }
        self.core.frames_done();
        drew
    }
}

fn output_density(info: &OutputInfo, panel_hint: Option<f32>) -> Option<(f32, f32)> {
    let rotated =
        matches!(info.transform, wl_output::Transform::_90 | wl_output::Transform::_270 | wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270);
    let physical = if rotated { (info.physical_size.1, info.physical_size.0) } else { info.physical_size };
    let logical = info.logical_size.or_else(|| {
        let mode = info.modes.iter().find(|mode| mode.current)?;
        let (w, h) = if rotated { (mode.dimensions.1, mode.dimensions.0) } else { mode.dimensions };
        Some((w / info.scale_factor.max(1), h / info.scale_factor.max(1)))
    })?;
    measured_density(physical, logical).or_else(|| {
        let hint = panel_hint?;
        let mode = info.modes.iter().find(|mode| mode.current)?;
        let (w, h) = if rotated { (mode.dimensions.1, mode.dimensions.0) } else { mode.dimensions };
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((hint * logical.0 as f32 / w as f32, hint * logical.1 as f32 / h as f32))
    })
}
fn measured_density(physical: (i32, i32), logical: (i32, i32)) -> Option<(f32, f32)> {
    if physical.0 <= 0 || physical.1 <= 0 || logical.0 <= 0 || logical.1 <= 0 {
        return None;
    }
    Some((logical.0 as f32 / physical.0 as f32, logical.1 as f32 / physical.1 as f32))
}
#[cfg(test)]
mod density_tests {
    use super::*;
    #[test]
    fn rotated_modes_and_host_scaling_use_reported_dimensions() {
        let mut info = smithay_client_toolkit::output::OutputData::new(1).with_output_info(Clone::clone);
        info.physical_size = (270, 480);
        info.transform = wl_output::Transform::_90;
        info.scale_factor = 2;
        info.modes.push(smithay_client_toolkit::output::Mode { dimensions: (1080, 1920), refresh_rate: 60000, current: true, preferred: true });
        assert_eq!(output_density(&info, None), Some((2.0, 2.0)));
        info.logical_size = Some((1440, 810));
        assert_eq!(output_density(&info, None), Some((3.0, 3.0)), "reported fractional scaling beats integer-scale inference");
        info.physical_size = (0, 0);
        assert_eq!(output_density(&info, None), None);
        assert_eq!(output_density(&info, Some(16.0)), Some((12.0, 12.0)), "only a matched panel may supply its native-density hint");
        info.physical_size = (270, 480);
        assert_eq!(output_density(&info, Some(16.0)), Some((3.0, 3.0)), "reported dimensions always beat a hint");
    }
    #[test]
    fn physical_dimensions_determine_density_and_missing_dimensions_stay_unknown() {
        assert_eq!(measured_density((480, 270), (1920, 1080)), Some((4.0, 4.0)));
        assert_eq!(measured_density((480, 270), (960, 540)), Some((2.0, 2.0)));
        assert_eq!(measured_density((0, 0), (1920, 1080)), None);
        assert_eq!(measured_density((480, 270), (0, 0)), None);
    }
}
