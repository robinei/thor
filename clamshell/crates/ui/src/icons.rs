//! Line icons, drawn as paths on a 24x24 grid scaled into a square. No icon font or image files. Which
//! icon stands for what (a tab, a level, a signal, an app) is decided here too, by the constructors.

use clamshell_core::Tab;
use clamshell_core::bluetooth::Kind;
use clamshell_core::settings::Setting;
use clamshell_core::wifi::signal_bars;
use tiny_skia::Color;

use crate::draw::{Canvas, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Apps,
    /// The front program's windows on the bottom screen, hidden by ClamShell's view: the two screens, a
    /// window on the lower one.
    Windows,
    /// The pad mouse toggle: a mouse.
    Mouse,
    /// Game settings: three sliders.
    Sliders,
    // Settings tiles.
    Bolt,
    Gauge,
    Refresh,
    Hud,
    Upscale,
    ScreenOff,
    Cores,
    Fan,
    Buttons,
    Rumble,
    Cpu,
    Gpu,
    Env,
    Reset,
    Gamepad,
    Keyboard,
    Touchpad,
    Sysinfo,
    // Keyboard keys.
    Shift,
    Backspace,
    Enter,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    /// Charge in tenths (0-10) and whether it is charging.
    Battery {
        tenths: u8,
        charging: bool,
    },
    /// Lit arcs (0-3); `None`: not connected.
    Wifi {
        bars: Option<u8>,
    },
    /// Sound waves (0-3) and mute.
    Volume {
        waves: u8,
        muted: bool,
    },
    /// The screens' brightness: a sun.
    Brightness,
    /// A network that needs a password.
    Lock,
    /// Bluetooth's rune; `connected`: with the dots beside it.
    Bluetooth {
        connected: bool,
    },
    Headphones,
    Phone,
    /// Sleep: a crescent moon.
    Moon,
    /// Nothing: a circle struck through.
    Off,
    /// Layers: three sheets stacked.
    Layers,
    // Shelf's tabs (library.json names them: see [`Icon::named`]).
    Film,
    Tv,
    Music,
    Star,
    Clock,
    /// A folder in shelf's list: a chevron, it opens.
    Chevron,
}

impl Icon {
    /// A tab's icon in the bar, as it looks without a reading.
    pub fn for_tab(tab: Tab) -> Icon {
        match tab {
            Tab::Settings => Icon::Sliders,
            Tab::Dock => Icon::Apps,
            Tab::Keyboard => Icon::Keyboard,
            Tab::Touchpad => Icon::Touchpad,
            Tab::Sysinfo => Icon::Sysinfo,
            Tab::Volume => Icon::Volume { waves: 3, muted: false },
            Tab::Brightness => Icon::Brightness,
            Tab::Wifi => Icon::Wifi { bars: Some(3) },
            Tab::Battery => Icon::Battery { tenths: 10, charging: false },
            Tab::Bluetooth => Icon::Bluetooth { connected: false },
            // (Its button is the clock.)
            Tab::Time => Icon::Gauge,
        }
    }

    /// A shelf tab's icon, by the name library.json gives (`gamepad`, `apps`, `film`, `tv`, `music`,
    /// `star`, `clock`); none for another.
    pub fn named(name: &str) -> Option<Icon> {
        Some(match name {
            "gamepad" => Icon::Gamepad,
            "apps" => Icon::Apps,
            "film" => Icon::Film,
            "tv" => Icon::Tv,
            "music" => Icon::Music,
            "star" => Icon::Star,
            "clock" => Icon::Clock,
            _ => return None,
        })
    }

    /// A volume (0-100): a wave per third.
    pub fn volume(level: u8, muted: bool) -> Icon {
        let waves = match level {
            0 => 0,
            1..=33 => 1,
            34..=66 => 2,
            _ => 3,
        };
        Icon::Volume { waves, muted }
    }

    /// A Wi-Fi signal's quality (0-100); `None`: not connected.
    pub fn wifi(quality: Option<u8>) -> Icon {
        Icon::Wifi { bars: quality.map(signal_bars) }
    }

    /// A battery's charge (0-100), to the nearest tenth.
    pub fn battery(level: u8, charging: bool) -> Icon {
        Icon::Battery { tenths: ((u16::from(level) + 5) / 10) as u8, charging }
    }

