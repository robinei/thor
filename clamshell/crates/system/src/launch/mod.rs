//! Launching and following what runs: launches in scopes of their own (launcher, job), the scopes
//! watched, paused and ended (scopes), and where mpv left a video (mpv).

pub(crate) mod freezer;
pub mod job;
pub(crate) mod launcher;
pub(crate) mod mpv;
pub(crate) mod scopes;
pub mod steam;
