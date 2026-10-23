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

- **The UI** (crates/ui) owns `State` and runs the event loop: Wayland input becomes events through the
  last frame's targets (each recording the item it showed), events from the system threads arrive on a
  channel, and every event goes through `State::apply`. Views are functions of `&State`; the canvas
  records commands, and a screen is drawn only where its recording changed.
- **The system crate** has a thread per concern (sway, the launcher and systemd, the pad, Wi-Fi,
  Bluetooth, sound, the store, files, ...). Each takes its own commands on a channel (`os::worker`; a
  thread that polls devices is also woken by an eventfd bell) and sends events back through a `Sink`. No
  thread touches `State`.
- **The binary** (`clamshell/src`) starts the threads, builds `Handles`, and gives the UI the closure that
  passes effects to it, so the UI and the system crate don't depend on each other. The UI carries out
  only what is its own: the screens' power and refresh rate over Wayland, and quitting.
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
- a transition the state crate can't test (it needs the device, sway or a service) says in the document
  how it is checked instead: the nested-sway test, the VM suite, or on the device.

## Code

- No panics by construction: `unwrap`, `expect`, `panic!` and indexing are denied outside tests, and
  `unsafe` is denied. Don't crash unless there's no way on, and log a failure handled by falling back
  (`warn` when it shouldn't happen, `debug` when expected or repeating on a timer).
- Comments and docs are plain sentences, matter-of-fact; they say what and why, not what the code
  plainly says.

## Checking

- `tools/check.sh` (fmt, clippy for debug, dev and release builds, all tests including the snapshots,
  rustdoc) runs as the pre-commit hook; every commit passes it. A plain `cargo test` in `clamshell/`
  runs only the binary's tests: use `--workspace` or the script.
- A change meant to change what is drawn: look at the snapshots (README), then update their hashes.
- Sway behaviour: `clamshell/tests/nested-sway.sh`. `tools/nested-session.sh` runs ClamShell as on the
  device, in a nested sway on the desktop.
- The VM suite (`tools/vm/vm-test.sh`, about twenty minutes) is the exception, for what nothing else can
  check: the pad and real input devices, the session's files, D-Bus services. Say why when running it.
- In shell wait loops, anchor `pgrep -f` patterns (`'^qemu-system'`): an unanchored one matches the loop
  itself and never ends.

## Commits and publishing

Short imperative subjects scoped by area (`state/shelf: ...`), one concern per commit. Push, and publish
packages (`tools/repo/repo.sh publish`), only when asked. The device is maintained on the device
(`.claude/commands/maintain.md`), from the published packages.