    /// A settings tile's icon.
    pub fn for_setting(s: Setting) -> Icon {
        match s {
            Setting::Power => Icon::Bolt,
            Setting::FpsCap => Icon::Gauge,
            Setting::Refresh => Icon::Refresh,
            Setting::Hud => Icon::Hud,
            Setting::Upscale => Icon::Upscale,
            Setting::BottomScreen => Icon::ScreenOff,
            Setting::BigCores => Icon::Cores,
            Setting::Fan => Icon::Fan,
            Setting::PadMouse => Icon::Mouse,
            Setting::Keymap => Icon::Buttons,
            Setting::Rumble => Icon::Rumble,
            Setting::CpuMax => Icon::Cpu,
            Setting::GpuMax => Icon::Gpu,
            Setting::ChargeLimit => Icon::Battery { tenths: 8, charging: true },
            Setting::ScreenOff => Icon::ScreenOff,
            Setting::Sleep => Icon::Moon,
        }
    }

    /// A Bluetooth device's icon, by its kind.
    pub fn for_device(kind: Kind) -> Icon {
        match kind {
            Kind::Audio => Icon::Headphones,
            Kind::Gamepad => Icon::Gamepad,
            Kind::Keyboard => Icon::Keyboard,
            Kind::Mouse => Icon::Mouse,
            Kind::Phone => Icon::Phone,
            Kind::Computer | Kind::Other => Icon::Bluetooth { connected: false },
        }
    }
}

/// The usual stroke width, and the thin one of a scale's unlit steps (levels show as thick lit strokes;
/// the rest of the scale stays visible, thin), in grid units.
const LINE: f32 = 1.8;
const THIN: f32 = 0.8;

/// Draws on the 24x24 grid, scaled into a square of the canvas.
struct Pen<'a, 'c> {
    c: &'a mut Canvas<'c>,
    x0: f32,
    y0: f32,
    /// Logical pixels per grid unit.
    u: f32,
    color: Color,
}

impl Pen<'_, '_> {
    fn p(&self, (x, y): (f32, f32)) -> (f32, f32) {
        (self.x0 + x * self.u, self.y0 + y * self.u)
    }

    fn rect(&self, (x, y, w, h): (f32, f32, f32, f32)) -> Rect {
        let u = self.u;
        Rect::new(self.x0 + x * u, self.y0 + y * u, w * u, h * u)
    }

    /// The points on the canvas (at most 12; an icon's paths are short).
    fn points(&self, points: &[(f32, f32)]) -> ([(f32, f32); 12], usize) {
        let mut out = [(0.0, 0.0); 12];
        let n = points.len().min(out.len());
        for (o, &p) in out.iter_mut().zip(points) {
            *o = self.p(p);
        }
        (out, n)
    }

    /// Lines through `points`, `width` wide.
    fn line(&mut self, points: &[(f32, f32)], width: f32) {
        let (points, n) = self.points(points);
        self.c.polyline(points.get(..n).unwrap_or(&[]), width * self.u, self.color);
    }

    /// A closed shape through `points`.
    fn shape(&mut self, points: &[(f32, f32)], width: f32) {
        let (points, n) = self.points(points);
        self.c.polygon(points.get(..n).unwrap_or(&[]), width * self.u, self.color);
    }

    /// A rounded rectangle's outline: x, y, w, h.
    fn frame(&mut self, r: (f32, f32, f32, f32), radius: f32, width: f32) {
        let r = self.rect(r);
        self.c.outline(r, radius * self.u, width * self.u, self.color);
    }

    /// A filled rounded rectangle: x, y, w, h.
    fn block(&mut self, r: (f32, f32, f32, f32), radius: f32) {
        let r = self.rect(r);
        self.c.rounded(r, radius * self.u, self.color);
    }

    fn dot(&mut self, x: f32, y: f32, radius: f32) {
        self.c.circle(self.x0 + x * self.u, self.y0 + y * self.u, radius * self.u, self.color);
    }

    /// An arc around `center` from `from` to `to` degrees (counterclockwise from the right).
    fn arc(&mut self, (cx, cy): (f32, f32), r: f32, (from, to): (f32, f32), width: f32) {
        let mut points = [(0.0, 0.0); 7];
        for (i, point) in points.iter_mut().enumerate() {
            let a = (from + (to - from) * i as f32 / 6.0).to_radians();
            *point = self.p((cx + r * a.cos(), cy - r * a.sin()));
        }
        self.c.polyline(&points, width * self.u, self.color);
    }
}

