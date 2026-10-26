//! Settings: a grid of tiles on black, one per setting, each an icon over its value. A tap steps to the
//! next value, and a long press goes back to the default. The icon's colour says what the value does:
//! grey for the stock value, green when it saves power, amber when it costs power, blue when it changes
//! how the game looks or plays.
//!
//! Tabs along the top choose the scope: All games (the defaults) and the current game, which is the
//! running one or else the shelf's selection. A game's own choices override the defaults one by one.
//! Those tiles carry a dot, and the rest show the default's value dimmed. Changes reach a running game at
//! once. The environment tile opens an editor over the on-screen keyboard.

use clamshell_state::bottom::editor::{Editor, KeymapName, Purpose};
use clamshell_state::profiles::picker::ProfileName;
use clamshell_state::settings::{Impact, Setting};
use clamshell_state::{Id, ListId, Scope, SettingsId, State, TileOrigin};

use crate::gfx::icons::Icon;
use crate::gfx::{Align, Rect, Rgb};
use crate::toolkit::Frame;
use crate::toolkit::format::{Fmt, Variables};
use crate::toolkit::style::{self, Text, Tone};
use crate::toolkit::widgets::{self, TileFace, TileShape};

/// The height of the scope tabs' row, as a fraction of the area's height.
const TABS_H: f32 = 0.1;
/// The largest width of a tab label, as a fraction of the area's width. Longer titles are cut.
const TAB_MAX: f32 = 0.5;
const COLUMNS: usize = 5;
const ROWS: usize = 4;
/// The icon's size and top, as fractions of a tile's height. Values are not cut, since they are short; in
/// a narrow window a long one runs over the tile's edges.
const TILE: TileShape = TileShape { icon: 0.36, icon_y: 0.17, cut: None };
/// The icon colour of tiles whose setting does nothing for what is edited, barely visible.
const INERT: Rgb = Rgb(0x34343c);

/// What a tile is: a setting, the environment, or Reset.
#[derive(Clone, Copy)]
enum Tile {
    Setting(Setting),
    Env,
    Reset,
}

/// The grid, in order. On a game's tab, Reset follows. The settings for the device rather than a game,
/// the charge limit and idle, are on the battery tab. Five columns and four rows hold it all with Reset.
const GRID: [Tile; 19] = [
    Tile::Setting(Setting::Power),
    Tile::Setting(Setting::FpsCap),
    Tile::Setting(Setting::Refresh),
    Tile::Setting(Setting::Hud),
    Tile::Setting(Setting::Curve),
    Tile::Setting(Setting::Resolution),
    Tile::Setting(Setting::Scaling),
    Tile::Setting(Setting::AntiAliasing),
    Tile::Setting(Setting::Sharpen),
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
    if let Some(screen) = f.s.keymapping().screen() {
        return super::keymaps::draw(f, screen);
    }
    if let Some(screen) = f.s.curving().screen() {
        return super::curves::draw(f, screen);
    }
    if let Some(setting) = f.s.prefs().menu() {
        return menu(f, setting);
    }
    let s = f.s;
    let panel = s.panel();
    let context = panel.context();
    let entry = panel.editing();
    let shown = panel.scope_shown();
    let pad = f.pad();
    let mut grid = style::content(body);
    let bar = grid.take_top(f.body().h * TABS_H);

    // Tabs: All (the defaults), then the item's levels (its runner's and its own), with a note on the
    // right.
    let inner = pad * 0.8;
    let levels = panel.levels();
    let tabs = std::iter::once((Scope::Global, "All")).chain(levels.iter().flatten().map(|(scope, level)| (*scope, level.title)));
    let items = tabs.map(|(scope, name)| (Id::Settings(SettingsId::Scope(scope)), None, name, scope == shown));
    let tabs_area = Rect::new(bar.x - inner, bar.y, bar.w + inner, bar.h);
    let tabs_end = widgets::tabs(f, tabs_area, f.text(Text::Heading), inner, bar.w * TAB_MAX, items);
    let running = context.is_some_and(|g| s.session().program() == Some(&g.id));
    let note = match (entry.is_some().then_some(()), running) {
        (Some(_), true) => "applies now",
        (Some(_), false) => "applies at launch",
        (None, _) => "defaults",
    };
    let size = f.text(Text::Small);
    let note_w = f.c.text_width(size, note);
    if tabs_end + pad + note_w < bar.x + bar.w {
        f.c.text_in(bar, size, Tone::Dim, note, Align::End);
    }

    // The grid below, in rows that scroll, as the dock's do.
    let grid = Rect::new(grid.x, grid.y + pad * 0.6, grid.w, grid.h - pad * 0.6);
    let gap = f.body().h * style::TILE_GAP;
    let list = Scrolled::begin(f, ListId::Settings, grid, grid.h / ROWS as f32, panel.grid().count().div_ceil(GRID_COLUMNS));
    for row in list.shown() {
        let r = list.row(row);
        for (column, tile) in panel.grid().skip(row * GRID_COLUMNS).take(GRID_COLUMNS).filter_map(Tile::of).enumerate() {
            draw_tile(f, Rect::new(r.x, r.y, r.w, r.h - gap).cell(GRID_COLUMNS, 1, gap, column), tile);
        }
    }
    list.end(f, grid.x + grid.w + pad * 0.3);
}

