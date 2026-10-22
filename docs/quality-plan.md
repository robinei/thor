# ClamShell code quality plan

From three code quality reviews of ClamShell on 2026-10-22 (one per area: `crates/core`; `crates/system`;
`crates/ui`, `crates/library` and `src` with the repository around them), asked to look for what a critic
of AI-written projects would point at, and what a senior Rust reviewer would flag. Their verdict: the
design holds (a pure core, a shared worker and supervisor, panics ruled out by lints, minimal sound
`unsafe`, meaningful tests); what reads badly is the residue of fast iteration without consolidation
passes, and some signals around the code.

Each item names where it is. Checked off as it lands, one commit per item or small group. Left out by
decision: CI (GitHub Actions) for now; the Upscale setting stays as it is.

## 1. Stale docs, dead code, bypassed helpers

Comments that describe what is no longer so:
- [x] Keymaps: "two layers" and "the Back layer / main layer" (now Base and named layers):
      `keymap/mod.rs:13-14,478`, `id.rs:192`, `keymap_screens/params.rs:17,41`,
      `keymap_screens/mod.rs:662`, `keymap_screens/tests.rs:504-508`.
- [x] The keymap file's old text format in comments: `param.rs:32`, `keymap/mod.rs:169,494`,
      `keymap/inputs.rs:121,139`.
- [x] The removed Setup tab: `core/resume.rs:65-66`, `ui/views/settings.rs:7`.
- [x] `core/sys.rs:1-3` (touchpad mode field), `core/editor.rs:20` (`Editor::tab`), `core/windows.rs:1-7`
      (the old dock instance / games split).
- [x] System comments that contradict their code: `sway/mod.rs:262` (`lineages`), `pad/mod.rs:113` (the
      retry), `pad/motion.rs:2-7`, `pad/calibrate.rs:17-20` (`SAMPLE_EVERY`), `pad/route.rs:80` (`Ranges`).
