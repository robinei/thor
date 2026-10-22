//! What was fetched about an entry.

use std::path::PathBuf;

use crate::Details;

/// What was fetched about an entry (clamshell-system's `meta`), each part if found: its cover, its
/// details, and a video's length.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Meta {
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub cover: Option<PathBuf>,
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub details: Details,
    /// A video's length (seconds; probed, not fetched, and kept apart: see clamshell-system's `store`).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub duration: Option<u32>,
}
