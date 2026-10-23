# Review of the new State

2026-10-23, after the State refactor (`state-plan.md`, done): an architecture review and a bug hunt by
reviewers new to the code, and the VM suite. The architecture reviewer checked claims by mutating a copy of
the code and running the tests.

## Verdict

The shape holds: core is pure and tested through `State::apply`; areas own their fields; sync's
`Wanted`/`Sent` diff is clean; the recording canvas is sound and well guarded (the damaged-vs-whole test
fails when damage drops the old commands or ignores buffer age). The bug hunt found no high-severity
regression: the settle order, the session gate, the event queue, the resends, the keymap versions, damage
bounds and buffer age all match the old behaviour. The VM suite passes (119 checks; its one failure was
a check looking for the log text `timedatectl` printed before timedated moved to D-Bus, now fixed).

The weak spots are at the seams.

## Bugs

- [x] **A hidden view freezes on a desktop.** The bottom view isn't drawn while `bottom().off()`, but on a
      desktop there is no bottom output to power off, so the panel stays visible with a frozen clock,
      levels and notices (after Home+Start, or 4 minutes untouched on battery). The top view is skipped
      while a launched program's window has focus, which on a multi-output desktop freezes a visible
      window. Against "show everything on desktop". (`ui/platform/app.rs:180`, `view.rs:165`,
      `session/mod.rs:157`.)
- [ ] **Lists are re-anchored only after readings.** `follow_lists` runs only from `on_reading`; the dock,
      keymaps, layers and time lists change through other paths and keep a stale anchor until the next
      reading, then jump. It belongs in settle. (`state.rs:850`, `:524`.)
- [ ] **The UI reorders effects across re-entrant events.** `App::handle` applies an event, then
      `run_actions` can emit another, whose nested `handle` carries out its effects before the outer
      event's. Emitted events should wait until the effects are done. (`ui/platform/app.rs`.)
- [ ] **The pressed highlight and the keyed action can land on different rows** when a list re-sorts
      between the frame and the press: the press acts on the item drawn there, the highlight follows the
      position. (`nav/pointer.rs:52`.) Cosmetic.

## Fragile

- [ ] **Settle reorders effects by position, untested.** `rotate_left` moves the window effects, and the
      Steam launches with them, after the later steps; deleting it passes every test; the step order in the
      doc doesn't match the code (quitting runs before closed). A buffer of its own for the window effects,
      appended at the right point, and a test. (`state.rs:386-407`.)
- [ ] **Settings sync has a back door.** `Sent` compares only the program, so every change to the settings
      in force must remember `Resend::Settings`; `follow_runner_keymaps` doesn't. `Values` is `Copy +
      PartialEq`: diff it and drop `Resend::Settings`. (`sync.rs`, `state.rs:1048`.)
- [ ] **Order rules documented but not pinned.** These mutations passed the whole suite: swapping
      settings/keymap or keymap/route in sync, the event queue last-in-first-out, show pending/reopened
      swapped, quitting before background, the made-own saves swapped. One exact-sequence test per rule
      that claims an order.
- [ ] **The session gate can't be tested** (replacing it with `true` passes everything), and changes made
      while handling (a launch, a quit) wait for the next tick to be settled. Either drop the gate
      (settle every event) or document which changes must bump the version. Dropping it changes when some
      effects come, so it's a decision, not a cleanup.

## Waste

- [ ] `Settled.running` is rebuilt every session pass and never read, a leftover from `Dirty`.
      (`state.rs:373-375, 410`.)
- [ ] Image eviction runs only after a top frame and drops anything not drawn in that frame, so the
      bottom's images can be evicted while shown and decoded again. Older than the refactor. Least
      recently used across both screens. (`gfx/images.rs:155`, `render/mod.rs:104`.)
- [ ] Rare, older: the screens going off and the bottom going off in the same apply leaves the bottom lit.
      (`ui/platform/app.rs:122`, `sync.rs:233`.)

## Hygiene

- [ ] Areas reach further than the crate doc says: `dock.on`, `device.tap` and `keymapping.on` take all of
      `&mut Bottom` (they need the editors and the notice); `follow_units`/`follow_windows` take
      `&mut Shelf` only to record plays.
- [ ] One path each: notices (`bottom.notify` vs `*notice = Some(..)` in five places), the keymap setting
      (the Choose event vs `prefs.set`), the editor on the keyboard (`EditOnKeyboard` vs opening it and
      choosing the tab, which doesn't set `raised_from`).
- [ ] `apply_event` holds rules beside dispatch (Home's guards, level resolution, Quit); boolean
      parameters without names (`launch(id, None, true, ..)`, `activate(&id, false, ..)`);
      `Session::launch`'s nine parameters.
- [ ] Two names for one thing: game and entry; `shown` for three things; `follow_*` for settle steps and
      helpers. The crate doc still says rules mark screens (`lib.rs:29`).
- [ ] The UI applies rules core could state: `Effect::Screens` reads `bottom().off()` when carried out;
      which screens are hidden is decided from session state in the UI.
- [ ] The damaged-vs-whole test doesn't catch bounds 2px too tight on text: the scenarios don't cover
      anti-aliasing margins.