/// Draws `icon` centred in `r`, as large as fits.
pub fn draw(c: &mut Canvas, icon: Icon, r: Rect, color: Color) {
    let size = r.w.min(r.h);
    let (cx, cy) = r.center();
    let mut pen = Pen { c, x0: cx - size / 2.0, y0: cy - size / 2.0, u: size / 24.0, color };
    let p = &mut pen;
    match icon {
        Icon::Sliders => sliders(p),
        Icon::Bolt => p.shape(&[(13.5, 2.0), (4.5, 14.0), (11.0, 14.0), (10.0, 22.0), (19.5, 10.0), (13.0, 10.0)], LINE),
        Icon::Gauge => gauge(p),
        Icon::Refresh => refresh(p),
        Icon::Hud => {
            p.frame((2.0, 4.0, 20.0, 16.0), 2.0, LINE);
            p.block((4.5, 6.5, 7.0, 4.5), 1.0);
        }
        Icon::Upscale => {
            p.frame((3.0, 13.0, 8.0, 8.0), 1.5, LINE);
            p.line(&[(9.0, 15.0), (20.0, 4.0)], LINE);
            p.line(&[(13.5, 4.0), (20.0, 4.0), (20.0, 10.5)], LINE);
        }
        Icon::ScreenOff => {
            p.frame((2.5, 5.0, 19.0, 14.0), 2.0, LINE);
            p.line(&[(3.0, 3.0), (21.0, 21.0)], LINE);
        }
        Icon::Cores => cores(p),
        Icon::Fan => fan(p),
        Icon::Buttons => {
            for (x, y) in [(12.0, 5.0), (19.0, 12.0), (12.0, 19.0), (5.0, 12.0)] {
                p.frame((x - 3.0, y - 3.0, 6.0, 6.0), 3.0, 1.6);
            }
        }
        Icon::Rumble => rumble(p),
        Icon::Cpu => cpu(p),
        Icon::Gpu => gpu(p),
        Icon::Env => {
            p.frame((2.0, 4.0, 20.0, 16.0), 2.0, LINE);
            p.line(&[(6.0, 9.0), (9.5, 12.0), (6.0, 15.0)], LINE);
            p.line(&[(11.5, 15.5), (17.5, 15.5)], LINE);
        }
        Icon::Reset => reset(p),
        Icon::Gamepad => gamepad(p),
        Icon::Windows => {
            p.frame((4.0, 2.5, 16.0, 8.5), 1.5, LINE);
            p.frame((4.0, 13.0, 16.0, 8.5), 1.5, LINE);
            p.line(&[(4.5, 16.0), (19.5, 16.0)], LINE);
        }
        Icon::Mouse => {
            p.frame((6.0, 3.0, 12.0, 18.0), 6.0, LINE);
            p.line(&[(12.0, 3.5), (12.0, 9.5)], LINE);
        }
        Icon::Apps => {
            for (x, y) in [(4.0, 4.0), (13.0, 4.0), (4.0, 13.0), (13.0, 13.0)] {
                p.frame((x, y, 7.0, 7.0), 1.5, LINE);
            }
        }
        Icon::Film => {
            p.frame((3.0, 4.0, 18.0, 16.0), 1.5, LINE);
            p.line(&[(7.5, 4.0), (7.5, 20.0)], LINE);
            p.line(&[(16.5, 4.0), (16.5, 20.0)], LINE);
            for y in [9.5, 14.5] {
                p.line(&[(3.0, y), (7.5, y)], LINE);
                p.line(&[(16.5, y), (21.0, y)], LINE);
            }
        }
        Icon::Tv => {
            p.frame((2.5, 7.0, 19.0, 13.0), 2.0, LINE);
            p.line(&[(8.0, 2.5), (12.0, 7.0), (16.0, 2.5)], LINE);
        }
        Icon::Music => {
            p.line(&[(9.0, 18.0), (9.0, 5.0), (19.5, 3.0), (19.5, 16.0)], LINE);
            p.dot(6.5, 18.0, 2.6);
            p.dot(17.0, 16.0, 2.6);
        }
        Icon::Star => {
            p.shape(&[(12.0, 2.8), (14.5, 9.3), (21.5, 9.7), (16.1, 14.1), (17.9, 20.9), (12.0, 17.1), (6.1, 20.9), (7.9, 14.1), (2.5, 9.7), (9.5, 9.3)], LINE)
        }
        Icon::Clock => {
            for quarter in 0..4 {
                let from = quarter as f32 * 90.0;
                p.arc((12.0, 12.0), 9.0, (from, from + 90.0), LINE);
            }
            p.line(&[(12.0, 6.5), (12.0, 12.0), (15.5, 14.5)], LINE);
        }
        Icon::Keyboard => keyboard(p),
        Icon::Touchpad => {
            p.frame((3.0, 4.0, 18.0, 16.0), 3.0, LINE);
            p.line(&[(3.0, 15.0), (21.0, 15.0)], LINE);
            p.line(&[(12.0, 15.0), (12.0, 20.0)], LINE);
        }
        Icon::Sysinfo => {
            p.line(&[(6.0, 20.0), (6.0, 13.0)], 2.4);
            p.line(&[(12.0, 20.0), (12.0, 5.0)], 2.4);
            p.line(&[(18.0, 20.0), (18.0, 10.0)], 2.4);
        }
        Icon::Shift => p.shape(&[(12.0, 3.0), (20.0, 11.5), (15.5, 11.5), (15.5, 18.5), (8.5, 18.5), (8.5, 11.5), (4.0, 11.5)], LINE),
        Icon::Backspace => {
            p.shape(&[(8.0, 6.0), (21.0, 6.0), (21.0, 18.0), (8.0, 18.0), (3.0, 12.0)], LINE);
            p.line(&[(12.0, 9.5), (17.0, 14.5)], LINE);
            p.line(&[(17.0, 9.5), (12.0, 14.5)], LINE);
        }
        Icon::Enter => {
            p.line(&[(19.0, 5.0), (19.0, 14.0), (5.0, 14.0)], LINE);
            p.line(&[(9.0, 10.0), (5.0, 14.0), (9.0, 18.0)], LINE);
        }
        Icon::ArrowUp => arrow(p, [(12.0, 19.0), (12.0, 5.0)], [(6.0, 11.0), (12.0, 5.0), (18.0, 11.0)]),
        Icon::ArrowDown => arrow(p, [(12.0, 5.0), (12.0, 19.0)], [(6.0, 13.0), (12.0, 19.0), (18.0, 13.0)]),
        Icon::ArrowLeft => arrow(p, [(19.0, 12.0), (5.0, 12.0)], [(11.0, 6.0), (5.0, 12.0), (11.0, 18.0)]),
        Icon::ArrowRight => arrow(p, [(5.0, 12.0), (19.0, 12.0)], [(13.0, 6.0), (19.0, 12.0), (13.0, 18.0)]),
        Icon::Chevron => p.line(&[(9.0, 5.0), (16.0, 12.0), (9.0, 19.0)], 2.2),
        Icon::Battery { tenths, charging } => battery(p, tenths, charging),
        Icon::Wifi { bars } => wifi(p, bars),
        Icon::Volume { waves, muted } => volume(p, waves, muted),
        Icon::Brightness => sun(p),
        Icon::Layers => {
            p.shape(&[(12.0, 3.5), (21.0, 8.0), (12.0, 12.5), (3.0, 8.0)], LINE);
            p.line(&[(3.0, 12.0), (12.0, 16.5), (21.0, 12.0)], LINE);
            p.line(&[(3.0, 16.0), (12.0, 20.5), (21.0, 16.0)], LINE);
        }
        Icon::Off => {
            p.frame((3.5, 3.5, 17.0, 17.0), 8.5, LINE);
            p.line(&[(6.5, 17.5), (17.5, 6.5)], LINE);
        }
        Icon::Moon => {
            // The lit edge (the long way round the left), and the shadow's edge joining its ends.
            p.arc((12.0, 12.0), 8.0, (60.0, 300.0), LINE);
            p.arc((19.0, 12.0), 7.55, (113.0, 247.0), LINE);
        }
        Icon::Lock => {
            p.block((5.0, 11.0, 14.0, 10.0), 1.5);
            p.arc((12.0, 10.0), 4.5, (0.0, 180.0), LINE);
            p.line(&[(7.5, 10.0), (7.5, 11.5)], LINE);
            p.line(&[(16.5, 10.0), (16.5, 11.5)], LINE);
        }
        Icon::Bluetooth { connected } => {
            p.line(&[(7.0, 7.5), (17.0, 16.5), (12.0, 21.0), (12.0, 3.0), (17.0, 7.5), (7.0, 16.5)], LINE);
            if connected {
                p.dot(3.5, 12.0, 1.3);
                p.dot(20.5, 12.0, 1.3);
            }
        }
        Icon::Headphones => {
            p.arc((12.0, 13.0), 8.5, (0.0, 180.0), LINE);
            p.block((2.5, 13.0, 4.5, 8.0), 1.5);
            p.block((17.0, 13.0, 4.5, 8.0), 1.5);
        }
        Icon::Phone => {
            p.frame((6.5, 2.0, 11.0, 20.0), 2.0, LINE);
            p.line(&[(10.5, 18.5), (13.5, 18.5)], LINE);
        }
    }
}