- [x] UI: `draw.rs:58` (a doc above the wrong function; `shifted` undocumented), `views/dock.rs:1-8,
      25-29,112-116` (the menu's items; `ART_TOP`, `ART_ROOM`), `src/main.rs:3` ("test pipe"),
      `config.rs` (`CLAMSHELL_START_MODE` sets a tab).
- [x] Doc comments on the wrong item: `core/lib.rs:307-309` (`game`), `core/scroll.rs` (`RowsOf`),
      `core/settings.rs:285` (`background`).
- [x] Lines spliced in without rewrapping, mid-sentence breaks: `core/found.rs:4`, `tray.rs:8`, `pad.rs:3`,
      `dock.rs:5`, `running.rs:20`, `instance.rs:3`, `lib.rs:999`, `keymap/mod.rs:6`, `actions.rs:3`,
      `keyboard.rs:102`; the ~24 long comment lines in ui and the 4 over 170 characters in system.

Dead or vestigial code:
- [x] The keymap text format kept alive by its tests: `Chord::write`, `Chord::parse`, `Small`, `named_name`,
      `parse_named` (`keymap/outputs.rs:27-178`); `Param`'s and `Actuation`'s unused `name`/`from_name`
      (a label-only form of `names!`); `parse` moved to a test helper.
- [x] `pad::Ack`'s always-true `PartialEq` and `Command`'s derives nothing uses (`pad/mod.rs:81-90`).
- [x] `Remap::get`'s ignored `&PadSnapshot` through its callers; `rest()` (`pad/remap/mod.rs:396,463`).
- [x] `SensorHub::turn` (always `THOR_TURN`); `request()`'s `io::Result` that can't be `Err`
      (`pad/sensor_hub.rs:69,92`).
- [x] sway's `DownRules`/`LandRules` dispatched in two places (`sway/mod.rs:197-222,244`).
- [x] Test-only API answering differently from the real one: `Settings::of`, `Settings::env_of`
      (`core/settings.rs:366,420`).
- [x] `Rect::intersect` unused beside a free `intersect` (`ui/draw.rs:52`, `toolkit/scroll.rs:102`).
- [x] 22 `pub` items used only in their own file (system); private.
- [x] Small oddities: two consecutive `if init.setup` (`core/lib.rs:239-245`), `time_back()`'s unused
      bool, `Session::new_name`, `Levels::leave` = `release`, split `impl` blocks (`idle.rs`, library
      `lib.rs:365,371`), `impl ListEvent` before its enum, `is_back_key`, `decide_layer_holds`' double
      match, `crc32`'s table rebuilt per call.

Helpers that exist but are bypassed:
- [x] cgroup parsing by hand: `launcher.rs:207,250,316,443`, `device.rs:240` → `cgroup::unit_of`.
- [x] Atomic writes reimplemented: `meta.rs:531-535,551-552` → `store::write_file` (`mpv.rs` keeps its rename:
      ffmpeg writes the file).
- [x] `/proc/<pid>/stat` parsed three times (`sys::start_time`, `sys::lineage`, `launcher::parents`) →
      `sys::parent`.
- [x] Subprocesses without the time limit `command.rs` exists for: `files.rs:421,426`,
      `archives.rs:39,122,128`, `device.rs:46,281`, `account.rs:62`; `device.rs:63`'s `timeout` binary;
      `launcher::end_processes`' `kill` subprocess → rustix.
- [x] FNV-1a written five times (`library/lib.rs:301,307`, `snapshot.rs:560`, `system/archives.rs:138`,
      `system/sway/commands.rs:84`) → one helper.
- [x] JSON escaped by hand (`core/enqueue.rs:68-80`) → `serde_json`.
- [x] `/proc/self/mounts` parsed three times (`files.rs:301,606`, `archives.rs:111`), one without
      unescaping.

## 2. Bugs

- [x] Durations timed by the wall clock, which the Time tab steps: bottom screen dim/off
      (`bottom_screen.rs:135,174`), background `GRACE` (`background.rs:123,171`), `HANDOFF`
      (`running.rs:245-260,311,328`), `REOPEN_SECS`, tray/found timing → `mono_secs`.
- [x] A missing or empty library shows the demo games (`src/main.rs` `load_library`): an empty shelf with a
      hint; the demo only for `--snapshot` and the desktop's demo mode, out of the library crate's
      non-test code.
- [x] `held.dedup()` without sorting (`pad/route.rs:301`).
- [x] `mounted_under` doesn't unescape `/proc/self/mounts` (`archives.rs:111`).
- [x] The TMDB key in curl's command line, visible in `/proc` (`meta.rs:342`) → `curl -K -`.
- [x] `command.rs:50`: `reader.join()` can hang on a grandchild holding stdout.
- [x] A test that checks only its first value (`core/settings.rs:458-462`).
- [x] The Wi-Fi password and the editor's text not `Secret` (`core/wifi.rs:123`, `editor.rs`).
- [x] Undo: a step popped then filtered by id is lost (`keymap_screens/undo.rs`).
- [x] Keymap id allocation at `u16::MAX` returns an existing id (`keymap/mod.rs:544,584`).
- [x] `relay` treats EINTR as a close; `wayland_filter.rs` loses spawn errors and exits from a worker
      thread (`:308-336,388`).

## 3. Structure

- [x] Typed enum settings (Power, Fan, BottomScreen, Background…): one `Choice` trait and `choices!` macro,
      each variant's value, label and impact in one table. (`ClockField`, `Modifier`, `FileAction::label`,
      `ListId::ALL` stay plain arrays and matches: nothing is repeated there.)
- [x] Keymap parameters in one table: `Param::default_for` reads `Binding::to`'s defaults; `Target`'s
      outputs are `Target::To(Output)`. (`Stroke` stays beside `Chord`: a typed stroke always has a key,
      a modifier's too; a chord's may be none.)
- [ ] `State`: notices out of `hardware.rs` into `notice.rs`; Home combos into `hardware.rs`; the keymap
      screens' state out of `Bottom` into its own struct; `PowerProfile` beside the settings.
- [x] Duplicated logic in core: `editor.rs` `words()` / `count_env()`; `levels.rs:247-263` and
      `backlights()`/`backlight_rows()`; the six `if changed { save_keymaps }`; identical match arms
      (`keymap_screens/mod.rs:419/428,610/614`); `panel.rs:47-52,77-83`.
