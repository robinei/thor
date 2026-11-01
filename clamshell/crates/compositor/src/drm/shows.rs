//! What each screen shows (composed, ClamShell's own buffers, or a window's), and presenting it.

use super::*;

/// What a screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shows {
    /// A frame composed from the scene.
    Composed,
    /// Its own buffers, with ClamShell's view (see `ui_scanout`).
    Own,
    /// Window `id` from the window's own buffer (see `window_scanout`).
    Window(i64),
    /// Window `id` drawn straight onto it, composed without the scene between (see `colour::render`).
    Straight(i64),
}

impl std::fmt::Display for Shows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Shows::Composed => write!(f, "composes its frames"),
            Shows::Own => write!(f, "shows its own buffers"),
            Shows::Window(id) => write!(f, "shows window {id} from its own buffer"),
            Shows::Straight(id) => write!(f, "draws window {id} straight onto it while it is one element"),
        }
    }
}

/// What a screen's showing is decided from (see [`shows()`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct Facts {
    pub(super) on: bool,
    /// It has its own buffers, not refused, and nothing but ClamShell's view is on it (but the overlay and
    /// cursor on their planes).
    pub(super) own: bool,
    /// The window alone on it (see `Server::lone_window`), if one is.
    pub(super) lone: Option<i64>,
    /// Whether that window's buffer can be shown as it is: else why not. None where the screen can't
    /// show a window's buffer.
    pub(super) buffer: Option<Result<(), &'static str>>,
    /// A screenshot is taken of the scene this turn.
    pub(super) screenshot: bool,
    /// Its part of the scene is mirrored on an external display, or it is that display: composed whole
    /// every frame, which the other shows too.
    pub(super) mirroring: bool,
}

/// What a screen shows by its `facts`: its own buffers while it can; the window alone on it from its own
/// buffer while that can be, else drawn straight; else a frame composed. For a screenshot a lone window
/// is composed, the scene being what it is taken of; while mirroring, everything.
pub(super) fn shows(facts: Facts) -> Shows {
    match facts.lone {
        _ if facts.mirroring => Shows::Composed,
        _ if facts.own => Shows::Own,
        Some(_) if facts.screenshot => Shows::Composed,
        Some(id) if facts.on && facts.buffer == Some(Ok(())) => Shows::Window(id),
        Some(id) => Shows::Straight(id),
        None => Shows::Composed,
    }
}

/// What a change of what a screen shows takes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Change {
    /// The next commit clears the CRTC's colour its own buffers set.
    pub(super) clear_own: bool,
    /// Its own buffers are drawn and committed whole, though the view didn't change: the screen shows
    /// something else.
    pub(super) reset_own: bool,
    /// The next commit clears the plane's colour a window's buffer set.
    pub(super) clear_window: bool,
    /// The scenes are drawn whole: they left the window out.
    pub(super) redraw: bool,
    /// The screen shows a new frame.
    pub(super) dirty: bool,
}

/// What going `from` showing one thing `to` another takes.
pub(super) fn change(from: Shows, to: Shows) -> Change {
    let mut change = Change::default();
    if from == to {
        return change;
    }
    match from {
        Shows::Own => (change.clear_own, change.dirty) = (true, true),
        Shows::Window(_) if !matches!(to, Shows::Window(_)) => (change.clear_window, change.redraw, change.dirty) = (true, true, true),
        _ => {}
    }
    match to {
        Shows::Own => change.reset_own = true,
        Shows::Window(_) => change.dirty = true,
        _ => {}
    }
    change
}

/// Has the screen show `to`, with what the change takes, logged.
pub(super) fn show(screen: &mut Screen, pipeline: &mut Pipeline, to: Shows) {
    let change = change(screen.shows, to);
    if change.clear_own
        && let Some(ui) = &screen.ui
    {
        screen.surface.clear_after(ui.clear());
    }
    if change.reset_own
        && let Some(ui) = &mut screen.ui
    {
        ui.reset();
    }
    if change.clear_window
        && let Some(scanout) = &screen.window
    {
        screen.surface.clear_after(scanout.clear());
    }
    if change.redraw {
        pipeline.redraw_all();
    }
    screen.dirty |= change.dirty;
    if to != screen.shows {
        if screen.surface.rendering() {
            screen.surface.cancel_drawing();
            screen.feedback.discard();
        }
        log::info!("drm: the {} screen {to}", screen.name());
    }
    screen.shows = to;
    if !matches!(to, Shows::Window(_)) {
        screen.candidate = None;
    }
}