fn sliders(p: &mut Pen) {
    for (y, knob) in [(6.0, 15.0), (12.0, 8.0), (18.0, 13.0)] {
        p.line(&[(3.0, y), (21.0, y)], LINE);
        p.dot(knob, y, 2.6);
    }
}

fn gauge(p: &mut Pen) {
    p.arc((12.0, 14.0), 9.0, (90.0, 210.0), LINE);
    p.arc((12.0, 14.0), 9.0, (-30.0, 90.0), LINE);
    p.line(&[(12.0, 14.0), (16.5, 8.5)], LINE);
    p.dot(12.0, 14.0, 1.8);
}

fn refresh(p: &mut Pen) {
    p.frame((2.0, 4.0, 20.0, 13.0), 2.0, LINE);
    p.line(&[(8.0, 20.5), (16.0, 20.5)], LINE);
    p.line(&[(5.5, 11.0), (7.5, 8.5), (9.5, 11.0), (11.5, 13.5), (13.5, 11.0), (15.5, 8.5), (17.5, 11.0)], 1.5);
}

fn cores(p: &mut Pen) {
    p.frame((3.0, 3.0, 11.0, 11.0), 1.5, LINE);
    for (x, y) in [(17.0, 3.0), (17.0, 9.0), (3.0, 17.0), (9.0, 17.0), (17.0, 17.0)] {
        p.frame((x, y, 4.5, 4.5), 1.0, 1.5);
    }
}

