# State plan

Goal (2026-10-22): `State` is one file with one `impl State`. It holds everything, by composition of
area types that each own their fields privately, with their rules and the getters the UI reads. State's
own impl only builds the areas, dispatches events to them, runs the order of what follows each event, and
carries out what one area asks of another.

Today: 294 methods in 38 `impl State` blocks across ~40 files, ~3,800 lines; State has 40 fields, ~15 of
them loose. Only 81 methods touch one area and call nothing else on State. Designed by four research
passes (session; keymap and settings; shelf, nav and bottom; the rest and State itself).

## Decisions

- **Explicit dependencies.** An area method takes what it reads from other areas as parameters
  (`self.radios.tap_network(i, long)`, `self.bottom.window_down(&self.session, now)`); disjoint field
  borrows inside State's impl make this work. Values are taken out of a borrow (an `Arc` clone, a `u16`)
  before a `&mut` call on another area.
- **A change with one owner is a method call on that owner.** An area rule that needs another area
  changed, where that area alone owns the change (a notice, waking the second screen, the keyboard's
  modifiers), gets it as a `&mut` parameter, as narrow as practical (`&mut Notice`, not `&mut Bottom`).
- **Requests State coordinates are events.** What needs several areas (entry acts: play, launch, show,
  show down, quit, force kill, suspend, toggle autostart, enqueue; a tab change; keymap choose/delete/
  capture; a recorded play) is an `Event` variant, so outside code can send the same commands. A rule
  queues such an event; State handles it with `apply_event` (the follow-up steps run once, at the end of
  the outer `apply`). There is no separate `Mutation` type.
- **One output buffer per event.** `apply` creates an `Out { effects: Vec<Effect>, events: Vec<Event>,
  dirty: Dirty }` and passes `&mut Out` down; rules push effects in the order things happen and queue
  events. After a rule returns, State handles the queued events in order, so their effects follow those
  pushed before them. `apply` returns `out.dirty` and `out.effects`. Rules no longer return `Vec<Effect>`.
- **`Clock`** `{ unix, mono }` replaces `unix_secs`/`mono_secs`, with `read()` and `unix_at(mono)`.
  Session rules take `clock` and `&Library`. Settings in force are not in it: they change during an
  event and are passed where needed.
- **State keeps `resume: Debounce`**: the resume record is shelf's place plus bottom's tab, a cross-area
  save. `Shelf` and `Bottom` mark themselves unsaved; `apply` touches the debounce at its end (equivalent:
  the debounce is only read by the next tick). `Saves` goes; brightness's debounce moves into `Levels`.
- **Where the loose fields go:**

  | field | area |
  |---|---|
  | `unix_secs`, `mono_secs` | `Clock` |
  | `ui_up`, `watching`, `account_watched`, `status_unwatched` | `Watches` (new, `watch.rs`) |
  | `tray`, `background`, `autostart`, `autostarted`, `device_session` | `Session` |
  | `pins_dropped` | `Dock` |
  | `files`, `fetching`, `shown` | `Shelf` |
  | `window_rules`, `clipboard`; `sent_down` from Session | `Bottom` |
  | `spender_menu`, `twelve_hour`; `time_page` from Bottom | `Sys` |
  | `first_start`; `account` from Sys | `Device` |
  | `settings`; `scope` from Bottom | `Prefs` (new) |
  | `keymaps`, `keymap_screens`, `keymap_sent` | `Keymapping` (new; `Screens` flattened into it) |

