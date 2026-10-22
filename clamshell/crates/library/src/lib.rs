//! The library's model: entries (games, apps, movies, groups of them such as a series' seasons and
//! episodes), the tabs they are in and how they sort, and the parts of Steam's files and of name patterns
//! that need no disk. No I/O and no dependencies: scanning (library.json's content dirs, Steam, desktop
//! files), play time and favourites are clamshell-system's (`library`).

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