/// How a tile looks: its icon and value, and what the value does, shown by the icon's colour. It also
/// shows where the value comes from: a dot marks the game's own choice, and on a game's tab a value that
/// follows the default is dimmed. A tile that does nothing is dimmed, with a note saying why: not yet, or
/// not for what is being edited.
struct Face {
    id: Id,
    icon: Icon,
    value: Fmt<32>,
    impact: Impact,
    origin: TileOrigin,
    active: bool,
    note: &'static str,
}

/// A tile's note when it does nothing for what is being edited.
fn note(no_effect: bool) -> &'static str {
    if no_effect { "no effect" } else { "" }
}

/// How `value` of `setting` reads, on its tile and in its menu: the keymap's and the curve's names, and
/// the resolution's size on the top screen once the compositor has said it.
fn value_text(s: &State, setting: Setting, value: u16) -> Fmt<32> {
    let resolution: clamshell_state::settings::Resolution = clamshell_state::settings::Choice::of(value);
    match setting {
        Setting::Resolution if resolution != clamshell_state::settings::Resolution::Native && s.top_size().is_some() => {
            let screen = s.top_size().unwrap_or_default();
            let (w, h) = resolution.size_on(screen);
            // A size the screen holds a whole number of times each way, which Integer fills.
            match (w > 0 && h > 0).then(|| (screen.0 / w, screen.1 / h)) {
                Some((x, y)) if x == y && x * w == screen.0 && y * h == screen.1 => Fmt::of(format_args!("{w}×{h} ({x}×)")),
                _ => Fmt::of(format_args!("{w}×{h}")),
            }
        }
        Setting::Keymap => Fmt::of(format_args!("{}", s.keymapping().name(value))),
        Setting::Curve => Fmt::of(format_args!("{}", s.curving().name(value))),
        _ => {
            let mut text = Fmt::new();
            setting.describe(value, &mut text);
            text
        }
    }
}

/// A tile's menu (a long press): its values, the one in force marked, and Reset to the level below when
/// the level edited chooses one itself (see [`State::menu_row`]).
fn menu(f: &mut Frame, setting: Setting) {
    let s = f.s;
    let panel = s.panel();
    let scope = panel.editing().map_or("all", |g| g.title);
    let title = Fmt::<128>::of(format_args!("{} for {scope}", setting.title()));
    let area = widgets::header(f, Some(Id::Settings(SettingsId::Back)), &title, false, widgets::HeaderRight::None);
    let list = widgets::ListArea::new(f, area, f.body().h * super::profiles::ROW_H);
    list.hint(f, "Tap to choose");
    let now = panel.tile_value(setting);
    let back_to = value_text(s, setting, panel.inherited_value(setting));
    let reset_note = match panel.below_name() {
        Some(below) => Fmt::<64>::of(format_args!("to {below}'s: {}", back_to.as_str())),
        None => Fmt::of(format_args!("to the default: {}", back_to.as_str())),
    };
    let rows = list.scrolled(f, ListId::Values, s.lists().len(ListId::Values));
    for i in rows.shown() {
        let Some(row) = s.menu_row(i) else {
            continue;
        };
        let text = row.map(|value| value_text(s, setting, value));
        let item = match (row, &text) {
            (Some(value), Some(text)) => {
                widgets::ListRow { label: text.as_str(), tone: Tone::high_if(value == now), mark: value == now, note: "", edit: widgets::EditCell::None }
            }
            _ => widgets::ListRow { label: "Reset", tone: Tone::Fn, mark: false, note: reset_note.as_str(), edit: widgets::EditCell::None },
        };
        widgets::list_row(f, ListId::Values.row(i as u16), rows.row(i), list.pad, list.size, item);
    }
    list.end(f, rows);
}

