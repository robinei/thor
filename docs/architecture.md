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
| `crates/system` | a thread per outside concern (systemd and launches, the controller, Wi-Fi, Bluetooth, sound, storage, files, ...), each turning effects into actions and what it sees into events |
| `crates/ui` | the UI without a backend (`ui::Core`): owns `State`, draws the views into pixmaps, turns input into events |
| `crates/compositor` | the Wayland compositor (Smithay): hosts the UI and runs the event loop; the screens through DRM and libinput (`--drm`) or a window on a desktop (`--nested`), the windows, Xwayland |
| `crates/device` | facts the hardware can't announce: the screens' connectors and rotation, the panels, the fan, the gyro |
| `crates/library` | the game library's model, sorting and parsing, without disk access |
| `crates/fan` | the fan daemon, a separate system service, and the small library with its mode protocol |
| `src/` | the program: starts the threads, connects effects to them, the command line and the control pipe |

The UI and the system crate don't depend on each other: the binary hands the compositor a closure that
passes each effect to the system crate's dispatcher.

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
   history: launches matched to systemd's units, entries started by their windows, the windows placed
   with their entries, the window rules, what is shown (a launch's first window, a window come back,
   what was before when the front window closed), background pausing, quits in progress, the dock, then
   the second screen's dimming, the idle stage, the program in force, the second screen's power and the
   pad's route. The list, with the test that pins each order that matters, is `State::settle`'s doc.
3. **Sync**: what the outside should have now (`Wanted`: watches, screen power, the settings applied, the
   keymap, the pad route, files in use, ...) is computed from the settled state and compared with what it
   was last sent (`Sent`); only differences become effects, in one fixed order. Effects that are
   commands (launch, quit, save) come from the rules; effects that describe a condition come from sync.

## Windows

The session holds the windows and their placement: each window's group (`Window.group`: an entry's, by
its key, or the window's own for a window of no entry), the one window the top screen shows and the one
placed on the bottom screen (each held once), each group's selected window, and the rules that send a
program's windows below (`session/windows.rs`, `session/below.rs`). The bottom screen's area (`Bottom`)
keeps its tabs and power and reads the window below; a window moved there opens its windows' tab.
Keyboard focus is separate and may name either screen's client or ClamShell's UI. The compositor reports
each window's life as events (opened, closed, changed, its title, keyboard focus, a client's request to
be shown); opened and changed carry only what the compositor knows of the window (`WindowFacts`), and
whether a new window comes in front is the state's to decide. Placing windows (focusing one, moving it
between groups, down to the second screen or back) is done during `State::apply`; the compositor reads
the result from the state after apply, so a new window is placed before it is first shown. The only
window effect is closing one (`Effect::Close`). Shelf is not a window: it is in front when no top window
is. A metadata change cannot restore an old placement. Foreground launch intent and recent-group history
are session rules. The detailed transitions and their tests are in `docs/state-machines.md`.

## The UI

- **Core and host.** `ui::Core` is the UI without a backend: the state and the event queue, input routed
  by screen, the timers as deadlines (the core says when it next needs to run), each screen recorded and
  drawn into a pixmap the host hands it, and the effects in order. The compositor hosts it: it gives it
  input by screen and the screens' sizes, runs it at its deadlines, shows each screen's pixmap, and
  carries out the effects that are its own (the screens' power, the refresh rate, closing windows,
  screenshots, quitting).
- **Drawing.** Views are functions of `&State` that draw on a canvas. The canvas records commands (fills,
  text, icons, images, clips) into a flat, reused buffer; after every event each visible screen is recorded
  and compared with its last recording. If nothing differs, nothing is drawn or committed. If something does,
  only the rectangle around the differing commands is redrawn: every command meeting it is played again,
  clipped to it, from the background up, so blending stays exact. Each pixmap knows the frame it holds, so a
  reused one also gets what changed since it last held a frame. An idle tick costs a recording and a
  comparison, about 0.02 ms.
- **Input.** A frame keeps its targets (a rectangle, an id, and the key of the list item it showed). A
  touch is matched against the frame the user saw and carries that item's key, so a list that re-sorted
  meanwhile doesn't redirect it. Pad navigation moves between the same targets by direction.
- **Text** comes from a built-in font with fallbacks found through fontconfig; glyphs are rasterised once
  and cached.

## The system threads

Each thread takes its own commands on a channel (`os::worker`); a thread that waits on devices is also
woken by an eventfd. Services are reached over D-Bus (systemd, logind, timedated, UDisks2, NetworkManager,
BlueZ, UPower), PipeWire through `pw-dump` and `wpctl`, the controller through evdev and uinput. Launches
are transient services of the user manager (`systemd-run --user`), each its own unit with its output in
the journal as the unit's, so what runs is systemd's to say: a launch is running while its unit is
listed, and ClamShell can be restarted without losing track.

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
| `clamshell/tests/nested-compositor.py` | ClamShell's compositor nested (in a headless sway as the host) with real clients: Wayland and X11 windows, Xwayland's restart, the clipboard, screenshots, moves between the screens | by hand, for compositor behaviour |
| `clamshell/tests/picture-scaling.py` | test games launched from shelf, their pictures filling the top screen at each Resolution and Scaling | by hand, for the picture |
| `tools/vm/vm-test.sh` (about 120 checks) | the whole device image in QEMU: a fake controller with gyro and rumble, simulated Wi-Fi and Bluetooth, sleep and wake, the session's files | by hand, for changes to devices, services and the session |

Every commit passes `tools/check.sh` (the first six rows) through the pre-commit hook, and CI runs it again
on GitHub.

## The device's system

ClamShell is one package of several (`packages/`): the kernel (`linux-thor`, built from a pinned commit of the
Armada kernel), the system configuration, the session (the autologin into ClamShell as a user service, the
session's programs and their settings), the audio profiles, the motion sensors' services, and a metapackage
that is the whole system. They are built on a PC from a checkout (ClamShell cross-compiled for aarch64),
published as a signed pacman repository on a GitHub release, and the device stays current with `pacman -Syu`.
The SD card image is assembled from the same packages with pacstrap, and the same system builds for x86_64 to
boot in the VM. Nothing personal goes into the image: no users, keys or time zones.