/// Decides for each screen what it shows (see [`shows()`]), from what is on it this turn. The window alone on
/// a screen that can show a window's buffer is told the plane's formats, and told the renderer's again
/// once it isn't; one that can't be shown so is composed, with the reason logged. A window shown from its
/// own buffer is committed again when its buffer changes.
pub(super) fn shows_decide(drawing: &mut Drawing, server: &mut Server) {
    let Drawing { screens, pipeline, .. } = drawing;
    for screen in screens.iter_mut() {
        let down = screen.down();
        let mirroring = screen.mirrored || screen.external.is_some();
        let planed = screen.planed();
        let area = Rectangle::new((screen.part.loc.x, screen.part.loc.y).into(), (screen.part.size.w, screen.part.size.h).into());
        let own = screen.ui.as_ref().is_some_and(|ui| !ui.refused) && ui_only(server, down, planed);
        let lone = server.lone_window(down, area, planed).map(|placed| (placed.id, placed.geometry));
        let mut candidate = None;
        let mut buffer = None;
        if let Some(scanout) = &screen.window {
            let alone = lone.filter(|_| !own && screen.on && !mirroring);
            if alone.map(|(id, _)| id) != screen.fed {
                if let (Some(id), Some(default)) = (screen.fed, &server.protocols.dmabuf_feedback) {
                    server.dmabuf_feedback(id, default);
                }
                if let (Some((id, _)), Some(feedback)) = (alone, &scanout.feedback) {
                    server.dmabuf_feedback(id, feedback);
                }
                screen.fed = alone.map(|(id, _)| id);
            }
            let chosen = alone.map(|(id, geometry)| match server.candidate(id, geometry, down, area, pipeline.output_description(down)) {
                Ok(candidate) if candidate.colour.is_some() && !scanout.has_pipeline() => Err("its colour needs a colour pipeline the plane hasn't"),
                Ok(candidate) if scanout.refused == Some(candidate.shape()) => Err("the display engine refused its buffer"),
                chosen => chosen,
            });
            let why = chosen.as_ref().and_then(|chosen| chosen.as_ref().err().copied());
            if why != screen.composed_why {
                if let (Some(why), Some((id, _))) = (why, alone) {
                    log::info!("window scanout: window {id}, alone on the {:?} screen, is composed: {why}", screen.role);
                }
                screen.composed_why = why;
            }
            buffer = Some(chosen.as_ref().map_or(Err("no window alone"), |chosen| chosen.as_ref().map(|_| ()).map_err(|why| *why)));
            candidate = chosen.and_then(Result::ok);
        }
        let facts = Facts { on: screen.on, own, lone: lone.map(|(id, _)| id), buffer, screenshot: server.screenshot_requested, mirroring };
        let to = shows(facts);
        if let Shows::Window(_) = to
            && screen.candidate.as_ref().zip(candidate.as_ref()).is_none_or(|(shown, now)| shown.commit != now.commit || shown.shape() != now.shape())
        {
            // A new buffer to show.
            screen.dirty = true;
        }
        show(screen, pipeline, to);
        if let Shows::Window(_) = to {
            screen.candidate = candidate;
        }
    }
}

/// Shows the window decided on (see [`shows_decide`]) from its own buffer, in a test commit
/// first when its shape is new. Returns when to try again while a fence is pending or it failed (see [`failed`]): refused, the
/// window is composed until its buffers' shape changes, and the scene, which left it out, is drawn whole.
pub(super) fn present_window(screen: &mut Screen, server: &mut Server, pipeline: &mut Pipeline, cursor: &crate::cursor::Cursor) -> Option<Instant> {
    let with_layers = prepare_layers(screen, server, cursor);
    let (Some(scanout), Some(candidate)) = (&mut screen.window, &screen.candidate) else { return None };
    let shape = candidate.shape();
    let test = screen.tested != Some(shape);
    // The window's place turned as the composed frame is, its buffer the other way, as the plane takes it.
    let logical = screen.transform.transform_size(screen.size);
    let dst = screen.transform.transform_rect_in(candidate.dst.to_physical(1), &logical);
    // A framebuffer or colour the window's buffer can't have is the window's alone, whatever the planes show.
    let (shown, with_layers) = match scanout.framebuffer(&candidate.dmabuf).and_then(|fb| Ok((fb, scanout.colour(candidate.colour)?))) {
        Ok((fb, properties)) => {
            let states = screen.layers.iter().flat_map(PlaneLayers::states);
            (screen.surface.show_window(candidate.buffer.clone(), fb, candidate.src, dst, screen.transform.invert(), properties, states, test), with_layers)
        }
        Err(error) => (Err(crate::presenter::CommitError::Refused(error.to_string())), false),
    };
    let down = screen.role == ScreenRole::Bottom;
    match shown {
        Ok(()) => {
            screen.tested = Some(shape);
            let state = smithay::backend::renderer::element::RenderElementState {
                visible_area: usize::try_from(dst.size.w).unwrap_or(0) * usize::try_from(dst.size.h).unwrap_or(0),
                presentation_state: smithay::backend::renderer::element::RenderElementPresentationState::ZeroCopy,
                needs_capture: false,
            };
            screen.queued.states.clear();
            screen.queued.states.insert(smithay::backend::renderer::element::Id::from_wayland_resource(&candidate.surface), state);
            server.frame_queued(&screen.queued, Some(down));
            server.take_feedback(&screen.queued, Some(down), &mut screen.feedback);
            committed(screen, Commit::Window { id: candidate.id });
            screen.dirty = false;
            screen.answered = server.pacing.commits;
            None
        }
        Err(error) => {
            let id = candidate.id;
            failed(screen, pipeline, Commit::Window { id }, with_layers, error)
        }
    }
}