fn fan(p: &mut Pen) {
    p.dot(12.0, 12.0, 2.0);
    for k in 0..3 {
        let a = (k as f32 * 120.0).to_radians();
        let b = (k as f32 * 120.0 + 55.0).to_radians();
        p.shape(&[(12.0, 12.0), (12.0 + 8.5 * a.cos(), 12.0 - 8.5 * a.sin()), (12.0 + 7.0 * b.cos(), 12.0 - 7.0 * b.sin())], 1.6);
    }
}

fn rumble(p: &mut Pen) {
    p.frame((8.0, 3.5, 8.0, 17.0), 2.0, LINE);
    p.line(&[(4.5, 7.0), (2.5, 9.5), (4.5, 12.0), (2.5, 14.5), (4.5, 17.0)], 1.5);
    p.line(&[(19.5, 7.0), (21.5, 9.5), (19.5, 12.0), (21.5, 14.5), (19.5, 17.0)], 1.5);
}

fn cpu(p: &mut Pen) {
    p.frame((6.0, 6.0, 12.0, 12.0), 1.5, LINE);
    p.block((9.5, 9.5, 5.0, 5.0), 0.8);
    for t in [9.0, 12.0, 15.0] {
        p.line(&[(t, 2.5), (t, 6.0)], 1.4);
        p.line(&[(t, 18.0), (t, 21.5)], 1.4);
        p.line(&[(2.5, t), (6.0, t)], 1.4);
        p.line(&[(18.0, t), (21.5, t)], 1.4);
    }
}

fn gpu(p: &mut Pen) {
    p.frame((2.0, 6.0, 20.0, 12.0), 1.5, LINE);
    p.frame((5.5, 8.5, 7.0, 7.0), 3.5, 1.5);
    p.line(&[(15.5, 9.5), (15.5, 14.5)], 1.5);
    p.line(&[(18.5, 9.5), (18.5, 14.5)], 1.5);
    p.line(&[(6.0, 18.0), (6.0, 21.0), (13.0, 21.0), (13.0, 18.0)], 1.4);
}

