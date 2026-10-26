# Architecture

How ClamShell is put together, and what keeps it honest. The README says how to run and check it; `AGENTS.md` holds the
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
- Two paths go beside the cycle, each for a reason:
  - **Identifying a client:** the compositor asks the system crate which process connected (its unit,
    and Steam's app id), on a thread per process, before it takes the client in, so that the client's
    first request is already answered as its game's (`identify`, `src/session.rs`).
  - **A screenshot's pixels** go from the compositor straight to the system's errands thread, which saves
    them; only the result comes back as an event. The pixels are too large to pass through the state.

## The crates

| Crate | Role |
|---|---|
| `crates/state` | the state and its rules; events in, effects out; no I/O, no threads |
| `crates/system` | a thread per outside concern (systemd and launches, the controller, Wi-Fi, Bluetooth, sound, storage, files, ...), each turning effects into actions and what it sees into events; the game library read from disk |
| `crates/ui` | the UI without a backend (`ui::Core`): owns `State`, draws the views into pixmaps on the CPU (tiny-skia, fontdue, the built-in Noto Sans), turns input into events |
| `crates/compositor` | the Wayland compositor (Smithay): hosts the UI and runs the event loop; the screens through DRM and libinput (`--drm`) or a window on a desktop (`--nested`), the windows, Xwayland |
| `crates/device` | facts the hardware can't announce: the screens' connectors and rotation, the panels, charging, the fan's curves (`fan.rs`), the gyro, the controls (`docs/device-profiles.md`) |
| `crates/library` | the game library's model (entries, groups, tabs), sorting, name patterns, and the parts of Steam's formats that need no disk |
| `crates/fan` | the fan daemon `clamshell-fan`, a system service setting the fan's speed from the temperatures by the curve of the mode the Fan setting picks, and the small library with its mode protocol (`clamshell_fan_mode`) |
| `src/` | the program: starts the threads, connects effects to them, the command line, the control pipe and the snapshots |

The UI and the system crate don't depend on each other: the binary hands the compositor a closure that
passes each effect to the system crate's dispatcher.

## The state

`State` (`crates/state/src/lib.rs`) is a coordinator over **areas**, each a type with private fields, its
rules and the getters the UI reads: `Shelf` (the game list), `Session` (launches, windows, background),
`Bottom` (the second screen, its tabs, editor and notices), `Radios`, `Levels`, `Sys` (status, battery,
time), `Setup` (the Device view: calibrations, account), `Idle`, `Prefs` (settings), `Keymapping`,
`Presentation` (what the compositor said of the screens), the pad route, the pointer and the lists'
scrolls. `State`'s impl holds what spans areas, in files by subject under `apply/` (launching, presses,
tabs, sliders, the settings tab, ...); nothing outside an area touches its fields.

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
window effect is closing one (`Compositor::Close`). Shelf is not a window: it is in front when no top window
is. A metadata change cannot restore an old placement. Foreground launch intent and recent-group history
are session rules. The detailed transitions and their tests are in `docs/state-machines.md`.

## Living with what apps do

Apps will do odd things with their windows, Steam and Proton above all. The tail can't be avoided, but
each oddity should be harmless to the session, found by us before the user meets it, fixed in one place,
and fixed once. Seven rules:

1. **Enforce, never negotiate.** The compositor tells a window its size and doesn't wait for agreement:
   a window that ignores it is scaled, positions it asks for are ignored, there is no stacking to request,
   and nothing waits on a client's acknowledgement. A misbehaving client can only make its own window look
   wrong, never placement, the other screen or the session.
2. **A client's error stays the client's.** Anything a client sends that we can't handle ends that
   client's connection, logged, never a crash. Xwayland is a supervised child: if it dies it is restarted;
   the X11 apps go, the session and the Wayland games stay.
3. **X11's oddities in one place.** The X11 adapter is a translation from a window's X11 properties to the
   same window facts the Wayland side produces (role, parent, size limits, title, identity, shown or not);
   the state never sees X11. An app quirk is a fact derived wrongly, fixed in that translation, and tested
   with the recorded properties as input and the expected facts as output, without running the app.
4. **The user can always get out.** Home goes through our pad router before any client sees it: Home
   returns to the shelf, Home+B closes the window, held it ends the program, whatever the window
   does. The worst case of an app bug is pressing Home.