/// Shows the screen's own buffer drawn with what changed in ClamShell's view, if anything did. Returns when
/// to try again if it failed (see [`failed`]).
pub(super) fn present_ui(screen: &mut Screen, server: &mut Server, pipeline: &mut Pipeline, cursor: &crate::cursor::Cursor) -> Option<Instant> {
    let ui = screen.ui.as_mut()?;
    let fb = ui.draw(&mut server.core, ui_screen(screen.role));
    if fb.is_none() && !screen.layers_dirty {
        return None;
    }
    server.core.frames_done();
    let with_layers = prepare_layers(screen, server, cursor);
    let ui = screen.ui.as_mut()?;
    let states = screen.layers.iter().flat_map(PlaneLayers::states);
    let shown = match fb {
        Some(fb) => screen.surface.show_ui(fb, ui.colour(), states),
        None => screen.surface.show_again(states),
    };
    match shown {
        Ok(()) => {
            if fb.is_some() {
                ui.committed();
            }
            committed(screen, Commit::Own { drew: fb.is_some() });
            None
        }
        Err(error) => failed(screen, pipeline, Commit::Own { drew: fb.is_some() }, with_layers, error),
    }
}

/// Shows screen `index` its part of the scene. Returns when to try again while a fence is pending or it failed (see [`failed`]):
/// drawing it failing too. A screen pacing its windows notes what the frame shows of them and takes their
/// presentation feedback; one following another's pace shows the frame only.
pub(super) fn present(drawing: &mut Drawing, server: &mut Server, index: usize) -> Option<Instant> {
    use crate::presenter::CommitError;
    let Drawing { renderer, pipeline, screens, states, cursor, handle, .. } = drawing;
    let screen = screens.get_mut(index)?;
    let states = states.get(screen.scene())?;
    if screen.needs_compose && !screen.surface.rendering() {
        return None;
    }
    let with_layers = prepare_layers(screen, server, cursor);
    let encoding = screen.external.as_ref().map(|external| &external.encoding);
    let sync = if screen.surface.rendering() {
        None
    } else {
        let drawn = screen.surface.next_buffer().and_then(|mut buffer| {
            let mut target = renderer.bind(&mut buffer)?;
            pipeline.present_screen(renderer, &mut target, screen.down(), screen.size, screen.transform, encoding)
        });
        let sync = match drawn {
            Ok(sync) => sync,
            Err(error) => return failed(screen, pipeline, Commit::Composed, false, CommitError::Failed(format!("drawing: {error}"))),
        };
        // Capture feedback now: clients can commit more content while this drawing is pending.
        screen.queued.states.clear();
        if screen.paces {
            screen.queued.states.extend(states.states.iter().map(|(id, state)| (id.clone(), *state)));
            server.frame_queued(states, Some(screen.down()));
            server.take_feedback(states, Some(screen.down()), &mut screen.feedback);
            screen.drawn_commits = server.pacing.commits;
        }
        screen.dirty = false;
        screen.timing.submitted(&sync, handle);
        Some(sync)
    };
    let layers = screen.layers.iter().flat_map(PlaneLayers::states);
    match screen.surface.queue(sync, layers) {
        Ok(false) => return Some(Instant::now() + (screen.interval / 4).max(std::time::Duration::from_millis(1))),
        Ok(true) => {
            committed(screen, Commit::Composed);
            if screen.paces {
                screen.answered = screen.drawn_commits;
            }
        }
        Err(error) => {
            screen.dirty = true;
            // Order: back on their surfaces before the screen is drawn again, which takes them.
            screen.feedback.restore();
            return failed(screen, pipeline, Commit::Composed, with_layers, error);
        }
    }
    None
}
