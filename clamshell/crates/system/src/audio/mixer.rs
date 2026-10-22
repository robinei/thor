//! What the volume and brightness tabs show, read every second while one is open: PipeWire's outputs and
//! the apps playing, from `pw-dump`, and the backlights' names. Setting them is done by the sound and
//! brightness threads.

use std::io;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use clamshell_core::levels::{Audio, AudioOutput, Stream};
use clamshell_core::{Event, Reading};
use serde_json::Value;

use crate::display::brightness::Backlights;
use crate::os::worker::{self, Changed, Next, Worker};
use crate::{Closed, Sink};

/// What to read. Each is read while its tab is open.
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

/// Reads what is watched every second and sends it when it changed, until the command channel or the UI
/// is gone.
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
            // Send a fresh reading when a tab opens, so changes made meanwhile go through.
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

/// The PipeWire graph, as `pw-dump` prints it. It is read every few seconds, so a failure is logged at
/// debug; a missing `pw-dump` is reported at start with the other tools.
fn dump() -> Option<Value> {
    use crate::os::command;
    let ran = command::run("pw-dump", &[] as &[&str], None, command::LIMIT).inspect_err(|e| log::debug!("{e}")).ok()?;
    if !ran.status.success() {
        log::debug!("pw-dump: {}", ran.status);
        return None;
    }
    serde_json::from_slice(&ran.stdout).inspect_err(|e| log::debug!("pw-dump: {e}")).ok()
}

fn read_audio() -> Audio {
    dump().map(|v| parse(&v)).unwrap_or_default()
}

/// The default output's volume (0-100) and whether it is muted.
pub fn read_volume() -> Option<(u8, bool)> {
    dump().as_ref().and_then(default_volume)
}

static NULL: Value = Value::Null;

/// The value at a JSON pointer (`/a/b`), or null.
fn at<'v>(v: &'v Value, pointer: &str) -> &'v Value {
    v.pointer(pointer).unwrap_or(&NULL)
}

/// A volume as wpctl shows it: the cube root of the average of PipeWire's linear channel volumes.
fn percent(volumes: &[Value]) -> u8 {
    let values: Vec<f64> = volumes.iter().filter_map(Value::as_f64).collect();
    if values.is_empty() {
        return 0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (mean.cbrt() * 100.0).round().clamp(0.0, 100.0) as u8
}

/// The name of the default output, from the "default" metadata.
fn default_sink(objects: &[Value]) -> Option<&str> {
    objects
        .iter()
        .filter(|o| at(o, "/type") == "PipeWire:Interface:Metadata" && at(o, "/props/metadata.name") == "default")
        .flat_map(|o| at(o, "/metadata").as_array().map(Vec::as_slice).unwrap_or_default())
        .find(|m| at(m, "/key") == "default.audio.sink")
        .and_then(|m| at(m, "/value/name").as_str())
}

/// A node's volume and mute, from the Props entries that have them. There may be several entries.
fn volume_of(node: &Value) -> (Option<u8>, bool) {
    let params = at(node, "/info/params/Props").as_array().map(Vec::as_slice).unwrap_or_default();
    let volume = params.iter().find_map(|p| at(p, "/channelVolumes").as_array()).map(|v| percent(v));
    (volume, params.iter().find_map(|p| at(p, "/mute").as_bool()).unwrap_or(false))
}

/// The default output's volume and mute in pw-dump's output.
fn default_volume(dump: &Value) -> Option<(u8, bool)> {
    let objects = dump.as_array()?;
    let name = default_sink(objects)?;
    let node = objects.iter().find(|o| at(o, "/type") == "PipeWire:Interface:Node" && at(o, "/info/props/node.name").as_str() == Some(name))?;
    let (volume, muted) = volume_of(node);
    Some((volume?, muted))
}

/// The outputs (`Audio/Sink`) and the apps' streams (`Stream/Output/Audio`) in pw-dump's output, with
/// the default sink from the "default" metadata.
fn parse(dump: &Value) -> Audio {
    let objects = dump.as_array().map(Vec::as_slice).unwrap_or_default();
    let default_sink = default_sink(objects);
    let mut audio = Audio::default();
    for o in objects.iter().filter(|o| at(o, "/type") == "PipeWire:Interface:Node") {
        let (Some(id), props) = (at(o, "/id").as_u64().and_then(|i| u32::try_from(i).ok()), &at(o, "/info/props")) else {
            continue;
        };
        let text = |key: &str| props.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        let (volume, muted) = volume_of(o);
        match at(props, "/media.class").as_str() {
            Some("Audio/Sink") => {
                let name = Some(text("node.description")).filter(|n| !n.is_empty()).unwrap_or_else(|| text("node.name"));
                let default = default_sink.is_some_and(|d| at(props, "/node.name").as_str() == Some(d));
                audio.outputs.push(AudioOutput { id, name, default });
            }
            Some("Stream/Output/Audio") => {
                let app =
                    [text("application.name"), text("application.process.binary"), text("node.name")].into_iter().find(|n| !n.is_empty()).unwrap_or_default();
                let volume = volume.unwrap_or(100);
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
    fn the_default_outputs_volume() {
        let dump = serde_json::json!([
            {"id": 30, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
             "metadata": [{"subject": 0, "key": "default.audio.sink", "value": {"name": "alsa_output.speaker"}}]},
            {"id": 58, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.speaker"},
             "params": {"Props": [{"volume": 1.0}, {"channelVolumes": [0.216, 0.216], "mute": true}]}}}
        ]);
        assert_eq!(default_volume(&dump), Some((60, true)));
        assert_eq!(default_volume(&serde_json::json!([])), None);
    }
}

#[cfg(test)]
mod live {
    /// With PipeWire running, as on a desktop, prints what the volume tab would show. Nothing is changed.
    /// Run it by hand with `--ignored --nocapture`.
    #[test]
    #[ignore = "needs PipeWire"]
    fn reads_pipewire() {
        let audio = super::read_audio();
        println!("{audio:#?}");
        assert!(audio.outputs.iter().filter(|o| o.default).count() <= 1);
        println!("{:?}", super::read_volume());
    }
}
