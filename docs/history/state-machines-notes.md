# State machines: suspects, decisions and change notes (history)

Moved out of `docs/state-machines.md` on 2026-10-26, when its second half was rewritten as state machines.
This is history: what was suspected and fixed in the state crate on 2026-10-23, the decisions taken on
those suspects, and the notes written while the compositor was built (2026-10-24 to 26). The file and line
references are as they were then, and some of the notes were no longer true when they were moved (for
example "X11 menus and clipboard bridging remain phase 2 work", "native dmabuf ... remain separate work",
"no libinput backend is enabled yet"). What holds now is in `docs/state-machines.md`.

The references to S16 and S24 that remain in `state-machines.md` point here.

## Suspected problems

Session and dock:
- S1. A tray-only program that exits by itself shows "didn't start" (`session/running.rs:261`): `seen` only
  means it had a window. Fixed: state/session: a program with a tray icon has started.
- S2. A scope of an entry not in the library (window-found, tray-found, browsed) is never re-adopted after
  a restart (`running.rs:222`); its windows and tray are skipped as ours, so it runs with no dock tile and
  can't be quit, and a Frozen hold for it is dropped so its slice stays frozen. Fixed: state/session: a found program's scope is found again after a restart.
- S3. After a tray program's scope is adopted, its tray menu is missing until the tray thread re-reads the
  icons (`tray.rs:122`). Fixed: state/session: a tray icon stays with its adopted program and gives no pid.
- S5. Show when every window is moved down focuses an empty top group (`running.rs:488`). Fixed: state/session: Show of an entry whose windows are all below shows them there.
- S6. A quit the program refuses (a "save changes?" cancelled) stays quitting indefinitely: never paused,
  and ended when its windows later close (`running.rs:566-631`). Fixed: state/session: a quit whose windows stay 10 s is given up.
- S7. A window-found entry without its own process gets a tray icon's pid; Quit then signals it (`tray.rs:111`).
  Fixed: state/session: a tray icon stays with its adopted program and gives no pid.
- S8. A runner paused again while a launch waits more than 10 s for its scope (`background.rs:447`). Fixed: state/session: a launch waiting for its scope keeps its runner resumed.
- S9. A launch the launcher never answers stays launching, blocking a relaunch (`running.rs:542`). Fixed: state/session: a launch with no answer in 30 s is given up.
- S20. Back at the top of the dock's tray menu closes the whole menu instead of going back to the entry menu.
  Fixed: state/dock: Back goes up to the entry's menu; menu and move end with their entry.
- S21. Dock leftovers: `close` keeps the tray path; the menu stays open after its entry left; a Library
  read can drop the item being moved; a long press while moving opens the menu with the move on.
  Fixed: state/dock: Back goes up to the entry's menu; menu and move end with their entry.

Second screen, device, pad, keymaps:
- S10. After the account password editor saves or cancels, the screen stays on Keyboard (`tabs.rs`, `editor_done`),
  against `Event::EditOnKeyboard`'s doc. Fixed: state/bottom: one editor on the keyboard, which goes back when it closes.
- S11. A text field losing focus goes back from Keyboard even while an editor is open there (`bottom/mod.rs:204`). Fixed: state/bottom: one editor on the keyboard, which goes back when it closes.
- S12. Opening the password editor while naming a file drops the name typed (`bottom/editor.rs:117`). Fixed: state/bottom: one editor on the keyboard, which goes back when it closes.
- S13. Modifiers armed on the capture screen survive leaving it by tab or Home (`keymap/screens/mod.rs:573`). Fixed: state/keymap: modifiers armed on the capture screen go with it.
- S14. The gyro's "hold still" notice expires after 2 s whatever the calibration does. Fixed: state/bottom: the gyro's hold still shows until it answers.
- S15. Showing the program's windows below (dock tap, BottomWindows) while the screen is off by hand shows
  them on a dark panel (`bottom/screen.rs:310`). Fixed: state/bottom: showing the windows below turns the screen on.
- S16. A screen dark for a game wakes only on the Wake tap, not a touch or pad focus, and not when the tap
  is swallowed by idle (`screen.rs:154, 214`). Kept, by decision.
- S17. Two taps on Sticks before the first progress start the calibration twice (`device/mod.rs:151`). Fixed: state/device: sticks and remote login wait for their answer.
- S18. Two taps on Ssh send it twice (`device/mod.rs:149`). Fixed: state/device: sticks and remote login wait for their answer.
- S19. The pad switched to the second screen goes back when the claimant changes (the keyboard coming
  up): the documented exception never applies (`nav/pad.rs:150`). Fixed: state/pad: switched to the second screen, the pad stays until View or shelf.
- S22. Forgetting a keymap that was set globally leaves games that chose it on their runner's keymap, not
  Default (`settings/mod.rs:569`). Fixed: state/settings: forgetting a keymap sets who chose it to Default.

Shelf, files, radios, levels, status:
- S23. A tap on an entry not in the list leaves the focus on it. Fixed: state/shelf: a tap on an entry not in the list does nothing.
- S24. A long press on the selected entry by touch plays it (`shelf/rules.rs:17`). Fixed: state/shelf: a long press on the selected entry opens its actions by touch too.
- S25. A relayout (favourite, play) doesn't move the pointer's focus with the selection (`rules.rs:141`). Fixed: state/shelf: a relayout moves the focus with the selection.
- S26. A Library read drops the typed filter. Fixed: state/shelf: a library read keeps what is typed.
- S27. Copy/Cut with only stale selected entries makes an empty clip that Paste offers and ignores (`files.rs:316`). Fixed: state/files: a listing drops selected entries that are gone.
- S28. A cut clip is cleared before the move finishes, so after a failed move it is gone. Fixed: state/files: count the operations running, and keep a cut entry until moved.
- S29. `busy` clears at the first of several operations' Done (`files.rs:415`); without a Done it never clears. Fixed: state/files: count the operations running, and keep a cut entry until moved.
- S30. A Wi-Fi or Bluetooth reading wipes the local error and busy mark, even one older than the command. Fixed: state/radios: a busy mark and a local error stay until a reading answers them.
- S31. Setting a muted stream's volume shows it unmuted but leaves it muted (`levels/mod.rs:370`). Fixed: state/levels: setting a muted stream's level unmutes it.
- S32. Dimming at brightness 0 raises it to 1 (`levels/mod.rs:264`). Fixed: state/levels: dimming never raises the brightness.
- S33. The battery warning fires on the charger in bypass mode (`status/mod.rs:103`). Fixed: state/status: the battery warning keys on discharging.
- S34. The battery tab's spender menu stays open across tabs and after its row is gone. Fixed: state/status: the battery menu closes when the tab is left or its row goes.
- S35. A relayout doesn't close a group that left a derived collection (Recent, Favourites). Fixed: state/shelf: a relayout closes a group no longer in its collection.
- S36. A folder listing that changes the entries but not shelf's view isn't asked for metadata first. Fixed: state/shelf: a listing that changes the entries shown asks for their metadata first.
- S37. The Mute key shows its notice when the volume is unknown and nothing changes (`keys.rs`). Fixed: state/levels: the Mute key shows its notice only when the mute changed.
- S38. The resume record can leave out a collection whose stored place is past its list after a relayout
  (`shelf/mod.rs:552`). Fixed: state/shelf: the resume record keeps a place past its shrunk list.
- S39. An entry known by its app id (a Steam game) whose launch's unit ends while its window is open
  runs on by its windows, but its play is then recorded from when the unit ended, not from its launch
  (`running.rs` `follow_ours`); found by the test filling the GAP. Fixed: state/session: a play that goes
  on by its windows counts from its launch.
- S40. Suspend of an entry in view (the usual case, from the dock below it) freezes it and thaws it in the
  same event, since its window is still shown when the event's steps follow and a held entry counts as
  in use when shown; the existing test handled the event without its steps. Fixed: state/session: a
  suspended entry stays frozen until shown after it was hidden.

Found by the randomized event-sequence test (`random.rs`):
- S50. A launch given up that starts late isn't followed while another launch of the entry waits: its
  scope is skipped (`running.rs`, `follow_units`), so its windows are taken for a program found by them,
  with its scope listed. Fixed: state/session: a scope listed runs its entry while another launch waits.
