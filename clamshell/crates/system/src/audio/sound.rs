//! Setting the sound from the bar's volume slider and the volume tab: the output's volume and mute, an
//! app's volume and mute, and which output plays, through PipeWire's `wpctl`. It runs on a thread of its
//! own. While a finger slides, requests come faster than wpctl runs, so only the latest of each kind is
//! carried out. Each command has a number; once a batch of them is carried out, the last of their numbers
//! is published ([`Carried`]), for the readings to say which commands they have seen.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use clamshell_state::Sound;

use crate::os::command;
use crate::os::worker::{self, Worker};

/// The number of the last sound command carried out, every command before it carried out too, shared
/// with the threads that read the sound (see `clamshell_state::levels::Levels`). It outlives the sound
/// thread's restarts.
#[derive(Debug, Clone, Default)]
pub(crate) struct Carried(Arc<AtomicU64>);

impl Carried {
    /// The number of the last command carried out. A reader asks before it reads, so that what it reads
    /// has seen at least that command.
    pub(crate) fn seen(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    fn done(&self, number: u64) {
        self.0.fetch_max(number, Ordering::Release);
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

/// Starts the thread that sets the sound, publishing to `carried` what it has carried out. Failures are
/// logged by [`command`].
pub(crate) fn spawn(carried: Carried) -> io::Result<Worker<(Sound, u64)>> {
    Worker::spawn(clamshell_state::Thread::Sound.name(), move |commands| {
        while let Ok(first) = commands.recv() {
            // Published once the whole batch is carried out: coalescing may put a later command before an
            // earlier one of another kind.
            let batch = worker::coalesce(first, commands, |(sound, _): &(Sound, u64)| sound.kind());
            let last = batch.iter().map(|(_, number)| *number).max();
            for (sound, _) in batch {
                let args = wpctl_args(&sound);
                command::output("wpctl", &args.iter().map(String::as_str).collect::<Vec<_>>());
            }
            if let Some(number) = last {
                carried.done(number);
            }
        }
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
            worker::coalesce(Sound::Mute(false), &rx, Sound::kind),
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
