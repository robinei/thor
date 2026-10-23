# State machines

What each feature's state is, how it moves, how features interact, and what pins each of these in tests.
`AGENTS.md` asks that a change to a feature updates its part here in the same change.

Notation: `state --event / condition--> state [effects]`. **Test:** the test that asserts the outcome
(file in `crates/state/src`), or **GAP** (nothing asserts it). Suspected problems are numbered (S1, ...)
and collected at the end. First written 2026-10-23 from three read-only passes over the code; the shelf
pass confirmed its suspicions with probe tests in a scratch copy.

## Session: launches (session/)

State: `Session.launching`, `running` (each with `origin: Ours | Found{pid, tray} | Windows{since,
until}`, `seen`, `app_id`), `units`, `pending_show`, `quitting`.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| Idle | Play of the Device tool | Idle | the Device view opens | device `the_device_entry_opens_it_…` |
| Idle | Play of another tool | Idle | `Tool` | GAP |
| Idle | Play of a group | Idle | shelf reveals it, `ShowShelf` | dock `shelf_pins_and_unpins_any_entry_…` |
| Idle | Play of an id not in the library | Idle | nothing | GAP |
| Idle | Play, OpenAnother, autostart, a Steam game found by app id | Launching | runner resumed if paused; `Save(Resume)`; `Launch`; `pending_show` if in front; Starting notice (from Play) | tests `a_launch_says_it_is_starting`; background `a_runner_paused_together…` |
| Launching | the same launched again | Launching | nothing | running `an_entry_runs_its_plays_count_…` |
| Launching | `Units` lists its instance | Running(Ours) | second screen may go dark again | running `a_launch_runs_while_a_scope_…`; tests `…start_and_end_come_to_the_same_in_any_order` |
| Launching | LaunchFailed, browsed file or app-id launch | Running(Windows, unseen, until now+15 s) | — | running `a_file_handed_to_another_program_…` (browsed); app id GAP |
| Launching | LaunchFailed otherwise | Idle | NotStarted notice | tests `a_launch_that_didnt_start_says_so` |
| Launching | no answer from the launcher | Launching, without end (S9) | — | GAP |
| Idle | `Units` lists a scope of a library entry (an earlier ClamShell) | Running(Ours) | — | running `entries_started_before_clamshell_…` |
| Idle | `Units` lists a scope of a key not in the library | ignored (S2) | — | GAP |
| Idle | a window with a library app id (not Steam) | Running(Windows, seen) | Focus once on its workspace | running `an_app_handed_over_…` |
| Idle | a Steam game's window by app id | Launching | as a launch | running `a_game_steam_started_is_followed…` |
| Idle | a stray window of no entry | Running(Found) | Move, Focus if focused, FindIcon, Adopt (own pid) | found `a_window_of_no_entry_…` and its siblings |
| Idle | a tray icon not in our scope (device session) | Running(Found{tray}) | Move/Focus, Adopt | tray `a_program_with_a_tray_icon_runs…` |
| Running(Found) | its scope listed | Running(Ours) (pid and tray key dropped, S3) | — | tray test |
| Running(Found) | icon gone and no windows | Idle, no play recorded | — | tray test |
| Running(Ours) | scope gone, app id and windows (or just started) | Running(Windows) | — | GAP (the `has && !soon` branch) |
| Running(Ours) | scope gone otherwise | Idle | `RecordPlay`; NotStarted if never seen and not quitting (S1) | tests `a_launch_that_ends_without_a_window_didnt_start` |
| Running(Windows, unseen) | a new stray window before `until` | Running(Windows, seen) | Move, Focus | running `a_file_handed_…` |
| Running(Windows, unseen) | past `until` | Idle, no play | — | running `a_file_handed_…` |
| Running(Windows, seen) | no windows left | Idle | `RecordPlay` from `since` | running `an_app_handed_over_…` |
| Running | Show / Play | same | thaw, runner resumed, Focus; else tray Activate and `pending_show`; else NoWindow notice | background `pauses_hidden_thawed_…`; dock `running_entries_join_the_dock…` |
| Running | Show when every window is moved down | same | focuses an empty top workspace (S5) | GAP |
| Running | Quit | Quitting | thaw, Close(windows) | running `quit_closes_the_windows_then_ends_…` |
| Quitting | windows gone, still runs | Quitting(ended) | EndRun, or EndProcesses (found, own pid) | same |
| Quitting | no longer runs | removed | runner stop (once) | same |
| Quitting | a close refused by the program | Quitting without end (S6) | — | GAP |
| Running | ForceKill | removed from quitting | thaw, EndRun{force} or EndProcesses{force} | same |
| not running | Quit / ForceKill | unchanged | nothing | GAP |