fn reset(p: &mut Pen) {
    p.arc((12.0, 13.0), 8.0, (60.0, 195.0), LINE);
    p.arc((12.0, 13.0), 8.0, (195.0, 330.0), LINE);
    p.arc((12.0, 13.0), 8.0, (-30.0, 15.0), LINE);
    p.line(&[(11.5, 3.5), (16.0, 6.1), (13.0, 10.0)], LINE);
}

fn gamepad(p: &mut Pen) {
    let body = [(6.5, 7.0), (17.5, 7.0), (21.0, 15.0), (19.5, 18.5), (16.0, 17.0), (14.0, 14.5), (10.0, 14.5), (8.0, 17.0), (4.5, 18.5), (3.0, 15.0)];
    p.shape(&body, LINE);
    p.line(&[(6.0, 10.8), (10.0, 10.8)], 1.5);
    p.line(&[(8.0, 8.8), (8.0, 12.8)], 1.5);
    p.dot(15.5, 10.0, 1.1);
    p.dot(17.5, 12.0, 1.1);
}

fn keyboard(p: &mut Pen) {
    p.frame((2.0, 6.0, 20.0, 12.0), 2.0, LINE);
    for y in [9.5, 12.5] {
        for x in [6.0, 9.0, 12.0, 15.0, 18.0] {
            p.dot(x, y, 0.9);
        }
    }
    p.line(&[(8.5, 15.3), (15.5, 15.3)], LINE);
}

/// A shaft and its head.
fn arrow(p: &mut Pen, shaft: [(f32, f32); 2], head: [(f32, f32); 3]) {
    p.line(&shaft, LINE);
    p.line(&head, LINE);
}

fn battery(p: &mut Pen, tenths: u8, charging: bool) {
    p.frame((1.0, 7.5, 15.5, 9.0), 2.0, 1.5);
    p.block((17.0, 10.0, 1.6, 4.0), 0.6);
    let level = f32::from(tenths.min(10)) / 10.0;
    if level > 0.0 {
        p.block((3.0, 9.5, 11.5 * level, 5.0), 0.8);
    }
    if charging {
        p.line(&[(22.5, 5.0), (19.8, 12.6), (22.8, 12.6), (20.2, 19.5)], 1.6);
    }
}

fn wifi(p: &mut Pen, bars: Option<u8>) {
    p.dot(12.0, 19.0, 1.6);
    let lit = bars.unwrap_or(0);
    for (i, r) in [5.5, 10.0, 14.5].into_iter().enumerate() {
        let width = if (i as u8) < lit { 2.2 } else { THIN };
        p.arc((12.0, 19.0), r, (45.0, 135.0), width);
    }
    if bars.is_none() {
        p.line(&[(4.0, 4.0), (20.0, 20.0)], LINE);
    }
}

fn volume(p: &mut Pen, waves: u8, muted: bool) {
    p.shape(&[(2.5, 9.0), (6.5, 9.0), (11.5, 4.5), (11.5, 19.5), (6.5, 15.0), (2.5, 15.0)], 1.6);
    if muted {
        p.line(&[(15.5, 9.0), (21.5, 15.0)], LINE);
        p.line(&[(21.5, 9.0), (15.5, 15.0)], LINE);
    } else {
        for (i, r) in [4.0, 7.0, 10.0].into_iter().enumerate() {
            let width = if (i as u8) < waves { 2.0 } else { THIN };
            p.arc((11.5, 12.0), r, (-50.0, 50.0), width);
        }
    }
}

