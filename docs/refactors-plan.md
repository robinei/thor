# The architecture refactors: a plan

The changes left from the whole-repo review of 2026-10-28 that need a design before they are made. Each
part says how it is now (file:line as of commit ea206759), the shape proposed, the steps as commits, the
risks and the tests that cover them, and the choices to make, each with a recommendation. The mechanical
changes from the same review (the drm.rs and wm.rs splits, shared system helpers, device facts, tool
fixes) are made without a plan.

Parts:

1. Compositor: one configure and facts path for X11 and Wayland, and placing the scene once per change.
2. clamshell-ui: views declare what the framework acts on; ClamShell's content out of the toolkit; the
   views' duplicates.
3. Identifying clients' processes without a thread each; an index of the library, built once per read.

## The choices, together

| Part | Choice | Recommended |
|---|---|---|
| 1.1 | A Wayland window's own size when centred | send none (0x0, the client decides within the bounds) |
| 1.1 | Process fields in a window's changed facts | the state keeps unit, pids and game, as it keeps the title |
| 1.2 | Placing: on demand behind a key, or at one point a turn | on demand behind the key |
| 2.1 | Declarations carry the state's types, or opaque ones | the state's types |
| 2.1 | A slide lets go of its control, or `sliding()` stays | the slide lets go |
| 2.1 | Tabs declared in a fixed array | a framework constant, logged when full |
| 2.2 | Generic number words stay in the toolkit, or all words move | all move to the views |
| 2.2 | The `Icon` glyph set stays in gfx, or becomes registered data | it stays |
| 2.3 | The card look in the toolkit, or a views helper | the toolkit (`Look::card(selected)`) |
| 2.3 | The editor's face shared, callers choosing where | shared face, callers choose |
| 2.3 | Key badges as `PadKey::A`, or a badge enum | `PadKey::A` |
| 3.1 | Bounding identification threads | one per distinct process (asks join), no pool |
| 3.2 | Who owns the library index | the Library itself |
| 3.2 | Guarding against a stale index | `entries` private, the index dropped on mutable access |
| 3.2 | Index keys | sorted (hash, slot) vectors, rebuilt in place |

## Part 1: the compositor's configure and facts, and placing once per change

Paths are under `clamshell/crates/compositor/src/` unless given otherwise. Line numbers checked against the
working tree on 2026-10-29.

### 1. Configure and window facts, written twice

#### As it is

Configure:
- Wayland: `Server::sync_windows`, `windows.rs:184-210`. For each window with a toplevel: area from
  `self.area(below)`, policy from `client.shown` (cached once per state revision, `windows.rs:173-179`),
  `fill_size` (`windows.rs:259-262`), `geometry::configure_size` (`geometry.rs:93-107`) with the committed
  geometry size as `current`; sets `size`, `bounds = area.size`, `Fullscreen` and `Activated` states; sent
  only after the initial configure (`windows.rs:207`); Smithay sends nothing if the pending state is unchanged.
- X11: `Window::configure`, `x11/wm.rs:857-917`. Its own policy cache (`policy`/`revision` fields,
  `wm.rs:83-84`, filled at `wm.rs:859-862`); clarity from the bound `ClientWindow`'s `shown`, else straight
  from the state (`wm.rs:865-866`); the same `fill_size` and `configure_size`, but `current` is `self.size`,
  the size the client asked for (creation or ConfigureRequest, `wm.rs:625-634`), and `None` falls back to
  that size unclamped (`wm.rs:868-869`); then the emulated mode override (`wm.rs:872-875`), a centring offset
  (`wm.rs:876-877`), and ConfigureWindow + `_NET_WM_STATE` + `WM_STATE` + MapWindow when geometry or layout
  differ from what was last sent (`wm.rs:879-914`).

Facts:
- Wayland: `Server::facts` (`windows.rs:79-115`). Opened on the first commit (`windows.rs:142-151`),
  immediately. On later commits, `Changed` immediately, during dispatch, if limits or colour differ from the
  state (`windows.rs:153-164`). App id, parent and dialog hint: `identity_changed` (`handlers.rs:247-253`)
  sends `Changed` immediately and without comparing. Title: `title_dirty` (`handlers.rs:210-216`), flushed
  at the frame against the state (`windows.rs:237-254`), as its own `Title` event.
- X11: `Window::sync_facts` (`wm.rs:811-839`), from `Properties::facts` (`x11/properties.rs:152-166`). Opened
  immediately in the WM's settle. Changes are compared against `sent` (app id, title, parent, limits, role;
  not colour, `wm.rs:828-829`), queued in `server.x11.changes`, and at the frame compared again against the
  state (app id, parent, limits, role, colour) by `flush_x11_changes` (`x11/mod.rs:61-80`), sent as
  `Changed`, `Title`, or both in one `WindowBatch`.
- The state's `change` (`crates/state/src/session/windows.rs:298-303`) keeps the old title and `x11`, and
  takes everything else, including `unit`, `pids` and `game`, from the event.

#### Where they differ, and which is right

