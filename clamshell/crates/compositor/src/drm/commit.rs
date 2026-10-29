//! Committing a screen: its planes' layers, and what a failed commit does next.

use super::*;

/// Decides for each screen with overlay and cursor planes what goes on them this turn, the cursor if its
/// hotspot is on the screen; a change to what they show has the screen commit (see `plane_layers`).
pub(super) fn layers_decide(drawing: &mut Drawing, server: &mut Server) {
    let Drawing { screens, cursor, renderer, .. } = drawing;
    let at = server.scene.shown().iter().find_map(|shown| match shown {
        crate::scene::Shown::Cursor { at } => Some(*at),
        _ => None,
    });
    // (Not for a screen that is off: nothing is planed there until it is on.)
    for screen in screens.iter_mut().filter(|screen| screen.on) {
        let Some(layers) = &mut screen.layers else { continue };
        let down = screen.role == ScreenRole::Bottom;
        let at = at.filter(|at| server.screens.below(at.x) == down);
        let origin = smithay::utils::Point::<i32, Logical>::from((screen.part.loc.x, screen.part.loc.y));
        screen.layers_dirty |= layers.decide(server, ui_screen(screen.role), cursor, renderer, at, origin);
    }
}

/// The screen's commit with its planes went through: they show as placed.
pub(super) fn layers_committed(screen: &mut Screen) {
    if let Some(layers) = &mut screen.layers {
        layers.committed();
    }
    screen.layers_dirty = false;
}

/// The screen's commit with its planes failed: they are off from now on, composed, and the screen is
/// committed again.
pub(super) fn layers_failed(screen: &mut Screen, error: &impl std::fmt::Display) {
    if let Some(layers) = &mut screen.layers {
        layers.fail(&format!("a commit with them failed: {error}").into(), &format!("{:?}", screen.role));
    }
    screen.dirty = true;
    screen.layers_dirty = true;
}

/// Commits the frame the screen shows again, with its overlay and cursor planes as they now are. Returns
/// when to try again if it failed.
pub(super) fn present_layers(screen: &mut Screen, server: &mut Server, pipeline: &mut Pipeline, cursor: &crate::cursor::Cursor) -> Option<Instant> {
    let with_layers = prepare_layers(screen, server, cursor);
    let states = screen.layers.iter().flat_map(PlaneLayers::states);
    match screen.surface.show_again(states) {
        Ok(()) => {
            committed(screen);
            None
        }
        Err(error) => failed(screen, pipeline, Commit::Again, with_layers, error),
    }
}

/// What a commit showed on the primary plane, for what its failure means.
#[derive(Debug, Clone, Copy)]
pub(super) enum Commit {
    /// A frame composed.
    Composed,
    /// The screen's own buffer, drawn anew for it if `drew`.
    Own { drew: bool },
    /// Window `id`'s buffer (the one decided on, see [`shows_decide`]).
    Window { id: i64 },
    /// The frame shown, again, with the planes changed.
    Again,
}

/// What is done about a commit that failed, by its error (see `CommitError`), and whether the overlay and
/// cursor planes showed something in it.
#[derive(Debug, PartialEq)]
pub(super) enum Remedy {
    /// The seat is away: nothing, until it is back.
    Wait,
    /// Tried again at the next refresh, as it was.
    Retry,
    /// The planes are off from now on: composed, at once.
    PlanesOff,
    /// What the primary plane showed is refused: the screen's own buffers, or the window's shape; composed
    /// at once.
    Refuse,
}

impl Commit {
    /// Whether the screen has something else to show if it is refused: composing, for the screen's own
    /// buffer or a window's; nothing, for a frame composed or the frame shown again.
    pub(super) fn refusable(self) -> bool {
        matches!(self, Commit::Own { .. } | Commit::Window { .. })
    }
}