5. **Bugs fixed once stay fixed.** A set of real clients in the headless harness (a Wayland app, an X11
   app, a fixed-size dialog, a window ignoring its size, one that hides and reappears, a menu), growing by
   a case with each app bug fixed. On the device, a fixed-size ring of the last few hundred window events,
   dumped by a control command, replays as a state test; the maintenance agent can attach a dump to a
   report.
6. **Defaults that avoid the hard paths:** games through Wine's Wayland driver, so most game traffic never
   reaches Xwayland, and Steam in Big Picture mode (`-gamepadui`): one fullscreen controller-driven window
   instead of the desktop client's many windows and menus.
7. **Not yet built:** a randomized client that maps, unmaps, resizes, asks for fullscreen and opens
   menus, with the session's invariants checked after every step, as the randomized state test does one
   layer up.

## The UI

- **Core and host.** `ui::Core` is the UI without a backend: the state and the event queue, input routed
  by screen, the timers as deadlines (the core says when it next needs to run), each screen recorded and
  drawn into a pixmap the host hands it, and the effects in order. Over each screen it records and draws
  another layer the same way, its overlay (over the top screen's window the HUD and the window arrows,
  over the bottom screen the notifications, with their targets); the host shows the part of it that
  draws, over a window or ClamShell's view. The compositor hosts it: it gives it
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
woken by an eventfd. A thread that panics runs again on the same channel, at once and then after waits
that double up to 5 s, and the state is told (`Event::Restarted`) so it sends again what the thread
held. Services are reached over D-Bus (systemd, logind, timedated, UDisks2, NetworkManager,
BlueZ, UPower), PipeWire through `pw-dump` and `wpctl`, the controller through evdev and uinput. Launches
are transient services of the user manager (`systemd-run --user`), each its own unit with its output in
the journal as the unit's, so what runs is systemd's to say: a launch is running while its unit is
listed, and ClamShell can be restarted without losing track.

## Rules that hold everywhere

- **No panics by construction:** `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`,
  `unimplemented!`, indexing and unchecked time subtraction are denied outside tests. A panic that happens
  anyway is logged at `error`; a system thread is started again and sent again what it lost
  (`state/src/apply/restart.rs`), unless it keeps panicking soon after starting, when it is given up; a panic on the compositor's thread ends ClamShell, which systemd starts again.
- **`unsafe` denied:** allowed only for a block at an FFI boundary, with a `SAFETY:` comment.
- **The compositor's thread doesn't block:** files, process walks, D-Bus calls and waiting on the GPU
  belong to a thread; drawing is bounded by what changed.
- **One place per effect:** the compositor carries out `Effect::Compositor` and passes every other effect
  on unchanged to the system's dispatcher.
- **No silent failures:** a failure handled by falling back is logged, at `warn` or, when expected or
  repeating, at `debug`.
- **No allocation on the hot paths:** applying a steady-state event and drawing a frame allocate nothing,
  enforced by tests with a counting allocator.
- **No compatibility code:** nobody depends on old formats yet, so formats change outright.

## What checks it

| Layer | What it catches | When it runs |
|---|---|---|
| clippy (debug, dev and release builds), rustfmt, rustdoc with warnings as errors | lint violations, the no-panic rules, broken doc links | every commit and CI |
| unit tests per area | each transition and interaction in `docs/state-machines.md`, with effects asserted in order | every commit and CI |
| randomized event-sequence test | invariants after every event over thousands of generated events: sync has nothing left to send, the pad's route and the session consistent, lists within their ends, ... | every commit and CI; more seeds by hand |
| snapshot test: pixels, focus, trace | every screen's pixels; where the pad's focus goes on every screen, with reachability rules; the effects of scripted scenarios | every commit and CI |
| damaged frames against whole frames | partial redraws identical to full ones, pixel for pixel, across every scenario | every commit and CI |
| allocation tests | the event path and frame drawing allocate nothing | every commit and CI |
| `clamshell/tests/nested-compositor.py` | ClamShell's compositor nested (in a headless sway as the host) with real clients: Wayland and X11 windows, Xwayland's restart, the clipboard, screenshots, moves between the screens | by hand, for compositor behaviour |
| `clamshell/tests/picture-scaling.py` (`--gpu` for the Wine games) | test games launched from shelf, their pictures filling the top screen at each Resolution and Scaling | by hand, for the picture |
| `tools/vm/vm-test.sh` | the whole device image in QEMU: a fake controller with gyro and rumble, simulated Wi-Fi and Bluetooth, sleep and wake, the session's files | by hand, for changes to devices, services and the session |

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