| # | Aspect | Wayland | X11 | Right |
|---|---|---|---|---|
| C1 | `current` size given to `configure_size` | committed geometry, including the size we configured (`windows.rs:191`) | the client's own requested size (`wm.rs:868`) | X11's: "its own size within the area" (`geometry.rs:90-92`). On Wayland a window that leaves Fullscreen (role or parent change) keeps the fill size, clamped. Rare: layout only flips on a role/parent change for non-fixed windows (`crates/state/src/presentation/mod.rs:160-180`). |
| C2 | No known size | `None`: client decides, with bounds | falls back to `self.size`, unclamped | Equivalent in practice (`self.size` >= 1 except a 0-sized CreateNotify). Shared function returns `None`; X11 maps `None` to its own size. |
| C3 | Centring offset | none (xdg has no position; `Geometry::new` centres when drawing, `geometry.rs:62-63`) | `(area - size) / 2` for Centred (`wm.rs:876-877`) | Both right for their protocol. X11 needs a position; menus are placed relative to `configured.loc` (`x11/menus.rs:83-84`). Compute it in the shared function; Wayland ignores it. |
| C4 | Emulated mode (RandR via Xwayland) | n/a | overrides size when Fullscreen, after fixed size and fill (`wm.rs:872-875`) | Keep, as an input (`Option<(u32, u32)>`, `None` for Wayland). No unit test covers it today. |
| C5 | Fullscreen state | xdg `Fullscreen` iff layout Fullscreen | `_NET_WM_STATE_FULLSCREEN` iff Fullscreen | Same. Shared function returns `fullscreen: bool`. |
| C6 | Bounds / Activated | `bounds = area`, `Activated` by keyboard focus | none; focus via `settle_focus` | Protocol-specific; stays outside the shared function. |
| C7 | Policy and clarity cache | `ClientWindow.shown` per revision | its own `policy`/`revision`, clarity from `shown` or the state | One cache: X11 `Window` holds a `Shown` refreshed by the same `Shown::of` per revision (needed before the surface is bound, when there is no `ClientWindow`). |
| F1 | When `Changed` is sent | immediately in dispatch (`windows.rs:163`, `handlers.rs:251`) | at the frame (`x11/mod.rs:61`) | X11's: one apply per frame, coalesced, as titles already are on both sides (test `a_burst_of_titles_reaches_the_state_once_per_frame`, `tests.rs:373`). `Opened` stays immediate on both: the first configure needs the state's placement. |
| F2 | Changed + title together | two applies: `Changed` now, `Title` at the frame | one `WindowBatch` (`x11/mod.rs:74`) | X11's: the UI never records half an update (test `x11_facts_deferred_to_the_frame_...`, `wm.rs:1320`). |
| F3 | What counts as changed | commit path: limits, colour; identity path: anything, no comparison | vs `sent`: app id, title, parent, limits, role; vs state: app id, parent, limits, role, colour | One comparison against the state (the state is what was told; `sent` duplicates it): app id, parent, limits, role, colour; title on its own. |
| F4 | Colour | `window_colour` (`windows.rs:130-135`) | always `None` (`properties.rs:164` via `..default`) | Wayland's. `window_colour` works on any `Surface`, so X11 windows get it once bound; no change while Xwayland declares none. |
| F5 | Process (`unit`, `pids`, `game`) | read from the state when building facts (`windows.rs:91-100`) | read from the state at the WM turn (`wm.rs:812-819`); `unit`/`pids` (not `game`) copied again at flush (`x11/mod.rs:66-71`) | Not a client fact. Nothing outside tests sends a `Changed` that alters them. See choice B. |
| F6 | Parent re-evaluated | only on `parent_changed` | every settle, until the transient parent opens (`wm.rs:724-727`, `811`) | Leave: Wayland's parent id exists once the parent's toplevel does. Noted only. |
| F7 | Open log | colour | limits | Trivial; one shared log line. |

#### Proposed shape

A module `window_sync.rs` (or `configure.rs`), pure where it can be:

```rust
pub(crate) struct Configure { pub size: Option<Size<i32, Logical>>, pub offset: Point<i32, Logical>, pub fullscreen: bool }
/// The configure for a window of `facts` shown as `shown` in `area`, whose own size is `own`.
pub(crate) fn configure(facts: &WindowFacts, shown: &Shown, area: Rectangle<i32, Logical>,
                        own: Option<Size<i32, Logical>>, emulated: Option<(u32, u32)>) -> Option<Configure>
/// What differs between the facts the state has and new ones: (changed, title changed).
pub(crate) fn differ(old: &WindowFacts, new: &WindowFacts) -> (bool, bool)
impl Server {
    /// Queues a window's current facts for the frame, replacing any queued for it.
    pub(crate) fn facts_changed(&mut self, facts: WindowFacts)
    /// At the frame: each queued window's facts compared with the state's, sent as Changed, Title or both.
    pub(crate) fn flush_facts(&mut self)
}
```

`fill_size` and `configure_size` move into it. `x11.changes` becomes `Server::changed_facts`, used by both
sides. Wayland marks a window `facts_dirty` (replacing `title_dirty`) from `title_changed`,
`identity_changed` and the commit path, and builds its facts once at the flush, so a burst of requests
costs one `facts()` call (it clones strings; only for windows that changed). X11 keeps queueing facts
(the properties live in `Wm`, out of reach of `frame()`), with colour filled at the flush from the bound
surface.

#### Steps (one commit each)

1. `compositor/geometry: one configure for X11 and Wayland` - add `configure()` with unit tests (Centred
   offset, emulated override only when Fullscreen, fixed size, fill at a lower resolution, unknown size);
   both sides call it. Behaviour unchanged (Wayland passes committed size, ignores offset).
2. `compositor/x11: read the presentation through Shown` - drop `policy`/`revision`, hold a `Shown`.
3. `compositor/windows: one facts comparison against the state` - `differ()` used by the commit path,
   `identity_changed` and the X11 flush; X11 facts get colour.
4. `compositor/windows: Wayland facts reach the state at the frame` - `facts_dirty`, shared queue and flush,
   `Changed`+`Title` batched. Behaviour change (F1, F2); say so in the commit.
5. Choice B, if taken.
6. Choice A, if taken.

#### Risks and tests

- Initial configure must still follow `Opened`: `an_xdg_window_is_placed_and_configured_before_its_first_buffer`
  (`tests.rs:283`), `placement_is_configured_before_mapping_and_focus_waits_for_the_servers_time` (`wm.rs:1216`).
- No X11 requests when nothing changed: `an_unchanged_window_sends_no_x11_requests` (`wm.rs:1287`).
- Coalescing: `a_burst_of_titles_reaches_the_state_once_per_frame` (`tests.rs:373`),
  `x11_title_changes_reach_the_state_once_per_frame` (`wm.rs:1304`), `x11_facts_deferred_...` (`wm.rs:1320`).
- Presentation flips: `a_windows_presentation_is_worked_out_again_only_when_the_state_changes` (`tests.rs:313`),
  `a_dialog_of_a_window_below_shows_over_it_there_at_its_fixed_size` (`tests.rs:459`),
  `a_toplevel_marked_a_dialog_has_the_dialog_role` (`tests/dialog_tests.rs:11`), geometry tests (`geometry.rs`).
- Deferring Wayland `Changed` (step 4) delays a limits change's reconfigure to the frame; on the DRM host
  the frame runs every loop turn (`drm.rs:629`), nested only on Redraw (`nested.rs:101`). Add:
  `a_wayland_app_id_and_title_change_reach_the_state_in_one_batch_at_the_frame`,
  `a_wayland_limits_change_is_reported_once_per_frame`.
