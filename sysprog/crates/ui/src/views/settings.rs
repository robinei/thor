//! Settings: a grid of tiles on black, one per setting, each an icon over its value. A tap steps to the
//! next value, a long press goes back to the default. The icon's colour says what the value does: grey
//! the stock value, green saves power, amber costs power, blue changes how the game looks or plays. Two
//! tabs: All games (the defaults) and the game in context (the running one, else shelf's selection),
//! whose own choices override the defaults one by one: those tiles carry a dot, the rest show the
//! default's value dimmed. Changes reach a running game at once. The environment tile opens an editor
//! over the on-screen keyboard.

use std::fmt::Write;

use sysprog_core::settings::{Impact, Setting};
use sysprog_core::{Id, Scope, State};

use super::{ACCENT, DIM, FN, Frame, GO, Look, PRESSED, SURFACE, SURFACE_HI, TEXT, TEXT_HI, keyboard, label};
use crate::draw::{Rect, rgb};
use crate::icons::Icon;

/// Padding, of the area's height.
const PAD: f32 = 0.03;
/// The tabs' height and text size, of the area's height.
const TABS_H: f32 = 0.1;
const TAB_TEXT: f32 = 0.045;
const NOTE: f32 = 0.032;
/// The tab label's width at most, of the area's width (long titles are cut).
const TAB_MAX: f32 = 0.5;
const COLUMNS: usize = 4;
const ROWS: usize = 4;
/// Gap between tiles, of the area's height.
const GAP: f32 = 0.014;
/// Icon size and value text size, of a tile's height.
const ICON: f32 = 0.36;
const VALUE: f32 = 0.15;
/// Tiles whose setting does nothing yet: barely there.
const LATER: u32 = 0x34343c;

/// What a tile is: a setting, the environment, or Reset.
#[derive(Clone, Copy)]
enum Tile {
    Setting(Setting),
    Env,
    Reset,
}

/// The grid, in order; the last tile is Reset on a game's tab, the charge limit on All games.
const GRID: [Tile; 15] = [
    Tile::Setting(Setting::Power),
    Tile::Setting(Setting::FpsCap),
    Tile::Setting(Setting::Refresh),
    Tile::Setting(Setting::Hud),
    Tile::Setting(Setting::Upscale),
    Tile::Setting(Setting::BottomScreen),
    Tile::Setting(Setting::BigCores),
    Tile::Setting(Setting::Fan),
    Tile::Setting(Setting::PadMouse),
    Tile::Setting(Setting::Buttons),
    Tile::Setting(Setting::Deadzone),
    Tile::Setting(Setting::Rumble),
    Tile::Setting(Setting::CpuMax),
    Tile::Setting(Setting::GpuMax),
    Tile::Env,
];

fn icon(s: Setting) -> Icon {
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
        Setting::Buttons => Icon::Buttons,
        Setting::Deadzone => Icon::Deadzone,
        Setting::Rumble => Icon::Rumble,
        Setting::CpuMax => Icon::Cpu,
        Setting::GpuMax => Icon::Gpu,
        Setting::ChargeLimit => Icon::Battery { tenths: 8, charging: true },
    }
}

