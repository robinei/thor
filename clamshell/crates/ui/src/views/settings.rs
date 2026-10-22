//! Settings: a grid of tiles on black, one per setting, each an icon over its value. A tap steps to the
//! next value, and a long press goes back to the default. The icon's colour says what the value does:
//! grey for the stock value, green when it saves power, amber when it costs power, blue when it changes
//! how the game looks or plays. Two scopes sit as tabs along the top: All games (the defaults) and the
//! game in context (the running one, else shelf's selection). A game's own choices override the defaults
//! one by one; those tiles carry a dot, and the rest show the default's value dimmed. Changes reach a
//! running game at once. The environment tile opens an editor over the on-screen keyboard.

use clamshell_core::bottom::editor::{Editor, KeymapName, Purpose};
use clamshell_core::settings::{Impact, Setting};
use clamshell_core::{Id, Scope, SettingsId, State, TileOrigin};

use crate::draw::{Align, Rect, Rgb};
use crate::icons::Icon;
use crate::toolkit::Frame;
use crate::toolkit::format::{Fmt, Variables};
use crate::toolkit::style::{self, Text, Tone};
use crate::toolkit::widgets::{self, TileFace, TileShape};

/// The scope tabs' row, of the area's height.
const TABS_H: f32 = 0.1;
/// The tab label's width at most, of the area's width (long titles are cut).
const TAB_MAX: f32 = 0.5;
const COLUMNS: usize = 4;
const ROWS: usize = 4;
/// The icon's size and top, of a tile's height.
/// (Values are short; in a narrow window, a long one runs over the tile's edges.)
const TILE: TileShape = TileShape { icon: 0.36, icon_y: 0.17, cut: None };
/// Tiles whose setting does nothing yet: barely there.
const LATER: Rgb = Rgb(0x34343c);

/// What a tile is: a setting, the environment, or Reset.
#[derive(Clone, Copy)]
enum Tile {
    Setting(Setting),
    Env,
    Reset,
}

/// The grid, in order; on a game's tab Reset follows. (Settings for the device rather than a game, the
/// charge limit and idle, are the battery tab's.)
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
    Tile::Setting(Setting::Keymap),
    Tile::Setting(Setting::Rumble),
    Tile::Setting(Setting::CpuMax),
    Tile::Setting(Setting::GpuMax),
    Tile::Setting(Setting::Background),
    Tile::Env,
];

pub(super) fn draw(f: &mut Frame) {
    let body = f.body();
    if f.s.bottom().editor().is_some() {
        return editor(f);
    }
    if let Some(screen) = f.s.keymap_screen() {
        return super::keymaps::draw(f, screen);
    }
    let s = f.s;
    let context = s.context();
    let game = s.editing();
    let shown = s.scope_shown();
    let pad = f.pad();
    let mut grid = style::content(body);
    let bar = grid.take_top(f.body().h * TABS_H);

    // Tabs: All (the defaults), then the item's levels (its runner's, its own); a note on the right.
    let inner = pad * 0.8;
    let levels = s.setting_levels();
    let tabs = std::iter::once((Scope::Global, "All")).chain(levels.iter().flatten().map(|(scope, level)| (*scope, level.title)));
    let items = tabs.map(|(scope, name)| (Id::Settings(SettingsId::Scope(scope)), None, name, scope == shown));
    let tabs_area = Rect::new(bar.x - inner, bar.y, bar.w + inner, bar.h);
    let tabs_end = widgets::tabs(f, tabs_area, f.text(Text::Heading), inner, bar.w * TAB_MAX, items);
    let running = context.as_ref().is_some_and(|g| s.program_in_force() == Some(&g.id));
    let note = match (game.is_some().then_some(()), running) {
        (Some(_), true) => "applies now",
        (Some(_), false) => "applies at launch",
        (None, _) => "defaults",
    };
    let size = f.text(Text::Small);
    let note_w = f.c.text_width(size, note);
    if tabs_end + pad + note_w < bar.x + bar.w {
        f.c.text_in(bar, size, Tone::Dim, note, Align::End);
    }

    // The grid below.
    let grid = Rect::new(grid.x, grid.y + pad * 0.6, grid.w, grid.h - pad * 0.6);
    let last = game.is_some().then_some(Tile::Reset);
    for (i, tile) in GRID.into_iter().chain(last).enumerate() {
        draw_tile(f, grid.cell(COLUMNS, ROWS, f.body().h * style::TILE_GAP, i), tile);
    }
}

/// How a tile looks: its icon, its value, what the value does (the icon's colour), where it comes from
/// (the game's own choice: a dot; following the default on a game's tab: dimmed value), and whether it
/// does anything (dimmed, with a note why not: not yet, or not for what is edited).
struct Face {
    id: Id,
    icon: Icon,
    value: Fmt<32>,
    impact: Impact,
    origin: TileOrigin,
    active: bool,
    note: &'static str,
}

