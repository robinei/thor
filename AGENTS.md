# Working on thor

Linux for the AYN Thor, a two-screen handheld: ClamShell (`clamshell/`, the program that is the device's
session) and the system around it, shipped as pacman packages (`packages/`). The README describes the
crates, the library format, how to run and how to check; `docs/` holds the design notes and the device
plan (`docs/thor-gameplan.md`).

## Principles

- **For anyone.** Anyone should be able to install the system and keep it current with `pacman -Syu`
  from our repo. Setup goes into packages and first-boot setup, never into one machine's home directory
  or the image script; no personal presets (users, keys, time zones).
- **Simple over complete.** A mechanism has to earn its place. When a design starts collecting special
  cases and fallbacks, say so and offer the simpler alternative before building it.
- **No compatibility code.** Nobody runs ClamShell yet: change a file format or setting outright, with no
  migrations or legacy parsing.
- **Everything shows on a desktop.** Never hide UI because the machine lacks the hardware (battery,
  backlight, Wi-Fi); draw it dimmed with no reading, still tappable. Development happens on a desktop.
- **One implementation per UI element.** The same control or card in two places is one shared widget,
  not two variants.
- **The look:** black background, grey text (never full white on static elements), colour only with
  meaning (amber focus, green launch), no boxes or separators where alignment suffices, line icons.

## How ClamShell runs

```
input (touch, pointer, keys, pad) ─▶ Event ─┐
readings, ticks, windows, units ─▶ Event ───┤
                                            ▼
                        State::apply(event, &mut effects)      crates/state: handle, settle, sync
                                   │
                 ┌─────────────────┴──────────────────┐
                 ▼                                    ▼
      UI records each visible screen         effects ─▶ Handles::dispatch ─▶ a system thread
      compares with the last recording       (crates/system/src/wiring)       carries it out
      draws and damages only what changed                                     and reports back
      keeps the frame's targets for input                                     with Events
```

- **The UI** (crates/ui) owns `State`; the compositor (crates/compositor) hosts it and runs the event loop:
  input becomes events through the last frame's targets (each recording the item it showed), events from the
  system threads arrive on a channel, and every event goes through `State::apply`. Views are functions of
  `&State`; the canvas records commands, and a screen is drawn only where its recording changed.
- **The system crate** has a thread per concern (the launcher and systemd, the pad, Wi-Fi,
  Bluetooth, sound, the store, files, ...). Each takes its own commands on a channel (`os::worker`; a
  thread that polls devices is also woken by an eventfd bell) and sends events back through a `Sink`. No
  thread touches `State`. A thread that panics is started again, at once and then after growing waits,
  and the state sends it again what it had been sent (`crates/state/src/restart.rs`).
- **The compositor's thread** runs the event loop, the state and the drawing. It does no blocking work:
  files, process walks, D-Bus calls and waiting on the GPU belong to a thread. It draws only what
  changed.
- **The binary** (`clamshell/src`) starts the threads, builds `Handles`, and gives the compositor the
  closure that passes effects to it, so the UI and the system crate don't depend on each other. The
  compositor carries out only its own effects, `Effect::Compositor` (the screens' power and refresh rate,
  closing windows, keys sent to the window in front, screenshots, quitting), and passes every other
  effect on unchanged.
- So the cycle is event → apply → effects → threads → events. A new feature adds events and effects to
  the state crate, a rule there, and the thread or view that carries them out.

## The state (crates/state)

- `State` (lib.rs) holds the areas (shelf, session, bottom, radios, ...) and coordinates them, in one
  `impl State`. An area owns its fields privately, with its rules and the getters the UI reads.