pub(super) fn draw(f: &mut Frame, body: Rect, buf: &mut String) {
    if f.s.editor.is_some() {
        return editor(f, body, buf);
    }
    let s = f.s;
    let context = s.context();
    let game = s.editing();
    let pad = body.h * PAD;
    let x = body.x + pad;
    let w = body.w - 2.0 * pad;

    // Tabs: All games, then the game in context.
    let size = body.h * TAB_TEXT;
    let tabs_h = body.h * TABS_H;
    let mut tx = x - pad * 0.6;
    let tabs = [(Scope::Global, Some("All games")), (Scope::Game, context.map(|g| g.title.as_str()))];
    for (scope, name) in tabs {
        let Some(name) = name else {
            continue;
        };
        let current = (scope == Scope::Global) == game.is_none();
        let text_w = f.c.text_width(size, name).min(w * TAB_MAX);
        let r = Rect::new(tx, body.y, text_w + pad * 1.2, tabs_h);
        let look = Look { radius: 6.0, fill: None, inset: 4.0, ring_inset: 2.0 };
        f.button(Id::Scope(scope), r, look, |c| {
            let color = if current { TEXT_HI } else { DIM };
            let ty = r.y + (r.h - size * 1.2) / 2.0;
            super::wrap(c, (r.x + pad * 0.6, ty, text_w + 1.0), size, color, name, 1);
            if current {
                c.fill(Rect::new(r.x + pad * 0.6, r.y + r.h - 4.0, text_w, 2.0), rgb(ACCENT));
            }
        });
        tx += r.w + pad * 0.4;
    }
    let running = context.is_some_and(|g| s.running.as_ref() == Some(&g.id));
    let note = match (game, running) {
        (Some(_), true) => "applies now",
        (Some(_), false) => "applies at launch",
        (None, _) => "defaults",
    };
    let note_w = f.c.text_width(body.h * NOTE, note);
    if tx + pad + note_w < x + w {
        f.c.text(x + w - note_w, body.y + (tabs_h - body.h * NOTE * 1.2) / 2.0, body.h * NOTE, rgb(DIM), note);
    }

    // The grid below.
    let gap = body.h * GAP;
    let top = body.y + tabs_h + pad * 0.6;
    let tw = (w - (COLUMNS - 1) as f32 * gap) / COLUMNS as f32;
    let th = (body.y + body.h - pad - top - (ROWS - 1) as f32 * gap) / ROWS as f32;
    let last = if game.is_some() { Tile::Reset } else { Tile::Setting(Setting::ChargeLimit) };
    for (i, tile) in GRID.into_iter().chain([last]).enumerate() {
        let r = Rect::new(x + (i % COLUMNS) as f32 * (tw + gap), top + (i / COLUMNS) as f32 * (th + gap), tw, th);
        draw_tile(f, r, tile, buf);
    }
}

/// How a tile looks: its icon, its value (into `buf`), what the value does (the icon's colour), its own
/// choice (a dot, on a game's tab), following the default (dimmed value), and whether it does anything
/// yet.
struct Face {
    id: Id,
    icon: Icon,
    impact: Impact,
    own: bool,
    inherited: bool,
    active: bool,
}

fn face(s: &State, tile: Tile, buf: &mut String) -> Face {
    let game = s.editing().map(|g| &g.id);
    match tile {
        Tile::Setting(setting) => {
            let value = s.tile_value(setting);
            setting.describe(value, buf);
            let per_game = game.filter(|_| !setting.global_only());
            let own = per_game.is_some_and(|g| s.overrides_of(g).get(setting).is_some());
            Face {
                id: Id::Tile(setting),
                icon: icon(setting),
                impact: setting.impact(value),
                own,
                inherited: per_game.is_some() && !own,
                active: setting.active(),
            }
        }
        Tile::Env => {
            let text = match game {
                Some(g) => s.env.get(g).map_or("", |e| e.as_str()),
                None => s.global_env.as_str(),
            };
            let count = sysprog_core::count_env(text);
            buf.clear();
            match count {
                0 => buf.push_str("No variables"),
                1 => buf.push_str("1 variable"),
                n => {
                    let _ = write!(buf, "{n} variables");
                }
            }
            let impact = if count > 0 { Impact::Changes } else { Impact::Stock };
            Face { id: Id::Env, icon: Icon::Env, impact, own: game.is_some() && count > 0, inherited: false, active: true }
        }
        Tile::Reset => {
            let any = game.is_some_and(|g| !s.overrides_of(g).is_empty() || s.env.contains_key(g));
            buf.clear();
            buf.push_str(if any { "Reset" } else { "Nothing to reset" });
            Face { id: Id::Reset, icon: Icon::Reset, impact: Impact::Stock, own: false, inherited: false, active: any }
        }
    }
}

