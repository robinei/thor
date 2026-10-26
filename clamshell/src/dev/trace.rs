//! One-line summaries of events and effects, for the snapshot scenario's behaviour trace
//! (tests/trace.txt). A summary names the kind and what identifies it, such as the entry launched; it
//! leaves out payloads such as settings, environments and readings, which other tests cover.

use clamshell_state::shelf::files::BrowseCommand;
use clamshell_state::{Effect, Event, Save, WindowEvent};

/// The longest Debug text kept whole. A longer one is cut to its kind.
const SHORT: usize = 72;

/// `value`'s Debug text if it is short, else the kind it starts with (the text before its first bracket
/// or brace).
fn short(value: &impl std::fmt::Debug) -> String {
    let text = format!("{value:?}");
    if text.len() <= SHORT {
        return text;
    }
    let kind = text.split(['(', ' ', '{']).next().unwrap_or_default();
    format!("{kind}(..)")
}

/// What `event` is, in one line.
pub fn event(event: &Event) -> String {
    match event {
        Event::Tick { unix_secs, .. } => format!("Tick {unix_secs}"),
        Event::Reading(r) => format!("Reading {}", short(r).split('(').next().unwrap_or_default()),
        Event::Units(units) => format!("Units [{}]", units.iter().map(|u| u.key.as_str()).collect::<Vec<_>>().join(", ")),
        Event::Window(change) => window_change(change),
        Event::WindowBatch(changes) => format!("Windows [{}]", changes.iter().map(window_change).collect::<Vec<_>>().join(", ")),
        Event::Library(_) => "Library".into(),
        Event::Meta { id, .. } => format!("Meta {id}"),
        other => short(other),
    }
}

fn window_change(change: &WindowEvent) -> String {
    match change {
        WindowEvent::Opened(w) => format!("open {}", w.app_id),
        WindowEvent::Closed(id) => format!("close {id}"),
        WindowEvent::Title { window, .. } => format!("title {window}"),
        WindowEvent::Changed(w) => format!("changed {}", w.app_id),
        WindowEvent::Activate(id) => format!("activate {id}"),
        WindowEvent::KeyboardFocus(id) => format!("keyboard focus {id:?}"),
    }
}

/// What `effect` asks for, in one line.
pub fn effect(effect: &Effect) -> String {
    match effect {
        Effect::Launch { entry, instance, .. } => format!("Launch {entry} as {}", instance.key),
        Effect::Apply { entry, .. } => format!("Apply settings of {}", entry.as_deref().unwrap_or("all")),
        Effect::Keymap(k) => format!("Keymap {} {:?}", k.id, k.name),
        Effect::Save(save) => format!("Save {}", self::save(save)),
        Effect::Touchpad(contacts) => format!("Touchpad {} contacts", contacts.len()),
        Effect::Brightness { level, shares } => format!("Brightness {level} over {} shares", shares.len()),
        Effect::Watch(watch, on) => format!("Watch {watch:?} {}", if *on { "on" } else { "off" }),
        Effect::PadRoute(route) => format!("PadRoute {route:?}"),
        Effect::FetchFirst(ids) => format!("FetchFirst {} entries, {} first", ids.len(), ids.first().map_or("none", |id| id)),
        Effect::ReadPosition { entry, .. } => format!("ReadPosition {entry}"),
        Effect::Browse(BrowseCommand::Open(Some(browse))) => format!("Browse open {}", browse.folder),
        Effect::Browse(BrowseCommand::Open(None)) => "Browse close".into(),
        Effect::Browse(BrowseCommand::InUse(files)) => format!("Browse in use {} files", files.len()),
        Effect::RecordPlay { entry, start, duration } => format!("RecordPlay {entry} from {start} for {duration}s"),
        Effect::Notify { summary, urgent, .. } => format!("Notify {summary:?}{}", if *urgent { " urgent" } else { "" }),
        other => short(other),
    }
}

fn save(save: &Save) -> String {
    match save {
        Save::Settings(_) => "Settings".into(),
        Save::Keymaps(_) => "Keymaps".into(),
        Save::Resume(_) => "Resume".into(),
        Save::Brightness(_) => "Brightness".into(),
        Save::WindowRules(_) => "WindowRules".into(),
        Save::Sticks(_) => "Sticks".into(),
        other => short(other),
    }
}