- A rule that needs another area's data gets it as a parameter; a change another area alone owns is
  that area's `&mut self` method, passed as narrowly as practical. A request that needs several areas is
  queued as an `Event` (as the rule's last act) and handled by State after the rule returns.
- `apply` runs handle, settle, sync. Settle holds what depends on history, as an ordered list; sync
  diffs what the outside should have (`Wanted`) against what it was last sent. What can be computed from
  state is computed, not stored and kept in step.
- Outside code reads areas through `State`'s accessors and changes state only through events.
- No allocation on the per-event path or in drawing a frame (`crates/state/tests/apply_alloc.rs` and the
  UI's no-alloc test hold this). No function in the state crate returns a `Vec`: append to one passed in.

## State machines and their tests

Every feature's state is described in `docs/state-machines.md`: its states, its transitions
(`state --event/condition--> state [effects]`, including failures, timeouts, things vanishing and repeated
events), and how it interacts with other areas and by which route. A new feature, or a change to one,
updates that document in the same change, and:

- each transition and each interaction has a test that asserts its outcome (effects in order, and the
  resulting state), not one that merely passes through it;
- an order that matters is pinned by a test of the exact sequence, with a comment saying why it matters;
- invariants that must hold after every event go into the randomized event-sequence test, and the
  feature's events into its generator;
- a transition the state crate can't test (it needs the device, the compositor or a service) says in the
  document how it is checked instead: the compositor's tests, the nested compositor test, the VM suite,
  or on the device.

## Code

- No panics by construction: `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`,
  indexing and unchecked time subtraction are denied outside tests. A panic that happens anyway is logged
  at `error` by the panic hook; a system thread is started again, and a panic on the compositor's thread
  ends ClamShell, which systemd starts again.
- `unsafe` is denied. Where an FFI boundary needs it, it is allowed for that one block, with a `SAFETY:`
  comment.
- Don't crash unless there's no way on, and log every failure handled by falling back (`warn` when it
  shouldn't happen, `debug` when expected or repeating on a timer). A failure that changes what the user
  gets is never logged only at `debug`.
- Device facts live in the device profile (`crates/device`, `docs/device-profiles.md`; detect what can be
  detected first); code reads facts and never checks which device it runs on.
- Comments and docs are plain sentences, matter-of-fact; they say what and why, not what the code
  plainly says.

## The compositor (crates/compositor)

Rules for living with what apps do (`docs/compositor-mvp.md`, "Keeping the app tail small"):

- Enforce, never negotiate: a window is told its size and place; one that ignores them is scaled, and
  nothing waits on a client.
- A client's error ends that client, logged, never the session; Xwayland is restarted if it dies.
- X11 is translated into the same window facts as Wayland in one adapter; the state never sees X11.
- Each app bug fixed adds a case to the real-client tests.

Smithay comes from our fork, `github.com/robinei/smithay`, branch `thor`: an upstream commit with our
patches on top as a clean series, each written as it would be proposed upstream (Smithay's subject
style, a DCO sign-off). Updating means rebasing the series onto a newer upstream commit and
force-pushing, never merging. The commit `Cargo.toml` pins is tagged `thor-N` on the fork, so older
commits of this repository still build after a rebase.

## Checking

- `tools/check.sh` (fmt, clippy for debug, dev and release builds, all tests including the snapshots,
  rustdoc) runs as the pre-commit hook; every commit passes it. A plain `cargo test` in `clamshell/`
  runs only the binary's tests: use `--workspace` or the script.
- The snapshot test keeps three references in `clamshell/tests/`: the screens' pixel hashes
  (`snapshots.txt`), where the pad's focus goes on each (`focus.txt`), and the scenario's effects
  (`trace.txt`). All are written by `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`.
  On a difference, look at the current images (with the focus overlay) and the text diff. Update only
  if every difference follows from the intended change, and say what and why in the commit. A
  behaviour-preserving change (refactor, rename) leaves the trace untouched, so a trace diff there is a
  regression; an unexpected difference is a bug.
- Real clients in the compositor: `clamshell/tests/nested-compositor.py`. `tools/nested-session.sh` runs
  ClamShell as on the device, nested in a window on the desktop.
- The VM suite (`tools/vm/vm-test.sh`, about twenty minutes) is the exception, for what nothing else can
  check: the pad and real input devices, the session's files, D-Bus services. Say why when running it.
- In shell wait loops, anchor `pgrep -f` patterns (`'^qemu-system'`): an unanchored one matches the loop
  itself and never ends.

## Commits and publishing

Short imperative subjects scoped by area (`state/shelf: ...`), one concern per commit. Push, and publish
packages (`tools/repo/repo.sh publish`), only when asked. The device is maintained on the device
(`.claude/commands/maintain.md`), from the published packages.

During long work, give a one-line note after each commit, or after a long stretch of working without
one (what landed or is under way, what's next), and say so when a step is blocked.
