//! Setting the sound from the bar's volume slider and the volume tab: the output's volume and mute, an
//! app's volume and mute, and which output plays, through PipeWire's `wpctl`. It runs on a thread of its
//! own. While a finger slides, requests come faster than wpctl runs, so only the latest of each kind is
//! carried out.

use std::io;

use clamshell_state::Sound;

use crate::os::command;
use crate::os::worker::{self, Worker};

/// Which commands replace each other: those for the same setting of the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Volume,
    Mute,
    StreamVolume(u32),
    StreamMute(u32),
    Output,
}

fn kind(sound: &Sound) -> Kind {
    match sound {
        Sound::Volume(_) => Kind::Volume,
        Sound::Mute(_) => Kind::Mute,
        Sound::StreamVolume(id, _) => Kind::StreamVolume(*id),
        Sound::StreamMute(id, _) => Kind::StreamMute(*id),
        Sound::Output(_) => Kind::Output,
    }
}

/// The wpctl arguments that carry out `sound`.
fn wpctl_args(sound: &Sound) -> Vec<String> {
    const SINK: &str = "@DEFAULT_AUDIO_SINK@";
    let flag = |on: bool| if on { "1" } else { "0" }.to_owned();
    match sound {
        Sound::Mute(m) => vec!["set-mute".into(), SINK.into(), flag(*m)],
        Sound::Volume(v) => vec!["set-volume".into(), SINK.into(), format!("{v}%")],
        Sound::StreamMute(id, m) => vec!["set-mute".into(), id.to_string(), flag(*m)],
        Sound::StreamVolume(id, v) => vec!["set-volume".into(), id.to_string(), format!("{v}%")],
        Sound::Output(id) => vec!["set-default".into(), id.to_string()],
    }
}

/// Starts the thread that sets the sound. Failures are logged by [`command`].
pub fn spawn() -> io::Result<Worker<Sound>> {
    worker::spawn_coalescing(clamshell_state::Thread::Sound.name(), kind, |sound| {
        let args = wpctl_args(&sound);
        command::output("wpctl", &args.iter().map(String::as_str).collect::<Vec<_>>());
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn only_the_latest_of_each_is_carried_out() {
        let (tx, rx) = mpsc::channel();
        for s in [Sound::Volume(10), Sound::StreamVolume(80, 5), Sound::Volume(20), Sound::StreamVolume(81, 7), Sound::StreamVolume(80, 6)] {
            tx.send(s).unwrap();
        }
        assert_eq!(
            worker::coalesce(Sound::Mute(false), &rx, kind),
            vec![Sound::Mute(false), Sound::Volume(20), Sound::StreamVolume(80, 6), Sound::StreamVolume(81, 7)],
            "a mute before a volume stays before it"
        );
    }

    #[test]
    fn as_wpctl_commands() {
        assert_eq!(wpctl_args(&Sound::Volume(35)), ["set-volume", "@DEFAULT_AUDIO_SINK@", "35%"]);
        assert_eq!(wpctl_args(&Sound::StreamMute(90, true)), ["set-mute", "90", "1"]);
        assert_eq!(wpctl_args(&Sound::Output(51)), ["set-default", "51"]);
    }
}