### Front and Home

| Trigger | Result | Test |
|---|---|---|
| settle: the front workspace has no window left | Focus the most recent top workspace, else ShowShelf; remember it 2 s | front `a_closed_window_goes_back_…` |
| settle: within 2 s a window reappears there | Focus it | front `a_window_in_front_replaced_at_once_…` |
| settle: `pending_show` and its window arrived | Focus, cleared | tests `a_launchs_first_window_is_shown_before_…` |
| Home, shelf not in front | ShowShelf | front `home_goes_to_shelf_…` |
| Home, shelf in front | Focus the most recent other workspace (Device view open: close it first) | front tests; Device view case GAP |
| settle: program in force | the focused top window's owner, else the kept one if it runs, else the last running | running `the_settings_follow_the_front`; fallbacks GAP |

### Background (per entry and per runner)

State: `Pauses{unused, paused: Hidden | Held}`.

| From | Condition / event | To | Effects | Test |
|---|---|---|---|---|
| Running | set to pause, windows, none visible, not quitting, 10 s | Paused(Hidden) | Freeze on | background `pauses_hidden_…` |
| Paused(Hidden) | shown, no windows, quitting, setting changed | Running | Freeze off | background tests, `a_paused_game_set_to_run_hidden_is_thawed_at_once` |
| any | Suspend | Held | ShowShelf if visible; Freeze on | background `a_suspended_entry_stays_frozen_…` |
| Held | shown, Show, Quit, ForceKill | Running | Freeze off | same |
| — | `Frozen(keys)` at start | Held | — | same (S2: keys of entries not in the library are dropped and stay frozen) |
| Running | set to quit when closed, seen, no windows | Quitting | as Quit | background `quits_closed_ends_once_…` |
| runner | none shown or starting for 10 s | runner paused | RunnerCommand(pause) | background `a_runner_paused_together…`; a slow launch re-pausing it (S8) GAP |

Freeze and RunnerCommand have no acknowledgement; the state assumes they worked.

### Tray, autostart, enqueue

- Tray: `set_tray` does nothing when unchanged; icons whose unit is ours give their menu to that entry
  (S3 after adoption); an icon of a running Found entry sets its tray and pid (S7). Test: tray
  `a_program_with_a_tray_icon_runs…`, `a_guest_in_a_desktops_session_adopts_none…`.
- Autostart: once, at the first settle after units are known, launching unshown what isn't running.
  Test: autostart `chosen_entries_start_once_…`.
- Enqueue: to the runner's running instance whose workspace was in front most recently; without one, a
  single file plays and several launch together; the file selection is cleared. Test: enqueue
  `enqueue_plays_with_nothing_playing_then_adds_…`; target by recency and single file GAP.

### Dock (session/dock.rs)

State: `items{id, pinned}`, `menu`, `tray` (submenu path), `moving`, `unsaved`.

| From | Event | To | Effects | Test |
|---|---|---|---|---|
| — | settle | unpinned items not running dropped, new running ones appended | — | dock `running_entries_join_the_dock…` |
| closed | tap on an entry not running | — | queued Play | same |
| closed | tap, running, all windows down | — | the bottom screen shows them | GAP |
| closed | tap, running | — | queued Show | dock tests |
| closed | long press | menu open | Battery watched (sync) | dock tests |
| menu | a choice | closed | OpenAnother, Autostart, Suspend, Quit, ForceKill queued; CloseWindow; Pin with `Save(Dock)`; Move starts moving | dock `the_menu_opens_another_…`; Autostart/Suspend via the menu GAP |
| menu | TrayMenu, a submenu, Back | tray path pushed / popped | Tray Click on a leaf | tray test; Back at the top closes the whole menu (S20) |
| moving | Step | reordered, clamped | `Save(Dock)` if moved | dock `move_mode_reorders_…` (Right only; others GAP) |
| moving | Done | closed | — | same |
| any | tab left | menu and move closed | — | GAP |
| — | Library read or start | entries gone from the library and not running dropped | `Save(Dock)` | dock `pins_of_entries_gone_…` |

Dock oddities: S20 (Back at the top of the tray menu closes it all), S21 (`close` leaves the tray path,
the menu stays open after its entry left, a Library read can drop the item being moved, a long press
while moving opens the menu with the move still on).