fn sun(p: &mut Pen) {
    p.frame((7.5, 7.5, 9.0, 9.0), 4.5, LINE);
    for k in 0..8 {
        let a = (k as f32 * 45.0).to_radians();
        let (dx, dy) = (a.cos(), a.sin());
        p.line(&[(12.0 + 7.0 * dx, 12.0 - 7.0 * dy), (12.0 + 10.0 * dx, 12.0 - 10.0 * dy)], LINE);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::draw::DrawCache;
    use tiny_skia::Pixmap;

    /// Every icon, each variant at least once; `variant` fails to compile when one is added.
    fn every() -> Vec<Icon> {
        let all = vec![
            Icon::Apps,
            Icon::Mouse,
            Icon::Sliders,
            Icon::Bolt,
            Icon::Gauge,
            Icon::Refresh,
            Icon::Hud,
            Icon::Upscale,
            Icon::ScreenOff,
            Icon::Cores,
            Icon::Fan,
            Icon::Buttons,
            Icon::Rumble,
            Icon::Cpu,
            Icon::Gpu,
            Icon::Env,
            Icon::Reset,
            Icon::Gamepad,
            Icon::Keyboard,
            Icon::Touchpad,
            Icon::Sysinfo,
            Icon::Shift,
            Icon::Backspace,
            Icon::Enter,
            Icon::ArrowUp,
            Icon::ArrowDown,
            Icon::ArrowLeft,
            Icon::ArrowRight,
            Icon::Battery { tenths: 0, charging: false },
            Icon::Battery { tenths: 7, charging: true },
            Icon::Wifi { bars: None },
            Icon::Wifi { bars: Some(2) },
            Icon::Volume { waves: 0, muted: true },
            Icon::Volume { waves: 3, muted: false },
            Icon::Brightness,
            Icon::Lock,
            Icon::Bluetooth { connected: false },
            Icon::Bluetooth { connected: true },
            Icon::Headphones,
            Icon::Moon,
            Icon::Phone,
            Icon::Off,
            Icon::Layers,
            Icon::Windows,
            Icon::Film,
            Icon::Tv,
            Icon::Music,
            Icon::Star,
            Icon::Clock,
            Icon::Chevron,
        ];
        let mut seen = std::collections::HashSet::new();
        for icon in &all {
            seen.insert(variant(*icon));
        }
        assert_eq!(seen.len(), 46, "a variant is missing from the list");
        all
    }

    /// The variant's number, without its data.
    fn variant(icon: Icon) -> u8 {
        match icon {
            Icon::Apps => 0,
            Icon::Mouse => 1,
            Icon::Sliders => 2,
            Icon::Bolt => 3,
            Icon::Gauge => 4,
            Icon::Refresh => 5,
            Icon::Hud => 6,
            Icon::Upscale => 7,
            Icon::ScreenOff => 8,
            Icon::Cores => 9,
            Icon::Fan => 10,
            Icon::Buttons => 11,
            Icon::Rumble => 13,
            Icon::Cpu => 14,
            Icon::Gpu => 15,
            Icon::Env => 16,
            Icon::Reset => 17,
            Icon::Gamepad => 20,
            Icon::Keyboard => 25,
            Icon::Touchpad => 26,
            Icon::Sysinfo => 27,
            Icon::Shift => 28,
            Icon::Backspace => 29,
            Icon::Enter => 30,
            Icon::ArrowUp => 31,
            Icon::ArrowDown => 32,
            Icon::ArrowLeft => 33,
            Icon::ArrowRight => 34,
            Icon::Battery { .. } => 35,
            Icon::Wifi { .. } => 36,
            Icon::Volume { .. } => 37,
            Icon::Brightness => 38,
            Icon::Lock => 39,
            Icon::Bluetooth { .. } => 40,
            Icon::Headphones => 41,
            Icon::Phone => 42,
            Icon::Moon => 43,
            Icon::Off => 44,
            Icon::Layers => 45,
            Icon::Windows => 46,
            Icon::Film => 47,
            Icon::Tv => 48,
            Icon::Music => 49,
            Icon::Star => 50,
            Icon::Clock => 51,
            Icon::Chevron => 52,
        }
    }

    #[test]
    fn every_icon_draws_something() {
        let mut cache = DrawCache::load();
        for icon in every() {
            let mut pixmap = Pixmap::new(48, 48).expect("pixmap");
            Canvas::new(&mut pixmap, 1.0, &mut cache).icon(icon, Rect::new(0.0, 0.0, 48.0, 48.0), Color::WHITE);
            let covered = pixmap.data().chunks_exact(4).filter(|px| px.get(3).is_some_and(|&a| a > 0)).count();
            assert!(covered > 20, "{icon:?}: {covered} pixels");
        }
    }

    #[test]
    fn levels_pick_their_icons() {
        assert_eq!(Icon::volume(0, false), Icon::Volume { waves: 0, muted: false });
        assert_eq!(Icon::volume(50, true), Icon::Volume { waves: 2, muted: true });
        assert_eq!(Icon::wifi(Some(80)), Icon::Wifi { bars: Some(3) });
        assert_eq!(Icon::wifi(None), Icon::Wifi { bars: None });
        assert_eq!(Icon::battery(44, false), Icon::Battery { tenths: 4, charging: false });
        assert_eq!(Icon::battery(100, true), Icon::Battery { tenths: 10, charging: true });
    }
}