- Add `an_x11_window_reports_its_surfaces_colour` (if Xwayland's surface has one in `colour.surfaces`).
- Real clients: `clamshell/tests/nested-compositor.py`, `real-apps.py`, and `picture-scaling.py` for C4.

#### Choices for the owner

- **A. Wayland's own size (C1).** (a) Leave it: echo the committed size; the Fullscreen-to-Centred case
  keeps the fill size. (b) Send `size: None` (0x0, client decides, bounded by `bounds`) for a Centred,
  non-fixed window; a client that picks larger is scaled down (rule 1). (c) Track an own size per Wayland
  window (committed while not configured Fullscreen), mirroring X11. Recommendation: (b): no new state,
  correct by the protocol; changes `geometry.rs` test expectations for dialogs (`geometry.rs:150-154`).
- **B. Process fields in `Changed` (F5).** (a) The compositor fills them from the state at the flush, for
  both sides (as now, plus `game`). (b) The state's `change` keeps `unit`/`pids`/`game` as it keeps the title
  (`state/src/session/windows.rs:298-303`; a state test and `docs/state-machines.md` update). Recommendation:
  (b): the facts a backend reports cannot undo what the state learnt; removes the copy in `x11/mod.rs:66-71`
  and the reads in both fact builders' change paths.

### 2. Placing and syncing many times per commit and turn

#### As it is

`Server::place` (`scene.rs:102-123`) refills the scene; `Server::sync_windows` (`windows.rs:172-234`)
refreshes `shown` per revision, places, then for every window configures, enters/leaves outputs, and
checks keyboard focus, X11 popups, capture and hidden touches.

Per commit of a Wayland toplevel's root (after it opened): `open_on_commit` (`handlers.rs:146`) ->
`sync_windows` (`windows.rs:166`) -> `place` (`windows.rs:180`); then `place` again (`handlers.rs:166`).
1 sync, 2 places. A desynchronised subsurface whose colour or picture place changed adds 1 sync + 1 place
(`handlers.rs:155`). Unmap: 1 sync (`handlers.rs:140`). Any other surface (popup, cursor, every X11
surface, since `open_on_commit` returns at `windows.rs:140`): 1 place.

Per loop turn (`base.rs:91-112`):
- `x11.turn` (`base.rs:93`) -> `Wm::turn`: `place` (`wm.rs:132`) + `sync_windows` (`wm.rs:134`): 2 places,
  1 sync, every turn while Xwayland runs, whether or not anything came from it.
- `server.turn()` (`base.rs:106`, `server.rs:186`): `update_idle` -> `place` (`server.rs:194`, `idle.rs:57`),
  `sync_windows` (`server.rs:224`): 2 places, 1 sync.
- DRM host `step`: `server.frame()` (`drm.rs:629`) runs `turn` again: 2 places, 1 sync; `record_overlays`
  1 place when an overlay was recorded (`drm.rs:1115`); per composed screen 2 (`render.rs:126,143`);
  `take_feedback` 1 (`pacing.rs:137`); `frame_callbacks` 1 per screen sending (`pacing.rs:179`). `step` also
  runs from each VBlank inside dispatch (`drm.rs:388`).
- Input: `sync_windows` after a press (`input.rs:157,180`, `touch.rs:84`), activation (`activation.rs:65`),
  resize (`screens.rs:110`), toplevel destroyed (`handlers.rs:207`).

An idle DRM turn with nothing composed: 6 places and 3 syncs with Xwayland, 4 and 2 without. Each sync
walks every window (`with_pending_state`, `output.enter/leave` per output). `place` allocates nothing
once the list has grown (`scene.rs:9-10`); the cost is repeated work, not allocation.

#### Proposed shape

Placement on demand behind a key, so every existing reader stays correct and repeated calls are a compare:

- `Scene` keeps the key it was placed at: `(state_revision, windows_epoch, screens size/top_width,
  overlay_area(Top), overlay_area(Bottom))`. `windows_epoch: u64` on `Server` is bumped where windows'
  drawn state changes: the commit handler (where `handlers.rs:166` places now), `add_toplevel`,
  `bind_surface` (`wm.rs:840`), every `windows.retain`, and `resize`. The state revision and overlay areas
  need no marking.
- `place()` first refreshes `shown` if the revision moved (moved out of `sync_windows`), then returns early
  if the key matches. The cursor entry (`scene.rs:119-122`) is redone on every call: it depends on time
  (`cursor_activity.visible(now)`), the pointer and the client's cursor image, and costs one insert.
- `sync_windows` split: the per-window configure/outputs loop runs when `(state_revision, windows_epoch,
  screens)` moved; the focus, popups, capture and touches checks run on every call (they depend on menu
  grabs and touches, which have no revision, and are cheap).
- One sync point per turn: the end of `Server::turn`. Calls kept only where an order requires it, each with
  an `// Order: ...` comment: first open (`windows.rs:166-169`), presses before the button/touch reaches a
  client (`input.rs:157,180`, `touch.rs:84`). Removed: commit (`handlers.rs:166`, `windows.rs:166` for
  later commits), `handlers.rs:140,207`, `activation.rs:65`, `screens.rs:110`, `wm.rs:134`.
- `x11.turn` moves after `event_loop.dispatch` and before `server.turn()` (`base.rs:93` -> after 105), so
  what the WM changes (an `Opened` in `sync_facts`) is synced by the same turn before the loop blocks again.
  `Wm::turn` keeps `server.place()` before `settle_menus` (now a compare when nothing changed).

#### Ordering hazards

1. `shown` stale under a new key: today `update_idle` places (`server.rs:194`) before `sync_windows`
   refreshes `shown`. With a key, that placement would be cached with an old presentation. Refreshing
   `shown` inside `place` removes the hazard.
2. Hit-testing after a commit in the same dispatch: `hit` (`input.rs:57-71`) reads the scene; today the
   commit placed it. With lazy placement, `input()` and `touch_down` call `place()` first (they are
   `&mut self`). The debug check in `Server::shown` (`scene.rs:180-193`) fails tests on a missed one.
3. Keyboard focus before the button: `input.rs:180` syncs before `pointer.button` (`input.rs:187`). Kept.
4. X11 menus are placed relative to the parent's geometry (`menus.rs:82-84`) and dismissed when it changes
   (`popup.rs:108-119`): both must see the same placement. Kept by placing before `settle_menus`.
5. Drawing changes the state (`frames_done`, `render.rs:141`): placed again after (`render.rs:143`). With the
   key that second call is free when nothing changed.
6. The cursor's hiding is time-driven; the loop wakes at its deadline (`base.rs:98`). Covered by redoing
   the cursor entry each call.
7. X11 configure reads the state and area, not the scene (`wm.rs:857-877`): unaffected.
8. Wayland configure (`windows.rs:191`) reads the committed size: needs no placement, but needs the sync
   after commits that change size, which the end-of-turn sync gives.

#### Steps (one commit each)

1. `compositor/scene: count placements in tests` - a `#[cfg(test)]` counter on `Scene`; a test pinning
   today's counts is not useful; add the tests below as they become true.
2. `compositor/scene: refresh the presentations as the scene is placed` (hazard 1).
3. `compositor/scene: place only when what it shows from changed` - the key and `windows_epoch`; cursor
   redone each call. All call sites unchanged.
4. `compositor/windows: configure windows only when the state or windows changed` - split `sync_windows`.
5. `compositor: run the X11 window manager's turn after dispatch` - move `x11.turn`, drop `wm.rs:134`.
6. `compositor: sync windows once per turn` - remove the calls listed above; `// Order:` comments on those kept.

#### Tests

Existing: `tests/scene_tests.rs` (5), `tests/x11_popup_tests.rs:12`, `tests/pointer_tests.rs`,
`tests/idle_tests.rs:14`, `tests/pacing_tests.rs:52,129`, `tests/presentation_tests.rs`,
`tests/viewport_tests.rs:10`, `tests.rs:343` (moved down, scaled at once), `tests.rs:283`, `tests.rs:313`,
`tests.rs:543` (touch), and the debug check in `Server::shown`. Add:
- `placing_again_with_nothing_changed_does_nothing` (counter; then a commit, a state event, an overlay
  change, a resize each place once).
- `an_idle_turn_places_and_configures_nothing` (with and without a WM; `Wm` tests' `placed()` harness,
  `wm.rs:1181`).
- `a_touch_right_after_a_resizing_commit_lands_on_the_new_geometry`.
- `a_window_opened_by_the_wm_is_configured_in_the_same_turn` (step 5).
- No allocation: the compositor has no counting-allocator test; if the owner wants one, an idle `turn()`
  with a window open is the case to cover.

#### Choice for the owner

- **Strict one point vs. on demand.** (a) On demand behind the key (above): readers always see a scene
  fresh for the current state and commits; the debug check keeps holding. (b) One point per turn, before
  drawing, with readers seeing the scene as last placed: matches "touch what was drawn", but X11 menus would
  be placed against stale geometry and then dismissed (hazard 4), and the debug check would have to go.
  Recommendation: (a).
- `frame()` runs `turn()` a second time per DRM loop turn (`base.rs:106`, `drm.rs:629`). With the key both
  are cheap; merging them is a separate change and not proposed here.

## Part 2: clamshell-ui: views declare, the framework acts

Paths are under `clamshell/crates/`. Line numbers are at ea206759.

Dependency direction today: views -> ui -> state (`ui/Cargo.toml` has no views dependency). Every
step below keeps it. The framework keeps using the state's `Id` and `Event` as opaque values; what goes
is matching on particular variants.

Check for every step: `tools/check.sh`. The snapshot pixel hashes in `clamshell/tests/` must not
change in any step here (all are refactors); a changed hash means a behaviour slipped.

### 1. Press and focus de-hardcoding

#### As it is

- `ui/src/input/press.rs:102-104` `points()`: `!id.on_top() && !matches!(id, Id::Tab | Id::Control |
  Id::PadMouse | Id::Wake)`. Used by `Press::may_point` (:176-178) and `core/input.rs:44`.
- `press.rs:222-228` picks the gesture by `Id::Control(control)` with a track (slide) and
  `Level::of(&target.id)` with a track (level drag). `press.rs:236` special-cases `Control::Param` to
  slide at once.
- `press.rs:311` `press.id == Id::Blank` picks the shorter point distance.
- `press.rs:366` `state.keyboard().hold(&press.id)` decides repeat vs long press (keyboard's own rule,
  `state/src/bottom/keyboard.rs:317`).
- `ui/src/toolkit/mod.rs:236-243` `Frame::sliding` matches `Id::Control` to keep a sliding control
  unlit.
- `ui/src/input/focus.rs:157` `Level::of(focus)`: left/right step a level. `focus.rs:87` falls back to
  `Id::Tab(Tab::default())`.
- `PadOut::tabs: [Option<(Tab, Id)>; Tab::ALL.len()]` (`toolkit/mod.rs:91`), filled by
  `Frame::bar_tab` (:313, drops silently when full); `focus.rs:216-226` compares with
  `s.bottom().tab()` and emits `Activate(Id::Tab(tab))`.
- `core/input.rs:56-70` `press_overlay` matches `WindowArrow`, `OverviewWindow`, `Notification` and
  emits their events. The state ignores those ids on Activate (`state/src/apply/press.rs:157`).
- `FrameOut::touchpad_area` (`toolkit/mod.rs:150-152`) is set by `views/src/lib.rs:135,147`; core uses
  it as both the client windows' area (`core/mod.rs:332-334,354`) and where a press on nothing becomes
  `Id::Blank` (`core/input.rs:28-37`).
- List moves are the top screen's only: `focus.rs:47` (scope looks at top targets), `:147`, `:233`
  (`d.pad(Screen::Top).moves`), and the wheel `core/input.rs:78-79`.

#### Shape

```rust
// toolkit/mod.rs
pub struct Target { rect, id, list, item, pad: PadUse, drag: Option<Drag>, hold: Hold }

/// What a finger along a target sets. Replaces `track: Option<Track>`.
pub enum Drag {
    /// Sets `level` where the finger is; on the pad, left and right step it.
    Level(Level, Track),
    /// Brings up `control`'s slider, after a hold or a sideways move, or at once.
    Slide { control: Control, track: Track, at_once: bool },
}

pub struct TabStop { pub open: Id, pub focus: Id, pub current: bool }
pub struct PadOut { ..., pub tabs: [Option<TabStop>; MAX_TABS] }   // MAX_TABS = 16

pub struct FrameOut {
    ...,
    /// Where the screen's content goes; the compositor lays client windows out in it.
    pub content: Option<Rect>,
    /// Where a finger may point: on nothing there, or on a tap target there without a drag.
    pub point_area: Option<Rect>,
}
```

Views declare: `f.drag(Drag::...)` replaces `f.track(..)` (`views/src/levels.rs:71`, `bar.rs:101`,
`curves.rs:97,129`, `keymaps/panel.rs:214` with `at_once: true`); `f.hold(Hold::Repeat)` on keys that
repeat (`keyboard.rs`, from `state.keyboard().hold`); `f.tab_stop(open, focus, current)` replaces
`bar_tab`; `f.out.content`/`f.out.point_area` replace `touchpad_area` (dark: content = screen, no point
area; normal: both = body).

Framework does: gesture from `target.drag`; pad left/right on a focused target with `Drag::Level` step
it; hold from `target.hold` (the pad's A gets the drawn target's, via `focus::press`); a finger may point
if its down position is in the screen's `point_area`, the target has no drag and the gesture is still
a tap (this equals today's rule: bar targets are above the area, the dark screen has none, the top
screen has none). The press on nothing keeps `Id::Blank` but carries `blank: bool` in `Press`.

Overlays: core emits a new `PointerEvent::Act(id)` for the overlay target pressed; the state's match
at `apply/press.rs:157` maps the three ids to their events. Not `Activate`, which would also move the
focus, count as using the second screen and be swallowed after a wake (`apply/press.rs:20-48`).

Shoulders: `TabStop` with `current` is found, stepped, and `Activate(open)` + `Focus(focus)` emitted.
`focus_bottom` falls back to the declared start, then the first candidate, and returns `Option`.

List moves: scope `List` when the focus is on a `PadUse::List` target on a screen the pad may use;
moves, pages, ends come from that screen's `PadOut`; the wheel uses its own screen's.

#### Steps (one commit each)

1. `ui: a target's drag says what a finger along it sets` — `Drag` replaces `track`; press.rs and
   focus.rs read it; views declare. Removes `Level::of`, `Id::Control`, `Control::Param` from input.
2. `ui: a target's hold is declared` — `Target.hold`; keyboard marks repeat; `PadPress::Press` carries
   the hold. Removes `state.keyboard()` from press.rs.
3. `ui: a slide lets go of its control` — `start_slide` emits `PointerEvent::Up(id)` and `up()` skips
   it after a slide; `Frame::sliding` goes.
4. `ui: pointing follows the declared point area` — split `touchpad_area`; `Input::touch` takes
   `points: bool`; `press::points` and `Id::on_top` use in input go.
5. `state, ui: overlay targets act through PointerEvent::Act` — `press_overlay` loses its match.
6. `ui: the shoulders step declared tab stops` — `TabStop`, `tab_stop`, logged when more than
   `MAX_TABS`; `focus_bottom` without `Id::Tab`.
7. `ui: list moves on either screen` — scope, shoulder, trigger, wheel per screen.

#### Risks and tests

- Gestures (1): `press/tests.rs` `a_slide_starts_when_moved_far_enough_or_held`,
  `a_keymap_parameter_slides_at_once`, `a_second_finger_on_a_control_meanwhile_is_a_tap`,
  `a_level_row_follows_the_finger_and_the_release_focuses_it`; the helpers `volume()`, `level()` build
  `Drag`. Add in focus.rs: left/right on a focused level row step it, and on one not drawn do not.
- Hold (2): `a_held_key_repeats`. Add in `views/src/pad_tests.rs`: A held on a drawn key repeats, on a
  modifier long-presses.
- Up at slide start (3) changes the event sequence; `a_slide_starts...` pins it. Risk: state code that
  reads `pointer().pressed` during a slide; grep `pressed()` users before.
- Pointing (4): `a_finger_moving_on_a_key_points...`, `on_a_list_up_and_down_scrolls_and_only_sideways
  _points`, `on_nothing_a_finger_points_as_soon_as_it_moves`; `views/src/tests.rs:162,253,292` read
  `touchpad_area` and switch to `content`. Add: a finger on a bar tab or the dark screen never points.
  The nested session should be tried (point on the keyboard, on blank, on the bar).
- Overlay (5): no test exists for `press_overlay`. Add state tests that `Act` of each id gives the
  event, and does not move the focus. Compositor callers (`compositor/src/touch.rs:58`,
  `input.rs:155`) are unchanged.
- Tabs (6): `the_shoulders_step_the_tabs...`, `with_a_game_in_front...`; fixtures in focus.rs:359 build
  `TabStop`s. `no_alloc.rs` keeps the fixed array honest.
- List moves (7): `a_lists_moves_are_its_own...`, `pad_tests::the_librarys_list_takes_the_pads_moves`.
  Add a focus test with a `PadUse::List` list on the bottom screen.

#### Owner choices

- Declarations carry state types (`Level`, `Control`, `Hold`) vs. opaque ones (views give
  `fn(&Id, u8) -> Event`, or new id-keyed events `SetValue(Id,u8)`/`Step(Id,i32)` that the state maps).
  Recommend state types: the framework already depends on state, and no variant is matched.
- Step 3 (Up at slide start) vs. keeping `sliding()` but reading the slider's id from a state method.
  Recommend Up: one rule, as a drag already lets go of its row (`press.rs:306`).
- `MAX_TABS` fixed array vs. `Tab::ALL.len()`. Recommend a framework const (16) with a log when full.

### 2. ClamShell content out of the toolkit

#### As it is

- `ui/src/toolkit/format.rs:66-225`: `Percent`, `Share`, `MaybeShare`, `Frequency` ("clock unknown"),
  `Temp`, `Fan`, `Level`, `PlayTime` ("Played 5 min"), `DaysAgo`, `Bytes`, `Variables` ("No
  variables"), `Duration`, `HoursMinutes`. Only views use them (sysinfo, battery, dock, levels,
  library_view, settings). `Fmt` (:6-64) is the framework part.
- `ui/src/toolkit/icons.rs:9-54`: `Icon::wifi`, `Icon::for_setting` (every `Setting`),
  `Icon::for_device` (Bluetooth kinds). Users: views bar, bluetooth, levels, settings, tabs, time, wifi.
- `ui/src/gfx/icons.rs:117-136` `Icon::named` maps library.json names; its test (:787-796) reads
  `clamshell_state::library_view::edit::ICONS`. Users: `views/src/library_view.rs:135`,
  `library_edit.rs:39,380`, and `toolkit/widgets.rs:412`.
- `ui/src/toolkit/widgets.rs:296-420`: entry art over `clamshell_state::Entry` — `GREY`, `greyed`,
  `picture`, `wide_picture`, `cover`, `shot`, `art`, `thumb`, `icon`; `progress` (:339) is a plain bar.
  Users: `library_view.rs` and `dock.rs:105` only.

#### Shape

What moves to views: `views/src/format.rs` (all the `Display` words, with their tests);
`views/src/icons.rs` (free functions `icons::wifi`, `icons::setting`, `icons::device`, `icons::named`,
and the `ICONS` test; inherent `impl Icon` is not possible outside ui); `views/src/art.rs` (the entry art
functions).

What stays framework: `Fmt`; `progress`; the `Icon` glyph set and its drawing, with `Icon::volume` and
`Icon::battery` (value to glyph parameter, no state type); `Images`/`Slot` (keyed by `Arc<str>`);
`style`; `scroll`; `clock::Scratch` (frame-held cache); the generic widgets (header, menu, tabs,
list_row, held_slider, card, switch, tile, status_row, key_action, hint, ListArea).

Nothing in ui uses the moved items except `widgets::icon`, which moves with them, so ui gains no
dependency on views.

#### Steps

1. `views: the format words are the views'` — move, update imports; toolkit `format` keeps `Fmt`.
2. `views: which icon stands for a setting, device, signal or name` — move `toolkit/icons.rs` and
   `Icon::named`; call sites `Icon::for_setting(x)` become `icons::setting(x)`.
3. `views: entry art is the Library's and the dock's` — move widgets.rs:296-420 except `progress`.
4. `ui: docs say the toolkit holds no ClamShell content` — `toolkit/mod.rs:1-5`, `widgets.rs:1-4`,
   `gfx/icons.rs:1-3`, the Library-named comments on `Icon` variants.

#### Risks and tests

Pure moves; risk is only a missed caller, which the compiler finds. Format tests (`format.rs:227-282`)
and `every_icon_a_place_may_have_is_named` move with the code. `views/src/no_alloc.rs` and the snapshot
hashes cover the drawing.

#### Owner choices

- Generic numbers (`Percent`, `Bytes`, `HoursMinutes`) stay in the toolkit vs. all words move. Recommend
  all move: one rule (toolkit has `Fmt` only), and only views use them.
- The `Icon` set stays in `gfx` vs. icons become data the views register. Recommend it stays: the glyphs
  are drawing code with cached masks, and registration adds machinery for no user.
- `clock::Scratch` ("the local time as the bar and the time tab show it") stays as a frame cache.
  Recommend stay; reword its doc.

### 3. View duplicates and key_action

#### As it is

- Path header twice: `views/src/library_view.rs:233-243` (`group_header`, `Groups` with " › ") and
  `library_edit.rs:71-79` (inline, `Path` with " › "): same button, arrow, `HEADER_TEXT`, `Keep::End`.
- Selected card look five times: `Look { fill: selected.then_some(SURFACE_HI), ..Look::CARD }` at
  `library_edit.rs:90,135,261,270` and `library_view.rs:270`. `Look::CARD` (`style.rs:143`) has no other
  user.
- Editor titles three times, each a `Purpose` match to (title, save label): `views/src/lib.rs:84-102`
  (`typing`), `settings.rs:262-288` (ends `_ => return`), `device.rs:153-166` (`draw_password`, shows
  dots via `editor::field`). Fmt sizes differ (128/160/192).
- `widgets::key_action(.., key: &str, ..)` (`ui/src/toolkit/widgets.rs:588-595`) finds the `PadKey` by
  comparing names; callers pass "A", "B", "Y", "Start", "" (`library_view.rs:153-178,435-441,506`);
  `key_hint` gets "A"/"B" (:522-523). A typo silently binds nothing.

#### Shape

```rust
// views/src/library_view.rs, used by library_edit too
pub(super) fn path_header(f: &mut Frame, id: Id, r: Rect, pad: f32, path: impl Display)

// ui/src/toolkit/style.rs
impl Look { pub const fn card(selected: bool) -> Look }   // replaces CARD

// views/src/editor.rs: one exhaustive match over Purpose
pub(super) struct Face { title: Fmt<192>, save: &'static str, secret: bool }
pub(super) fn face(s: &State) -> Option<Face>
pub(super) fn draw(f: &mut Frame)          // title, field (dots if secret), Cancel, save

// ui/src/toolkit/mod.rs
pub enum PadKey { A, B, Y, X, Start }      // A: badge only; f.key(A, _) binds nothing
pub fn key_action(f, id, r, key: Option<PadKey>, label, tone, faded)
pub fn key_action_width(f, h, key: Option<PadKey>, label) -> f32
pub fn key_hint(f, r, key: PadKey, label) -> f32
```

`Groups` and `Path` stay (different data), only the drawing is shared. The callers of the editor keep
deciding where it shows (keyboard tab vs settings); they share `face` and `draw`. `library_view.rs:435`
maps `Shortcut` to `PadKey` with a local fn (no `From` across two foreign crates).

#### Steps

1. `views: one path header for the Library and its edit mode`.
2. `ui: Look::card says whether it is selected` — replace the five sites.
3. `views: one editor face per purpose` — `editor::face`, exhaustive; `typing`, `settings::editor`,
   `device::draw_password` call `editor::draw`.
4. `ui: key badges are typed` — `PadKey::A`, `Option<PadKey>`; library_view callers.

#### Risks and tests

- Pixel hashes must not change for 1, 2, 4. For 3, a common Fmt size can cut a long title differently
  only past 128 bytes; check with the snapshot and `every_tab_lays_out`.
- Step 3 ends the silent `_ => return` (`settings.rs:286`); a new `Purpose` then fails to compile until
  it has a title. Add a test: every editor purpose the state can open has a face.
- Step 4: `pad_tests::y_x_and_start_act_on_library_view_only_while_the_pad_is_there`,
  `picking_b_cancels...`, `b_on_library_views_actions_goes_back_to_the_entry` cover the binding.

#### Owner choices

- `Look::card(selected)` in the toolkit vs. a views helper. Recommend toolkit: `CARD` has no unselected
  user, so the look itself is "a card, lit when selected".
- Editor: shared `face` with callers choosing where (recommended) vs. one draw site that also decides
  the tab. The second moves routing into views that the state already decides.
- `PadKey::A` vs. a separate badge enum. Recommend `PadKey::A`: one list of the pad's buttons.

### Not covered, seen on the way

- `core/input.rs:24-26,44` reads `state.bottom().pointing()` and emits `Event::BottomTouched`: the
  framework still knows the second screen's touchpad by name.
- `focus.rs:1-7,26-36` docs and `PadScope` speak of the Library; reword with step 1.7.
- `Frame::bar_tab` drops a tab silently when full (`toolkit/mod.rs:314`); fixed by step 1.6's log.

## Part 3: identifying processes, and the library index

Paths are relative to `clamshell/`.

### 1. Compositor: one thread per ask to identify a process

#### As it is
- `Process` (crates/compositor/src/clients.rs:17-24): `unit: Option<String>`, `pids: Vec<u32>` (the lineage), `game: Option<String>`.
- `identify` (src/session.rs:100-104): `lineage(pid)` reads `/proc/<p>/stat` for up to 64 ancestors (crates/system/src/os/sys.rs:94-107); `launcher_game_of` reads `/proc/<pid>/environ` and `/proc/<p>/cmdline` of every ancestor (sys.rs:85-92); `unit_of` reads `/proc/<pid>/cgroup` (crates/system/src/os/cgroup.rs:13-20). environ and cmdline take the target's mmap lock (the reason given at clients.rs:66-70); stat and cgroup do not.
- `Identities::ask` (clients.rs:88-104) starts one thread per call. `Server::admit` (clients.rs:121-133) asks once per connecting client; the X11 WM asks once per window with a pid (src/x11/wm.rs:442-457). There is no limit and no sharing between asks.
- A waiting client is `Waiting::Client(UnixStream)` in `Identities::waiting` (clients.rs:60-65, 78). If the read never returns, the stream fd and the thread stay forever; a client that hung up meanwhile is only noticed once answered (clients.rs:134-136).
- Every X11 window starts with `outstanding: PROPERTIES.len() + 2` (wm.rs:486), one of which is the identification (wm.rs:453-456, 138-145). Override-redirect windows (menus, tooltips) wait on it too (src/x11/menus.rs:53), but the menu logic uses only `pid` (menus.rs:29, 62). The process matters for a menu only if `promote` (menus.rs:33-40) makes it a managed window, whose facts carry it (wm.rs:811-820).
- Pid reuse: `SEEN`/`same_process` (sys.rs:53-80) tell processes apart by `(pid, start time)` from `/proc/<pid>/stat` field 22. Nothing in the compositor does this.
- `unit` is not fixed for a process: ClamShell adopts found programs into scopes of its own (crates/state/src/session/found.rs:85, `adopt`). A cache of `Process` by pid would hand out a stale unit after adoption.

#### Proposed shape
Not a pool and not a cache of answers. One read per distinct process at a time, menus that do not wait, and waiting clients dropped on hang-up.

```rust
// clients.rs
struct Ask { pid: u32, token: u64, waiting: Vec<Waiting> }   // one thread per Ask
pub(crate) struct Identities { identify, wake, send, answers, asks: Vec<Ask>, next: u64 }
impl Identities {
    /// Joins the ask in flight for `pid` if there is one, else starts one (a thread).
    pub(crate) fn ask(&mut self, pid: u32, waiting: Waiting);
    /// Every waiting thing of the answered ask, each with a clone of the Process.
    pub(crate) fn answered(&mut self, out: &mut Vec<(Waiting, Process)>);
    /// Drops waiting clients whose socket hung up (zero-timeout poll on the waiting streams).
    pub(crate) fn drop_hung_up(&mut self);
}
```
- Threads are then bounded by the number of distinct processes being identified at once. A stuck process costs one thread however often it reconnects or maps windows.
- X11: an override-redirect window counts the identification out of `outstanding` (`PROPERTIES.len() + 1`) and does not ask; `promote` asks and adds one to `outstanding`, so a splash still opens with its process.
- `drop_hung_up` runs in `Server::take_identified` while any client waits: one `rustix::event::poll` over the waiting streams, timeout zero; a hung-up stream is dropped and logged at debug. The stuck thread itself cannot be ended.

#### Steps (commits)
1. `compositor: identify each process once while it is asked` - `Ask` with joined waiters.
2. `compositor: open X11 menus without waiting for their process` - outstanding count, `promote` asks.
3. `compositor: drop waiting clients that hung up`.
4. Optional: `compositor: cap processes identified at once` (see choices).

#### Risks
- `a_client_gone_or_stuck_while_identified_holds_up_nothing` (src/tests/identify_tests.rs:96-) connects three `UnixStream::pair` ends from the test process: all share one pid. After step 1 they join the stuck ask and the test fails as written. It becomes a test of `Identities` directly with distinct pids, plus a server test that two connections of one stuck process both wait and a third of another process does not.
- Pid reuse while an ask is in flight: a process that exits during its read and whose pid is taken by a new client gets the old answer. This needs pid wrap-around (pid_max 4194304) within one read. Accepted; joining could also compare start times, at the cost of a stat read on the compositor thread.
- A promoted window now opens one identification later than before.

#### Tests
- Existing: identify_tests.rs (both), wm.rs:1076-1120 `a_window_opens_once_its_process_is_identified`.
- Add: same-pid asks start one `identify` call and both are answered (count calls); an override-redirect window with a stuck `identify` maps as a menu; a promoted one waits for its process; a hung-up waiting client is dropped while its read is stuck (waiting count goes to zero).

#### Owner's choices
- Thread bound: (a) per-process joining only (recommended: bounds by distinct processes, no new failure mode); (b) also a cap on asks in flight (e.g. 16), beyond which a client is admitted unidentified with a warning: a flood of distinct stuck processes cannot exhaust threads, but a game can then be admitted as no game's; (c) a fixed pool of N workers: a stuck read blocks a worker, so N stuck processes stall everyone; needs per-ask deadlines. Not recommended.
- X11 reuse of a sibling window's process: skip (recommended) because of the unit going stale on adoption; joining in step 1 already covers windows of one process mapped together.

### 2. State: library scans per event

#### As it is
`Library::entry` and `Library::has` are linear scans by id (crates/library/src/library.rs:82-89), used throughout. Scans that run in every settle (crates/state/src/lib.rs:589-) or every `lists()` build:
- `background_of` (session/background.rs:140-157): `Library::entry`, then for a runner's level a filter over all entries. Called per running entry of ours (`follow_background`, :171) and per pausable runner (`follow_runner_pauses`, :208), which also does `library.entry` per launching entry (:213).
- `start_by_app_id` (session/running.rs:346-373): per window not owned, `entries.iter().find(has_mark)`. For a Lutris entry `has_mark` -> `is_marked` -> `lutris_unique` scans all entries again (session/launchers.rs:125-147): windows x entries x Lutris entries.
- `follow_units` (running.rs:256): `find(g.key == key)` per listed unit not yet running.
- `wanted_down` (session/below.rs:154-164): `find(g.key == key)` per main window without an owner.
- `front_launcher` (session/mod.rs:346-350): key scan, then `Launcher::entry` (launchers.rs:62-64), a scan by desktop file name.
- `entry_of_file` (library_view/mod.rs:290-295): `entry(id)` scan, then a scan by file. Called from the Library's `subject` (builtin_apps.rs:88) via `State::subject` in `lists()` (lib.rs:391) and `panel()` (apply/queries.rs:44-46), and in launching (apply/launching.rs:49, 144).
- Less often: `found::library_entry` (session/found.rs:120-126, case-insensitive desktop name or `wm_class`) per window being found; status/battery.rs:86, 90; LibraryView `position` by id (library_view/mod.rs:501, 601, 681, 696, 732).
- apply/queries.rs:31-35 already avoids one of these scans by reading the running entry's copy.

How the library changes: replaced on read (`LibraryView::reload`, library_view/mod.rs:589-614, which re-applies listings); a listing replaces a folder's items, reusing and freeing slots (`Library::set_listing`, library.rs:195-252, via `LibraryView::listed`, :680-699); `relayout` (favourites, plays, :558-571) and `apply_meta` (:733) change fields no scan above reads. All three paths end in `library_changed` (:576). The Library is built off the state thread (crates/system/src/library/mod.rs:232) and by struct literal in demo.rs and many tests. `entries` is `pub`.

#### Proposed shape
One index in `clamshell_library`, built where the library is built and by `set_listing`, read through `&Library` so no signature changes in the session.

```rust
// crates/library/src/index.rs
pub(crate) struct Index {
    ids: HashMap<Arc<str>, u32>,        // id -> first slot; empty (freed) ids left out
    keys: Vec<(u64, u32)>,              // hash(Entry.key) -> slot, sorted by (hash, slot)
    files: Vec<(u64, u32)>,             // hash(file) of entries neither browsed nor groups
    desktop: Vec<(u64, u32)>,           // hash(ASCII-lowercased desktop file name, and wm_class)
    marks: Vec<(u64, u32)>,             // hash(launcher mark or handed-over app id) -> slot
    runners: Vec<(Arc<str>, Runner)>,   // runner key -> what its entries are, sorted by key
}
pub struct Runner { pub entries: u32, pub all_games: bool, pub pause: bool }
impl Library {
    pub fn entry(&self, id: &str) -> Option<&Entry>;           // via ids
    pub fn entry_by_key(&self, key: &str) -> Option<&Entry>;
    pub fn entry_of_file(&self, file: &Path) -> Option<&Entry>;
    pub fn entry_by_desktop(&self, name: &str) -> Option<&Entry>; // ignoring ASCII case
    pub fn entry_by_mark(&self, mark: &str, handed_over_only: bool) -> Option<&Entry>;
    pub fn runner(&self, key: &str) -> Option<&Runner>;
}
```
- Every lookup confirms the slot's entry against the query, so a hash collision or a stale index gives a miss, never a wrong entry. Duplicates keep the lowest slot, which is what `iter().find` returns today.
- No allocation per lookup: `&str`/`&Path` hashed with a fixed `DefaultHasher::new()`, binary search, `HashMap::get(&str)`. Rebuilds `clear()` and refill, so they allocate only when the library grows.
- Marks move to the library crate: `handed_over_app`, `of_entry`, `is_marked`, `heroic_game`, `lutris_unique` and the mark prefixes depend only on `Entry` and `Library` (session/running.rs:181-200, launchers.rs:112-147). Lutris uniqueness is worked out once per build. `launchers.rs` keeps `Launcher`, `headless` and `of_process`, using the moved prefixes.
- `Session::background.runners` (`set_pausable_runners`, background.rs:194-202) is read from `Library::runner` keys with `pause`, so it no longer goes stale after a listing (today it is set only on read: lib.rs:273, apply/incoming.rs:84).

#### Keeping it current
Make `entries` private: `entries()` gives `&[Entry]`, `entries_mut()` drops the index. `Library::new(...)`/`with_entries` builds it; `set_listing` and the system loader rebuild at the end. Who changes entries through `entries_mut` (`relayout`'s closure, `apply_meta`, tests) gets a rebuild at the next `LibraryView::library_changed`, or `apply_meta` keeps it (fields not indexed). In debug builds `library_changed` asserts the index matches a fresh build.

#### Steps (commits)
1. `library: make entries private` - accessors, mechanical (system loader, demo, state, tests).
2. `library: index entries by id` - `entry`, `has`, LibraryView positions.
3. `library: index entries by key, file and desktop name` - follow_units, wanted_down, front_launcher, `Launcher::entry`, found.rs, battery.rs, `entry_of_file`.
4. `library: index runners` - background_of, follow_runner_pauses, pausable runners.
5. `library: index launcher marks` - move mark code, use in start_by_app_id, follow_units' `mark_of`.

#### Risks
- Step 1 touches many files (86 `library.entries`/`entry` uses in state alone) but changes no behaviour; it is what keeps the index from going stale.
- Building on the system thread adds the index to the `Event` carrying the Library; state-side builds (tests, demo) cost one pass per change.
- Semantics that must not move: first match by slot order, browsed entries included in id/key/mark lookups, `entry_of_file` excluding browsed and groups, case-insensitive desktop/wm_class match being ASCII-only (`eq_ignore_ascii_case`, matching ASCII lowercasing in the hash).

#### Tests
- Existing: session/running/tests.rs, session/launchers/tests.rs, library_view/files/tests.rs, library_view/tests.rs, the background tests named in lib.rs:557-560, and tests/apply_alloc.rs (must stay at zero).
- Add (library): each lookup equals the linear scan for every entry of `demo::library()` and after `set_listing` frees and reuses slots; duplicate ids/keys/marks return the first slot; a collision-forced index (test hasher) still confirms.
- Add (apply_alloc): a running entry with a runner that pauses, and the Library showing a selected browsed file, so `background_of` and `entry_of_file` are measured.

#### Owner's choices
- Owner: (a) the Library, as above (recommended: `Library::entry` gets faster everywhere, no session signature changes); (b) LibraryView owns the index and the session takes an `Indexed<'_> { library, index }` (some twenty session functions change, `Library::entry` stays linear); (c) the Session keeps a marks/runners index rebuilt with the pausable runners (smallest change, but needs a rebuild after listings and still scans by id).
- Staleness guard: (a) private `entries` (recommended); (b) keep `pub entries` plus a debug assertion only: smaller diff, stale index possible in release after a missed rebuild.
- Keys: sorted `(hash, slot)` vectors (recommended: rebuilds reuse capacity, paths not cloned) or `HashMap<String/PathBuf, u32>` (simpler code, clones every key and path on each build).
