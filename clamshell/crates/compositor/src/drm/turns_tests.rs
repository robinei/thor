//! A DRM screen turn by turn, as `step` drives it, with a fake presenter: the real pieces that decide
//! (`composes`, `next_shown`, the composition's `Schedule`, `screen_turn`) in `step`'s order (a flip
//! first, then the composition if due, then the screen's turn), and a presenter that only records what is
//! committed and flips a refresh later. What `step` adds beyond that order (drawing on the GPU, the
//! planes' states, feedback) is the real presenter's, checked in the VM's DRM checks and on the device.

use std::time::{Duration, Instant};

use super::{Does, Shows, Timing, composes, earlier, next_shown, screen_turn};
use crate::frame_schedule::Schedule;

/// 60 Hz.
const INTERVAL: Duration = Duration::from_micros(16_667);
/// What drawing a frame takes on the GPU.
const COST: Duration = Duration::from_millis(3);

/// One screen, composed, pacing its windows; times are from `base`.
struct Screen {
    base: Instant,
    schedule: Schedule,
    /// The last flip, and the flip a frame committed waits for.
    flipped: Option<Duration>,
    flip_at: Option<Duration>,
    /// Its part of the scene changed since it was last composed; composed, and not yet shown.
    needs_compose: bool,
    dirty: bool,
    /// Its overlay or cursor plane changed since its last commit.
    layers_dirty: bool,
    /// The clients' commits, and those answered with frame callbacks.
    commits: u64,
    answered: u64,
    next_callbacks: Option<Instant>,
    /// What the fake presenter saw: frames shown, plane-only commits, idle answers, and the frames shown
    /// later than planned.
    frames: u32,
    planes: u32,
    answers: Vec<Duration>,
    misses: u32,
}

impl Screen {
    fn new() -> Screen {
        Screen {
            base: Instant::now(),
            schedule: Schedule::default(),
            flipped: None,
            flip_at: None,
            needs_compose: false,
            dirty: false,
            layers_dirty: false,
            commits: 0,
            answered: 0,
            next_callbacks: None,
            frames: 0,
            planes: 0,
            answers: Vec::new(),
            misses: 0,
        }
    }

    /// A client's commit: a change shown (its window on this screen), or none.
    fn commit(&mut self, shown: bool) {
        self.commits += 1;
        self.needs_compose |= shown;
    }

    /// One turn of the loop at `now`; when it wants the next, if not at a flip or an event.
    fn turn(&mut self, now: Duration) -> Option<Duration> {
        let at = self.base + now;
        // The flip, which the loop hears before its turn.
        if self.flip_at.is_some_and(|flip| flip <= now) {
            let flip = self.flip_at.take().unwrap_or(now);
            self.flipped = Some(flip);
            self.misses += u32::from(self.schedule.presented(self.base + flip, INTERVAL));
        }
        // The composition, when free and due (`compose`, with a hardware clock).
        let mut wake = None;
        if composes(true, self.flip_at.is_some(), false, false) && self.needs_compose {
            let target = self.base + next_shown(self.flipped, INTERVAL, self.flip_at.is_some(), now);
            match self.schedule.deadline(at, target, INTERVAL) {
                Some(deadline) => wake = Some(deadline),
                None => {
                    self.schedule.started(at);
                    self.schedule.observe(COST);
                    self.needs_compose = false;
                    self.dirty = true;
                }
            }
        }
        let timing = Timing {
            in_flight: self.flip_at.is_some(),
            rendering: false,
            composing: self.needs_compose,
            shows: Shows::Composed,
            dirty: self.dirty,
            layers_dirty: self.layers_dirty,
            on: true,
            paces: true,
            held: None,
            answered: self.answered == self.commits,
            next_callbacks: self.next_callbacks,
        };
        let turn = screen_turn(timing, at);
        wake = earlier(wake, turn.wake);
        // The fake presenter: a commit is shown at the next refresh.
        match turn.does {
            Does::Frame => {
                self.dirty = false;
                self.layers_dirty = false;
                self.answered = self.commits;
                self.flip_at = Some(next_shown(self.flipped, INTERVAL, false, now));
                self.frames += 1;
            }
            Does::Layers => {
                // (`committed` with `Commit::Again`: the plan for this flip is the planes' now.)
                self.schedule.superseded();
                self.layers_dirty = false;
                self.flip_at = Some(next_shown(self.flipped, INTERVAL, false, now));
                self.planes += 1;
            }
            Does::Answer => {
                self.answered = self.commits;
                self.next_callbacks = Some(at + INTERVAL);
                self.answers.push(now);
            }
            Does::Own | Does::Window | Does::Nothing => {}
        }
        wake.map(|wake| wake.saturating_duration_since(self.base))
    }
}