## Second screen (bottom/)

### Tabs, keyboard and editor

| Transition | Test |
|---|---|
| `ChooseTab(U)` or a tab tap, U new → tab U, `raised_from` cleared; `left_tab` closes the dock menu, releases levels, resets the time page | `the_touchpad_tab_always_points`, wifi `the_wifi_tab_watches_while_open` |
| the same tab again → nothing saved | resume `choosing_the_tab_in_front_saves_nothing` |
| `TextInput(true)`, no editor on the tab → Keyboard, `raised_from` = the tab | `a_text_field_brings_up_the_keyboard_and_back` and siblings |
| `TextInput(true)` with Keyboard shown or an editor on the tab → nothing | settings `a_text_field_doesnt_take_the_screen_…` |
| `TextInput(false)` with `raised_from` → back to it, even with an editor open on Keyboard (S11) | same; the editor case GAP |
| `EditOnKeyboard(e)` → editor opened (replacing one on Keyboard, S12), raised from the tab | device `the_password_is_typed_below_…`, `renaming_types_on_the_keyboard_and_goes_back` |
| editor Save/Cancel for a file name → back from the keyboard; for the account password it stays on Keyboard (S10) | renaming test (Save); Cancel GAP |
| Pointing contacts → pointing, `Touchpad` effect; idle → off | `it_points_until_idle_…` |

Editor: one per tab; Char appends, Backspace pops, Enter saves, Esc cancels, Ctrl+V and Paste insert the
clipboard as one line. Tests: `typing_editing_and_finishing`, `a_password_being_typed_stays_out_of_logs`,
`the_editor_pastes_…`, `an_editor_stays_with_its_tab`.

On-screen keyboard modifiers (`Latch{armed, held, used}` and caps): Down holds, Up releases (arming only
if unused), a tap toggles armed, a long press does nothing, a stroke clears armed latches. Tests:
`a_held_shift_acts_while_held`, `a_tapped_shift_arms_one_key`, `a_long_press_alone_neither_arms_nor_locks`,
`modifiers_combine_for_one_stroke`, caps and Fn tests. A Down without Up leaves a modifier held (GAP);
modifiers armed on the capture screen survive leaving it other than by Back (S13).

### Notice

`show(kind, now)` replaces any notice; a tick after 2 s removes it. Tests: `keys_step_the_levels_and_show_
them_for_a_moment`, `a_screenshot_is_asked_for_…`, `a_window_moved_down_…`. BottomRule(false), Gyro and
Account notices GAP. The gyro's "hold still" notice expires after 2 s whatever the calibration does (S14).

### View, power and dim (bottom/screen.rs)

State: `screen{windows, off, idle_off}`, `used_at`, `dimmed`, `woken`, `sent_down`, `window_rules`.

| Transition | Test |
|---|---|
| Home+Select: show ClamShell's view or the windows moved down | `a_window_moved_down_is_shown_there_and_remembered` |
| WindowDown: the focused top window moves down, the screen shows it, a rule is added (notice, `Save(WindowRules)`, `DownRules`) | same; `a_programs_only_window_is_remembered_by_its_app_id_alone`; no focused window GAP |
| WindowUp: the visible down window goes back, its rule is forgotten | same; `two_windows_down_are_tabs_…` |
| settle: a window matching a rule moves down once | `a_rule_sends_the_window_down_when_its_program_opens_it_again` |
| dock tap or BottomWindows → windows shown, `ShowDown` (without clearing `off`, S15) | `the_front_programs_windows_hidden_below_…` |
| Home+Start: wake if off by itself, else toggle off | `the_bottom_screen_turns_off_and_on`, `home_start_brings_back_…`, `…off_by_hand_and_by_itself_at_once` |
| settle: off by itself when on battery, not refreshing, not set to stay on, no windows shown, no editor, and dark in a game or untouched 4 min | `on_battery_the_untouched_…` and siblings; refreshing and editor GAP |
| settle: dimmed after 60 s untouched, unless windows are shown | `the_bottom_screen_dims_untouched_and_lights_when_used` |
| Wake tap sets `woken`; a game starting resets it | `dark_for_a_game_…`, `a_game_can_start_the_pad_mouse_…`; reset GAP; touch or pad focus don't wake a dark screen (S16) |

## Idle and sleep (device/idle.rs)

