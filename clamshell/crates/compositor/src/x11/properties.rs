//! X11 properties become ordinary window facts here. No X11 types reach State.
use clamshell_state::session::{
    presentation::{WindowLimits, WindowRole},
    windows::Window,
};
use x11rb_protocol::protocol::xproto::{AtomEnum, GetPropertyReply};

pub(super) const NAMES: &[&str] = &[
    "WM_S0",
    "_NET_WM_CM_S0",
    "WM_STATE",
    "WM_PROTOCOLS",
    "WM_DELETE_WINDOW",
    "WM_TAKE_FOCUS",
    "WM_NAME",
    "_NET_WM_NAME",
    "UTF8_STRING",
    "WM_CLASS",
    "WM_TRANSIENT_FOR",
    "WM_NORMAL_HINTS",
    "WM_SIZE_HINTS",
    "_NET_WM_WINDOW_TYPE",
    "_NET_WM_WINDOW_TYPE_DIALOG",
    "_NET_WM_WINDOW_TYPE_SPLASH",
    "_NET_WM_WINDOW_TYPE_UTILITY",
    "WL_SURFACE_ID",
    "_NET_SUPPORTED",
    "_NET_SUPPORTING_WM_CHECK",
    "_NET_ACTIVE_WINDOW",
    "_NET_WM_STATE",
    "_NET_WM_STATE_FULLSCREEN",
    "WM_HINTS",
    "_NET_WM_WINDOW_TYPE_TOOLTIP",
    "CLIPBOARD",
    "TARGETS",
    "TIMESTAMP",
    "INCR",
    "TEXT",
    "_THOR_SELECTION",
    "_THOR_TIMESTAMP",
    "_XWAYLAND_RANDR_EMU_MONITOR_RECTS",
];
pub(super) const PROPERTIES: &[&str] = &[
    "WM_NAME",
    "_NET_WM_NAME",
    "WM_CLASS",
    "WM_TRANSIENT_FOR",
    "WM_NORMAL_HINTS",
    "_NET_WM_WINDOW_TYPE",
    "WM_PROTOCOLS",
    "WM_HINTS",
    "_XWAYLAND_RANDR_EMU_MONITOR_RECTS",
];
#[derive(Default)]
pub(super) struct Atoms(pub Vec<(&'static str, u32)>);
impl Atoms {
    pub fn get(&self, name: &str) -> u32 {
        self.0.iter().find(|(n, _)| *n == name).map_or(0, |(_, value)| *value)
    }
}

#[derive(Default, PartialEq, Eq, Debug)]
pub(super) struct Properties {
    pub name: String,
    pub utf8_name: Option<String>,
    pub class: String,
    pub transient: Option<u32>,
    pub limits: WindowLimits,
    pub role: WindowRole,
    pub delete: bool,
    pub take_focus: bool,
    pub input: Option<bool>,
    pub tooltip: bool,
    /// The size of the mode its client switched to, which Xwayland emulates for a window of that size by
    /// scaling it to the screen (the first of `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`, x, y, width, height).
    pub emulated: Option<(u32, u32)>,
}
impl Properties {
    pub fn update(&mut self, name: &str, reply: &GetPropertyReply, atoms: &Atoms) {
        let absent = reply.type_ == 0;
        let text = |kind: u32| if reply.format == 8 && reply.type_ == kind { Some(reply.value.as_slice()) } else { None };
        let words = |kind: u32| -> Vec<u32> {
            if reply.format != 32 || reply.type_ != kind {
                return Vec::new();
            }
            reply.value.chunks_exact(4).filter_map(|bytes| bytes.try_into().ok().map(u32::from_ne_bytes)).collect()
        };
        match name {
            "WM_NAME" => self.name = text(AtomEnum::STRING.into()).map(|bytes| bytes.iter().map(|b| char::from(*b)).collect()).unwrap_or_default(),
            "_NET_WM_NAME" => {
                self.utf8_name = text(atoms.get("UTF8_STRING")).and_then(|bytes| std::str::from_utf8(bytes).ok()).map(|s| s.trim_end_matches('\0').to_owned())
            }
            "WM_CLASS" => {
                self.class = text(AtomEnum::STRING.into())
                    .and_then(|bytes| bytes.split(|b| *b == 0).nth(1))
                    .map(|bytes| bytes.iter().map(|b| char::from(*b)).collect())
                    .unwrap_or_default()
            }
            "WM_TRANSIENT_FOR" => self.transient = words(AtomEnum::WINDOW.into()).first().copied().filter(|v| *v != 0),
            "WM_NORMAL_HINTS" => {
                let values = words(atoms.get("WM_SIZE_HINTS"));
                let flags = values.first().copied().unwrap_or_default();
                let size = |flag, offset| {
                    if flags & flag != 0 {
                        (
                            values.get(offset).copied().filter(|v| *v <= i32::MAX as u32).unwrap_or_default(),
                            values.get(offset + 1).copied().filter(|v| *v <= i32::MAX as u32).unwrap_or_default(),
                        )
                    } else {
                        (0, 0)
                    }
                };
                self.limits = WindowLimits { min: size(1 << 4, 5), max: size(1 << 5, 7) };
            }
            "_NET_WM_WINDOW_TYPE" => {
                let types = words(AtomEnum::ATOM.into());
                self.tooltip = types.contains(&atoms.get("_NET_WM_WINDOW_TYPE_TOOLTIP"));
                self.role = types
                    .into_iter()
                    .find_map(|atom| {
                        if atom == atoms.get("_NET_WM_WINDOW_TYPE_DIALOG") {
                            Some(WindowRole::Dialog)
                        } else if atom == atoms.get("_NET_WM_WINDOW_TYPE_SPLASH") {
                            Some(WindowRole::Splash)
                        } else if atom == atoms.get("_NET_WM_WINDOW_TYPE_UTILITY") {
                            Some(WindowRole::Utility)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
            }
            "_XWAYLAND_RANDR_EMU_MONITOR_RECTS" => {
                self.emulated = match words(AtomEnum::CARDINAL.into()).as_slice() {
                    [_, _, w, h, ..] if *w > 0 && *h > 0 => Some((*w, *h)),
                    _ => None,
                };
            }
            "WM_HINTS" => {
                let hints = words(AtomEnum::WM_HINTS.into());
                self.input = hints.first().filter(|flags| **flags & 1 != 0).and_then(|_| hints.get(1)).map(|v| *v != 0);
            }
            "WM_PROTOCOLS" => {
                let protocols = words(AtomEnum::ATOM.into());
                self.delete = !absent && protocols.contains(&atoms.get("WM_DELETE_WINDOW"));
                self.take_focus = !absent && protocols.contains(&atoms.get("WM_TAKE_FOCUS"));
            }
            _ => {}
        }
    }
    pub fn facts(&self, id: i64, parent: Option<i64>, process: crate::Process) -> Window {
        Window {
            id,
            app_id: self.class.clone(),
            title: self.utf8_name.as_ref().unwrap_or(&self.name).clone(),
            parent,
            limits: self.limits,
            role: if self.role == WindowRole::Normal && self.transient.is_some() { WindowRole::Dialog } else { self.role },
            unit: process.unit,
            pids: process.pids,
            ..Window::default()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;
    #[derive(serde::Deserialize)]
    struct Field {
        name: String,
        #[serde(rename = "type")]
        type_: String,
        format: u8,
        bytes: Vec<u8>,
        words: Vec<u32>,
    }
    #[derive(serde::Deserialize)]
    struct Fixture {
        label: String,
        properties: Vec<Field>,
        app: String,
        title: String,
        role: WindowRole,
        limits: WindowLimits,
        parent: Option<u32>,
    }
    #[test]
    fn property_fixtures_translate_to_the_same_protocol_neutral_facts() {
        let atoms = Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect());
        let fixtures: Vec<Fixture> = serde_json::from_str(include_str!("../../fixtures/x11-properties.json")).unwrap();
        for fixture in fixtures {
            let mut properties = Properties::default();
            for field in fixture.properties {
                let type_ = match field.type_.as_str() {
                    "STRING" => AtomEnum::STRING.into(),
                    "WINDOW" => AtomEnum::WINDOW.into(),
                    "ATOM" => AtomEnum::ATOM.into(),
                    "NONE" => 0,
                    name => atoms.get(name),
                };
                let value = if field.format == 32 { field.words.iter().flat_map(|v| v.to_ne_bytes()).collect() } else { field.bytes };
                properties.update(&field.name, &GetPropertyReply { format: field.format, type_, value, ..GetPropertyReply::default() }, &atoms);
            }
            assert_eq!(properties.transient, fixture.parent, "{}", fixture.label);
            let facts = properties.facts(5, fixture.parent.map(|_| 9), crate::Process { unit: Some("game.scope".into()), pids: vec![123], steam_app: None });
            assert_eq!(facts.app_id, fixture.app, "{}", fixture.label);
            assert_eq!(facts.title, fixture.title, "{}", fixture.label);
            assert_eq!(facts.role, fixture.role, "{}", fixture.label);
            assert_eq!(facts.limits, fixture.limits, "{}", fixture.label);
            assert_eq!(facts.parent, fixture.parent.map(|_| 9));
            assert_eq!(facts.pids, vec![123]);
            assert_eq!(facts.unit.as_deref(), Some("game.scope"));
            assert_eq!(facts.colour, None, "X11 metadata makes no claim about colour or buffer precision");
        }
    }
    #[test]
    fn focus_protocols_and_input_hints_are_validated_and_removed() {
        let atoms = Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect());
        let mut properties = Properties::default();
        let words = |kind, values: &[u32]| GetPropertyReply {
            format: 32,
            type_: kind,
            value: values.iter().flat_map(|v| v.to_ne_bytes()).collect(),
            ..GetPropertyReply::default()
        };
        properties.update("WM_PROTOCOLS", &words(AtomEnum::ATOM.into(), &[atoms.get("WM_DELETE_WINDOW"), atoms.get("WM_TAKE_FOCUS")]), &atoms);
        assert!(properties.delete && properties.take_focus);
        properties.update("WM_HINTS", &words(AtomEnum::WM_HINTS.into(), &[1, 0]), &atoms);
        assert_eq!(properties.input, Some(false));
        properties.update("WM_HINTS", &words(AtomEnum::WM_HINTS.into(), &[1, 1]), &atoms);
        assert_eq!(properties.input, Some(true));
        properties.update("WM_HINTS", &GetPropertyReply::default(), &atoms);
        properties.update("WM_PROTOCOLS", &GetPropertyReply::default(), &atoms);
        assert_eq!(properties.input, None);
        assert!(!properties.delete && !properties.take_focus);
    }
}