/// Runs `screen` for `refreshes` refreshes, with `event` called at each millisecond (a client committing,
/// the cursor moving), the loop turning at each event, flip and wake asked for, as `run` does.
fn run(screen: &mut Screen, refreshes: u32, mut event: impl FnMut(&mut Screen, Duration) -> bool) {
    let end = INTERVAL * refreshes;
    let mut wake: Option<Duration> = None;
    let mut now = Duration::ZERO;
    while now < end {
        let next_ms = Duration::from_millis(u64::try_from(now.as_millis()).unwrap_or(u64::MAX - 1) + 1);
        now = [Some(next_ms), wake, screen.flip_at].into_iter().flatten().filter(|t| *t > now).min().unwrap_or(end);
        let happened = now.as_micros().is_multiple_of(1000) && event(screen, now);
        let due = wake.is_some_and(|w| w <= now) || screen.flip_at.is_some_and(|f| f <= now);
        if happened || due {
            wake = screen.turn(now);
        }
    }
}

/// A game drawing every refresh is shown every refresh, each frame on time once the schedule has learned
/// its cost, and the loop turns only for its events, the composition's deadline and the flips.
#[test]
fn a_frame_a_refresh_is_shown_every_refresh_on_time() {
    let mut screen = Screen::new();
    run(&mut screen, 120, |screen, now| {
        let commit = now.as_millis() % 17 == 2;
        if commit {
            screen.commit(true);
        }
        commit
    });
    assert!(screen.frames >= 110, "{} frames in 120 refreshes", screen.frames);
    assert_eq!(screen.misses, 0);
}

/// The cursor moving on its plane every refresh, just after the game's commit, while that commit's
/// composition waits for its deadline: the composition carries the planes, before the same flip, and the
/// game is shown every refresh, on time. Committed alone, the planes took that flip, the composition waited
/// for the next, and the cursor moving again before it kept the game's frames from being shown at all;
/// before that (review F36) the frame came a refresh late, counted a miss.
#[test]
fn the_cursor_moving_while_a_composition_waits_goes_with_it_and_the_game_is_shown() {
    let mut screen = Screen::new();
    run(&mut screen, 120, |screen, now| {
        let ms = now.as_millis() % 17;
        if ms == 2 {
            screen.commit(true);
        }
        if ms == 3 {
            screen.layers_dirty = true;
        }
        ms == 2 || ms == 3
    });
    assert!(screen.frames >= 110, "{} frames in 120 refreshes ({} plane commits)", screen.frames, screen.planes);
    assert_eq!(screen.misses, 0);
}

/// The cursor moving alone, with nothing composed, is committed on its plane with the frame shown, once a
/// refresh at most.
#[test]
fn the_cursor_moving_alone_is_committed_on_its_plane() {
    let mut screen = Screen::new();
    run(&mut screen, 60, |screen, _| {
        screen.layers_dirty = true;
        true
    });
    assert_eq!(screen.frames, 0);
    assert!((55..=61).contains(&screen.planes), "{} plane commits in 60 refreshes", screen.planes);
}

/// Clients committing without a visible change are answered with their frame callbacks at most once a
/// refresh, so that an idle screen never spins, and no frame is drawn for them.
#[test]
fn commits_with_nothing_to_show_are_answered_at_most_once_a_refresh() {
    let mut screen = Screen::new();
    run(&mut screen, 60, |screen, _| {
        screen.commit(false);
        true
    });
    assert_eq!(screen.frames, 0);
    assert!(screen.answers.len() >= 55, "{} answers", screen.answers.len());
    assert!(
        screen.answers.iter().zip(screen.answers.iter().skip(1)).all(|(a, b)| b.saturating_sub(*a) >= INTERVAL.saturating_sub(Duration::from_millis(1))),
        "{:?}",
        screen.answers
    );
}