- [ ] Wi-Fi and Bluetooth: one vocabulary and one error model; `Effect::Browse`/`FilesInUse` as other
      commands are.
- [ ] Entry ids' prefixes (`desktop:`, `tray:`, `window:`, `runner:`) as constants or helpers in
      `clamshell_library`.
- [ ] Anonymous tuples in state as named structs: shelf's view (`shelf.rs:242`), `cost_of`, `Files`'
      `armed`/`busy`/`browsing`, `Calibrator`'s `(Calibrator, Instant, Instant)`,
      `remap::wanted`'s 6-tuple.
- [ ] Silent fallbacks the no-panic lints produce where an invariant should hold: `debug_assert!`
      (`Values::get`, `Lists::get`, `Actions::push`, library indices).
- [ ] Per-event waste: `follow_keymap` clones the keymap each event; `app_id_of` `format!`s per entry ×
      window each second; `found.rs:73` lowercases every id.
- [ ] Giant functions split: `core::apply_event`, `keymap_screens::on_keymap`, `running::follow_windows`,
      `remap::wanted`, `PadThread::route_events` (the rumble mixing a pure, tested function), `main`
      (one error path).
- [ ] The system thread: Account and AskAwake off it (as CalibrateRefresh); `?` on `Closed`.
- [ ] One way each: bus reconnection (`dbus::Kept` for both buses, `units.rs:76`), mutex poisoning,
      thread spawning (`worker`), the Unix-seconds clock (7 copies), `Duration`→`Timespec` (4 copies),
      the /dev/input watch (two watchers), reader on/off commands and `Command`/`Request` names.
- [ ] The keymap views' wildcard imports (`ui/views/keymaps/mod.rs:39-43`): explicit.
- [ ] UI: the confirm dialog written twice (`keymaps/mod.rs:196-222`); layout literals named or helped
      (`Column::para`), shared gaps in `style`.
- [ ] Dev tooling out of the device binary: `--snapshot`, demo data and the dev commands behind a feature.
- [ ] Rendering: a comment on whole-screen redraw and the shm copy (or drawing straight into shm).
- [ ] The `Library` arena's `Entry`: browsed and video fields in sub-structs.

## 4. Tests

- [ ] The screenshot test hashes pixels, not PNG bytes, and keeps its output on failure.
- [ ] Test helpers shared: core's testkit (the shelf/window closures, `start_end`), ui's
      `tap`/`key`/`device()` and `snapshot.rs`' copies, the Frame setup in `views/mod.rs`.
- [ ] One test `TempDir` with `Drop` for the ~20 hand-made temp dirs (system).
- [ ] The sensor hub's parser against captured bytes, not its own encoder (on the device: a fixture).
- [ ] The no-alloc test's gaps: Time and Touchpad tabs, the file name editors, the battery, dock and
      tray menus, an open group, a video; the menus without per-frame `Vec`/`format!`.
- [ ] `#[ignore = "why"]` on the live tests; rustdoc's 9 warnings.

## 5. How it reads

- [ ] Plain sentences in docs and comments: module docs first (`keymap_screens`, `tray`, `found`,
      `keymaps` view, `dock`, `snapshot`, `worker`), then the README; the parenthesised asides where they
      don't help.
- [ ] One name for one concept: entry ids are "game" in core (`State::game`, `Shelf::game`, `on_game`,
      `Effect::Launch { game }`) → entry; `KeymapId` (a button enum) renamed, the keymap's id a newtype;
      `Init.setup` / `first_run` / `SetupDone`; the name collisions (`Output`, `Device`, `Kind`,
      `Action`).
- [ ] Commit messages: short imperative scoped subjects, one concern each (from 2026-10-22 on).
- [ ] `docs/review-2026-10.md` framed as the design log it is.
- [ ] Cargo metadata: `description`, `repository`, `publish = false`, `rust-version`.
- [ ] `clamshell/README`: what the config folders beside the workspace are; `.claude/commands/maintain.md`
      named in the README as the device's maintenance playbook.
