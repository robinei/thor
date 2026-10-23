# Architecture

How ClamShell is put together, and what keeps it honest. About 48,000 lines of Rust (state 16k, system
17k, ui 11k, the rest small) and 627 tests. The README says how to run and check it; `AGENTS.md` holds the
rules changes follow; `docs/state-machines.md` describes each feature's behaviour.

## The cycle

ClamShell is one process with one state, in the style of the Elm architecture:

```
input (touch, pointer, keys, pad) ─▶ Event ─┐
readings, ticks, windows, units ─▶ Event ───┤
                                            ▼
                        State::apply(event, &mut effects)          crates/state
                                   │
                 ┌─────────────────┴──────────────────┐
                 ▼                                    ▼
      the UI records each visible screen     effects ─▶ Handles::dispatch ─▶ a system thread,
      and draws only what changed            (crates/system/src/wiring)       which reports back
                                                                              with events
```

- **Model:** `State`. **Messages:** `Event`, both what happened outside and requests between parts of the
  state. **Update:** `State::apply`. **Commands:** `Effect`, carried out by the system threads, whose
  results come back as events. **View:** functions of `&State`.
- Nothing but `apply` changes the state, and the state does no I/O, so every behaviour can be tested by
  applying events and looking at the effects.

## The crates

| Crate | Role |
|---|---|
| `crates/state` | the state and its rules; events in, effects out; no I/O, no threads |
| `crates/system` | a thread per outside concern (sway, systemd and launches, the controller, Wi-Fi, Bluetooth, sound, storage, files, ...), each turning effects into actions and what it sees into events |
| `crates/ui` | the Wayland client: owns `State`, runs the event loop, draws the views, turns input into events |
| `crates/library` | the game library's model, sorting and parsing, without disk access |
| `crates/fan` | the fan daemon, a separate system service, and the small library with its mode protocol |
| `src/` | the program: starts the threads, connects effects to them, the command line and the control pipe |

The UI and the system crate don't depend on each other: the binary hands the UI a closure that passes each
effect to the system crate's dispatcher.

## The state

`State` (`crates/state/src/lib.rs`) is a coordinator over **areas**, each a type with private fields, its
rules and the getters the UI reads: `Shelf` (the game list), `Session` (launches, windows, background),
`Bottom` (the second screen, its tabs, editor and notices), `Radios`, `Levels`, `Sys` (status, battery,
time), `Device` (the device view, calibrations, account), `Idle`, `Prefs` (settings), `Keymapping`, the pad
route, the pointer and the lists' scrolls. One `impl State` holds what spans areas; nothing outside an
area touches its fields.

How areas cooperate:

- A rule that needs another area's data receives it as a parameter. A change another area alone owns is
  that area's method, passed narrowly (`&mut NoticeSlot`, `&mut Editors`), never the whole area.
- A request that needs several areas, such as playing an entry or changing tab, is queued as an `Event`
  and handled by `State` once the rule returns, so its effects follow those already pushed.
- Rules push into one output buffer per event (`Out`); no function returns a list.

`apply` runs in three phases:

1. **Handle** the event and the events it queued.
2. **Settle**: one ordered list of steps that bring the state up to date with what happened and depend on
   history: launches matched to systemd's scopes, windows to their entries, background pausing, quits in
   progress, the front window closing, the dock, the idle stage, the program in force, the pad's route.
3. **Sync**: what the outside should have now (`Wanted`: watches, screen power, the settings applied, the
   keymap, the pad route, files in use, ...) is computed from the settled state and compared with what it
   was last sent (`Sent`); only differences become effects, in one fixed order. Effects that are
   commands (launch, quit, save) come from the rules; effects that describe a condition come from sync.

## Windows

The session owns each window's home group, top or bottom placement, and the selected content on both
screens. Keyboard focus is separate and may name either screen's client or ClamShell's UI. Individual
lifecycle events update window facts; batches settle one backend dispatch atomically. Placement is
committed during `State::apply`, so the planned in-process compositor can read it before configuring
a new window.

The current sway executor converts tree snapshots into sparse lifecycle and presentation changes and
translates placement effects into IPC commands. Landing workspaces, marks, rules and retrying failed
placement belong to that adapter. A metadata change cannot restore an old placement. Foreground launch
intent and recent-group history remain session rules shared by both backends. The detailed transitions
and their tests are in `docs/state-machines.md`; the migration stages are in `docs/compositor-mvp.md`.

