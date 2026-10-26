//! Launching and following what runs. `launcher` and `job` start launches as services of their own,
//! `units` watches and ends their units, `freezer` pauses them, and `mpv` reads where mpv left a video.

pub(crate) mod freezer;
mod job;
pub(crate) mod launcher;
pub(crate) mod mpv;
pub mod steam;
pub(crate) mod units;
