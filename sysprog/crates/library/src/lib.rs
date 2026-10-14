//! The game library: today a fixed demo list. Later the Pegasus metadata format, `stats.db` and Steam's
//! manifests (docs/plans.md in the GPD repo, "Rust and a project structure").

use std::sync::Arc;

/// One game as the rest of the program sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// Stable id (focus, settings and play time are keyed by it). Shared, so the UI can put it in a
    /// frame's targets without allocating.
    pub id: Arc<str>,
    pub title: String,
    /// Stand-in for box art until images load: 0xRRGGBB.
    pub color: u32,
}

/// Placeholder games for development on the desktop.
pub fn demo_games() -> Vec<Game> {
    [
        ("zelda-lttp", "A Link to the Past", 0x3f8f4f),
        ("metroid-zm", "Metroid: Zero Mission", 0xb0503a),
        ("mk-ds", "Mario Kart DS", 0xc8a038),
        ("ff6", "Final Fantasy VI", 0x5060b0),
        ("dark-souls", "Dark Souls", 0x606060),
        ("hades", "Hades", 0xa03060),
        ("celeste", "Celeste", 0x8050b0),
    ]
    .into_iter()
    .map(|(id, title, color)| Game { id: id.into(), title: title.into(), color })
    .collect()
}
