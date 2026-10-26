//! Bounded window diagnostics. Recordings replay against a matching state fixture.

use std::collections::VecDeque;
use std::path::PathBuf;

use clamshell_state::{Event, State, WindowEvent, device::buttons::DeviceKey};
use serde::{Deserialize, Serialize};

const CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Input {
    Window(WindowEvent),
    Batch(Vec<WindowEvent>),
    Home,
    HomeTap,
    HomeHeld(bool),
    PadActive,
    Device(DeviceKey),
}
impl Input {
    fn capture(event: &Event, state: &State) -> Option<Self> {
        Some(match event {
            Event::Window(window) => Self::Window(window.clone()),
            Event::WindowBatch(windows) => Self::Batch(windows.clone()),
            Event::Home => Self::Home,
            Event::HomeTap => Self::HomeTap,
            Event::HomeHeld(held) => Self::HomeHeld(*held),
            Event::PadActive if state.session().top_window().is_some_and(|w| Some(w.id) != state.session().keyboard_focus()) => Self::PadActive,
            Event::Device(
                key @ (DeviceKey::WindowDown
                | DeviceKey::WindowUp
                | DeviceKey::WindowPrevious
                | DeviceKey::WindowNext
                | DeviceKey::CloseWindow
                | DeviceKey::Quit
                | DeviceKey::BottomView),
            ) => Self::Device(*key),
            _ => return None,
        })
    }
    pub fn into_event(self) -> Event {
        match self {
            Self::Window(window) => window.into(),
            Self::Batch(windows) => Event::WindowBatch(windows),
            Self::Home => Event::Home,
            Self::HomeTap => Event::HomeTap,
            Self::HomeHeld(held) => Event::HomeHeld(held),
            Self::PadActive => Event::PadActive,
            Self::Device(key) => Event::Device(key),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub top: Option<i64>,
    pub bottom: Option<i64>,
    pub keyboard: Option<i64>,
    pub windows: usize,
}
impl Selection {
    pub fn of(state: &State) -> Self {
        Self {
            top: state.session().top_window().map(|w| w.id),
            bottom: state.session().bottom_window().map(|w| w.id),
            keyboard: state.session().keyboard_focus(),
            windows: state.session().windows().len(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub sequence: u64,
    pub unix_secs: u64,
    pub mono_secs: u64,
    pub before: Selection,
    pub input: Input,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub records: VecDeque<Record>,
    pub after: Selection,
    /// The windows as the state has them at the end: their groups, which screen, which are shown and
    /// focused (what the VM's checks read).
    pub windows: Vec<clamshell_state::session::windows::Window>,
}
impl Recording {
    /// Replay with the recording's library, rules and launch context supplied by the fixture.
    /// A truncated ring needs a fixture at its first retained event.
    pub fn replay(&self, state: &mut State) -> Result<(), String> {
        let mut effects = Vec::new();
        for record in &self.records {
            if state.clock().unix() != record.unix_secs || state.clock().mono() != record.mono_secs {
                state.apply(Event::Tick { unix_secs: record.unix_secs, mono_secs: record.mono_secs }, &mut effects);
                effects.clear();
            }
            let actual = Selection::of(state);
            if actual != record.before {
                return Err(format!("event {}: expected {:?}, got {actual:?}", record.sequence, record.before));
            }
            state.apply(record.input.clone().into_event(), &mut effects);
            effects.clear();
        }
        let actual = Selection::of(state);
        if actual != self.after {
            return Err(format!("after replay: expected {:?}, got {actual:?}", self.after));
        }
        Ok(())
    }
}

pub(crate) struct Ring {
    records: VecDeque<Record>,
    sequence: u64,
}
impl Ring {
    pub fn new() -> Self {
        Self { records: VecDeque::with_capacity(CAPACITY), sequence: 0 }
    }
    pub fn record(&mut self, event: &Event, state: &State) {
        let Some(input) = Input::capture(event, state) else { return };
        if self.records.len() == CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(Record {
            sequence: self.sequence,
            unix_secs: state.clock().unix(),
            mono_secs: state.clock().mono(),
            before: Selection::of(state),
            input,
        });
        self.sequence = self.sequence.wrapping_add(1);
    }
    pub fn snapshot(&self, state: &State) -> Recording {
        Recording { records: self.records.clone(), after: Selection::of(state), windows: state.session().windows().to_vec() }
    }
}

pub(crate) fn save(recording: Recording, path: PathBuf) {
    let spawned = std::thread::Builder::new().name("window trace".into()).spawn(move || match write_recording(&recording, &path) {
        Ok(()) => log::info!("window events: {}", path.display()),
        Err(error) => log::warn!("window events: {}: {error}", path.display()),
    });
    if let Err(error) = spawned {
        log::warn!("window events worker: {error}");
    }
}

fn write_recording(recording: &Recording, path: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(path).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, recording).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clamshell_state::{Init, session::windows::WindowFacts};
    use clamshell_ui::Core;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn recorded_window_batches_and_chords_replay_through_state() {
        let ring = Rc::new(RefCell::new(Ring::new()));
        let mut core = Core::new(State::new(Init::default()), Default::default(), || {}).unwrap();
        let observed = ring.clone();
        core.observe_events(move |event, state| observed.borrow_mut().record(event, state));
        core.handle(Event::Start);
        core.handle(Event::Tick { unix_secs: 1000, mono_secs: 10 });
        let window = WindowFacts { id: 1, app_id: "trace-test".into(), ..WindowFacts::default() };
        core.handle(Event::WindowBatch(vec![WindowEvent::Opened(window), WindowEvent::Title { window: 1, title: "recorded".into() }]));
        core.handle(Event::Device(DeviceKey::WindowDown));
        core.handle(Event::Device(DeviceKey::WindowUp));
        core.handle(Event::Home);
        core.handle(WindowEvent::Activate(1).into());
        let recording = ring.borrow().snapshot(core.state());
        assert_eq!(recording.records.len(), 5, "an atomic batch occupies one record");
        assert!(matches!(recording.records.front().map(|r| &r.input), Some(Input::Batch(events)) if events.len() == 2));
        let json = serde_json::to_vec(&recording).unwrap();
        let decoded: Recording = serde_json::from_slice(&json).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("events.json");
        write_recording(&decoded, &file).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        let saved: Recording = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved.records.len(), decoded.records.len());
        assert!(write_recording(&decoded, &directory.path().join("missing/events.json")).is_err());
        let mut mismatch = State::new(Init::default());
        mismatch.apply(WindowEvent::Opened(WindowFacts { id: 9, app_id: "other".into(), ..WindowFacts::default() }).into(), &mut Vec::new());
        assert!(decoded.replay(&mut mismatch).is_err());
        let mut replay = State::new(Init::default());
        replay.apply(Event::Start, &mut Vec::new());
        decoded.replay(&mut replay).unwrap();
        assert_eq!(replay.session().windows(), core.state().session().windows());
        assert_eq!(replay.session().window(1).map(|w| w.title.as_str()), Some("recorded"));
        assert_eq!(replay.session().keyboard_focus(), Some(1));
    }
    #[test]
    fn the_ring_keeps_only_the_last_256_window_inputs() {
        let state = State::new(Init::default());
        let mut ring = Ring::new();
        for id in 0..300 {
            ring.record(&WindowEvent::Closed(id).into(), &state);
        }
        let capacity = ring.records.capacity();
        assert_eq!(ring.records.len(), CAPACITY);
        assert_eq!(ring.records.front().map(|r| r.sequence), Some(44));
        assert_eq!(ring.records.back().map(|r| r.sequence), Some(299));
        for _ in 0..1000 {
            ring.record(&Event::Idle(clamshell_state::device::idle::IdleEvent::DeviceInput), &state);
        }
        assert_eq!(ring.records.len(), CAPACITY);
        assert_eq!(ring.records.capacity(), capacity);
        let mut fixture = State::new(Init::default());
        ring.snapshot(&state).replay(&mut fixture).unwrap();
    }
}