- **The settings panel** is a read-only view, `State::panel() -> Panel<'_>`, built from session, shelf,
  prefs and keymapping (what it edits, its levels, the tiles' values). Mutating handlers take owned values
  out of it first.
- **What stays in State's impl:** construction; area accessors; `apply`/`apply_event`/`activate`/
  `on_pointer`/`tick`/`ui_started`/`library_read`; `launch` (shelf, device, settings, session
  and the resume save); `reconcile` (its order kept exactly); `choose_tab`/`left_tab`; `in_force`,
  `settings_of`, `panel`; `follow_watch`, `follow_runner_keymaps`, `power()` for bottom; the cross-area
  queries the UI uses (`entry`, `rows`, `scroll`, `list_len`, `bottom_dark`, `pad_back`); `item_key`,
  `find_item`, `resolve`; `editor_done`. Estimated 650-800 lines with docs, in `state.rs`; `lib.rs` keeps
  the crate doc, modules, re-exports and `Init`.
- **Outside code reads the areas directly**: `State` has one accessor per area returning `&Area`, and no
  pass-through getters (`s.radios().wifi()`, `s.sys().time_zones()`, `s.keymapping().screen()`,
  `s.panel().editing()`, `s.shelf().in_actions(s.pointer())`, `s.clock().unix()`). It changes state
  only through events (`apply`), so the steps that follow every event always run: no `_mut` accessors;
  tests set up state with events or `#[cfg(test)]` helpers.

## Order

Each step is one or a few commits that pass `tools/check.sh` with the snapshot hashes unchanged; tests
and the UI change with the code they call; doc links to moved methods are fixed in the same commit.

- [x] 1. `Clock`.
- [x] 2. `Mutation` and `State::settle`, with `Notify` and `Effect` first; `State::notify` stays as a wrapper
      until the areas return notices.
- [x] 3. Radios: private fields, `tap_network`/`tap_device`/`join`/power, `read_wifi`/`read_bluetooth`.
- [x] 4. Levels: brightness debounce in, `change`/`on_audio`/`on_stream`; `read_status` split between
      `Sys` and `Levels`.
- [x] 5. Sys: private fields and getters; `spender_menu`, `twelve_hour`, `time_page` in; the time tab's
      rules (taking the Time list's scroll) and the battery tab's.
- [x] 6. Device and Idle: `first_start`, `account` in; taps, sticks, refresh; `Idle::follow`.
- [x] 7. `Watches`.
- [x] 8. Bottom: notices, screen rules (`Power`), window rules and `sent_down`, tabs, clipboard, pointing,
      text input, the editor (`Bottom::edit`, State's `editor_done`); `Pointer` methods; `Keyboard::hold`.
- [x] 8b. `Out`, and `Mutation` folded into `Event`: rules push into `&mut Out` instead of returning
      effect lists; `Tab` and entry acts as `Event` variants.
- [x] 9. Shelf: `Files`, `fetching`, `shown` in; files and rules and actions onto `Shelf`; the unsaved
      flag and the resume debounce on State.
- [x] 10. Lists: `move_list`; drop `list_offset`, `list_can_scroll`, `anchor`.
- [ ] 11. Keymapping: data methods on `Settings`; the struct; getters and editing; the handlers pushing into
      `Out` (choose/delete/capture as events).
- [ ] 12. Prefs and the `Panel` view.
- [ ] 13. Session: fields in; session-only rules; units, windows, found, tray, background; `launch` and
      `reconcile`; dock with `EntryAct`; enqueue.
- [ ] 13b. `Wanted`: the outside's desired state computed from the final state after each event (watches,
      screens and dim, second screen power, Apply for the program in force, keymap `(id, version)`, pad
      route, fetch-first, browsed folder, files in use as a hash, window rules version) and one diff
      against what was last sent, in a fixed order matching today's tests; a resend mask for Start,
      Resumed, RefreshCalibrated, Library/Fetching. Before it, `apply` keeps a short settle order
      (reconcile, idle stage, program, pad mouse, pad claimant). Replaces `follow_watch`, `follow_power`,
      `follow_keymap`, `follow_shown`, `follow_browse`, `follow_dim` and their last-sent fields; session's
      reconcile and the event-driven rules stay. Prepared by step 9 (files in use hashed, no `shown`) and
      step 11 (`Keymaps` version counter, `keymap_sent` as `(id, version)`).
- [ ] 14. Fields private everywhere; `#[cfg(test)]` helpers where tests poked fields.
- [ ] 15. `state.rs`: the one `impl State`, `apply.rs` folded in.

## Risks to watch

- Effect order: tests compare exact vectors (watch effects on a tab change, a launch's resume effect
  first, keymap Choose before `Save::Keymaps`, Deleted after). Queued events keep order by construction
  (handled after the effects pushed before them); the areas must push in today's order.
- `apply`'s order of follow steps, and `reconcile`'s (steam launches before window following).
- `before = in_force()` taken before a settings change; `tick`'s first-tick resume before the clock moves.
- `bottom().dimmed()` read from the cached field: only valid because `follow_bottom_dim` runs on every
  apply after the event.
