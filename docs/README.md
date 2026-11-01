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
- `runners.md`: the library's places, runners and sources, and how the Library starts what it shows.
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

- `review-2026-11-01.md`: six read-only reviews (compositor, state, system, UI, packages,
  architecture) merged into 142 findings with priorities, architecture themes and an order of work.
- `window-management-ideas.md`: candidate improvements to window management, not yet decided.
- `steam-android-plan.md`: Steam, per-game PC-gaming settings (FEX, Turnip, frame generation) and
  Android, after the 2026-10-30 survey of the other Thor systems. S2, P1 and P2 implemented; device
  checks and later phases remain.

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
- `history/review-2026-10-26b.md`: the second review after the compositor became the only session;
  every phase of its plan done.
- `history/refactors-plan.md`: the architecture refactors from the review of 2026-10-28; built as
  recommended (one window-facts path, `Look::card`, the library's index, identities without a thread
  each).
- `history/state-facts-plan.md`: State as facts and the rest derived; every step done.
- `history/top-screens-plan.md`: the Library, Device and Setup as built-in screens, the library's format,
  editors and the file picker; built. Packaging the emulators the repos lack (Dolphin, Azahar, ARMSX2,
  DOSBox Pure, FBNeo) was left to the device work.
- `history/review-2026-10-31.md`: the review after the Vulkan-driver setting, R01–R20, all fixed.
- `history/device-profiles-steps.md`
- `history/screen-commits-plan.md`: a screen's commit errors by kind, and what it shows decided once.
- `history/window-model-plan.md`: the session's windows, keyboard and slots computed from state where
  they can be.
- `history/state-machines-notes.md`: the suspects of 2026-10-23, the decisions on them, and the notes
  kept while the compositor was built, moved out of `state-machines.md`

Their file and line references are as they were then.
