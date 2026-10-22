//! What the volume and brightness tabs show, read every second while one is open: PipeWire's outputs
//! and the apps playing (`pw-dump`), and the backlights' names. Setting them is the sound and brightness
//! threads'.

use std::io;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use clamshell_core::levels::{Audio, AudioOutput, Stream};
use clamshell_core::{Event, Reading};
use serde_json::Value;

use crate::brightness::Backlights;
use crate::worker::{self, Changed, Next, Worker};
use crate::{Closed, Sink};

/// What to read: each while its tab is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Audio(bool),
    Backlights(bool),
}

const READ_EVERY: Duration = Duration::from_secs(1);

pub fn spawn(backlights: Backlights, sink: impl Sink) -> io::Result<Worker<Command>> {
    Worker::spawn("mixer", move |commands| {
        let _ = follow(&backlights, &commands, &sink);
    })
}

/// Reads what is watched every second and sends it when it changed, until the commands or the UI go.
fn follow(backlights: &Backlights, commands: &Receiver<Command>, sink: &impl Sink) -> Result<(), Closed> {
    let (mut audio, mut lights) = (false, false);
    let (mut last_audio, mut last_lights) = (Changed::default(), Changed::default());
    loop {
        if audio && let Some(read) = last_audio.offer(read_audio()) {
            sink.send(Event::Reading(Reading::Audio(read)))?;
        }
        if lights && let Some(read) = last_lights.offer(backlights.names()) {
            sink.send(Event::Reading(Reading::Backlights(read)))?;
        }
        match worker::next(commands, (audio || lights).then_some(READ_EVERY)) {
            // Read again when opened: what changed meanwhile goes through.
            Next::Command(Command::Audio(on)) => {
                audio = on;
                last_audio.forget();
            }
            Next::Command(Command::Backlights(on)) => {
                lights = on;
                last_lights.forget();
            }
            Next::Elapsed => {}
            Next::Disconnected => return Ok(()),
        }
    }
}

fn read_audio() -> Audio {
    crate::command::output("pw-dump", &[]).and_then(|out| serde_json::from_slice(&out).ok()).map(|v| parse(&v)).unwrap_or_default()
}

/// `wpctl get-volume` output, `Volume: 0.60` or `Volume: 0.60 [MUTED]`: the volume 0-100 and mute.
pub fn parse_get_volume(text: &str) -> Option<(u8, bool)> {
    let rest = text.trim().strip_prefix("Volume:")?;
    let volume: f32 = rest.split_whitespace().next()?.parse().ok()?;
    Some(((volume * 100.0).round().clamp(0.0, 100.0) as u8, rest.contains("[MUTED]")))
}

static NULL: Value = Value::Null;

/// The value at a JSON pointer (`/a/b`), or null.
fn at<'v>(v: &'v Value, pointer: &str) -> &'v Value {
    v.pointer(pointer).unwrap_or(&NULL)
}

/// A volume as wpctl shows it (as `parse_get_volume` reads it): the cube root of PipeWire's linear
/// channel volumes (their average).
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
                audio.outputs.push(AudioOutput { id, name, default });
            }
            Some("Stream/Output/Audio") => {
                let app =
                    [text("application.name"), text("application.process.binary"), text("node.name")].into_iter().find(|n| !n.is_empty()).unwrap_or_default();
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
            vec![AudioOutput { id: 58, name: "Speakers".into(), default: false }, AudioOutput { id: 59, name: "alsa_output.hdmi".into(), default: true }]
        );
        assert_eq!(audio.streams, vec![Stream { id: 90, app: "Firefox".into(), title: "A video".into(), volume: 60, muted: true }]);
    }

    #[test]
    fn wpctl_volume_and_mute() {
        assert_eq!(parse_get_volume("Volume: 0.60\n"), Some((60, false)));
        assert_eq!(parse_get_volume("Volume: 0.35 [MUTED]\n"), Some((35, true)));
        assert_eq!(parse_get_volume("garbage"), None);
    }
}

#[cfg(test)]
mod live {
    /// With PipeWire running (a desktop): prints what the volume tab would show; nothing is changed (run
    /// by hand: `--ignored --nocapture`).
    #[test]
    #[ignore = "needs PipeWire"]
    fn reads_pipewire() {
        let audio = super::read_audio();
        println!("{audio:#?}");
        assert!(audio.outputs.iter().filter(|o| o.default).count() <= 1);
    }
}