- S51. Suspend of a quitting entry freezes it, so it can't close its windows and end, and it stays quitting
  and paused (`background.rs`, `suspendable`). Fixed: state/session: a quitting entry isn't suspended.

## Decisions on the suspects (2026-10-23)

Decided without a design discussion, to get them fixed. **Revisit** marks the ones that might need it,
with what would make us change our minds.

- S1. A program that had a tray icon counts as seen: ending without a window isn't "didn't start".
- S2. A scope of an entry not in the library is adopted as an entry found by its scope (named from its
  windows or tray icon as when it was found), so it is on the dock and can be quit and thawed.
  **Revisit** if adopting what an earlier ClamShell found turns out to bring back programs the user
  doesn't think of as theirs.
- S3. Adoption keeps the tray key, so the menu stays.
- S5. Show of an entry whose windows are all moved down shows them below, as the dock tap does.
- S6. A quit whose windows are still there 10 s after the last close is abandoned: the entry runs on as
  before, unpaused rules apply again. **Revisit** the 10 s, and whether a second Quit should then
  force-kill.
- S7. A tray icon gives its pid only to an entry found by that program's own process.
- S8. A launch waiting for its unit counts as starting, so its runner isn't paused again.
- S9. A launch with no answer from the launcher in 30 s is given up, with the "didn't start" notice.
  **Revisit** the 30 s for slow starts (Proton games on first run). The launcher's failure arriving after
  that is ignored, so the notice isn't shown twice.
- S10, S11, S12. An editor opened on the keyboard goes back where it came from when it closes, whatever
  its purpose; a text field losing focus doesn't switch away from an open editor; a second editor for
  the keyboard while one is open there is refused with a notice, so nothing typed is lost. **Revisit**
  if refusing turns out to block something the user expects to interrupt (the account password). S12
  added a notice kind for it, KeyboardBusy.
- S13. Leaving the capture screen any way drops the modifiers armed there.
- S14. The gyro's notice stays until the calibration answers, at most 30 s.
- S15. Showing the windows below turns the screen on if it was off by hand.
- S16. Kept: a screen dark for a game wakes only on the Wake target, so a stray touch mid-game doesn't
  light it. **Revisit** after using it on the device.
- S17, S18. The sticks calibration and the remote-login toggle wait for their answer before taking
  another tap, at most 30 s (for the sticks, 30 s since the last progress), as the gyro's notice does, so
  a dead thread or helper can't block them for good.
- S19. Implement the documented exception: the pad switched to the second screen stays there when the
  claimant changes, until View is pressed or shelf comes to the front. **Revisit** if it feels sticky
  on the device. Applied literally, this includes the pad mouse: the pad stays on the second screen when
  the mouse turns on (by hand or by a game), so the mouse does nothing until View.
- S20. Back at the top of the tray menu goes back to the entry's menu.
- S21. The dock menu closes when its entry leaves the dock; a move ends when its item goes; `close`
  clears the tray path; a long press while moving ends the move first.
