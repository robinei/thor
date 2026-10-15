//! What the volume and brightness tabs show, read every second while one is open: PipeWire's outputs
//! and the apps playing (`pw-dump`), and the backlights' names. Setting them is the controls thread's.

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use clamshell_core::Event;
use clamshell_core::mixer::{Audio, Output, Stream};
use serde_json::Value;

use crate::Sink;
use crate::controls;

/// What to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Audio(bool),
    Backlights(bool),
}

const READ_EVERY: Duration = Duration::from_secs(1);

pub fn spawn(sink: impl Sink, commands: Receiver<Command>) -> io::Result<()> {
    thread::Builder::new().name("mixer".into()).spawn(move || {
        let (mut audio, mut backlights) = (false, false);
        let (mut last_audio, mut last_backlights) = (None, None);
        loop {
            if audio {
                let read = read_audio();
                if last_audio.as_ref() != Some(&read) {
                    if !sink.send(Event::Audio(read.clone())) {
                        return;
                    }
                    last_audio = Some(read);
                }
            }
            if backlights {
                let read = controls::backlight_names();
                if last_backlights.as_ref() != Some(&read) {
                    if !sink.send(Event::Backlights(read.clone())) {
                        return;
                    }
                    last_backlights = Some(read);
                }
            }
            let wait = if audio || backlights { READ_EVERY } else { Duration::from_secs(3600) };
            match commands.recv_timeout(wait) {
                // Read again when opened: what changed meanwhile goes through.
                Ok(Command::Audio(on)) => (audio, last_audio) = (on, None),
                Ok(Command::Backlights(on)) => (backlights, last_backlights) = (on, None),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    })?;
    Ok(())
}

fn read_audio() -> Audio {
    match std::process::Command::new("pw-dump").output() {
        Ok(out) if out.status.success() => serde_json::from_slice(&out.stdout).map(|v| parse(&v)).unwrap_or_default(),
        Ok(out) => {
            log::debug!("pw-dump: {}", out.status);
            Audio::default()
        }
        Err(e) => {
            log::debug!("pw-dump: {e}");
            Audio::default()
        }
    }
}

static NULL: Value = Value::Null;

/// The value at a JSON pointer (`/a/b`), or null.
fn at<'v>(v: &'v Value, pointer: &str) -> &'v Value {
    v.pointer(pointer).unwrap_or(&NULL)
}

/// A volume as wpctl shows it: the cube root of PipeWire's linear channel volumes (their average).
fn percent(volumes: &[Value]) -> u8 {
    let values: Vec<f64> = volumes.iter().filter_map(Value::as_f64).collect();
    if values.is_empty() {
        return 0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (mean.cbrt() * 100.0).round().clamp(0.0, 100.0) as u8
}

/// The outputs (`Audio/Sink`) and the apps' streams (`Stream/Output/Audio`) in pw-dump's output, with
/// the default sink from the "default" metadata.
fn parse(dump: &Value) -> Audio {
    let objects = dump.as_array().map(Vec::as_slice).unwrap_or_default();
    let default_sink = objects
        .iter()
        .filter(|o| at(o, "/type") == "PipeWire:Interface:Metadata" && at(o, "/props/metadata.name") == "default")
        .flat_map(|o| at(o, "/metadata").as_array().map(Vec::as_slice).unwrap_or_default())
        .find(|m| at(m, "/key") == "default.audio.sink")
        .and_then(|m| at(m, "/value/name").as_str());
    let mut audio = Audio::default();
    for o in objects.iter().filter(|o| at(o, "/type") == "PipeWire:Interface:Node") {
        let (Some(id), props) = (at(o, "/id").as_u64().and_then(|i| u32::try_from(i).ok()), &at(o, "/info/props")) else {
            continue;
        };
        let text = |key: &str| props.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        // The Props entry with the volumes (there may be several).
        let params = at(o, "/info/params/Props").as_array().map(Vec::as_slice).unwrap_or_default();
        let volumes = params.iter().find_map(|p| at(p, "/channelVolumes").as_array());
        let muted = params.iter().find_map(|p| at(p, "/mute").as_bool()).unwrap_or(false);
        match at(props, "/media.class").as_str() {
            Some("Audio/Sink") => {
                let name = Some(text("node.description")).filter(|n| !n.is_empty()).unwrap_or_else(|| text("node.name"));
                let default = default_sink.is_some_and(|d| at(props, "/node.name").as_str() == Some(d));
                audio.outputs.push(Output { id, name, default });
            }
            Some("Stream/Output/Audio") => {
                let app = [text("application.name"), text("application.process.binary"), text("node.name")]
                    .into_iter()
                    .find(|n| !n.is_empty())
                    .unwrap_or_default();
                let volume = volumes.map_or(100, |v| percent(v));
                audio.streams.push(Stream { id, app, title: text("media.name"), volume, muted });
            }
            _ => {}
        }
    }
    audio.outputs.sort_by_key(|o| o.id);
    audio.streams.sort_by_key(|s| s.id);
    audio
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_outputs_streams_and_the_default() {
        let dump = serde_json::json!([
            {"id": 30, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
             "metadata": [{"subject": 0, "key": "default.audio.sink", "value": {"name": "alsa_output.hdmi"}}]},
            {"id": 58, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.speaker", "node.description": "Speakers"},
             "params": {"Props": [{"channelVolumes": [0.125, 0.125], "mute": false}]}}},
            {"id": 59, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.hdmi"}, "params": {}}},
            {"id": 90, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Stream/Output/Audio", "application.name": "Firefox", "media.name": "A video"},
             "params": {"Props": [{"volume": 1.0}, {"channelVolumes": [0.216, 0.216], "mute": true}]}}},
            {"id": 91, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Stream/Input/Audio", "application.name": "Recorder"}}}
        ]);
        let audio = parse(&dump);
        assert_eq!(
            audio.outputs,
            vec![Output { id: 58, name: "Speakers".into(), default: false }, Output { id: 59, name: "alsa_output.hdmi".into(), default: true }]
        );
        assert_eq!(audio.streams, vec![Stream { id: 90, app: "Firefox".into(), title: "A video".into(), volume: 60, muted: true }]);
    }

    /// With PipeWire running (a desktop): its view reads (nothing is changed).
    #[test]
    fn reads_pipewire() {
        let audio = read_audio();
        assert!(audio.outputs.iter().filter(|o| o.default).count() <= 1);
    }
}

#[cfg(test)]
mod live {
    /// Prints what the volume tab would show (run by hand: `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn print_audio() {
        println!("{:#?}", super::read_audio());
    }
}
