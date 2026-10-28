# Plan, 2026-10-28: a screen's commits and what it shows

From the third review round's whole reading of the DRM path (`drm.rs`, `presenter.rs`, `plane_layers.rs`,
`window_scanout.rs`, `ui_scanout.rs`). Two weaknesses remain after its fixes:

- Every failed commit counts as the display engine refusing it. A temporary error (`EBUSY` after a flip
  taken as lost, `ENOMEM`) turns a screen's planes or its own buffers off for the session, and a failure
  doesn't wake the loop to try again.
- What a screen shows is spread over `ui_shown`, `window_shown` and `direct`, kept apart only by the order
  of four decide passes over about 30 fields, and whether a frame is in flight is kept twice
  (`Screen.pending`/`queued_at`, `Presenter.pending`/`lost`). None of it is testable without a device.

Also: commits timed with `wp_commit_timing` are released against the top screen's next frame for windows
on both screens.

Settled: a refusal with the overlay and cursor planes showing still turns them off for the session; only
a real refusal does. Each step ends in commits that pass `tools/check.sh`; the VM suite runs after steps 2
and 4 and at the end.

## Steps

- [ ] 1. **Commit errors by kind** (`presenter.rs`). Commits return `Refused` (the test commit failed, or
  `EINVAL`/`ERANGE`), `Busy` (`EBUSY`), `Away` (`DeviceInactive`) or `Failed` (anything else), sorted by
  one function tested against Smithay's errors.
- [ ] 2. **One present path, one outcome table** (`drm.rs`). The four present paths return an outcome;
  one function applies it. Committed as now. Refused with the planes showing: planes off. Refused
  otherwise: the screen's own buffers, or that window's shape, refused. Busy or Failed: the screen stays
  dirty and the loop wakes at its next refresh, logged once a run of failures. Away: nothing.
- [ ] 3. **A frame in flight kept once.** `Screen.pending`/`queued_at` and the lost-flip timeout move into
  `Presenter`, beside its `pending` and `lost`.
- [ ] 4. **What a screen shows, decided once.** `Shows { Composed, Own, Window(Candidate), Straight(id) }`
  replaces `ui_shown`, `window_shown` and `direct`. A pure function decides it from a screen's facts (on,
  only ClamShell's view on it, the lone window and its candidate or why not, refusals, a screenshot); a
  second gives each change's effects (properties cleared after, drawn whole, scene redrawn, own buffers
  reset). Both tested by tables; the decide passes only gather the facts and apply the result.
- [ ] 5. **Commit timing per screen.** Each screen's windows' timed commits are released against that
  screen's next frame, the loop waking for the first due. The nested session, one host window, stays as
  it is.