State: `stage: Awake < Dimmed < Off < Asleep`, compositor activity, `last_input`, `woke_at`,
`sleep_not_before`. The stage is recomputed every settle from the time idle and the policy.

| Transition | Test |
|---|---|
| Awake → Dimmed → Off → Asleep by time; effects through sync: dim brightness, `Screens{on: false}`, status watch off, `Sleep` | `the_state_dims_turns_the_screens_off_and_back_and_sleeps_only_when_allowed` |
| any input, KeptAwake, Resumed or a running refresh → Awake: undim, `Screens{on: true}` | same; `off_by_itself_as_the_screens_go_off_…` |
| Resumed at State level (stage, and Apply resent) | GAP |
| a press on dark screens, or within 1 s of waking, is swallowed | idle unit test only; State level GAP |
| Busy → sleep postponed 5 min | `a_busy_machine_sleeps_later` |
| compositor input/quiet; watching off | `the_compositor_holds_idle_off_…`; watching on→off GAP |
| AskAwake 10 s before dimming, once, with the shown app's command | `an_app_shown_that_keeps_the_screen_on_…` |
| first tick, wall-clock jumps | `what_is_heard_before_the_clock_is_known_…`, `a_step_of_the_wall_clock_…` |
| a suspend that fails without Busy leaves the stage Asleep with screens off until input | GAP (low) |

## Device view (device/)

| Transition | Test |
|---|---|
| first start → open, focus on Wi-Fi; Done → closed, `Save(FirstStartDone)`, the Sysinfo tab | `the_first_start_opens_the_device_view_…` (focus and tab GAP) |
| the Device tool → open; Close or Done → closed, focus to shelf | `the_device_entry_opens_it_and_close_goes_back_to_shelf` |
| Home with the view open → closed | GAP |
| Wifi / Zone rows → their tab | GAP |
| Password → editor for the current then the new password → `Account(Password)` | `the_password_is_typed_below_the_current_one_first` |
| Ssh with a password → `Account(Ssh)` (no pending state: two taps send it twice, S18) | `remote_login_needs_a_password` |
| Sticks → `CalibrateSticks`; progress; done saves or fails (two taps before the first progress start it twice, S17) | `a_sticks_calibration_shows_its_progress_…` |
| Gyro → notice + `CalibrateGyro`; result notice | GAP |
| Refresh → `CalibrateRefresh` once at a time; result → BottomPower resent | `the_refresh_calibration_runs_once_at_a_time_…`; the resend GAP |

## Pad route and pointer (nav/pad.rs, nav/pointer.rs)

Claimant: pad mouse, else shelf in front, else Typing on the Keyboard tab, else the game. Route: Ui for
shelf; else Ui if switched there by View, else Game/Keys/Mouse; Listen while a keymap screen listens; Ui
while a dock entry is being moved. The focus ring hides when the route leaves Ui.

| Transition | Test |
|---|---|
| front or tab changes the claimant, the route follows | `the_pad_goes_to_shelf_the_program_or_typing_…` |
| View switches to the second screen and back | `view_moves_the_pad_between_…`; with shelf in front GAP |
| a claimant change resets the switch (the documented exception never applies, S19) | GAP |
| pad mouse by hand, by game, kept by hand | `pad_mouse_toggles_independently_…`, `the_pad_mouse_follows_the_game_…` |
| listening, dock move, focus ring | `a_new_keymap_is_named_edited_and_mapped`, `moving_a_dock_entry_takes_the_pad_…`, `the_focus_ring_hides_when_the_pad_leaves_…` |
| Down/Up light a target; Activate resolves the item pressed | `down_and_up_light_a_target`, `the_pressed_item_is_lit_where_it_is_now`, `a_press_acts_on_the_item_that_was_drawn`; a Down without Up leaves a stale press (GAP) |

## Keymaps (keymap/)

Screens: List, Menu, Edit, Pick, Holds, Mouse, Layers, Capture, LayerPicker, LayerMenu, Load, SaveAs,
Overwrite, Delete; Undo per keymap since the edit screen opened.

