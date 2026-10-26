# Source organisation plan

From a review of how ClamShell's source is laid out (2026-10-22): reusable machinery sat next to concrete
features, core was one flat directory of 51 files with a 1450-line `lib.rs`, and several files held two or
three unrelated things. Each step below is a commit (or a few) that passes `tools/check.sh` and changes no
behaviour unless it says so; the snapshot hashes stay the same throughout.

The rule each crate follows afterwards: generic machinery in a directory of its own that knows nothing of
the features (`kit/` in core, `os/` in system, `gfx/` in ui), and each feature's model and rules together
in a directory named for the area a reader would look under.

## Core

```
core/src/
  lib.rs        crate doc, mods, re-exports, Init, State, new, getters
  apply.rs      event dispatch and the after-every-event order
  event.rs effect.rs id.rs resume.rs testkit.rs tests.rs
  kit/          names!, list (Scroll, Rows, ItemKey), Debounce, Secret, testdir, replace
  session/      running, program, instance, windows, found, tray, background, autostart, enqueue, dock,
                front (Home, a closed window's successor)
  shelf/        mod.rs (the Shelf model), rules, actions, files
  bottom/       Tab, Bottom, screen, notice, editor, keyboard, touch
  nav/          pointer, items (presses on list items), lists (ListId, Lists), pad
  keymap/       as now, with keymap_screens nested as keymap/screens
  levels/       Levels and their rules, slider
  radios/       Radios, wifi, bluetooth
  settings/     table, env (parsing, from editor), panel, the fan Mode
  device/       view, account, sticks, buttons (was hardware), idle
  status/       Sys, metrics, battery, time
```

- [x] Clock readers (`unix_secs`, `mono_secs`) out of core (I/O in a pure crate) into system's `clock`.
- [x] Fan curve (the daemon's algorithm) into crates/fan; core keeps the Mode choice and `MODE_FILE`.
- [x] `lib.rs`: tests and testkit out; event dispatch into `apply.rs`; each area's rules into its area.
- [x] `kit/`; `scroll.rs` and `items.rs` split into their generic and feature halves.
- [x] One commit per area directory, as in the tree.
- [x] Every screen's ids in `id.rs` (`TimeId`, `DeviceId`, `BatteryId` were in feature files).
- [x] The crate doc describes the tree; the root `pub use` grouped by area.

## System

```
system/src/
  lib.rs paths.rs store.rs device.rs
  os/        worker, command, dbus, sys, cgroup, drm, qrtr, evdev/, fs, text, http, xdg
  wiring/    mod.rs (start, from the binary's wiring.rs), handles, settings (the Tuning router)
  input/     watch, keyboard, pad/ (sensor_hub/{mod, wire}), touchpad/
  launch/    launcher, steam, job, units, freezer, mpv
  library/   as now, plus meta/{mod, libretro, tmdb, steam, steamgriddb, probe, titles, cache}
  files/     browse, ops, trash, mounts, archives
  audio/     mixer, sound, pipewire
  display/   brightness, screenshot, refresh
  power/     tuning, sleep, lid, logind
  time/      clock, wake, timedated
  readings/  status, metrics, battery, usage, frames, mangohud
  network/   wifi/, bluetooth/
  session/   tray, clipboard, account, notify, icons
  sway/      as now
  waydroid/  filter, wire
```

`os/` uses nothing from `clamshell_core` or the rest of the crate.

- [x] `os/` from the flat infrastructure files, `pad/qrtr` and `input/{mod, device}` (as `os/evdev`).
- [x] Duplicates merged:
  - percent encoding and decoding (in files and meta) and `sys::unescape` into `os/text`;
  - the hand-written date conversion (`device::civil`) replaced by jiff, already a dependency;
  - MD5 (mpv) and CRC32 (meta) into `clamshell_library::hash`;
  - the two cgroup CPU readers into `os/cgroup`;
  - logind's names: suspending moved into `power/sleep`, which had them (no module of its own needed);
  - icon-theme lookup and `desktop::data_dirs` into `os/xdg` (both XDG specs; the tray keeps its own
    theme-path rule);
  - MangoHud's format (config in tuning, reader in frames, writer in wayland_filter) into `readings/mangohud`.
- [x] The files that hold several things split:
  - `meta.rs` into `library/meta/*`;
  - `files.rs`, `device.rs`, `sleep.rs`, `units.rs` (scopes, freezer), `launcher.rs` (Steam following),
    `battery.rs` (usage sampling), `tuning.rs` (hardware, MangoHud, the router);
  - `sensor_hub.rs` (wire codec);
  - `wayland_filter.rs` into `waydroid/{filter, wire}`;
  - `store.rs`, whose metadata cache goes to `library/meta/cache`.
- [x] The area directories, as in the tree.
- [x] Thread start-up (`src/wiring.rs`) into `wiring/`, next to the routing of effects to those threads.
  The binary keeps the UI sink and the control pipe. Modules no other crate uses become `pub(crate)`
  (53 of 75).

## UI

```
ui/src/
  lib.rs        crate doc, run, snapshot, Config, EventSender, Error, Screen
  screens.rs
  gfx/          geom, color, canvas, text, fonts, icons (drawing), images; no clamshell_core
  toolkit/      Frame, style, widgets, scroll, format, clock (LocalTime), icons (which icon for a tab etc.)
  input/        press (was input.rs), fling, focus, nav
  render/       render, frame events and list changes (from lib.rs), no_alloc
  platform/     app (App and the event loop), routing (input to App), handlers (was wayland.rs),
                view, display, text_input, notify
  views/        as now
```

- [x] `draw.rs` split into `gfx/{geom, color, canvas, text, fonts}`.
- [x] `icons.rs`: drawing to `gfx/icons`, the choices typed by core to `toolkit/icons`; `images` to `gfx`.
- [x] `input/`, `render/`, `platform/`; `lib.rs` down to the crate's API.
- [x] `LocalTime` and `Scratch` out of `toolkit/mod.rs` into `toolkit/clock.rs`.

## Library crate

- [x] `lib.rs` split into `entry`, `runner`, `meta`, `library`.
- [x] Pure title parsing (`titles`: `season_of`, `episode_title`, `stem`, ...), `sort::list_order` and
  `Library::first_frame` from system's library scanning; the scanning (disk, mounts, the store) stays in
  system, as Steam's already does.

## Binary

- [x] `control.rs` into `control/{mod (grammar), words (tables), pipe (the server)}`.
- [x] `main.rs`: session start into `session.rs` (with the UI sink from `wiring.rs`), the one-shot
  printers into `commands.rs`; `snapshot.rs` and `demo.rs` under `dev/`.

## Repository

- [x] The session's configs (mako, foot, mpv, retroarch) from `clamshell/` into
  `packages/thor-session/root/` at their installed paths; ClamShell's own (sway's config, android helpers)
  stay. The packages' file lists stay the same.

## Not done

- Separate crates for `gfx/` or `os/`: a directory keeps the boundary for now; a crate would make every
  item public, and `os/` would hit the orphan rule over `lost_on!`.
- Views grouped into top/bottom directories; `Fmt` into `gfx`: path churn for little.
- Splitting `running.rs`, moving `Calibrator` to system, the shared poll loop, the pad dispatch out of
  `App`: changes of code, not of place; for another time if they keep growing.