/// The remedy for a commit that failed with `error`, `with_layers` showing, `refusable` or not (see
/// [`Commit::refusable`]).
pub(super) fn remedy(refusable: bool, with_layers: bool, error: &crate::presenter::CommitError) -> Remedy {
    use crate::presenter::CommitError;
    match error {
        CommitError::Away => Remedy::Wait,
        CommitError::Busy(_) | CommitError::Failed(_) => Remedy::Retry,
        CommitError::Refused(_) if with_layers => Remedy::PlanesOff,
        CommitError::Refused(_) if refusable => Remedy::Refuse,
        // Nothing else to show: tried again.
        CommitError::Refused(_) => Remedy::Retry,
    }
}

/// Applies the remedy for the screen's commit `kind` that failed with `error`, `with_layers` showing.
/// Returns when to try again.
pub(super) fn failed(screen: &mut Screen, pipeline: &mut Pipeline, kind: Commit, with_layers: bool, error: crate::presenter::CommitError) -> Option<Instant> {
    if let (Commit::Own { drew: true }, Some(ui)) = (kind, &mut screen.ui) {
        ui.failed();
    }
    let role = screen.role;
    match remedy(kind.refusable(), with_layers, &error) {
        Remedy::Wait => None,
        Remedy::Retry => {
            if !screen.failing {
                log::warn!("drm: the {role:?} screen's commit failed ({error}): tried again at each refresh until one goes through");
                screen.failing = true;
            }
            // (The screen's own buffer, not drawn anew, is committed again as the frame shown is.)
            match kind {
                Commit::Again | Commit::Own { drew: false } => screen.layers_dirty = true,
                Commit::Own { drew: true } | Commit::Window { .. } | Commit::Composed => screen.dirty = true,
            }
            Some(Instant::now() + screen.interval)
        }
        Remedy::PlanesOff => {
            layers_failed(screen, &error);
            Some(Instant::now())
        }
        Remedy::Refuse => {
            match kind {
                Commit::Own { .. } => {
                    log::warn!("ui scanout: the {role:?} screen: {error}: composed from now on");
                    if let Some(ui) = &mut screen.ui {
                        ui.refused = true;
                    }
                }
                Commit::Window { id } => {
                    log::info!(
                        "window scanout: the {role:?} screen can't show window {id} from its own buffer: {error}: composed until its buffers' shape changes"
                    );
                    if let Some(scanout) = &mut screen.window {
                        scanout.refused = screen.candidate.as_ref().map(Candidate::shape);
                    }
                }
                Commit::Composed | Commit::Again => {}
            }
            // Composed from the next turn, which decides again (see `shows_decide`) without it.
            show(screen, pipeline, Shows::Composed);
            Some(Instant::now())
        }
    }
}

/// Readies the screen's overlay and cursor planes for its next commit (see `PlaneLayers::prepare`).
/// Returns whether they show something in it, so that a refusal of it may be theirs.
pub(super) fn prepare_layers(screen: &mut Screen, server: &mut Server, cursor: &crate::cursor::Cursor) -> bool {
    if let Some(layers) = &mut screen.layers {
        let failed = layers.failed();
        layers.prepare(server, ui_screen(screen.role), cursor);
        if layers.failed() && !failed {
            // Off from now on: composed, from the next frame.
            screen.dirty = true;
            screen.layers_dirty = true;
        }
    }
    screen.layers.as_ref().is_some_and(PlaneLayers::shows)
}

/// The screen's commit went through: its planes show as placed, and its flip is awaited.
pub(super) fn committed(screen: &mut Screen) {
    if std::mem::take(&mut screen.failing) {
        log::info!("drm: the {:?} screen's commits go through again", screen.role);
    }
    layers_committed(screen);
}

/// Records the screens' overlays that are stale, and places the scene again if any was: what an overlay
/// shows decides whether it goes on a plane, a screen shows its own buffers, or a window is shown from its
/// own, all of which follow. (Composing would record it too, but only after those decisions, a turn late.)
pub(super) fn record_overlays(server: &mut Server) {
    let mut recorded = false;
    for screen in [clamshell_ui::Screen::Top, clamshell_ui::Screen::Bottom] {
        let overlay = clamshell_ui::Layer::Overlay(screen);
        if server.core.stale(overlay) {
            server.core.record(overlay, false);
            recorded = true;
        }
    }
    if recorded {
        server.place();
    }
}
