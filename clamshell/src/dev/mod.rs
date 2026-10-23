//! Snapshots of every screen (`clamshell --snapshot`) and the demo state they show. Built with the `dev`
//! feature and for tests.

pub(crate) mod demo;
#[cfg(test)]
mod focus_map;
pub(crate) mod snapshot;
#[cfg(test)]
mod trace;