| Transition | Test |
|---|---|
| Keymap tile → List; a row → chosen for the level edited | `the_picker_chooses_a_keymap_for_the_scope_edited` |
| Own → made, settings saved then keymaps | `making_a_games_own_keymap_saves_the_settings_then_the_keymaps` |
| Menu: rename, duplicate, load into own, save as, delete (asks if in use; settings forget it) | `rename_duplicate_and_delete_…`, `a_named_keymap_is_loaded_…`, `a_games_own_keymap_is_saved_…` (forgetting a keymap set globally leaves games on their runner's, S22) |
| Edit: select, bind, map to (pick, capture, mouse, layers), toggles, slides, undo, layers | `a_new_keymap_is_named_edited_and_mapped`, `targets_offered_fit_the_input`, `the_hold_and_double_tabs_…`, `parameters_slide_…`, layer tests, `undo_goes_back_…` |
| Calibrate from Edit; MakeOwn from Edit; Back from Load and the pickers; an empty name | GAP |
| Listened only while the route is Listen | `the_keymap_screens_stay_with_their_tab_and_listen_only_in_view` |

## Shelf (shelf/)

| Transition | Test |
|---|---|
| list steps, collection steps (each keeps its place), drag scrolls | `the_list_scrolls_to_keep_the_selection_on_screen`, `collections_keep_their_own_place`, `a_drag_scrolls_…` |
| tap selects, tap again opens a group or plays; Up leaves a group | `selecting_then_activating_again_launches`, `a_group_opens_and_back_goes_out_to_it` |
| tap on an entry not in the list leaves the focus on it | GAP (S23) |
| long press on the selected entry: actions with the focus ring shown, but plays it without | `a_held_on_the_selected_entry_…` asserts the focus only (S24) |
| typing filters; Enter activates; Clear | `typing_with_shelf_in_front_finds_entries_…` |
| favourite on/off: relayout by name and id, `Save(Favourites)`; the pointer isn't moved (S25) | `a_favourite_shows_in_its_collection_…`; removing, and the Favourites tab vanishing, GAP |
| a play recorded: relayout, `RecordPlay`, `ReadPosition` for videos | tests `a_launchs_start_and_end…` (count only) |
| a group launched from elsewhere: its collection's places reset, the path opened, `ShowShelf` | `a_group_started_from_elsewhere_opens_in_shelf` |
| Library read: places restored by name and id, the typed filter dropped (S26) | `listings_outlive_the_library_read_again`; selection gone and filter GAP |
| Continue, Meta, Position | `continue_plays_on_where_the_series_was_left`; Meta GAP |
| actions and button shortcuts (Y, X, Start) | `an_entrys_actions_come_from_…`, `copy_then_paste_…`, `selected_entries_are_what_the_actions_act_on`; shortcuts off in the actions mode GAP |

### Files tab (shelf/files.rs)

State: `clip`, `selected`, `armed`, `busy`, `hidden`, the listings.

| Transition | Test |
|---|---|
| Delete / Empty Trash arm, then confirm on the same target | `deleting_asks_first` |
| another action or the details changing disarms | GAP |
| Select toggles; Trash/Restore/Delete act on the selection and clear it | `selected_entries_are_what_the_actions_act_on` |
| Copy/Cut set the clip; with only stale selected ids the clip is empty and Paste does nothing (S27) | `copy_then_paste_into_the_folder_open` (copy) |
| Paste; a cut clip is cleared before the move finishes (S28) | copy tested; cut GAP |
| Rename, New folder, Extract → editor on the keyboard; Save gives the operation, Cancel goes back | `renaming_types_on_the_keyboard_and_goes_back`, `an_archive_is_a_folder_…`; New folder, Cancel GAP |
| Open in, Eject, Mounts (rescan) | GAP |
| Listed: listing set, gone folders closed, selection kept by id | `the_folder_open_is_listed_and_watched_while_open`; closing gone folders GAP |
| Busy, Done: `busy` cleared at the first Done of several (S29) | GAP |
| sync: `Browse(Open)` on folder or hidden changes, `Browse(InUse)` by hash | `the_folder_open_is_listed…`, `an_archive_is_a_folder_…` |

### Fetch priority and resume

- While metadata is fetched, each change of shelf's view sends `FetchFirst`; Fetching and Library resend
  it. Test: tests `while_metadata_is_fetched_shelfs_view_goes_first`; the Library resend GAP.
- Shelf or tab changes touch the resume debounce; a tick 2 s later saves it; Quit saves at once. Tests:
  resume `the_place_is_saved_after_a_pause_and_restored`, `open_groups_are_restored`.

## Lists and presses (nav/)

`Scroll{offset, visible, anchor}` per list. Scrolling anchors on the top row's item; settle keeps
anchored lists on their item and every list within its ends; Focus scrolls just enough. Tests:
`rows_under_the_view_stay_when_the_list_changes`, `a_list_changed_by_any_event_stays_within_its_ends`,
`the_pads_focus_on_a_row_shows_it`. Presses resolve by item key: rows tested; dock, collections and audio
outputs GAP.

## Settings (settings/)

| Transition | Test |
|---|---|
| tile tap cycles the value at the level edited (global-only settings always global); long press resets it | `games_override_the_global_settings` |
| Scope selects the level; Reset drops an item's overrides and env | `the_panel_offers_the_items_levels`, `a_runner_can_leave_its_items_without_settings_…` |
| Env editor | `the_environment_editor_takes_the_keyboard` |
| runner keymaps given on Library / Start | `a_runner_gets_its_keymap_once`, `a_runners_keymap_given_while_its_item_runs_is_applied` |
| a change applies to the program in force (sync's Apply) | tests `a_running_game_takes_changes_at_once`, `a_game_coming_in_front_gets_its_settings_…` |

## Radios (radios/)

Wi-Fi and Bluetooth readings replace the state whole; the system is the authority.

- Wi-Fi: a tap joins (a password editor for a secure unsaved network; NeedsLogin for an enterprise
  one); a long press forgets a saved one; power toggles. Tests: `joining_asks_for_a_password_only_when_
  needed`, `a_network_that_needs_a_login_says_so`. Cancel and power GAP. A reading wipes the local error
  and the optimistic busy mark, even a reading older than the command (S30).
- Bluetooth: a tap connects (pairing) or disconnects; a long press forgets; discovery runs while the
  tab is open. Test: `taps_connect_or_disconnect_and_a_long_press_forgets`.

## Levels (levels/)

Volume and mute hold what was set until released; streams, outputs, brightness and per-screen shares;
the slider; the brightness save. Tests: `the_volume_tab_watches_and_sets_levels`, `leaving_the_tab_mid_
drag_lets_readings_in_again`, `brightness_is_the_level_times_each_screens_share`, `the_pad_steps_levels`,
the slider tests. Setting a muted stream's volume shows it unmuted but doesn't unmute it (S31); dimming
at brightness 0 raises it to 1 (S32); brightness sent at Start GAP.

## Status (status/)

- Battery warning: a notification at 10 % and every 5 % below while not charging. GAP; it warns on the
  charger in bypass mode, keyed on not charging rather than discharging (S33).
- Spender menu: opens on an endable row; Quit/ForceKill end a process or queue the entry's act. Test:
  `a_rows_menu_quits_or_kills_what_it_is`. It stays open across tabs and after its row is gone (S34).
- Time pages: regions, a region's zones, set, back; reset on leaving the tab (GAP); NTP and stepping the
  clock by hand. Tests: `regions_then_their_zones_…`, `the_clock_set_by_the_network_or_by_hand…`.

## Sync's resends (sync.rs)

| Resend | Raised by | Re-sends | Test |
|---|---|---|---|
| Start | `Event::Start` | keymap, pad route (and Start sends dock save, brightness, DownRules) | watches tested; keymap, route, DownRules GAP |
| Resumed | `Event::Resumed` | Apply | GAP |
| RefreshCalibrated | `Event::RefreshCalibrated` | BottomPower | GAP |
| FetchFirst | Fetching, Library | FetchFirst | Fetching tested; Library GAP |

## Interactions

| From | Route | To | Test |
|---|---|---|---|
| shelf play, Continue, Open in, Pin | queued `Event::Entry` | session, dock | shelf tests |
| launch of a group / the Device tool | `Session::launch` returns what to do instead | shelf, Device view | `a_group_started_…`, device tests |
| plays ended | settle fills a reused buffer, then `shelf.record_play` | shelf | tests (count only) |
| a game starting | settle returns it | bottom (`unwake`) | `dark_for_a_game_…` |
| notices | `&mut NoticeSlot` | bottom | per area |
| settings in force | parameters to launch and background; sync Apply | session, system | settings tests |
| program in force | settle → pad mouse; sync Apply, Keymap, PadRoute (keymap before route: the pad thread takes the game's keymap before it has the pad) | pad, system | `a_game_coming_in_front_gets_its_settings_then_its_keymap_then_the_pad` |
| front, tab, listening, dock move | `reconcile_pad` | pad | pad tests |
| windows down, program, battery, refresh, editor | `power()` for the bottom screen | bottom | screen tests |
| idle stage | sync: dim and Screens; status watch | levels, system | idle test |
| tabs | sync watches; `left_tab` closes the dock menu, releases levels, resets the time page | dock, levels, status | per tab; time page GAP |
| keymap chosen/deleted/made own | queued `KeymapEvent` → prefs | settings | keymap tests |
| device view, files, keymaps naming | queued `EditOnKeyboard` / editors | bottom | per area |
| Library read | problems notice, shelf reload, pausable runners, runner keymaps, land rules, dock pins, FetchFirst | bottom, session, keymap, settings, dock | partly |
| battery tab quit | queued `Event::Entry` | session | `a_rows_menu_quits_…` |
| any event | settle `follow_lists` | every scroll | `a_list_changed_by_any_event_…` |

Effects answered by events: Launch → Units / LaunchFailed; Adopt → Units; EndRun / EndProcesses → Units,
Windows, Tray; Window commands → Windows; Tray Activate → Windows; FindIcon → Icon; CalibrateSticks →
Sticks; CalibrateRefresh → RefreshCalibrated; CalibrateGyro → GyroCalibrated; Account → AccountChanged and
Account; AskAwake → KeptAwake; Sleep → Busy or Resumed; Screenshot → Screenshot; PadRoute(Listen) →
Listened; Watch → readings. Freeze, RunnerCommand, Enqueue and saves are not answered.

## Invariants (for the randomized event-sequence test)

After every event:

1. **Sync converges:** running sync again emits nothing.
2. **What was sent mirrors the state:** the last PadRoute is `pad().route()`; the last Keymap is the
   keymap in force; Status is watched iff the screens are on; Account iff the UI is up and the Device
   view is open; the last Apply is `(program, in_force)`.
3. **Pad route:** consistent with claimant, listening and dock move; reconciling again changes nothing;
   the focus ring shows only when the route is Ui.
4. **Session:** running ids unique, at most one launch per entry; no launching instance listed in units;
   Ours ⇒ its scope is listed; Found ⇒ not; program in force is running and is the focused window's owner
   when it has one; quitting ⇒ running and not paused; paused ⇒ Ours; suspended ⇒ paused.
5. **Dock:** ids unique; every running entry is on it; every item is pinned, running or being moved.
6. **Shelf:** the cursor within the list; `selected` is the entry at the cursor; the collection index in
   range; every entry listed matches the filter.
7. **Lists:** every scroll offset within its ends.
8. **Bottom:** dimmed ⇒ no windows shown; the chosen tab differs from the tab only on Keyboard; an editor
   is on the tab shown; the Touchpad tab ⇒ pointing.
9. **Keymaps:** the open screen's keymap exists; its layer exists; every keymap id in the settings exists.
10. **Settings:** no empty override; no global-only setting in overrides.
11. **Effect stream across events:** Freeze alternates per slice; at most one EndRun per Quit; at most one
    RecordPlay per end; no Launch for an entry already launching.

The generator draws events valid for the current state (ids that exist, taps on what is shown, Listened
only while listening, Focus only while the route is Ui, results only after their requests), with a library
holding a Steam game, a handover runner, an enqueue runner, a browsed file, a group, the Device tool and an
app that opens another window; clocks advance by 0-400 s to cross the timeouts. A first probe (60 seeds ×
400 events, shelf, lists, settings, levels, radios, status) held invariants 6, 7 and 10.

## Suspected problems

Session and dock:
- S1. A tray-only program that exits by itself shows "didn't start" (`session/running.rs:261`): `seen` only
  means it had a window.
- S2. A scope of an entry not in the library (window-found, tray-found, browsed) is never re-adopted after
  a restart (`running.rs:222`); its windows and tray are skipped as ours, so it runs with no dock tile and
  can't be quit, and a Frozen hold for it is dropped so its slice stays frozen.
- S3. After a tray program's scope is adopted, its tray menu is missing until the tray thread re-reads the
  icons (`tray.rs:122`).
- S5. Show when every window is moved down focuses an empty top workspace (`running.rs:488`).
- S6. A quit the program refuses (a "save changes?" cancelled) stays quitting indefinitely: never paused,
  and ended when its windows later close (`running.rs:566-631`).
- S7. A window-found entry without its own process gets a tray icon's pid; Quit then signals it (`tray.rs:111`).
- S8. A runner paused again while a launch waits more than 10 s for its scope (`background.rs:447`).
- S9. A launch the launcher never answers stays launching, blocking a relaunch (`running.rs:542`).
- S20. Back at the top of the dock's tray menu closes the whole menu instead of going back to the entry menu.
- S21. Dock leftovers: `close` keeps the tray path; the menu stays open after its entry left; a Library
  read can drop the item being moved; a long press while moving opens the menu with the move on.

Second screen, device, pad, keymaps:
- S10. After the account password editor saves or cancels, the screen stays on Keyboard (`lib.rs:1025`),
  against `Event::EditOnKeyboard`'s doc.
- S11. A text field losing focus goes back from Keyboard even while an editor is open there (`bottom/mod.rs:204`).
- S12. Opening the password editor while naming a file drops the name typed (`bottom/editor.rs:117`).
- S13. Modifiers armed on the capture screen survive leaving it by tab or Home (`keymap/screens/mod.rs:573`).
- S14. The gyro's "hold still" notice expires after 2 s whatever the calibration does.
- S15. Showing the program's windows below (dock tap, BottomWindows) while the screen is off by hand shows
  them on a dark panel (`bottom/screen.rs:310`).
- S16. A screen dark for a game wakes only on the Wake tap, not a touch or pad focus, and not when the tap
  is swallowed by idle (`screen.rs:154, 214`).
- S17. Two taps on Sticks before the first progress start the calibration twice (`device/mod.rs:151`).
- S18. Two taps on Ssh send it twice (`device/mod.rs:149`).
- S19. The pad switched to the second screen goes back when the claimant changes (the keyboard coming
  up): the documented exception never applies (`nav/pad.rs:150`).
- S22. Forgetting a keymap that was set globally leaves games that chose it on their runner's keymap, not
  Default (`settings/mod.rs:569`).

Shelf, files, radios, levels, status:
- S23. A tap on an entry not in the list leaves the focus on it.
- S24. A long press on the selected entry by touch plays it (`shelf/rules.rs:17`).
- S25. A relayout (favourite, play) doesn't move the pointer's focus with the selection (`rules.rs:141`).
- S26. A Library read drops the typed filter.
- S27. Copy/Cut with only stale selected entries makes an empty clip that Paste offers and ignores (`files.rs:316`).
- S28. A cut clip is cleared before the move finishes, so after a failed move it is gone.
- S29. `busy` clears at the first of several operations' Done (`files.rs:415`); without a Done it never clears.
- S30. A Wi-Fi or Bluetooth reading wipes the local error and busy mark, even one older than the command.
- S31. Setting a muted stream's volume shows it unmuted but leaves it muted (`levels/mod.rs:370`).
- S32. Dimming at brightness 0 raises it to 1 (`levels/mod.rs:264`).
- S33. The battery warning fires on the charger in bypass mode (`status/mod.rs:103`).
- S34. The battery tab's spender menu stays open across tabs and after its row is gone.
- S35. A relayout doesn't close a group that left a derived collection (Recent, Favourites).
- S36. A folder listing that changes the entries but not shelf's view isn't asked for metadata first.
- S37. The Mute key shows its notice when the volume is unknown and nothing changes (`lib.rs:1058`).
- S38. The resume record can leave out a collection whose stored place is past its list after a relayout
  (`shelf/mod.rs:552`).

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
- S8. A launch waiting for its scope counts as starting, so its runner isn't paused again.
- S9. A launch with no answer from the launcher in 30 s is given up, with the "didn't start" notice.
  **Revisit** the 30 s for slow starts (Proton games on first run).
- S10, S11, S12. An editor opened on the keyboard goes back where it came from when it closes, whatever
  its purpose; a text field losing focus doesn't switch away from an open editor; opening an editor on
  the keyboard while another is open there cancels the other first only if it is empty, otherwise the
  new one waits — **Revisit**: simplest was refusing the second editor with a notice; chosen: refuse,
  with the notice, since two editors on one tab can't both show.
- S13. Leaving the capture screen any way drops the modifiers armed there.
- S14. The gyro's notice stays until the calibration answers, at most 30 s.
- S15. Showing the windows below turns the screen on if it was off by hand.
- S16. Kept: a screen dark for a game wakes only on the Wake target, so a stray touch mid-game doesn't
  light it. **Revisit** after using it on the device.
- S17, S18. The sticks calibration and the remote-login toggle wait for their answer before taking
  another tap.
- S19. Implement the documented exception: the pad switched to the second screen stays there when the
  claimant changes, until View is pressed or shelf comes to the front. **Revisit** if it feels sticky
  on the device.
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
