# Docs

Read `architecture.md` first, then `../AGENTS.md` for how work is done here.

## The design as it is

- `architecture.md`: how ClamShell is put together. It covers the cycle, the crates, the state, windows,
  living with what apps do, the UI, the system threads, the rules that hold everywhere, and what checks
  them.
- `state-machines.md`: every feature's states and transitions, the tests that pin each, and the
  invariants of the randomized test.
- `process-control.md`: what runs and who decides. That covers launches as services, adopted programs,
  slices, freezing and ending.
- `runners.md`: the library's places, runners and sources, and how shelf starts what it shows.
- `files-tab.md`: the Files tab, its operations and archives.
- `picture.md`: a game's picture, from the tone curves and the colour pipeline to anti-aliasing,
  upscaling and sharpening.
- `device-profiles.md`: what is specific to a device and where it is kept.
- `android.md`: Android apps through Waydroid.
- `security.md`: what is kept, accepted or run, and why.

## The device

- `thor-gameplan.md`: the plan for the AYN Thor, from the kernel to the session, with what was found on
  the way.

## Plans under way

- `review-2026-10-26b.md`: the second review after the compositor became the only session, and its
  plan.

## History

Finished plans and reviews, kept for the reasons they record:

- `history/review-2026-10.md`
- `history/quality-plan.md`
- `history/organisation-plan.md`
- `history/critique-2026-10.md`
- `history/robustness-plan.md`
- `history/state-plan.md`
- `history/state-review.md`
- `history/compositor-mvp.md`
- `history/review-2026-10-26.md`

Their file and line references are as they were then.