fn draw_tile(f: &mut Frame, r: Rect, tile: Tile, buf: &mut String) {
    let face = face(f.s, tile, buf);
    // No background: the grid's alignment is the structure; pressed still lights it.
    let look = Look { radius: 10.0, fill: None, inset: 0.0, ring_inset: -2.0 };
    let text: &str = buf;
    let later = !face.active && !matches!(tile, Tile::Reset);
    f.button(face.id, r, look, |c| {
        let icon_color = match (face.active, face.impact) {
            (false, _) => LATER,
            (true, Impact::Stock) => DIM,
            (true, Impact::Saves) => GO,
            (true, Impact::Costs) => ACCENT,
            (true, Impact::Changes) => FN,
        };
        let size = r.h * ICON;
        c.icon(face.icon, Rect::new(r.x + (r.w - size) / 2.0, r.y + r.h * 0.17, size, size), rgb(icon_color));
        let text_color = if face.active && !face.inherited { TEXT } else { DIM };
        label(c, Rect::new(r.x, r.y + r.h * 0.62, r.w, r.h * VALUE * 1.6), r.h * VALUE, text_color, text);
        if face.own {
            c.circle(r.x + r.w - r.h * 0.1, r.y + r.h * 0.1, r.h * 0.03, rgb(TEXT));
        }
        if later {
            c.text(r.x + r.h * 0.07, r.y + r.h * 0.05, r.h * 0.09, rgb(DIM), "later");
        }
    });
}

/// The environment editor: what it edits, the text so far (its end, if long), Cancel and Save, and the
/// keyboard below (its keys type here, Enter saves, Esc cancels).
fn editor(f: &mut Frame, body: Rect, buf: &mut String) {
    let s = f.s;
    let Some(editor) = &s.editor else {
        return;
    };
    let pad = body.h * PAD;
    let x = body.x + pad;
    let w = body.w - 2.0 * pad;
    let size = body.h * NOTE;
    let title = match &editor.game {
        Some(id) => s.library.games.iter().find(|g| &g.id == id).map_or("this game", |g| g.title.as_str()),
        None => "all games",
    };
    buf.clear();
    let _ = write!(buf, "Environment for {title}: KEY=value, space-separated");
    f.c.text(x, body.y + pad * 0.5, size, rgb(DIM), buf);

    // The field, with a caret after the text; its end shows when it is too long.
    let field = Rect::new(x, body.y + pad * 0.5 + size * 1.6, w * 0.7, body.h * 0.1);
    f.c.rounded(field, 8.0, rgb(SURFACE_HI));
    let text_size = field.h * 0.42;
    let room = field.w - pad * 1.5;
    let text = editor.text.as_str();
    let mut start = 0;
    while start < text.len() && f.c.text_width(text_size, text.get(start..).unwrap_or("")) > room {
        start = text.char_indices().map(|(i, _)| i).find(|&i| i > start).unwrap_or(text.len());
    }
    let shown = text.get(start..).unwrap_or("");
    let ty = field.y + (field.h - text_size * 1.2) / 2.0;
    let tw = f.c.text(field.x + pad * 0.6, ty, text_size, rgb(TEXT_HI), shown);
    f.c.fill(Rect::new(field.x + pad * 0.6 + tw + 2.0, ty, 2.0, text_size * 1.2), rgb(ACCENT));

    // Cancel and Save beside it.
    let bw = (w * 0.3 - pad * 1.5) / 2.0;
    let look = Look { radius: 8.0, fill: Some(SURFACE), inset: 0.0, ring_inset: -2.0 };
    let cancel = Rect::new(field.x + field.w + pad * 0.5, field.y, bw, field.h);
    f.button(Id::EnvCancel, cancel, look, |c| label(c, cancel, cancel.h * 0.36, TEXT, "Cancel"));
    let save = Rect::new(cancel.x + bw + pad * 0.5, field.y, bw, field.h);
    let look = Look { fill: Some(PRESSED), ..look };
    f.button(Id::EnvSave, save, look, |c| label(c, save, save.h * 0.36, TEXT_HI, "Save"));

    let keys_y = field.y + field.h + pad;
    keyboard::draw(f, Rect::new(body.x, keys_y, body.w, body.y + body.h - keys_y));
}