/// A tile's note when it does nothing: not yet (`later`), or not for what is edited (`no_effect`).
fn note(later: bool, no_effect: bool) -> &'static str {
    if later {
        "later"
    } else if no_effect {
        "no effect"
    } else {
        ""
    }
}

fn face(s: &State, tile: Tile) -> Face {
    let game = s.editing().map(|g| g.id);
    match tile {
        Tile::Setting(setting) => {
            let value = s.tile_value(setting);
            // The keymap's tile shows its name.
            let text = match setting {
                Setting::Keymap => Fmt::of(format_args!("{}", s.keymap_name(value))),
                _ => {
                    let mut text = Fmt::new();
                    setting.describe(value, &mut text);
                    text
                }
            };
            Face {
                id: Id::Settings(SettingsId::Tile(setting)),
                icon: Icon::for_setting(setting),
                value: text,
                impact: setting.impact(value),
                origin: s.tile_origin(setting),
                active: setting.active() && !(setting.through_launch() && s.handed_over()),
                note: note(!setting.active(), setting.through_launch() && s.handed_over()),
            }
        }
        Tile::Env => {
            let count = clamshell_core::settings::env::count_env(s.settings().env_text(game.as_deref()));
            let impact = if count > 0 { Impact::Changes } else { Impact::Stock };
            let origin = if game.is_some() && count > 0 { TileOrigin::Own } else { TileOrigin::Global };
            let value = Fmt::of(format_args!("{}", Variables(count)));
            let off = s.handed_over();
            Face { id: Id::Settings(SettingsId::Env), icon: Icon::Env, value, impact, origin, active: !off, note: note(false, off) }
        }
        Tile::Reset => {
            let any = game.as_ref().is_some_and(|g| s.settings().has_own(g));
            let value = Fmt::of(format_args!("{}", if any { "Reset" } else { "Nothing to reset" }));
            Face { id: Id::Settings(SettingsId::Reset), icon: Icon::Reset, value, impact: Impact::Stock, origin: TileOrigin::Global, active: any, note: "" }
        }
    }
}

/// A global setting's tile outside the grid (the battery tab's), drawn as in it.
pub(super) fn setting_tile(f: &mut Frame, r: Rect, setting: Setting) {
    draw_tile(f, r, Tile::Setting(setting));
}

fn draw_tile(f: &mut Frame, r: Rect, tile: Tile) {
    let face = face(f.s, tile);
    let tile_face = TileFace {
        caption: &face.value,
        caption_tone: if face.active && face.origin != TileOrigin::Inherited { Tone::Normal } else { Tone::Dim },
        dot: (face.origin == TileOrigin::Own).then_some(Tone::Normal),
        note: face.note,
    };
    // No background: the grid's alignment is the structure; pressed still lights it.
    widgets::tile(f, face.id.clone(), r, &TILE, tile_face, |f, r| {
        let color = match (face.active, face.impact) {
            (false, _) => LATER,
            (true, Impact::Stock) => Tone::Dim.rgb(),
            (true, Impact::Saves) => Tone::Go.rgb(),
            (true, Impact::Costs) => Tone::Accent.rgb(),
            (true, Impact::Changes) => Tone::Fn.rgb(),
        };
        f.c.icon(face.icon, TILE.square(r), color);
    });
}

/// The editor over the settings: the environment, or a keymap's name; then the shared editor (field,
/// Cancel and Save, keyboard).
fn editor(f: &mut Frame) {
    let s = f.s;
    let title = match s.bottom().editor() {
        Some(Editor { purpose: Purpose::Env { entry: game }, .. }) => {
            let title = match game {
                Some(id) => s.entry(id).map_or("this game", |g| g.title.as_str()),
                None => "all games",
            };
            Fmt::<160>::of(format_args!("Environment for {title}: KEY=value, space-separated"))
        }
        Some(Editor { purpose: Purpose::KeymapName(name), .. }) => match name {
            KeymapName::New => Fmt::of(format_args!("Name of the new keymap")),
            KeymapName::Rename(id) => Fmt::of(format_args!("New name for {}", s.keymap_name(*id))),
            KeymapName::Duplicate(id) => Fmt::of(format_args!("Name of the copy of {}", s.keymap_name(*id))),
            KeymapName::SaveAs(_) => Fmt::of(format_args!("A name for the copy (other games can use it)")),
            KeymapName::NewLayer(..) => Fmt::of(format_args!("Name of the new layer")),
            KeymapName::RenameLayer(id, layer) => {
                let name = s.keymaps().get(*id).and_then(|k| k.layer_name(*layer)).unwrap_or("the layer");
                Fmt::of(format_args!("New name for {name}"))
            }
        },
        _ => return,
    };
    super::editor::draw(f, &title, "Save");
}
