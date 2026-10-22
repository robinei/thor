//! The library's model: entries (games, apps, movies, and groups of them such as a series' seasons and
//! episodes), the tabs they are in and how they sort. It also has the parts of reading Steam's files and
//! matching name patterns that need no disk access.
//!
//! The crate has no I/O and no dependencies. Scanning (library.json's content dirs, Steam, desktop
//! files), play time and favourites live in clamshell-system's `library`.

#[cfg(any(test, feature = "demo"))]
pub mod demo;
pub mod hash;
pub mod ids;
pub mod pattern;
pub mod plays;
pub mod sort;
pub mod steam;
pub mod titles;

mod entry;
mod library;
mod meta;
mod runner;

pub use entry::{BrowseKind, Browsed, DONE, Details, Entry, Launch, Playback, Tool, key_of};
pub use library::{Collection, Library, OwnTab};
pub use meta::Meta;
pub use plays::Plays;
pub use runner::{RunnerOf, runner_key};
