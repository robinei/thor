# Plan, 2026-10-28: State as facts, the rest derived

From four read-only reviews of the state crate (the session; shelf, the bottom screen, the pad and
pointer; levels, radios, status, idle, presentation, setup and settings; and `apply`, `settle` and
`sync`), after `history/window-model-plan.md`.

## The aim

State holds facts: what the outside reported, what the user chose, and records of the past that decisions
need. A fact can be stale or missing, but not wrong. Everything else is derived: a function of the facts,
or recomputed whenever they change, so that it can only be wrong by a bug in its derivation, never by a
change that forgot to update it. The areas that kept producing bugs should become obviously correct.

And it stays cheap. Measured on the desktop (release, the random test's stream): `apply` takes 2.4 µs at
the median and 7.2 µs at p99, and a second settle with nothing to do 1.8 µs; settle is already about
three quarters of apply and already walks everything each event. Deriving instead of storing adds no
walks. Every step below says what it costs; none may allocate per event (`tests/apply_alloc.rs`).

## Where it stands

Much is already fact-shaped: the settings in force, idle's stage, the pad's route, the dock's running
items, the folder selection, the lists' scrolls are reconciled each settle; `sync` sends the difference
between what the state wants and what was sent; keymaps' versions are kept by their only mutators.

What isn't, and where the bugs were:

- **Who owns a window, and its group.** Ownership has several definitions (`running_of` walks to the
  root and falls back on the stored group; finding a program goes by the window's own process;
  `start_found` by the root), and the group is both derived from the owner and read to find it. Three of
  the latest bugs, and a live one: a found program adopted again on every event (below).
- **Stored recomputes:** the pad's route, the bottom screen's dimming and idle-off, shelf's selected
  entry beside its cursor: each kept current by a settle step or by its callers.
- **Edges kept by callers:** shelf's `moved()` (12 callers) and its `unsaved` flags, the tab left
  (4 immediate calls and a deferred one), `remember_front` called after some commands and events.
- **Readings edited in place:** the volume and mute held while a finger is on them (`Held`, `holding`,
  released from three places), stream mute and output written into the audio reading, Wi-Fi's and
  Bluetooth's `busy` and `error` overwritten by our attempt (`Local`; two past fixes). The fact is "we
  sent this at time t"; what to show is derived from it and the readings.
- **`Sent` beyond "what was sent":** `tab_again`, a send-again flag modelling one tab slot where the
  outside has a watch per reader; "ClamShell has touched the backlights" kept in `Sent` and erased by a
  restart. Freeze and runner pause sent as edges from settle, not as a wanted condition, which needs
  `freeze_again` on restart and half of a random invariant to check.
- **Settle doesn't reach a fixed point.** Settling a second time after an event changes the state in
  three ways (verified over 150 seeds): the adoption loop, a found program picked up an event late
  (`end_windowless` runs after `follow_found_windows`), and a background grace timer started an event late.

## Bugs found, fixed first

- [x] **A found program adopted on every event.** A window of another program over a starting launch's
  window: it is found by its own process, but `running_of` walks to the root, which it doesn't own, so the
  entry has no windows and ends in the same settle; the next settle finds it again, each time asking for a
  new scope. Predates today's changes.
- [x] **The screens can stay dimmed after waking.** A backlight reading taken while dimmed that arrives
  after the wake is taken as the brightness on a device where the user hasn't set one, and each dim
  compounds it (the failure 65ec9f62 fixed, still open as a race). Fixed by step 5's readings tagged with
  the command they have seen: a reading from before the wake is older than the wake's command.
- [x] **A launcher's dialog opens behind on three paths.** `FrontLaunch.launcher` is set by a second call
  on two of the five ways a launch starts waiting (not the re-wait after the window in front closed, nor
  a found or tray program's), so Steam's "which version" dialog opens behind there. Fix: derive it.

## Steps

Each ends in commits that pass `tools/check.sh` and the randomized test at 3000 seeds.

- [x] 1. **Settle converges, checked.** The randomized test settles and syncs a second time after each
  event (every Nth, to keep it quick) and requires no effects, no queued events and no change. The scratch
  buffers (`queue`, `plays`, `with_windows`) move into one `Scratch` left out of the comparison. Also: no
  Adopt or FindIcon repeated for an entry. This is the net for every ordered-correction bug from here on.
- [x] 2. **One owner function from facts.** `owner(w)`: the root's program by mark, unit or process, never
  by a stored group. A window's group is its owner's, else a claim, else its own; the claims (a file's
  window taken by the entry waiting for it, a service dialog given to the program in front) are stored as
  history on the window. Finding programs goes by roots. `FrontLaunch.launcher` derived. Cost: an owner
  and root table built once per settle into a reused buffer when the windows or programs changed (a
  generation counter, as keymaps' versions), replacing today's O(programs × windows²) of repeated
  `running_of`/`root_of`; `Running` holds its `Group` so that comparing doesn't allocate. The library mark
  lookup per window (`start_by_app_id`, O(windows × entries) each settle) cached on the window when its
  facts or the library change.
  Done for the owner function and finding by roots. The owner and root table and the mark cache weren't
  built: `running_of` still walks to the root on each call, which profiling hasn't asked to change.
- [x] 3. **Stored recomputes become getters.** `Pad::route(..)`, `Bottom::dimmed(..)`, the bottom
  screen's idle-off, shelf's selected entry from its cursor. Each drops a settle step, its writers and a
  random invariant that only checked the copy. Cost: O(1) each; the settings in force computed once per
  settle rather than two or three times.
  Done but for the pad's route: it is recomputed each settle and checked to converge, and its change is
  the focus ring's edge (hidden as the pad leaves ClamShell's screens), so it stays as recorded.
- [x] 4. **Edges from recorded values.** One `Followed` record written at the end of settle: the tab, shelf's
  place, the program, the route, the front. A step compares the derived value with it: the tab left
  (replacing `Bottom.left` and the four immediate calls), shelf moved (replacing `moved()` and its 12
  callers), the program's pad mouse, the focus ring hidden, the front remembered once per apply. Resume
  saved when a hash of the place differs from the last saved, replacing both `unsaved` flags. Cost: a few
  compares and a hash over a few dozen ids, no allocation.
  Done for the tab left and shelf's moves and saves. The program's pad mouse and the focus ring's hiding
  were already edges found in settle; the front's history stays recorded by the window commands, a front
  shown within an event being part of that history.
- [x] 5. **Commands as facts, readings kept as received.** Three kinds of value:
  - **Only ClamShell changes it:** the wanted value is stored (the user's choice), `wanted()` derives the
    rest, and sync sends the difference from what was sent. No readings needed.
  - **Others change it too:** volume and mute (PipeWire restores each output's own on a switch, a
    headset's buttons, other apps) and brightness (another tool, a desktop session, the firmware). Each
    command carries a number, and the thread applying commands reads the value back with the number of the
    last command it applied. What is shown is our latest command until a reading has seen it, then the
    reading; a reading that has seen our latest command and differs from it is a change from outside,
    taken as the user's value (for brightness: not while ClamShell has dimmed the screens, whose level is
    ours). No timing, and a reading from before a command can't win over it. This replaces `Held`,
    `holding` and its three releases, the edits to the audio reading, and the brightness reading filtered
    by the idle stage; the volume reading moves from the status thread to the mixer thread, which applies
    the commands.
  - **An attempt with an outcome:** Wi-Fi and Bluetooth connections. The attempt is stored as a fact;
    busy and error are derived from it and the latest reading, and a reading that answers it (the target
    busy, connected, or a new error) clears it. The timeout stays, the outcome possibly never coming. This
    replaces `Local`, `give_up` and their follow, and the edits to the readings.

  One slide gesture on State instead of four `slider` fields. Cost: a compare per value, no allocation.

  Done for the sound (numbered commands, the sound thread publishing the last carried out), Wi-Fi and
  Bluetooth (attempts as facts). Brightness came out simpler: the reading stands for the level only until
  ClamShell first sets the backlights, after which ClamShell's level is the brightness; a change from
  outside isn't taken as the user's, since the reading is the first backlight's level times its share and
  can't be told from ClamShell's own. The four sliders stay: each module also ends its slide when its
  screen closes or an edit is undone, which a single gesture on State would have to take over.
- [x] 6. **`Sent` only what was sent.** A watch's state per watch (on, off, unknown), a restart setting
  what its thread held to unknown (replacing `tab`, `tab_again`, `status_off`, `account`); freezing and
  runner pauses wanted as a condition and diffed in sync (replacing `freeze_again`); the backlights
  touched as a fact in Levels.
  Done for the watches and the backlights touched (a fact in `Sent`, kept through a restart). Freezing
  stays an effect of the background rules: a program is thawed before its windows are closed in the
  same event, which sync, running after the event, would send too late.
- [x] 7. **Shelf's library changes through one place.** One `library_changed` doing what `relayout`,
  `listed` and `reload` each repeat by hand (close gone groups, find again, select again by id, prune the
  file selection); `reload` replacing only the library and places rather than rebuilding and copying
  fields back.

Cost fixes on the way (no behaviour change):
- [x] The settings in force (`in_force`, `settings_of`) scan the whole library per call: 2 to 3 times per
  event in the state, and about 7 times per window per event in the compositor. Keep the runner's key on
  the running entry; look entries up by an index map built when the library changes.
  Done by the entry a running program started with, which `Running` already keeps (its runner is the one
  it started with). The lookups left search the library per action, not per event; an index would need
  `Library.entries` made private, which tests add to directly.
- [x] `window_presentation` allocates a list of sizes per call (twice per window per event, from the
  compositor): an iterator instead.
  Done by passing on only the window's fixed size when the screen offers it, the one thing it is checked
  against.
- [x] Typing a shelf filter allocates a lowercased copy of every title per keystroke: a case-insensitive
  compare instead.

## Afterwards: a review of the session's order

A read-only review of `follow_windows` and settle's session steps found their order to be a real data
flow (a file's claim before finding programs, ended entries before finding again, finding before the
claim for the program in front, placing last), with no step out of order. Folding them into one ownership
pass would turn the sequence into a precedence list without removing a dependency, so it wasn't done.
It found three bugs, fixed: a browsed file's window that opened before its launch ended wasn't taken (a
window's open time is now kept, and the file takes those opened since it began); a window of no program
never joined the group in front, and while a launch waited was taken and brought in front over it; and a
program to find whose entry already runs was built and dropped at every settle. The random test now
checks settle's convergence after every event for one seed in eight.

Left: `claim_unowned` and finding by tray icons still act on what changed (`opened`, the tray list), which
the convergence check can't see. No sequence that goes wrong is known.
