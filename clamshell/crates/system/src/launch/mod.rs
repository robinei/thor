//! Launching and following what runs: launches in scopes of their own (launcher, job), the scopes
//! watched, paused and ended (scopes), and where mpv left a video (mpv).

pub mod freezer;
pub mod job;
pub mod launcher;
pub mod mpv;
pub mod scopes;