fn face(s: &State, tile: Tile) -> Face {
    let p = s.panel();
    let entry = p.editing().map(|g| g.id.clone());
    match tile {
        Tile::Setting(setting) => {
            let value = p.tile_value(setting);
            let text = value_text(s, setting, value);
            // At Integer the picture is nearest-neighbour, whatever Scaling says.
            let overruled = setting == Setting::Scaling && p.tile_value(Setting::Size) == clamshell_state::settings::Size::Integer as u16;
            Face {
                id: Id::Settings(SettingsId::Tile(setting)),
                icon: Icon::for_setting(setting),
                value: text,
                impact: setting.impact(value),
                origin: p.tile_origin(setting),
                active: (setting != Setting::Refresh || !s.device().display_modes().rates().is_empty())
                    && (setting != Setting::Fan || s.profile().fan.is_some())
                    && !(setting.through_launch() && p.handed_over())
                    && (!setting.compositor_only() || s.compositor()),
                // Only ClamShell's own compositor shows a game's picture differently; under sway these do
                // nothing.
                note: if setting == Setting::Resolution && s.compositor() && s.resolution_waits() {
                    "next start"
                } else {
                    note((setting.through_launch() && p.handed_over()) || (setting.compositor_only() && !s.compositor()))
                },
            }
        }
        Tile::Env => {
            let count = clamshell_state::settings::env::count_env(s.prefs().settings().env_text(entry.as_deref()));
            let impact = if count > 0 { Impact::Changes } else { Impact::Stock };
            let origin = if entry.is_some() && count > 0 { TileOrigin::Own } else { TileOrigin::Global };
            let value = Fmt::of(format_args!("{}", Variables(count)));
            let off = p.handed_over();
            Face { id: Id::Settings(SettingsId::Env), icon: Icon::Env, value, impact, origin, active: !off, note: note(off) }
        }
        Tile::Reset => {
            let any = entry.as_ref().is_some_and(|g| s.prefs().settings().has_own(g));
            let value = Fmt::of(format_args!("{}", if any { "Reset" } else { "Nothing to reset" }));
            Face { id: Id::Settings(SettingsId::Reset), icon: Icon::Reset, value, impact: Impact::Stock, origin: TileOrigin::Global, active: any, note: "" }
        }
    }
}

/// A global setting's tile outside the grid, as on the battery tab, drawn the same as in the grid.
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
    // No background, since the grid's alignment gives the structure. A press still lights it.
    widgets::tile(f, face.id.clone(), r, &TILE, tile_face, |f, r| {
        let color = match (face.active, face.impact) {
            (false, _) => INERT,
            (true, Impact::Stock) => Tone::Dim.rgb(),
            (true, Impact::Saves) => Tone::Go.rgb(),
            (true, Impact::Costs) => Tone::Accent.rgb(),
            (true, Impact::Changes) => Tone::Fn.rgb(),
        };
        f.c.icon(face.icon, TILE.square(r), color);
    });
}

/// The editor over the settings, for the environment or a keymap's name, using the shared editor (field,
/// Cancel and Save, keyboard).
fn editor(f: &mut Frame) {
    let s = f.s;
    let title = match s.bottom().editor() {
        Some(Editor { purpose: Purpose::Env { entry }, .. }) => {
            let title = match entry {
                Some(id) => s.entry(id).map_or("this game", |g| g.title.as_str()),
                None => "all games",
            };
            Fmt::<160>::of(format_args!("Environment for {title}: KEY=value, space-separated"))
        }
        Some(Editor { purpose: Purpose::ProfileName(kind, name), .. }) => match name {
            ProfileName::New => Fmt::of(format_args!("Name of the new {}", kind.noun())),
            ProfileName::Rename(id) => Fmt::of(format_args!("New name for {}", s.profile_name(*kind, *id))),
            ProfileName::Duplicate(id) => Fmt::of(format_args!("Name of the copy of {}", s.profile_name(*kind, *id))),
            ProfileName::SaveAs(_) => Fmt::of(format_args!("A name for the copy (other games can use it)")),
        },
        Some(Editor { purpose: Purpose::KeymapName(name), .. }) => match name {
            KeymapName::NewLayer(..) => Fmt::of(format_args!("Name of the new layer")),
            KeymapName::RenameLayer(id, layer) => {
                let name = s.keymapping().keymaps().get(*id).and_then(|k| k.layer_name(*layer)).unwrap_or("the layer");
                Fmt::of(format_args!("New name for {name}"))
            }
        },
        _ => return,
    };
    super::editor::draw(f, &title, "Save");
}