## The UI

- **Core and host.** `ui::Core` is the UI without a backend: the state and the event queue, input routed
  by screen, the timers as deadlines (the core says when it next needs to run), each screen recorded and
  drawn into a pixmap the host hands it, and the effects in order. `ui::platform` hosts it as a Wayland
  client: the surfaces and their shm buffers, the protocols, and the effects that are the UI's own (the
  screens' power, the refresh rate, quitting).
- **Drawing.** Views are functions of `&State` that draw on a canvas. The canvas records commands (fills,
  text, icons, images, clips) into a flat, reused buffer; after every event each visible screen is
  recorded and compared with its last recording. If nothing differs, nothing is drawn or committed. If
  something does, only the rectangle around the differing commands is redrawn: every command meeting it is
  played again, clipped to it, from the background up, so blending stays exact. Each shm buffer knows its
  age, so a reused one also gets what changed since it last held a frame. An idle tick costs a recording
  and a comparison, about 0.02 ms.
- **Input.** A frame keeps its targets (a rectangle, an id, and the key of the list item it showed). A
  touch is matched against the frame the user saw and carries that item's key, so a list that re-sorted
  meanwhile doesn't redirect it. Pad navigation moves between the same targets by direction.
- **Text** comes from a built-in font with fallbacks found through fontconfig; glyphs are rasterised once
  and cached.

## The system threads

Each thread takes its own commands on a channel (`os::worker`); a thread that waits on devices is also
woken by an eventfd. Services are reached over D-Bus (systemd, logind, timedated, UDisks2, NetworkManager,
BlueZ, UPower), sway over its IPC socket, PipeWire through `pw-dump` and `wpctl`, the controller through
evdev and uinput. Launches run in systemd scopes, so what runs is systemd's to say: a launch is running
while its scope is listed, and ClamShell can be restarted without losing track.

## Rules that hold everywhere

- **No panics by construction:** `unwrap`, `expect`, `panic!`, indexing and `unsafe` are denied outside
  tests.
- **No silent failures:** a failure handled by falling back is logged, at `warn` or, when expected or
  repeating, at `debug`.
- **No allocation on the hot paths:** applying a steady-state event and drawing a frame allocate nothing,
  enforced by tests with a counting allocator.
- **No compatibility code:** nobody depends on old formats yet, so formats change outright.

## What checks it

| Layer | What it catches | When it runs |
|---|---|---|
| clippy (debug, dev and release builds), rustfmt, rustdoc with warnings as errors | lint violations, the no-panic rules, broken doc links | every commit and CI |
| unit tests per area (most of the 627) | each transition and interaction in `docs/state-machines.md`, with effects asserted in order | every commit and CI |
| randomized event-sequence test | invariants after every event over thousands of generated events: sync has nothing left to send, the pad's route and the session consistent, lists within their ends, ... | every commit and CI; more seeds by hand |
| snapshot test: pixels, focus, trace | every screen's pixels; where the pad's focus goes on every screen, with reachability rules; the effects of scripted scenarios | every commit and CI |
| damaged frames against whole frames | partial redraws identical to full ones, pixel for pixel, across every scenario | every commit and CI |
| allocation tests | the event path and frame drawing allocate nothing | every commit and CI |
| `clamshell/tests/nested-sway.sh` | ClamShell in a real (headless) sway: launches, windows, the dock, restart | by hand, for sway behaviour |
| `tools/vm/vm-test.sh` (about 120 checks) | the whole device image in QEMU: a fake controller with gyro and rumble, simulated Wi-Fi and Bluetooth, sleep and wake, the session's files | by hand, for changes to devices, services and the session |

Every commit passes `tools/check.sh` (the first six rows) through the pre-commit hook, and CI runs it again
on GitHub.

## The device's system

ClamShell is one package of several (`packages/`): the kernel (`linux-thor`, built from a pinned commit
of the Armada kernel), the system configuration, the session (sway, the session's programs and their
settings), the audio profiles, the motion sensors' services, and a metapackage that is the whole system.
They are built on a PC from a checkout (ClamShell cross-compiled for aarch64), published as a signed
pacman repository on a GitHub release, and the device stays current with `pacman -Syu`. The SD card image
is assembled from the same packages with pacstrap, and the same system builds for x86_64 to boot in the
VM. Nothing personal goes into the image: no users, keys or time zones.
