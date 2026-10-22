//! Launching and following what runs. `launcher` and `job` start launches in scopes of their own,
//! `scopes` watches, pauses and ends the scopes, and `mpv` reads where mpv left a video.

pub(crate) mod freezer;
pub mod job;
pub(crate) mod launcher;
pub(crate) mod mpv;
pub(crate) mod scopes;
pub mod steam;