- S22. Forgetting a keymap sets those who chose it to Default, as the docs say.
- S23. A tap on an entry not in the list does nothing.
- S24. A long press on the selected entry opens its actions, by touch as with the pad.
- S25. A relayout moves the focus with the selection.
- S26. A library read keeps the typed filter.
- S27. Copy or Cut with nothing valid selected does nothing; a listing drops selected ids that are gone.
- S28. A cut clip is cleared when its paste succeeds.
- S29. `busy` counts the operations running.
- S30. A local error (needs a login) and the busy mark stay until the next attempt or a reading that
  answers it (connected, or the system's own error). **Revisit** if a reading can't tell an old attempt
  from a new one.
- S31. Setting a muted stream's level unmutes it, as the output volume does.
- S32. Dimming never raises the brightness.
- S33. The battery warning keys on discharging.
- S34. The battery menu closes when the tab is left or its row goes.
- S35. A relayout closes a group no longer in its collection.
- S36. A listing that changes the entries shown asks for their metadata first.
- S37. The Mute key shows its notice only when something changed.
- S38. The resume record keeps every collection's place, clamped to its list.
- S39. A launch that goes on by its windows (app id or browsed file) keeps its launch's start as the
  play's start.
- S40. A suspended entry counts as shown again only once its windows have been seen hidden; one found
  frozen at start counts as hidden already.
- S50. A unit listed runs its entry whatever launch of it waits; the waiting launch joins it when its
  own unit comes.
- S51. A quitting entry can't be suspended; the dock menu's Suspend does nothing for it.

## Notes from building the compositor (2026-10-24 to 26)

The sections below are as they stood; their headings name the modules of the time.

### In-process window presentation (compositor/)

Window limits, role and optional colour description are facts. Changing them preserves selection and
placement. Normal game windows fill their viewport with aspect-preserving fitting; normal apps use
native scale. Fixed-size windows, dialogs and utility/splash roles are centred at native scale and
shrink only when necessary. Rendering and pointer input share the same viewport, origin and scale;
black around the client does not activate the shelf behind it. Colour metadata is reserved for future
colour management; the host does not advertise HDR.

The state decides on `Opened` whether a window comes in front. While a launch waits for its window,
only its own windows select themselves (in its unit, of its entry
by app id or process, or a dialog of one); others open behind, as Steam's windows behind a Steam game's
launch (`steams_windows_open_behind_a_steam_games_launch`). Otherwise an external client's first window
selects itself, a first window of our own unit waits for foreground launch intent, and a later window
may select itself only when that program is shown. A dialog selects itself when its parent's program is
shown. Tests: `opening_focus_respects_launch_intent_and_shown_program`,
`a_window_opens_in_front_as_its_opening_focus_says`, the presentation
policy tests, metadata-change and Home-tap tests in session/windows, and the randomized state test.
The headless compositor protocol test checks first configure, fixed-size child placement and close;
geometry tests check native sizing, fitting and inverse coordinates. `tests/nested-compositor.py` checks
real client pixels across top, bottom, return and Home. The remaining protocols are pending.

### Compositor touch routing (compositor/touch)

A contact starts on a client surface, a UI screen, an overlay's target, or black. Client contacts retain
their surface and inverse geometry from down until up; each slot has its own target, so both screens can
be touched simultaneously. Motion across screens keeps that target. UI contacts use the existing Core
press/move/release path on their initial screen. An overlay's targets, those of the screen's overlay
frame drawn last (`Core::press_overlay`), act once on down and consume the contact: a Home arrow
switches windows, a notification is tapped, its close mark dismisses it (tests
`touching_a_home_arrow_switches_once_and_consumes_home_release`,
`touching_a_notification_taps_it_and_its_close_mark_dismisses_it`);
black stays ignored even when the finger moves over a client. Unknown motion/up events do nothing.
Tracking is bounded to 32 simultaneous contacts; additional downs are ignored. Reusing an active slot
cancels the previous sequence before accepting the new down.

Cancel, loss of host focus, resize, or a client being hidden/unmapped/removed cancels the active sequence
and clears UI presses. Late up/motion after cancellation does nothing. The pinned Smithay cancellation
path only visits slots pending since its last frame; the adapter repeats active client positions before
cancel so stationary fingers also receive cancellation. Winit reports physical window coordinates and
has no touch frames; the nested adapter finishes each down/motion/up as a frame. These host details stay
outside the contact routing, which the DRM/libinput backend can call after output mapping.

`touch_contacts_keep_independent_surfaces_and_native_coordinates` uses real wl_touch traffic over a
socket pair. It pins independent surfaces, native and scaled coordinates, cross-screen dragging,
black margins and bottom-bar exclusion, cancellation after a completed frame, new contacts after
cancellation, hiding a touched client, and ignored late events.
`touching_a_home_arrow_switches_once_and_consumes_home_release` checks that an arrow contact switches
once and consumes Home release. Physical touchscreen input remains a nested-host/device check; no
libinput backend is enabled yet.

### Compositor text input and on-screen keys (compositor/keyboard)

Text-input-v3 enable/commit from the focused client sends TextInput(true) through Core; disable,
destruction of the active text input, or a keyboard focus change sends TextInput(false). State owns
raising and restoring the keyboard tab, including its existing editor guards. The compositor acts as
the seat's input method; no external input-method process is required.

A printable OSK stroke without Ctrl/Alt/Super commits its shifted character to the active text input,
followed by done with the client's commit serial. Without active text input it becomes a seat key
press/release. Shortcuts and named keys always use seat events: modifiers down, key down/up, modifiers
up in reverse order. A separate keyboard source preserves physically held keys when OSK modifiers are
released. Injected keys bypass nested development shortcuts. Unsupported keys and strokes without
keyboard focus do nothing. Bottom UI interactions retain the client's keyboard focus; the shelf can
claim it. UI editors continue to consume their own strokes in state.

`the_bottom_keyboard_commits_text_and_sends_shortcuts_without_stealing_focus` exchanges real
wl_keyboard and text-input-v3 messages over a socket pair. It pins commit strings and serials,
modifier order, native key fallback before enable and after disable, physical Shift retention, F12
delivery, the state-to-effect route, tab restoration and silence after focus loss. The touch protocol
test continues to pin the two-screen routing. Real toolkit activation is checked in nested mode.

### Compositor idle inhibition (compositor/idle)

Nested input restarts a ten-second quiet timer. Quiet and resumed transitions send CompositorQuiet /
CompositorInput to the existing device idle machine through Core. A mapped surface belonging to a
visible window, including a subsurface or popup, can inhibit that timer with idle-inhibit-v1. Hidden,
unmapped and dead surfaces cannot. Each inhibitor is counted separately; destroying one removes only
that inhibitor. Dead surfaces are discarded. Ending inhibition starts a fresh quiet interval.

`quiet_input_and_inhibition_report_only_transitions_and_restart_the_timeout` pins timeout thresholds,
repetition, resumption and inhibitor removal.
`inhibitors_count_individually_and_only_mapped_visible_surfaces_keep_idle_off` uses real protocol
objects to check mapping, hiding/showing, subsurface ownership, multiple inhibitors, unmapping and
surface destruction. The existing state idle tests pin dim/off/sleep effects and the desktop sleep
guard; no separate policy is introduced in the compositor. The nested host still leaves physical
output power to its parent; actual output power belongs to the DRM backend.

### Nested clipboard and selection focus (compositor, system/session/clipboard)

Smithay owns data-device, primary-selection and ext-data-control protocol objects and offer transfer.
Keyboard focus selects the client receiving clipboard and primary offers; losing focus clears both.
The existing keeper remains a system thread and retains its text-only, primary-mirroring, password
manager and size-limit rules. The nested host passes its socket path to that thread explicitly.
The process environment is never changed.

Nested saved text goes under `$XDG_RUNTIME_DIR/clamshell/compositor/<socket>/clipboard`, separately
from the session's `$XDG_RUNTIME_DIR/clamshell/clipboard`. The nested compositor neither reads the
parent's saved text nor replaces it. `tests/nested-compositor.py` checks CLI set/get, persistence after
the source exits, native Foot OSC52 copy through wl_data_device, primary paste back into Foot, and
an unchanged parent clipboard.

### One-shot game-pad activity (state/sync, system/input/pad)

Top content without keyboard focus arms PadActivity(true). A game-pad press or deflection sends one
PadActive and disarms the router locally, before another input can be reported. State restores top
keyboard focus and sync sends PadActivity(false). Further game input sends no activity events. Losing
keyboard focus again rearms the watch; repeated focus observations do not. The pad supervisor retains
the watch while the controller is absent and installs it on reconnection.

`game_activity_is_reported_once_per_keyboard_focus_loss` exercises hundreds of presses and deflections
with no watch, then one report per arming. The session keyboard-focus and pad-activity tests pin the
effect sequence and unchanged placement. The randomized invariant checks the watch against top
keyboard focus after every event. Snapshot pixels and focus are unchanged; the trace adds only watch
arming/disarming in the three scenarios that move keyboard focus away and back.

The nested host caches each client's window presentation at Core's state revision. Repeated frames
and pointer hit tests read that cache. Any applied event, including UI-generated events, invalidates
it; the next window sync derives it again. The protocol test changes a live window's role and checks
its new configure policy, as well as unchanged revisions across repeated syncs.

### Client failure containment (compositor/server, render)

One wl_surface has at most one live toplevel. A duplicate is rejected before another window enters
the model. A failed client buffer import or popup setup rejects that client, with a logged reason;
other windows and the event loop continue. Wayland protocol disconnects are logged; normal disconnects
are debug messages. Host renderer/display failures remain session failures, as there is no usable
display to continue on. No X11-specific path has been introduced in state.

`a_protocol_error_disconnects_only_its_client` constructs a duplicate role on a separate real Wayland
connection and then changes a healthy mapped client's title, asserting its window and session remain
alive. `xdg_clients_are_placed_before_configure_and_follow_state_without_a_gpu` now also stops
acknowledging configure, retains an oversized buffer across a screen move, and checks the fitted
transform. It unmaps and remaps the same surface, reapplying xdg properties and checking that the
unowned window starts above again. Fixed-size child presentation remains in the same test. Renderer import paths are exercised by
`tests/nested-compositor.py`; a failed client import never propagates as a host-loop error. Xwayland
supervision and X11 toplevel fixtures are implemented; X11 menus and clipboard bridging remain phase 2 work.

### Window-event diagnostics and replay (compositor/trace)

Core's optional observer sees inputs before apply and cannot change state. The compositor retains the
last 256 window inputs, preserving atomic batches, plus Home/window chords and pad focus restoration.
Each record has a sequence, both clocks and the preceding top/bottom/keyboard selection and window
count. Irrelevant inputs do not enter the ring. At capacity, a new record discards the oldest. No
recording work occurs in an unchanged frame; strings are copied only for recorded window metadata.

`window events FILE` becomes one DumpWindowEvents effect. The compositor snapshots the ring and final
selection, then a worker writes JSON with mode 0600. Success or failure is logged; an export failure
does not change the session.

`trace::Recording::replay` applies the recorded clocks and inputs to a matching state fixture and
checks each preceding selection and the final one. The fixture supplies the library, rules and launch
context; a wrapped ring starts at its first retained event. A mismatch is reported with its sequence.
`recorded_window_batches_and_chords_replay_through_state` checks round-trip serialization, batch
preservation, replayed window facts and keyboard focus, fixture mismatch, file permissions and export
failure. `the_ring_keeps_only_the_last_256_window_inputs` pins wrap order and capacity. State and control
tests pin the diagnostic effect and path; its event is in the randomized generator. The nested smoke
test exports the live ring through the control command and parses its final empty-window selection.

Window facts carry no Wayland/X11 transport tag. Adapters translate client properties into parent, role,
limits, identity and colour facts; state rules use those facts uniformly. Session tests check identity changes
and parent inheritance without a transport-dependent branch. This removal preserves snapshot pixels, focus and
trace.

### Coalesced client titles (compositor/server)

A title request for an open Wayland toplevel marks that client's title dirty. At the next rendered
frame, the host reads its final title and emits one Title event if it differs from state. Repeating
the same title emits nothing. Initial titles arrive with Opened; unmapping clears a pending title,
and a removed window has nothing left to flush. The ordinary event-loop turn does not flush titles,
so several protocol dispatches before a redraw still produce at most one update per rendered frame.

The headless lifecycle client sends 101 different titles in one batch and then 100 identical ones.
`xdg_clients_are_placed_before_configure_and_follow_state_without_a_gpu` asserts the final title and
exactly one Title record in the event ring across both batches, alongside the unmap/remap checks.
Headless tests step the same frame boundary as the nested host.

### In-process screenshots (compositor/render, system/display/screenshot)

Screenshot effects in the nested host request one capture of the next rendered scene. Requests before
that frame coalesce. Readback copies the combined logical screens to top-first SDR RGBA8 pixels, then
restores the window EGL target before presentation. Readback failure is logged and forwarded as a
screenshot error; it does not escape into the host loop. The binary passes pixels or the error to
Handles, whose system worker encodes/saves PNG and sends the existing Screenshot result event.
The state owns the success/failure notice. The DRM host captures the same way.

`screenshots_are_consumed_by_the_host_instead_of_the_parent_capture_tool` pins coalescing and that the
effect stays with the host. System screenshot tests decode a saved PNG to check size and row
order, and check invalid images, missing folders and write failure. The nested smoke test saves a
complete PNG in an isolated home, checks its dimensions, and continues with client rendering,
clipboard, screen moves and Home. This catches losing the EGL window binding during readback.
Readback is an SDR file conversion after composition; it does not set client/target precision or
advertise HDR. Future HDR screenshot tone mapping remains part of the colour-management work.

### Client viewports (compositor)

The nested host advertises viewporter. A committed source rectangle crops the buffer after its
buffer transform and scale; the destination sets the logical surface size. Presentation fits that
logical size to the selected screen, and hit testing uses the same inverse transform. Destroying a
viewport clears source and destination on the next commit, restoring the buffer-derived logical size.
Smithay validates viewport requests through the existing buffer commit handler; protocol errors end
that client under the ordinary client-isolation rule. No viewport facts enter state, and no pixel
format conversion is introduced.

`viewport_destination_drives_geometry_and_destroy_restores_buffer_size` drives a real socket-pair
client with a 640×480 buffer at scale 2, a cropped source and a 600×400 destination. It checks logical
geometry and centre hit coordinates before and after destroying the viewport, with one state window
throughout. Buffer cropping and sampling use the renderer's existing surface-tree path.

### Real EGL client coverage (compositor)

The nested smoke harness starts the standalone `egl-client` example alongside Foot. The example is a
Wayland EGL client using the existing safe renderer API, with no session state or system workers.
Its distinct solid colour lets the harness check top presentation, moving below and returning above
while Foot remains visible on the other screen. A close chord above delivers xdg close, the client
exits normally, and the previous window reappears; Home still returns to the shelf afterwards.

`tests/nested-compositor.py` checks those pixels and process outcomes under headless sway with software
EGL, alongside its shared-memory, clipboard and screenshot checks. The EGL fixture is built separately
as stated in the script. This exercises EGL rendering over the available software transport; native
dmabuf advertisement/import and hardware GPU-client coverage remain separate work.

### Native dmabuf imports (compositor/dmabuf)

A native EGL renderer advertises exactly its dmabuf formats and modifiers, retaining higher-precision
formats. A software EGL device advertises no native dmabuf global: its import-format list does not
guarantee that native Vulkan allocations can be sampled, and a deferred import can fail inside Mesa
after EGL accepted it. The software smoke test asserts the absence of that global in a real EGL
client's registry traffic, then checks rendering, moves and close. A renderer with no formats advertises no dmabuf global. When EGL identifies a render device,
feedback describes that device and its sampling formats; otherwise the host advertises version 3.
Feedback-file creation failure is logged and also uses version 3. No scanout preference is advertised
by the nested backend.

A create request queues an import for the next redraw. The renderer validates it before scene drawing:
success completes the buffer notification once; failure logs the reason and posts a fatal client
protocol error. Each client may have at most 64 pending imports in one dispatch; exceeding that limit
ends that client. Queued notifications are failed on host shutdown. A frame with imports forces a full
render, restoring the EGL window target even if scene damage would otherwise be empty. No window event
or state apply is produced by buffer imports, and no intermediate 8-bit conversion is used.

`a_rejected_dmabuf_disconnects_only_its_client` receives real version-3 format events and version-4
feedback, including a 10-bit format and the native-byte format table/device. It submits a buffer to a
mock renderer boundary, asserts that boundary was reached exactly once and rejected the requesting
connection, then changes a healthy mapped window's title. The accepted-import protocol test uses a
mock successful boundary to assert one created buffer and no work on an empty queue. This tests
protocol completion, not a GPU importing an ordinary file. The nested software-EGL smoke test remains
green. Its `--gpu` mode uses a headless GLES2 host and hardware EGL, checks rendered pixels across
screen moves/close and requires native dmabuf create requests in the fixture's protocol log. It fails
rather than accepting shared-memory transport in that mode. Both modes passed on the development
desktop; the hardware run submitted AMD-modified 10-bit buffers and rendered the expected colours.

### Popup menu grabs (compositor/popup)

A menu belongs to its root toplevel and never creates a state window. A visible, keyboard-focused
parent may grant a popup grab on the session seat; other parents or seats receive popup_done.
Malformed roots and failed popup setup end that client with a logged reason. A conflicting seat grab
dismisses the popup chain instead of replacing the other grab. Nested popups share the parent's grab
chain. The pointer grab is installed before the keyboard grab: replacing a parent pointer grab releases
its matching keyboard grab, so reversing this order would release the newly installed submenu grab.

Normal window synchronisation keeps keyboard focus on the current popup while the root remains
selected, visible and mapped. Destroying a submenu restores its parent popup; destroying the last menu
restores the toplevel. Touch or pointer presses outside the parent window dismiss the chain before
routing the new input. Home, root unmapping, root destruction, hiding the root or moving keyboard focus
to another window dismisses the chain and releases its seat grabs. Home still clears keyboard focus
and shows the shelf, even if the client leaves its dismissed popup resources alive.

`menus_keep_popup_focus_and_dismiss_on_outside_touch_home_or_client_destroy` uses a real socket-pair
xdg client and seat input serials. It checks popup hit coordinates and focus, outside touch dismissal,
Home while the menu still exists, nested menu focus and restoration, parent unmapping/remapping and
client destruction. The exact pointer-before-keyboard order is pinned by the nested submenu's live
keyboard grab. State's window count remains one throughout menus, and zero while the parent is unmapped.
No new state events or protocol-specific state facts are introduced. The nested software and GPU smoke
harnesses check that the ordinary client presentation/input paths still run with menu support enabled.

### Compositor cursors (compositor/cursor)

Pointer motion makes the compositor cursor visible and restarts a three-second timeout. Touch down,
motion or up hides it immediately; nested host focus loss also hides it. Only pointer motion shows
it again. The visibility deadline wakes the loop even without a further input event. The
host window's own cursor is disabled. Client cursor requests are delivered by Smithay's seat handler:
Named selects an xcursor theme image, Hidden draws nothing over that client, and Surface draws the
client's committed cursor tree with its hotspot. Cursor-shape-v1 provides named requests. The protocol
handler ignores stale enter serials. Cursor surfaces retain their buffer format; import failure rejects
that client and restores the default cursor instead of ending the session. Dead cursor surfaces also
fall back to the default. No cursor fact enters State.

A cursor is composed above the UI and windows in output coordinates, independently of window fitting.
When the pointer targets the shell, or Home hides the window, a client's custom or hidden cursor no
longer applies. Theme images are chosen by nominal size, uploaded once per requested icon and retained.
Animation follows frame delays, wraps and skips zero-delay frames; all-zero delays select the first
frame. A missing or unreadable theme uses the built-in pointer with a logged reason. An unchanged
cached theme frame does not reload or upload its pixels. Cursor surface frame callbacks allow client
animations to continue.

`cursor_requests_keep_hotspots_ignore_old_serials_and_release_on_home` drives real pointer and
cursor-shape requests, asserting named, hidden and custom states, hotspot, stale serial handling,
Home and cursor destruction. The visibility test pins the three-second boundary, repeated movement, immediate hiding and motion
restoration; the real touch test checks down, motion and up hiding a visible cursor. The animation
test pins frame boundaries and wrap. The nested smoke
harness creates a virtual pointer only in its isolated parent sway, checks cursor pixels inside the
presented scene (without the parent's cursor in grim's capture), moves it and checks both damage positions,
then checks the shell's themed cursor, its idle disappearance and reappearance on motion. Both software and GPU modes exercise these steps alongside
screenshots and client rendering.

### Vulkan WSI client coverage (compositor)

The GPU smoke harness runs `vkcube --wsi wayland` beside the EGL fixture. It checks that the cube
presents changing frames above, remains animated after moving below and resizing, and keeps presenting
when returned above. Closing delivers the normal close request and exits the client successfully;
the previous terminal returns and the existing Home, clipboard and event-ring checks still pass.

The Vulkan protocol log must contain Wayland buffer attachments, native dmabuf create requests,
a feedback request, a format table and feedback completion. Animation is checked from successive
scene captures, not inferred from process liveness or protocol requests alone. Vulkan runs in the
explicit `tests/nested-compositor.py --gpu` mode, with a native EGL renderer, accessible render node,
Vulkan driver and Wayland-capable vkcube. Software mode keeps the EGL/shared-memory tests and pins that
software EGL advertises no native dmabuf transport. No software-WSI override or pixel-format downgrade
is added to application launches. Both modes passed on the development desktop; their screenshots
and full protocol logs are retained by the harness.

### Xdg activation (compositor/activation)

A client may mint a token only while its mapped window owns keyboard focus in State and on the seat,
using this seat's serial no older than its latest keyboard enter. An optional source surface must
belong to that client. Missing/old serials and background clients still receive an opaque token string,
but it is untracked and grants no activation. Tokens expire after five seconds and unused expired
tokens are pruned on event-loop turns.

A known token is consumed on its first activation request, including a rejected request. A recent token
for a mapped toplevel becomes the existing WindowEvent::Activate; ordinary state selection, keyboard
focus and bottom presentation rules settle before synchronisation. Activation never moves a window
between screens. Unknown surfaces, unmapped windows and expired or reused tokens do nothing. A token's
app_id is advisory and does not override window identity or placement.

Returning keyboard focus to the shelf clears outstanding tokens. Both creation and redemption read
State's current focus, so Home also wins if it settles before the seat is synchronised in the same
dispatch. A pending activation cannot undo Home. Policy denial is an ordinary protocol outcome; it
does not disconnect the client or change the session.

`activation_tokens_require_focus_expire_are_single_use_and_cannot_undo_home` drives real activation,
keyboard and xdg-shell traffic. It pins missing/old serials, a background connection borrowing another
client's serial, Home revocation, successful activation, reuse, expiry, unknown surfaces, activating
below without changing placement, and a target unmapping. Direct callbacks with real token data also
pin the pre-synchronisation Home race and an expired request before periodic pruning. Existing state
activation tests pin resulting effects and their order; no new state event or randomized-generator
branch is introduced. The nested software and GPU harnesses check coexistence with the EGL, Vulkan,
clipboard, cursor and screen-move paths.

### Steam's default interface (system/launch, thor-session)

Steam's packaged desktop entry, including steam:// links, requests `-gamepadui`. Starting its client
for a Steam game also requests `-gamepadui`, keeping `-silent -applaunch ID` so a cold game launch
still suppresses the client window. An existing client receives the same game request. Scope ownership,
following Steam's game launcher and the separation between the client's and game's environments stay
as before. The state continues to emit the same Launch::Steam effect.

The desktop entry is validated with desktop-file-validate when available; argument forwarding through
the packaged Steam wrapper is checked using a temporary client that records argv, without starting
Steam or changing its installation. The client UI itself is checked with Steam on the device; the
workspace tests continue to pin launch effects and scope/follower behaviour.

### Relative pointer and constraints (compositor/pointer)

A backend declares whether it supplies relative motion when constructing the server. Relative-pointer
and pointer-constraints globals are offered only with that capability. The headless harness enables it
and injects relative motion, as the libinput backend will on the device. The current nested winit host
is absolute-only and offers neither global; a separate upstream forwarding patch is not a build
dependency. No local Cargo source override is needed by this checkout or CI.

Relative-only input reports both accelerated and unaccelerated deltas to the focused client, using the
window's inverse scale, and moves the free or confined compositor cursor. A locked cursor stays fixed
and hidden while relative events continue. Winit's dual absolute/relative stream, if supplied by a
future upstream update, positions a free cursor from the absolute event without moving it twice.
Motion never becomes a state event or triggers a UI recording merely to deliver camera input.

A pointer over the keyboard-focused, visible, mapped window may activate its surface's lock or
confinement inside the requested region, after the backend accepts capture. Confinement rejects
positions outside that surface/region and retains a permitted axis of motion. Changing a committed
region to exclude the cursor ends capture. Destroying a constraint, unmapping/hiding the window,
changing focus or geometry, opening a menu, Home and host focus loss release it. Persistent constraints
may reactivate on return; one-shot constraints are removed on release. Cursor-position hints are
advisory and are not applied. No constraint is represented in State.

A failure to obtain host capture posts and flushes a fatal protocol error for the requesting client;
the session continues. Merely marking a client dead after request dispatch can leave its socket
waiting indefinitely, so the shared rejection helper now posts wl_display.implementation on the actual
display object. A host that cannot release capture leaves no usable input path; the nested session
ends rather than trapping the user's pointer. Ordinary releases reach the backend before a later
capture activates.

`relative_motion_and_constraints_follow_geometry_home_regions_and_host_focus` uses a real socket-pair
client, an oversized buffer and injected relative input. It pins free-cursor movement, exact scaled
relative channels, 100 camera events without a state revision, fixed/hidden locked cursors, persistent
unmap/remap, confinement, Home, region commits, one-shot focus loss and rejection when capture fails.
The exact host mode sequence is asserted. `a_host_that_cannot_release_capture_ends_the_unusable_session`
pins the host failure policy. The real-client popup test also acquires a lock and checks that opening
a menu releases it. The nested software/GPU smoke tests assert that an absolute-only host
omits both globals and continue testing cursor, EGL, Vulkan and screen moves with upstream Smithay.

### Presentation feedback (compositor/presentation)

The server advertises wp_presentation with CLOCK_MONOTONIC. A feedback request belongs to the next
surface commit. A successful nested host submission completes the committed callbacks only for mapped,
visible windows and surfaces included in the renderer's presentation states; this includes their
subsurfaces and popups. Rendering alone does not complete feedback, and failed submission ends the
unusable nested host before any presentation is reported. The output comes from the window's current
screen. Completed callbacks are drained once. Hidden or occluded content retains feedback until it is
shown; a newer feedback-bearing commit supersedes it with discarded, and destruction discards pending
callbacks through Smithay's cached surface state.

Winit does not expose a display timestamp or refresh sequence. Its feedback uses the monotonic time
immediately after a successful swap, unknown refresh, zero sequence and no flags: no vsync, hardware
clock, hardware completion or zero-copy claim. This is an estimated software timestamp. The DRM backend
must supply actual flip timing and its matching flags instead of reusing this submission estimate.

`feedback_follows_submission_visibility_superseding_and_surface_destruction` uses a real socket-pair
client to check the clock, exact timestamps, output notification, completion once, missing renderer
states, synchronised subsurfaces, Home/activation, superseded commits and destruction. `tests/nested-compositor.py` also runs a
real feedback client against the software and GPU hosts: both a newly attached buffer and a subsequent
unchanged frame must receive ordered monotonic feedback with unknown refresh and no hardware flags.

### Xwayland windows, transport and activation (compositor/x11)

The adapter owns one reserved X11 display socket and at most one private Xwayland child. The socket
is advertised to nested launches; the process environment and parent display are untouched. The
first connection in its listen queue starts Xwayland with that socket, its private Wayland/WM sockets
and display-number socket. The parent never accepts the pending X11 connection: Xwayland does.

`dormant --X11 listener readable--> starting [spawn, pass listener, insert Wayland client]`.
`starting --spawn failure--> retrying [log, retry after one second while demand exists]`.
`starting --display number, X11 setup, atom and extension replies--> connected
[claim WM/compositing selections, redirect subwindows]`.
`starting/connected --EOF, malformed data, queue limit, request/write timeout or I/O failure-->
stopping [kill only Xwayland, disconnect its Wayland client, close its model windows, log]`.
`starting/connected/stopping --pidfd readable, child exited--> dormant
[reap without waiting, close only its windows, rearm listener]`.
The next X11 connection starts a replacement on the same display. Shutdown kills and reaps any child
on a separate thread and removes only this instance's display socket and lock file.

All socket I/O is non-blocking. x11rb supplies protocol types and sequence tracking, never blocking
connections or Smithay XWM commit hooks. Read/write/displayfd/listener readiness and pidfd are calloop
sources; write readiness is registered only with unsent bytes. Dormant and idle connected Xwayland
have no adapter deadline, probe traffic or periodic wakeup. A selection-ownership reply is requested
only after 32,768 unacknowledged requests to anchor X11's 16-bit sequence before it wraps.
The display-number deadline is five
seconds; setup, queued writes and outstanding replies have three seconds. Partial messages retain
their bytes. Each turn permits at most 64 writes, 64 setup reads, 64 packet reads of at most 4096 bytes
and 64 event deliveries. Packets and outgoing bytes are capped at 1 MiB; packet lengths are checked
before allocating their payload. At most 256 queries are outstanding, deferring further property
requests. At most 128 X11 windows are tracked; an excess ends the offending X11 client. command-fds
passes the four private descriptors only in child-side exec setup, retaining the parent's CLOEXEC.

`created --MapRequest--> querying [attributes, properties, XRes]`.
`querying --initial replies--> placed [Opened → final configure → map → focus]`.
The initial queries are issued after MapRequest, so properties set between creation and mapping
are included in the first placement.
Input-only and override-redirect surfaces are excluded from these toplevels. Class, UTF-8 title with
Latin-1 fallback, transient parent, normal size hints and dialog/splash/utility role become the same
Window facts as Wayland. PIDs come from XRes, never the forgeable _NET_WM_PID property or Xwayland's
own socket credentials. WM_TRANSIENT_FOR also implies dialog presentation when the parent has not
mapped or has gone away, even without an explicit window type. Properties have a 4 KiB query bound and validated types/formats; absent or
malformed properties clear their facts. Title/property notifications coalesce while a query is in
flight, retaining a refresh bit for changes that race its reply. Changed facts coalesce per window
until the next recorded frame, so title traffic never drives repeated UI applies within that frame.
Metadata and title changing together form one ordered batch, preserving a scope adoption that arrived
while the properties waited.
Initial facts and presentation policy
are cached, so unchanged turns emit no state events or X11 requests. The adapter's turn runs named
steps in order: begin setup, dispatch replies, finish setup, anchor the wire sequence, dispatch events,
query window properties, synchronise selections, settle managed windows, settle menus and synchronise
the shared windows. Managed-window settlement publishes each window's facts, binds its surface and
configures/maps it, then settles focus and close requests. The refactor preserves that order and the
existing request-order and state-trace tests.

`placed --WL_SURFACE_SERIAL, the surface given that serial through xwayland_shell_v1 committed--> drawable`.
Pairing by serial (never reused, unlike a surface's id) can't pair a window with a later surface.
The serial is 64 bits, in two 32-bit halves on each side (tested by
`a_window_is_paired_with_the_surface_committed_with_its_serial` and
`a_committed_serial_pairs_its_surface_once_and_only_xwayland_sees_the_shell`).
The shared surface wrapper uses the same rendering, geometry, input, frame and presentation-feedback
paths. A window ignoring its configured size is treated by the existing geometry policy. X11 size
requests cannot choose placement or stacking. WM_HINTS input and WM_TAKE_FOCUS are honoured when focus
changes. Each focus change first gets a fresh server time: `focus changed --> stamping [empty append to
_THOR_TIMESTAMP on our window]`, `stamping --its PropertyNotify--> [SetInputFocus and WM_TAKE_FOCUS with
that time]`; the time is used once, and one arriving after the change was undone is dropped; no
PropertyNotify within 3 s restarts Xwayland like a missed reply. The server ignores a focus request older
than the last focus change, which an active client may have made itself, so an old observed time could be
ignored silently ([ICCCM](https://xorg.freedesktop.org/releases/current/doc/xorg-docs/icccm/icccm.html#input_focus),
2.1 for the append). Tested by `placement_is_committed_and_configured_before_mapping_and_focus`.
WM_DELETE_WINDOW closes a supporting client, otherwise KillClient ends it.
`mapped --unmap--> known [Closed, detach drawable]`; a later map reopens it with the same model id.
`known/mapped --destroy/BadWindow--> removed [Closed if open]`.
Late replies use unique model ids, so XID reuse cannot modify a replacement. Selection/focus/placement
remain state rules. The clipboard bridge imports and exports both seat selections; there is no
drag and drop. The clipboard bridge's INCR and asynchronous transfer plan is explicit in
`docs/history/compositor-mvp.md`; the implemented export transitions and tests are described below.

Property fixtures check ordinary and fixed transient windows, all supported roles, malformed types,
negative sizes and property deletion, plus focus protocols/hints. Socket-pair tests check partial
setup/replies, full buffers, EOF, oversize rejection, idle silence and outstanding-request deadlines.
The dormant listener test asserts no child/deadline before a connection. The request-order test pins
configure before map and focus, and no repeated commands for unchanged state. The real software/GPU
harness starts a font-independent X11 fixture, checks pixels through top/bottom moves, configure sizes,
unmap/remap, WM_DELETE_WINDOW and a transient preceding its parent, then stops/kills Xwayland while Wayland keeps moving. It checks
reaping, no eager restart, and activation of a replacement by another X11 client.


The focus request-order fixture covers all four ICCCM models: passive (input true, no take-focus),
locally active (input true, take-focus), globally active (input false, take-focus) and no-input (input
false, no take-focus). It asserts direct focus or None, the matching protocol message, the same
nonzero timestamp in both requests, and no repeated commands without a change. The real X11 fixture
changes between those models, rejects zero-time WM_TAKE_FOCUS, responds using the offered timestamp,
and checks actual key press/release delivery. No-input receives no keys. The real harness also checks
Home releasing focus while the bottom parent remains visible, and focus returning after unmap/remap.

### X11 menus and tooltips

Override-redirect windows remain adapter-owned surfaces, never State windows.
`known --map, metadata and buffer ready, visible focused parent--> menu layer`.
WM_TRANSIENT_FOR resolves through live menu ancestors; absent a hint, the focused managed window
must have the same XRes PID. Ownership stays fixed while mapped. Tooltips may use the hovered
same-process parent and do not take input or keyboard focus. Cycles and missing ancestors reject the
menu. Rendering and hit testing share the parent's scale and screen clip, including submenus.
`menu layer --outside press, focus loss, Home, parent geometry change/hide/unmap/destroy--> dismissed
[detach, asynchronous UnmapWindow]`.
`submenu --ancestor unmap/destroy--> dismissed [detach, asynchronous UnmapWindow]`.
`tooltip --parent geometry change/hide/unmap/destroy--> dismissed`.
`layer --surface destroyed--> detached`. Repeated dismissal is harmless. Detached touch targets are
cancelled, and an interactive menu releases pointer capture until dismissed.

The headless surface fixture asserts scaled menu/submenu touch coordinates, passive tooltips,
stationary-touch cancellation and unchanged State window count. Adapter tests assert same-PID
inference, tooltip hover ownership, fixed ownership, ancestor unmap and bounded transient cycles.
The real-client software/GPU harness checks menu pixels and local pointer coordinates, submenu
stacking and ancestor dismissal, parentless inference, clipping at the screen boundary, tooltip input pass-through, outside presses,
parent screen moves and Home while the bottom parent remains visible.


### Wayland selections exported to X11

Seat selection callbacks cache a revision and at most 64 advertised types of at most 255 bytes for
CLIPBOARD and PRIMARY separately. They do not enter State.
`new Wayland revision --adapter turn--> preparing [cancel previous transfers, asynchronously intern
MIME atoms]`. Late atom replies from a previous revision are drained and ignored.
`preparing --atoms ready--> awaiting timestamp [change hidden owner's TIMESTAMP property]`.
`awaiting timestamp --matching-generation PropertyNotify--> exported [SetSelectionOwner with that
server timestamp]`. A timestamp requested for an older revision cannot acquire a replacement offer.
TARGETS lists MIME atoms and supported text/metadata targets. TIMESTAMP returns the acquisition time.
UTF8_STRING and TEXT request UTF-8 from the Wayland source; STRING translates it to Latin-1, replacing
unrepresentable or invalid scalars with question marks and preserving split UTF-8 scalars.
`exported --SelectionClear from a newer owner--> inactive [cancel transfers, forget formats]`.
An unchanged Wayland revision never reclaims another X11 client's selection. Clearing a Wayland
selection releases an owned X11 selection using its acquisition timestamp, so an older clear cannot
replace ownership acquired at a later X11 time. Stale SelectionClear events are ignored.

`exported --supported SelectionRequest--> reading [request Wayland source into pipe, watch read fd]`.
None as the requested property uses the target atom. Unsupported targets, a stale seat revision,
missing source, a reused requestor/property pair or more than eight simultaneous transfers receive
SelectionNotify with None. MULTIPLE is not advertised.
`reading --EOF before a full chunk--> complete [ChangeProperty, SelectionNotify, close pipe]`.
`reading --16 KiB buffered--> incremental [INCR marker, SelectionNotify]`.
`incremental --PropertyNotify/Delete--> permitted`.
`permitted --buffered bytes--> incremental [one ChangeProperty chunk]`.
`permitted --EOF with empty buffer--> complete [zero-length property, close pipe]`.
Each transfer buffers at most 16 KiB, including at most three incomplete UTF-8 bytes
between reads. A full buffer or EOF removes the read watch; the next deletion or pipe readiness drives
progress. No timer runs when there are no pending requests or transfers.
`reading/incremental --five seconds without progress, source revision replaced, SelectionClear,
requestor destroyed or Xwayland stopped--> cancelled [close pipe, remove watch]`.
Cancellation before INCR notifies refusal; cancellation after INCR never fakes a successful truncated
EOF. Other selections and transfers continue. X server atom/timestamp requests have three-second
response deadlines; lack of response restarts Xwayland through the existing supervisor.

Socket-pair tests pin the property-before-notify order, every INCR acknowledgement, bounded chunk
sizes and final zero length; they assert empty data, split-scalar encoding, timeout, selective
cancellation, stale atom/timestamp replies, refusal limits, watch cleanup and no remaining idle deadline. The software/GPU real-client harness reads both seat
selections through X11, including large slow INCR reads, None properties, metadata, refusal, dead and
stalled readers, an external X11 owner taking over, a UTF-8 scalar at the chunk boundary, selection
replacement and nested-host isolation.
X11-owned selections follow the incoming transitions below.


### X11 selections imported into Wayland

The adapter negotiates XFixes once, subscribes to CLIPBOARD and PRIMARY ownership and queries their
initial owners once. Setup replies and initial-owner queries share the bounded asynchronous transport;
an initial-owner reply cannot overwrite a newer notification. There is no ownership poll or idle timer.
`owner changed --external nonzero owner--> discovering [cancel old conversions, create fresh hidden
requestor, ConvertSelection(TARGETS)]`.
`discovering --valid TARGETS, atom-name replies--> offered [publish generation-tagged seat selection]`.
UTF8_STRING takes precedence over STRING for text; STRING converts Latin-1 to UTF-8. Other advertised
MIME atoms retain their names. Metadata is capped at 4 KiB, 64 MIME types and 255 bytes per name.
Unknown non-MIME atoms are skipped. TARGETS itself may use INCR. A replacement keeps the previous
seat offer until its metadata is ready, avoiding a temporary empty selection and a keeper restore.
A Wayland selection changed during discovery wins over the pending X11 offer. Old metadata, stale
read requests and late property replies cannot alter a replacement generation.
`offered --Wayland reader requests live MIME/generation--> converting [fresh requestor,
ConvertSelection, non-blocking reader fd]`.
`converting --SelectionNotify with expected property--> reading [GetProperty slice]`.
`reading --normal value--> draining [write to Wayland pipe]`.
`reading --initial INCR marker--> awaiting chunk [DeleteProperty]`.
`awaiting chunk --PropertyNotify/NewValue--> reading [GetProperty slice]`.
`draining --pipe accepts slice, more property bytes--> reading [GetProperty at next offset]`.
`draining --pipe accepts entire non-final INCR chunk--> awaiting chunk [DeleteProperty]`.
`draining --normal property complete or final zero INCR chunk--> complete [DeleteProperty,
destroy requestor, close pipe]`.
The DeleteProperty acknowledgement follows every accepted byte, including multiple slices of one
large property. A blocked reader holds one bounded slice and a writable-fd watch, with no further
property reads or acknowledgements. UTF-8/raw slices are at most 16 KiB; Latin-1 slices are at most
8 KiB before conversion, yielding at most 16 KiB. There are at most eight data conversions plus one
metadata conversion per selection. A full queue refuses a new reader by closing its fd.
`discovering/converting/reading/draining --owner replaced, owner disappears, newer Wayland copy,
refusal, wrong property type/format, malformed INCR, dead reader or five seconds without progress-->
cancelled [destroy requestor, close pipe, remove watch]`.
Failures affect that conversion. A newer seat selection is never cleared by cancellation or Xwayland
exit. An imported offer cannot acquire X11 ownership again. Owner disappearance or Xwayland death
clears only its current imported offer, allowing the existing keeper to restore cached text. The
keeper's restore and mirrors are ordinary subsequent Wayland selection changes. Outstanding X server
reply deadlines remain three seconds; a stuck X server is handled by its supervisor.

Socket-pair tests pin partial-property offsets, no INCR acknowledgement while a pipe is blocked,
chunk deletion before requestor destruction, bounded Latin-1 expansion, malformed/refused replies,
dead readers, timeouts and harmless late replies. Offer tests assert delayed MIME-name publication,
generation tags, echo suppression, replacement without an empty offer and preserving newer Wayland
selections on adapter shutdown. The real-client software/GPU harness covers both selections, normal
properties larger than a slice, INCR metadata/data, arbitrary binary MIME data, slow/abandoned/stalled
readers, refusal and bad formats, owner replacement during metadata, Latin-1, keeper persistence,
parent-desktop isolation and fresh clipboard generations after Xwayland crashes and restarts.


### Device profile at startup

`startup --CLAMSHELL_DEVICE or kernel identity--> identified [construct typed profile]`.
An explicit thor/vm/generic override wins over the device tree and DMI. Compatible entries match whole
NUL-separated strings; QEMU's DMI selects the VM. Unknown hardware selects the generic profile; an
unknown explicit override fails startup with a diagnostic. Identification performs no periodic work.
`identified --build Init--> running [immutable profile available through State]`.
The VM service now forces CLAMSHELL_DEVICE=vm, replacing its bottom-output environment override.
Detected modes, inputs, backlights and readings remain runtime detection; each fixed fact is moved to
the profile separately.
Pure identity tests cover precedence, exact matching and unknown names. A filesystem fixture checks
kernel identity reads without host-specific assumptions. State, allocation and snapshot checks cover
passing the profile without changing per-event behaviour. VM service boot is checked by the VM suite
once its output-generation script has been switched to the profile too.


The compositor takes both connectors and their clockwise transforms from the profile; ClamShell's
service identifies the same profile. (`thor-outputs` and `clamshell device outputs`, which wrote sway's
output configuration from it, are gone.) `clamshell device panel` supplies connectors to the calibration
tool, whose mode and vblank discovery stays runtime detection. An integration test forces each known
profile and checks the calibration command's facts.


Backlight discovery remains runtime enumeration. The brightness view resolves each detected kernel
name through the profile's top/bottom role hints; an unrecognised name stays its kernel name. Missing
hints do not invent backlights or hide the view. The role test checks both Thor screens, an unrelated
backlight and a generic profile given the same kernel name. Snapshot and allocation references remain
unchanged.


The virtual touchpad prefers density measured from the second screen's DRM connector (its physical size
against its logical size, turned as the screen is). Only the matched Thor panel, when its connector
reports no physical size, uses its 16.5 px/mm hint;
VM and generic profiles have no invented density. Both axes retain their reported resolution. With
neither measurement nor hint, the touchpad stays unavailable rather than publishing made-up uinput
resolution; after the initial layout arrives, it waits for input rather than polling for density.
Unit tests pin measured-over-hint precedence and unknown/invalid dimensions. The VM suite checks the
resulting uinput device and real finger forwarding; UI snapshot references remain unchanged. The
compositor turns on tap and tap-and-drag for the virtual touchpad's libinput device (`drm.rs`).


Refresh calibration's capability, helper program, frame-length attributes/ranges, step and tolerances
now come from the profile. The package tool still detects the actual panel's sysfs files and available
modes; it no longer embeds ICNA3520 ranges or chooses tolerances by rate. The Device row remains drawn
and tappable without the capability, dimmed and labelled Unavailable. A tap follows the existing
Running → Done(error) route without starting a helper when no capability exists. Package-query tests
check Thor/VM capability facts and the worker test asserts the unavailable outcome. Snapshots now
explicitly use the Thor profile and retain the same pixels, focus and trace. Actual frame-length
calibration remains a device check.


Panel colour is immutable profile data, exposed by `clamshell device panel` for inspection. The top
panel preserves the gamescope starting profile's primaries, D65, gamma 2.2, 650-nit peak/frame-average
and 0.002-nit black. Its HDR capability describes compositor tone mapping into available headroom,
not a PQ link; both current Thor links are eight bits. VM/generic link depth stays unknown,
so no compositor can mistake an invented eight-bit default for a hardware limit. Unprofiled panels use sRGB primaries at gamma 2.2,
SDR-only, with luminance left unknown. No brightness-register calibration is invented. Unit tests pin
these values and the package query checks their public representation. Phase 2h will consume these
facts for physical outputs; nested outputs take their description from the host instead.


Auxiliary GPIO-key identity/code and the controller's physical Back code now come from the profile.
The GPIO device is still discovered by advertised name/capabilities. Physical presses are normalised
into fixed logical auxiliary/Back roles before routing and keymaps; resynchronisation applies the same
normalisation. The virtual gamepad excludes the physical Back key as well as the logical role. The VM
profile models the Thor's auxiliary controls for its input fixtures; generic has no auxiliary GPIO key.
Pure tests check alternate physical codes, unrelated keys/axes and virtual-pad filtering. Existing
router, held-key, disconnect and randomized state tests retain the same canonical semantics. The VM
suite checks actual GPIO-key discovery and uinput forwarding.


The pad's actual driver is still detected through its sysfs device link. The profile supplies the
list of drivers whose X/Y codes use letters rather than positions; input code no longer names those
drivers in its decision. Thor/VM include rsinput and xpad, generic includes xpad. The existing swap,
resynchronisation and routing paths are unchanged. Profile tests pin each convention and reject an
unlisted driver; the existing event tests pin the resulting canonical button meanings.


Motion discovery still tries a controller's advertised evdev sensor first. Only if absent does it use
the profile's optional sensor hub. Its QMI service, sample rate, clock rate and mounting matrix are
profile facts; the input thread no longer reads compatible or identifies a Thor. Reconnection after
sleep takes the same facts. Thor retains 400/200 Hz/19.2 MHz and the quarter-turn axis transform;
VM/generic claim no hub, while detected evdev sensors remain usable. Existing wire/sample tests pin
protocol parsing and the profile-driven transform. Profile tests assert hub presence and timing;
firmware service appearance/disappearance and physical mounting remain device checks.


The fan daemon identifies the profile once, using the same pure identification as ClamShell. Its
hwmon target, Auto/Quiet curves, minimum/safe/emergency duties and ramp policy come from the profile;
actual hwmon files and thermal readings stay discovered. A profile without a userspace fan policy
exits successfully and the service does not restart it. Fan controls remain visible and tappable,
dimmed when no policy exists. Thor's curve and safe-stop behaviour are unchanged. Existing daemon
filesystem tests now supply the Thor policy explicitly; interpolation, ramp, emergency and missing-
sensor tests retain their exact outcomes. The service uses Restart=on-failure to preserve recovery
without a retry loop on unsupported machines.

### Charging control discovery

The tuning worker receives immutable charging quirks and optional OEM attribute names from the device
profile. State settings and their effects are unchanged. Without an override or firmware quirk, each
application discovers the battery's end-threshold attribute; a missing threshold uses the charge
current where its attributes exist. The Thor's known ignored threshold selects current instead.
Explicit `CLAMSHELL_CHARGE_LIMIT` selects current, threshold or firmware for trials.

- `detected --threshold appears--> threshold [hold requested limit in firmware]`;
  `detected --threshold vanishes--> current [hold requested limit with current]`.
- `current quirk --threshold present--> current`; an explicit threshold override takes precedence.
- `firmware --both OEM switches present / limit 80--> firmware [enable 80-percent switch]`;
  absent switches or limit 90 use current. Bypass uses zero current and, when selected and present,
  disables charging through the switch. Disconnect releases the hold and resets the switches.

System tuning tests assert discovery changes, quirk and override precedence, both-attribute detection,
firmware fallback, Bypass and release targets. The existing hysteresis test pins charge-current holds.
Actual battery firmware behaviour is checked on the device; attribute presence alone cannot establish
that its firmware honours a write.

### Control diagram facts

The keymap screen reads immutable control-layout and printed-legend facts from the startup profile.
The Thor and VM fixture retain their diagram and Nintendo-style face legends. Unknown hardware shows
a virtual-pad reference diagram with canonical legends; this does not announce physical capabilities.
Changing a binding, selecting an input or choosing an output uses the same legends throughout. The
existing keymapping events, effects and transitions are unchanged. UI tests check input/output legends
and swapped group placement; Thor snapshots pin the existing pixels, focus targets and effect trace.

### Preview geometry

Startup profile preview dimensions request the two desktop windows' initial size, the nested host's
initial canvas and its split ratio. Live configure events still replace their dimensions. Offline
Thor snapshots explicitly use its reference canvas. These values never supply physical millimetres
or claim an output mode. Compositor resize tests exercise the split and clamped tiny hosts; snapshot
pixel, focus and trace references stay unchanged.

### Announced display modes

`DisplayModes` arrives from the DRM host at start, containing only refresh rates at the top screen's
current dimensions. State stores a bounded sorted unique list; zero always
means restore the original mode. No modes are invented from the profile. The Refresh tile stays
visible and tappable, dimmed when no controllable rates are known. Frame-cap choices are generic
limiter policy. Resolution, Scaling, Anti-aliasing and Sharpen (`docs/picture.md`) are the compositor's
alone; Resolution's tile shows its size on the
top screen once the compositor has said it (`Reading::Screens`).

- `unknown --mode report--> known [no effects]`; repeated reports replace the facts without effects.
- `known --mode change/hotplug--> known or unknown [no effects]`; saved choices remain intact.
- `known --Refresh tap--> next announced rate, then zero [save settings, apply, refresh]`.
- `unknown or saved rate absent --Refresh tap--> zero [save settings, apply when changed]`.
- `known --output manager finished--> unknown [no effects]`.

State tests pin the saved values, save-before-apply ordering and refresh effects through reports,
repeats, unplugging and replacement. Random sequences include reports and assert sorted unique valid
rates after every event; the allocation test covers changing reports. The DRM host reports the top
screen's modes at start. Offline snapshots explicitly announce their fixture modes.

### Parametric window colour facts

An immutable shared description holds named or explicit CIE xy primaries, the transfer function,
primary luminance and reference white, and optional mastering/CLL/FALL metadata. The description is
shared by reference so window events remain small and copying a description during apply allocates
nothing. Physical panel descriptions come from the startup profile; nested output descriptions come
from the host's preferred description. Unknown physical measurements stay unknown.

`window --Changed(colour)--> same placement, selection and focus with new colour facts [no effects]`.
Repeating the same facts has no effects. The existing presentation-facts test asserts the resulting
colour, placement, policy and focus; random window-fact changes now include colour descriptions.
Matching source/output primaries, transfer and primary luminance permit colour-preserving scanout;
mastering metadata does not change the signal encoding. The compositor still checks buffer and plane
constraints before scanout. Numeric conversion and protocol commit behaviour are compositor tests.
