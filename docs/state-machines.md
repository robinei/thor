# State machines

What each feature's state is, how it moves, how features interact, and what pins each of these in tests.
`AGENTS.md` asks that a change to a feature updates its part here in the same change.

Notation: `state --event / condition--> state [effects]`. **Test:** the test that asserts the outcome
(file in `crates/state/src`), or **GAP** (nothing asserts it). Suspected problems are numbered (S1, ...)
and collected at the end. First written 2026-10-23 from three read-only passes over the code; the shelf
pass confirmed its suspicions with probe tests in a scratch copy.

## Session: launches (session/)

### Window lifecycle and placement

`Event::Window` applies one change; `WindowBatch` applies one backend dispatch before settling once.
State commits placement and selection during `apply`. A compositor reads `Window::place`,
`Session::top_window`, `bottom_window` and `keyboard_focus` immediately after it returns. Home groups
are strings shared with the sway executor, not physical output locations: a window below retains its
home group. `Window.focused` records the selected top content; keyboard focus is independent and may
belong to a bottom client or ClamShell's UI (`None`).

`Opened` supplies identity and optional initial backend presentation. State resolves the home from a
known parent, library app id, owner or launch scope before returning. A known parent's bottom placement
is inherited. `Changed` updates only identity (app id, unit, process ancestry, parent and client kind);
it cannot undo placement or change title. `Title` is independent. Sway translates stable tree snapshots
into sparse changes; unchanged observations cannot overwrite a newer state decision.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| absent | Opened | present, home assigned synchronously | Move before exact Focus when needed | windows `lifecycle_open_and_close_follow_launch_ownership`, `a_scope_window_and_its_rule_are_placed_before_the_scope_is_listed` |
| present | duplicate Opened | unchanged | — | windows `lifecycle_messages_keep_one_window_and_ignore_late_messages` |
| present | Title | updated title | matching bottom rule sends Down once | windows `lifecycle_title_applies_a_bottom_rule_once` |
| present | Changed | updated identity, same place and selection | ownership rules may assign its home | windows `stale_identity_facts_cannot_undo_a_placement` |
| present | Closed | absent; select remaining tab or recent top group, shelf last | exact Focus or ShowShelf when a group empties | windows `lifecycle_close_falls_back_once_and_retains_the_other_windows`; front tests |
| absent | late Closed, Title, Changed, Activate or keyboard focus | unchanged | — | windows `lifecycle_messages_keep_one_window_and_ignore_late_messages`, `backend_changes_preserve_titles_and_tab_order`, `keyboard_focus_is_independent_of_both_screens_selected_content` |
| present on top | validated Activate | selected now | thaw/resume owner before exact Focus or ShowShelf | windows `activation_selects_the_window_on_its_screen`, `activation_thaws_a_suspended_owner_before_selecting_its_window` |
| present below | validated Activate | bottom shows this window now | thaw/resume owner, ShowDown | same |
| either screen selected | KeyboardFocus(Some(live id)) or None | keyboard target changes, content stays | — | windows `keyboard_focus_is_independent_of_both_screens_selected_content` |
| top, regardless of keyboard focus | Down, including a remembered rule | previous bottom returned to its home without activation; requested window below; same-program next top or recent group/shelf selected | Return before Down, then exact top Focus/fallback | bottom `moving_another_window_down_swaps_the_single_bottom_slot`; windows `displacing_another_programs_bottom_window_does_not_activate_it`, `moving_the_top_window_does_not_require_keyboard_focus_there` |
| below, shown or covered | Up | top at retained home, selected now; bottom shows ClamShell | Up to explicit home | bottom `moving_another_window_down_swaps_the_single_bottom_slot` |
| any | Presentation / Order from sway | supplied observations / traversal order applied | normal selection and ownership rules | windows `backend_changes_preserve_titles_and_tab_order` |
| front launch waiting | Home or another explicit selection | foreground intent cancelled | navigation's own effects | windows `home_cancels_a_launchs_late_request_to_take_the_front`, `activating_another_window_cancels_a_launchs_late_request_to_take_the_front` |

Remembering a new bottom rule leaves existing sibling placements alone, including other launches of
the same app (`remembering_a_bottom_rule_does_not_move_existing_siblings_of_other_launches`). Subsequent
matching windows use the rule and displace the current bottom occupant.

The foreground launch intent (`front_launch`) and two-second replacement history are domain rules:
an explicitly requested launch may show its first window, and a splash may be replaced without losing
its place. They are not acknowledgements of sway commands. Placement and exact focus commands remain
effects for the sway executor; the in-process compositor reads the committed state instead.

Live ids are unique, at most one top window is selected and one window placed below, remembered
selections belong to their home groups, and keyboard focus names a live window or none. The randomized
event-sequence test checks these after every event, including lifecycle, activation and keyboard focus.
The allocation test includes repeated lifecycle events, Home-held changes, controller focus and activation with live bottom rules.

Sway's requested-layout mirror retries failed placement/focus without becoming the source of policy.
Acknowledged focus accepts later user tab selection; late group moves leave bottom windows below.
Pure `sway::desired` tests cover failures, reconnects and idempotent acknowledgements. The nested-sway
suite checks real IPC placement and focus; the VM suite checks session adoption, input routes and
services. Parent inheritance is checked in state tests; sway does not report protocol parents, so the
nested compositor must exercise that path with real clients in phase 2.

### Launches

State: `Session.launching`, `running` (each with `origin: Ours | Found{pid} | Windows{since, until}`,
`seen`, `app_id`, `tray`), `units`, `front_launch`, `quitting`.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| Idle | Play of the Device tool | Idle | the Device view opens | device `the_device_entry_opens_it_…` |
| Idle | Play of another tool | Idle | `Tool` | running `play_of_a_tool_asks_for_it_and_of_an_unknown_id_does_nothing` |
| Idle | Play of a group | Idle | shelf reveals it, `ShowShelf` | dock `shelf_pins_and_unpins_any_entry_…` |
| Idle | Play of an id not in the library | Idle | nothing | same |
| Idle | Play, OpenAnother, autostart, a Steam game found by app id | Launching | runner resumed if paused; `Save(Resume)`; `Launch`; `front_launch` if in front; Starting notice (from Play) | tests `a_launch_says_it_is_starting`; background `a_runner_paused_together…` |
| Launching | the same launched again | Launching | nothing | running `an_entry_runs_its_plays_count_…` |
| Launching | `Units` lists its instance | Running(Ours) | second screen may go dark again | running `a_launch_runs_while_a_scope_…`; tests `…start_and_end_come_to_the_same_in_any_order` |
| Launching | LaunchFailed, browsed file or app-id launch | Running(Windows, unseen, until now+15 s) | — | running `a_file_handed_to_another_program_…` (browsed); `a_failed_launch_known_by_app_id_waits_for_its_window` |
| Launching | LaunchFailed otherwise | Idle | NotStarted notice | tests `a_launch_that_didnt_start_says_so` |
| Launching | no answer from the launcher in 30 s | Idle | NotStarted notice; `front_launch` cleared | running `a_launch_never_answered_is_given_up` |
| Idle | LaunchFailed of no launch waiting (a late answer to one given up) | Idle | nothing | running `a_launch_failure_after_it_was_given_up_says_nothing` |
| Idle | `Units` lists a scope of a library entry (an earlier ClamShell) | Running(Ours) | — | running `entries_started_before_clamshell_…` |
| Idle | `Units` lists a scope of a key not in the library, a window or tray icon in it | Running(Ours), as found, named by the name whose key is the scope's (until a window or icon comes, nothing) | — | found `a_found_program_in_our_scope_is_found_again_after_a_restart` |
| Idle | a window with a library app id (not Steam) | Running(Windows, seen) | Focus once on its workspace | running `an_app_handed_over_…` |
| Idle | a Steam game's window by app id | Launching | as a launch | running `a_game_steam_started_is_followed…` |
| Idle | a stray window of no entry | Running(Found) | Move, Focus if focused, FindIcon, Adopt (own pid) | found `a_window_of_no_entry_…` and its siblings |
| Idle | a tray icon not in our scope (device session) | Running(Found{tray}) | Move/Focus, Adopt | tray `a_program_with_a_tray_icon_runs…` |
| Running(Found) | its scope listed | Running(Ours) (pid dropped, tray key kept) | — | tray test; tray `an_adopted_tray_program_keeps_its_menu` |
| Running(Found) | icon gone and no windows | Idle, no play recorded | — | tray test |
| Running(Ours) | scope gone, app id and windows (or just started) | Running(Windows) since its launch | — | running `a_game_known_by_app_id_runs_on_by_its_windows_after_its_scope` |
| Running(Ours) | scope gone otherwise | Idle | `RecordPlay`; NotStarted if it never had a window or tray icon and isn't quitting | tests `a_launch_that_ends_without_a_window_didnt_start`; tray `a_program_with_only_a_tray_icon_started` |
| Running(Windows, unseen) | a new stray window before `until` | Running(Windows, seen) | Move, Focus | running `a_file_handed_…` |
| Running(Windows, unseen) | past `until` | Idle, no play | — | running `a_file_handed_…` |
| Running(Windows, seen) | no windows left | Idle | `RecordPlay` from `since` | running `an_app_handed_over_…` |
| Running | Show / Play | same | thaw, runner resumed, Focus; else tray Activate and `front_launch`; else NoWindow notice | background `pauses_hidden_thawed_…`; dock `running_entries_join_the_dock…` |
| Running | Show when every window is moved down | same | thaw, runner resumed; the bottom screen shows them (`ShowDown`) | running `show_of_an_entry_whose_windows_are_all_below_shows_them_there` |
| Running | Quit | Quitting | thaw, Close(windows) | running `quit_closes_the_windows_then_ends_…` |
| Quitting | windows gone, still runs | Quitting(ended) | EndRun, or EndProcesses (found, own pid) | same |
| Quitting | no longer runs | removed | runner stop (once) | same |
| Quitting | its windows still there 10 s after they were asked to close (a close refused) | Running, not quitting: paused hidden again, not ended when its windows close later | — | running `a_quit_refused_is_given_up` |
| Running | ForceKill | removed from quitting | thaw, EndRun{force} or EndProcesses{force} | same |
| not running | Quit / ForceKill | unchanged | nothing | running `quit_and_force_kill_of_an_entry_not_running_do_nothing` |

### Front and Home

| Trigger | Result | Test |
|---|---|---|
| settle: the front workspace has no window left | Focus the most recent top workspace, else ShowShelf; remember it 2 s | front `a_closed_window_goes_back_…` |
| settle: within 2 s a window reappears there | Focus it | front `a_window_in_front_replaced_at_once_…` |
| settle: `front_launch` and its window arrived | Focus, cleared | tests `a_launchs_first_window_is_shown_before_…` |
| Home, shelf not in front | ShowShelf; cancel foreground launch intent | front `home_always_shows_shelf_and_repeated_home_stays_there` |
| Home, shelf in front | stays; close Device view first if open | same; device `home_closes_the_device_view_first` |
| WindowPrevious / WindowNext | wrap within the selected top program, skipping the bottom slot; absent/single/shelf do nothing | windows `top_window_switching_wraps_within_the_program_and_skips_the_bottom_slot` |
| HomeHeld(bool) | remember button state for the future compositor's arrows; repeated values do nothing | windows `pad_activity_returns_keyboard_focus_to_top_without_moving_windows` |
| PadActive | keyboard focus returns to selected top content; placement stays | same |
| settle: program in force | the focused top window's owner, else the kept one if it runs, else the last running | running `the_settings_follow_the_front`, `the_program_in_force_falls_back_to_the_last_started` |

### Background (per entry and per runner)

State: `Pauses{unused, paused: Hidden | Held}`.

| From | Condition / event | To | Effects | Test |
|---|---|---|---|---|
| Running | set to pause, windows, none visible, not quitting, 10 s | Paused(Hidden) | Freeze on | background `pauses_hidden_…` |
| Paused(Hidden) | shown, no windows, quitting, setting changed | Running | Freeze off | background tests, `a_paused_game_set_to_run_hidden_is_thawed_at_once` |
| any | Suspend | Held | ShowShelf if visible; Freeze on | background `a_suspended_entry_stays_frozen_…`; dock `the_menus_autostart_and_suspend` |
| Held | shown after its windows were seen hidden (one suspended in view stays frozen while its window is still shown), Show, Quit, ForceKill | Running | Freeze off | same |
| — | `Frozen(keys)` at start | Held (kept while the scope is listed, until its entry runs) | — | same; found `a_found_program_in_our_scope_is_found_again_after_a_restart` |
| Running | set to quit when closed, seen, no windows | Quitting | as Quit | background `quits_closed_ends_once_…` |
| runner | none shown, launching or starting for 10 s | runner paused | RunnerCommand(pause) | background `a_runner_paused_together…`, `a_runner_isnt_paused_while_a_launch_waits_for_its_scope` |

Freeze and RunnerCommand have no acknowledgement; the state assumes they worked.

### Tray, autostart, enqueue

- Tray: `set_tray` does nothing when unchanged; icons whose unit is ours give their menu to that entry,
  and an adopted entry keeps the key of the icon it was found by; an icon of a running Found entry sets
  its tray key, not its pid. Test: tray `a_program_with_a_tray_icon_runs…`,
  `a_guest_in_a_desktops_session_adopts_none…`, `an_adopted_tray_program_keeps_its_menu`,
  `a_tray_icon_gives_no_process_to_an_entry_found_in_another_programs`.
- Autostart: once, at the first settle after units are known, launching unshown what isn't running.
  Test: autostart `chosen_entries_start_once_…`.
- Enqueue: to the runner's running instance whose workspace was in front most recently; without one, a
  single file plays and several launch together; the file selection is cleared. Test: enqueue
  `enqueue_plays_with_nothing_playing_then_adds_…`, `enqueue_goes_to_the_launch_most_recently_in_front`,
  `a_file_alone_with_nothing_playing_plays_as_itself`.

### Dock (session/dock.rs)

State: `items{id, pinned}`, `menu`, `tray` (submenu path), `moving`, `unsaved`.

| From | Event | To | Effects | Test |
|---|---|---|---|---|
| — | settle | unpinned items not running dropped, new running ones appended | — | dock `running_entries_join_the_dock…` |
| closed | tap on an entry not running | — | queued Play | same |
| closed | tap, running, all windows down | — | queued Show, which shows them on the bottom screen | running `show_of_an_entry_whose_windows_are_all_below_…` |
| closed | tap, running | — | queued Show | dock tests |
| closed | long press | menu open | Battery watched (sync) | dock tests |
| menu | a choice | closed | OpenAnother, Autostart, Suspend, Quit, ForceKill queued; CloseWindow; Pin with `Save(Dock)`; Move starts moving | dock `the_menu_opens_another_…`, `the_menus_autostart_and_suspend` |
| menu | TrayMenu, a submenu, Back | tray path pushed / popped; Back at the top goes to the entry's menu | Tray Click on a leaf | tray test; dock `back_at_the_top_of_the_tray_menu_goes_to_the_entrys_menu` |
| menu | its entry leaves the dock | closed, tray path cleared | — | dock `the_menu_and_the_move_end_with_their_entry` |
| moving | Step | reordered, clamped | `Save(Dock)` if moved | dock `move_mode_reorders_…`, `move_mode_steps_left_and_by_rows` |
| moving | Done | closed | — | same |
| moving | long press | move ended, menu open | Battery watched (sync) | dock `a_long_press_while_moving_ends_the_move` |
| any | tab left | menu, tray path and move closed | — | dock `back_at_the_top_of_the_tray_menu_goes_to_the_entrys_menu` |
| — | Library read or start | entries gone from the library and not running dropped; the menu or move of one ends | `Save(Dock)` | dock `pins_of_entries_gone_…`, `the_menu_and_the_move_end_with_their_entry` |

## Second screen (bottom/)

### Tabs, keyboard and editor

| Transition | Test |
|---|---|
| `ChooseTab(U)` or a tab tap, U new → tab U, `raised_from` cleared; `left_tab` closes the dock menu, releases levels, resets the time page | `the_touchpad_tab_always_points`, wifi `the_wifi_tab_watches_while_open` |
| the same tab again → nothing saved | resume `choosing_the_tab_in_front_saves_nothing` |
| `TextInput(true)`, no editor on the tab → Keyboard, `raised_from` = the tab | `a_text_field_brings_up_the_keyboard_and_back` and siblings |
| `TextInput(true)` with Keyboard shown or an editor on the tab → nothing | settings `a_text_field_doesnt_take_the_screen_…` |
| `TextInput(false)` with `raised_from` → back to it | same |
| `TextInput(false)` with an editor open on Keyboard → nothing, `raised_from` kept | `a_text_field_done_leaves_an_editor_on_the_keyboard_open` |
| `EditOnKeyboard(e)` → editor opened, raised from the tab | device `the_password_is_typed_below_…`, `renaming_types_on_the_keyboard_and_goes_back` |
| `EditOnKeyboard(e)` with an editor on Keyboard → refused, KeyboardBusy notice | `a_second_editor_for_the_keyboard_is_refused_and_what_is_typed_stays` |
| an editor on Keyboard saved or cancelled → back from the keyboard, unless another took its place (the new password after the current one) | renaming test (Save); device `the_password_editor_goes_back_where_it_came_from`; file name Cancel: `a_text_field_done_leaves_an_editor_on_the_keyboard_open`, files `a_new_folder_is_named_on_the_keyboard` |
| Pointing contacts → pointing, `Touchpad` effect; idle → off | `it_points_until_idle_…` |

Editor: one per tab; Char appends, Backspace pops, Enter saves, Esc cancels, Ctrl+V and Paste insert the
clipboard as one line. Tests: `typing_editing_and_finishing`, `a_password_being_typed_stays_out_of_logs`,
`the_editor_pastes_…`, `an_editor_stays_with_its_tab`.

On-screen keyboard modifiers (`Latch{armed, held, used}` and caps): Down holds, Up releases (arming only
if unused), a tap toggles armed, a long press does nothing, a stroke clears armed latches. Tests:
`a_held_shift_acts_while_held`, `a_tapped_shift_arms_one_key`, `a_long_press_alone_neither_arms_nor_locks`,
`modifiers_combine_for_one_stroke`, caps and Fn tests. A Down without Up leaves a modifier held until its Up
(`a_modifier_down_without_its_up_stays_held`).
Modifiers armed on the capture screen go when it is left by Back or by leaving the Settings tab
(`modifiers_armed_on_the_capture_screen_go_when_its_tab_is_left`); Home doesn't leave it, since the
capture screen doesn't listen and Home acts on the top screen.

### Notice

`show(kind, now)` replaces any notice; a tick after 2 s removes it. Tests: `keys_step_the_levels_and_show_
them_for_a_moment`, `a_screenshot_is_asked_for_…`, `a_window_moved_down_…`; BottomRule(false):
`a_window_moved_up_says_its_rule_is_forgotten`; Account: device `an_account_change_says_how_it_went`. The gyro's "hold still" notice stays until the calibration answers, at most 30 s:
`the_gyros_hold_still_shows_until_it_answers_at_most_30_s`.

### View, power and dim (bottom/screen.rs)

State: `screen{windows, off, idle_off}`, `used_at`, `dimmed`, `woken`, `sent_down`, `window_rules`.

| Transition | Test |
|---|---|
| Home + right stick press: show ClamShell's view or the windows moved down | `a_window_moved_down_is_shown_there_and_remembered` |
| WindowDown: the selected top window moves down without needing keyboard focus; the old bottom window returns to its group, the screen shows it, a rule is added (notice, `Save(WindowRules)`, `DownRules`) | same; `a_programs_only_window_is_remembered_by_its_app_id_alone`; `window_down_with_no_window_in_front_does_nothing` |
| WindowUp: the visible down window goes back, its rule is forgotten | same; `moving_another_window_down_swaps_the_single_bottom_slot` |
| settle: a window matching a rule moves down once | `a_rule_sends_the_window_down_when_its_program_opens_it_again` |
| dock tap or BottomWindows → windows shown, `ShowDown`, the screen turned on if it was off by hand | `the_front_programs_windows_hidden_below_…`, `showing_the_windows_below_turns_the_screen_on_if_off_by_hand` |
| Home+Start: wake if off by itself, else toggle off | `the_bottom_screen_turns_off_and_on`, `home_start_brings_back_…`, `…off_by_hand_and_by_itself_at_once` |
| settle: off by itself when on battery, not refreshing, not set to stay on, no windows shown, no editor, and dark in a game or untouched 4 min | `on_battery_the_untouched_…` and siblings; `a_refresh_calibration_or_an_editor_keeps_it_on` |
| settle: dimmed after 60 s untouched, unless windows are shown | `the_bottom_screen_dims_untouched_and_lights_when_used` |
| Wake tap sets `woken`; a game starting resets it | `dark_for_a_game_…`, `a_game_can_start_the_pad_mouse_…`, `a_game_starting_lets_a_woken_screen_go_dark_again`; touch or pad focus don't wake a dark screen (S16) |

## Idle and sleep (device/idle.rs)

State: `stage: Awake < Dimmed < Off < Asleep`, compositor activity, `last_input`, `woke_at`,
`sleep_not_before`. The stage is recomputed every settle from the time idle and the policy.

| Transition | Test |
|---|---|
| Awake → Dimmed → Off → Asleep by time; effects through sync: dim brightness, `Outputs{top: false}`, status watch off, `Sleep` (after `Outputs`) | `the_state_dims_turns_the_screens_off_and_back_and_sleeps_only_when_allowed` |
| any input, KeptAwake, Resumed or a running refresh → Awake: undim, `Outputs{top: true}` | same; `off_by_itself_as_the_screens_go_off_…` |
| Resumed at State level (stage, and Apply resent) | tests `back_from_sleep_applies_the_settings_again_and_wakes_the_screens` |
| a press on dark screens, or within 1 s of waking, is swallowed | idle `a_press_on_dark_screens_only_wakes_them`, `a_tap_on_dark_screens_wakes_them_and_does_nothing_else` |
| Busy → sleep postponed 5 min | `a_busy_machine_sleeps_later` |
| compositor input/quiet; watching off | `the_compositor_holds_idle_off_…`, `with_the_devices_no_longer_read_nothing_goes_idle` |
| AskAwake 10 s before dimming, once, with the shown app's command | `an_app_shown_that_keeps_the_screen_on_…` |
| first tick, wall-clock jumps | `what_is_heard_before_the_clock_is_known_…`, `a_step_of_the_wall_clock_…` |
| a suspend that fails without Busy leaves the stage Asleep with screens off until input | `a_suspend_that_failed_waits_for_input` |

## Device view (device/)

| Transition | Test |
|---|---|
| first start → open, focus on Wi-Fi; Done → closed, `Save(FirstStartDone)`, the Sysinfo tab | `the_first_start_opens_the_device_view_…`, `the_first_start_begins_on_wifi_and_done_leaves_shelf_and_the_default_tab` |
| the Device tool → open; Close or Done → closed, focus to shelf | `the_device_entry_opens_it_and_close_goes_back_to_shelf` |
| Home with the view open → closed, focus to shelf's selection; repeated Home stays on shelf | `home_closes_the_device_view_first` |
| open while the UI is up → `Watch(Account)` on; closed → off | `the_account_is_watched_while_the_view_is_open` |
| Wifi / Zone rows → their tab, its watch | `the_wifi_and_zone_rows_open_their_tabs` |
| Password → editor for the current then the new password → `Account(Password)` | `the_password_is_typed_below_the_current_one_first` |
| Ssh with a password → `Account(Ssh)`; further taps do nothing until AccountChanged, at most 30 s | `remote_login_needs_a_password`, `remote_login_waits_for_its_answer_before_another_tap`, `remote_login_is_asked_again_after_30_s_without_an_answer` |
| Sticks → `CalibrateSticks`; progress; done saves or fails; further taps do nothing until done, or 30 s without progress | `a_sticks_calibration_shows_its_progress_…`, `the_sticks_calibration_waits_for_its_end_before_another_tap`, `the_sticks_calibration_is_asked_again_after_30_s_without_word` |
| Gyro → notice + `CalibrateGyro`; result notice; without one the notice goes after 30 s | `the_gyros_hold_still_shows_until_it_answers_at_most_30_s` |
| Refresh → `CalibrateRefresh` once at a time; result → `Outputs` resent | `the_refresh_calibration_runs_once_at_a_time_…`; tests `a_refresh_calibration_ending_sets_the_bottom_screens_power_again` |

## Pad route and pointer (nav/pad.rs, nav/pointer.rs)

Claimant: pad mouse, else shelf in front, else Typing on the Keyboard tab, else the game. Route: Ui for
shelf; else Ui if switched there by View, else Game/Keys/Mouse; Listen while a keymap screen listens; Ui
while a dock entry is being moved. The focus ring hides when the route leaves Ui.

| Transition | Test |
|---|---|
| front or tab changes the claimant, the route follows | `the_pad_goes_to_shelf_the_program_or_typing_…` |
| View switches to the second screen and back | `view_moves_the_pad_between_…`, `view_does_nothing_with_shelf_in_front` |
| a claimant change keeps the switch to the second screen; shelf claiming the pad clears it | `the_pad_moved_to_the_second_screen_stays_there_until_view_or_shelf` |
| pad mouse by hand, by game, kept by hand | `pad_mouse_toggles_independently_…`, `the_pad_mouse_follows_the_game_…` |
| listening, dock move, focus ring | `a_new_keymap_is_named_edited_and_mapped`, `moving_a_dock_entry_takes_the_pad_…`, `the_focus_ring_hides_when_the_pad_leaves_…` |
| Down/Up light a target; Activate resolves the item pressed | `down_and_up_light_a_target`, `the_pressed_item_is_lit_where_it_is_now`, `a_press_acts_on_the_item_that_was_drawn`; a Down without Up leaves the press until the next Down: `a_down_without_its_up_lasts_until_the_next_down` |

### Home chords and directional flicks (system/input/)

The reusable `Flick` recogniser receives normalised X/Y, arms below 30%, and reports the axis further
out once it passes 70%. It stays disarmed until both axes return below 30%. `begin` with an already
deflected stick waits for that return. Tests: `input::flick` pins thresholds, diagonals, repetition and
initial deflection; it knows neither Home nor window actions.

The pad router owns the chord policy. Home press emits `HomeHeld(true)` and centres the game's right
stick through the current keymap; its physical position is still tracked. While Home is held, right
stick flicks become WindowDown / WindowUp / WindowPrevious / WindowNext. Home release emits
`HomeHeld(false)`, and emits `HomeTap` only if no chord used it. State consumes that tap when a
Home-held arrow already switched a window; otherwise it shows shelf. Direct Home always shows shelf. A stick still out on release remains taken
until it returns, including during remap ticks and gyro samples. Unplugging releases Home for state.
Tests: `home_flicks_once_and_neutralises_the_games_stick_until_it_returns`,
`losing_the_controller_releases_home_for_the_state`. The VM `pad_windows` check exercises real evdev
routing, tab switching and bottom-slot swaps; nested sway checks the same placement through IPC.

Home + right stick press switches the bottom view; Home + Start controls bottom power; Home + R1 is
still Screenshot; Home + B closes or quits. Select and the D-pad no longer move windows or switch the
bottom view. Tests: `home_chords_use_the_right_stick_and_not_select_or_the_dpad`, the existing B and R1
chord tests. Sway keeps its tab bar, with no arrow overlay; the in-process compositor draws its arrows
in its nested host. `Presentation` and the snapshot-diff helper `session::windows::changes` belong to sway and
tests, never to the in-process compositor.

## Keymaps (keymap/)

Screens: List, Menu, Edit, Pick, Holds, Mouse, Layers, Capture, LayerPicker, LayerMenu, Load, SaveAs,
Overwrite, Delete; Undo per keymap since the edit screen opened.

| Transition | Test |
|---|---|
| Keymap tile → List; a row → chosen for the level edited | `the_picker_chooses_a_keymap_for_the_scope_edited` |
| Own → made, settings saved then keymaps | `making_a_games_own_keymap_saves_the_settings_then_the_keymaps` |
| Menu: rename, duplicate, load into own, save as, delete (asks if in use; settings forget it: all games back to Default if it was theirs, and who chose it chooses Default) | `rename_duplicate_and_delete_…`, `a_named_keymap_is_loaded_…`, `a_games_own_keymap_is_saved_…`; settings `forgetting_a_keymap_sets_who_chose_it_to_default` |
| Edit: select, bind, map to (pick, capture, mouse, layers), toggles, slides, undo, layers | `a_new_keymap_is_named_edited_and_mapped`, `targets_offered_fit_the_input`, `the_hold_and_double_tabs_…`, `parameters_slide_…`, layer tests, `undo_goes_back_…` |
| Calibrate from Edit; MakeOwn from Edit; Back from Load and the pickers; an empty name | `calibrate_on_the_edit_screen_calibrates_the_gyro`, `make_own_on_a_shared_keymaps_edit_screen`, `back_goes_up_from_load_and_the_pickers`, `an_empty_name_makes_nothing` |
| Listened only while the route is Listen | `the_keymap_screens_stay_with_their_tab_and_listen_only_in_view` |

## Shelf (shelf/)

| Transition | Test |
|---|---|
| list steps, collection steps (each keeps its place), drag scrolls | `the_list_scrolls_to_keep_the_selection_on_screen`, `collections_keep_their_own_place`, `a_drag_scrolls_…` |
| tap selects, tap again opens a group or plays; Up leaves a group | `selecting_then_activating_again_launches`, `a_group_opens_and_back_goes_out_to_it` |
| tap on an entry not in the list does nothing, the focus included | `a_tap_on_an_entry_not_in_the_list_does_nothing` |
| long press on the selected entry, by touch or pad: its actions open, the focus ring on the first | `a_held_on_the_selected_entry_opens_its_actions` |
| typing filters; Enter activates; Clear | `typing_with_shelf_in_front_finds_entries_…` |
| favourite on/off: relayout by name and id, `Save(Favourites)`; a focus in the list follows the selection | `a_favourite_shows_in_its_collection_…`, `a_favourite_removed_moves_the_focus_with_the_selection`, `the_favourites_tab_goes_with_its_last_entry` |
| a play recorded: relayout (a focus in the list follows the selection), `RecordPlay`, `ReadPosition` for videos | tests `a_launchs_start_and_end…`, `a_video_ended_asks_where_it_was_left`; shelf `a_play_recorded_keeps_shelfs_place_and_focus` |
| a relayout closes a group no longer in its collection (Recent, Favourites) | `a_group_that_leaves_its_collection_is_closed_there` |
| a group launched from elsewhere: its collection's places reset, the path opened, `ShowShelf` | `a_group_started_from_elsewhere_opens_in_shelf` |
| Library read: places restored by name and id, with what is typed in each list | `listings_outlive_the_library_read_again`, `a_library_read_keeps_what_is_typed`; the selection gone: the list's first, `a_library_read_without_the_selected_entry_selects_the_first` |
| Continue, Meta, Position | `continue_plays_on_where_the_series_was_left`, `fetched_metadata_goes_to_its_entry`, tests `a_video_ended_asks_where_it_was_left` |
| actions and button shortcuts (Y, X, Start) | `an_entrys_actions_come_from_…`, `copy_then_paste_…`, `selected_entries_are_what_the_actions_act_on`, `the_shortcuts_are_off_on_the_actions` |

### Files tab (shelf/files.rs)

State: `clip`, `selected`, `armed`, `busy`, `hidden`, the listings.

| Transition | Test |
|---|---|
| Delete / Empty Trash arm, then confirm on the same target | `deleting_asks_first` |
| another action or the details changing disarms | `another_action_or_the_details_changing_disarms` |
| Select toggles; Trash/Restore/Delete act on the selection and clear it | `selected_entries_are_what_the_actions_act_on` |
| Copy/Cut set the clip, and with nothing to copy do nothing; a listing drops the selected ids that are gone | `copy_then_paste_into_the_folder_open` (copy), `a_listing_drops_the_selected_entries_that_are_gone`; nothing to copy can't be reached once gone ids are dropped |
| Paste; a cut entry leaves the clip when its move succeeds | copy tested; `a_cut_entry_stays_in_the_clip_until_it_is_moved` |
| Rename, New folder, Extract → editor on the keyboard; Save gives the operation, Cancel goes back | `renaming_types_on_the_keyboard_and_goes_back`, `an_archive_is_a_folder_…`, `a_new_folder_is_named_on_the_keyboard` |
| Open in, Eject, Mounts (rescan) | `open_in_eject_and_mounts` |
| Listed: listing set, gone folders closed, selection kept by id | `the_folder_open_is_listed_and_watched_while_open`, `a_listing_closes_the_folders_gone_from_it` |
| Busy, Done: each operation sent ends in one Done, in order; `busy` lasts while any runs; Failed (outside an operation, an archive not mounted) only tells | `busy_lasts_while_any_operation_runs` |
| sync: `Browse(Open)` on folder or hidden changes, `Browse(InUse)` by hash | `the_folder_open_is_listed…`, `an_archive_is_a_folder_…` |

### Fetch priority and resume

- While metadata is fetched, each change of shelf's view sends `FetchFirst`; Fetching and Library resend
  it. Test: tests `while_metadata_is_fetched_shelfs_view_goes_first`,
  `a_library_read_while_fetching_asks_for_shelfs_view_first_again`.
- A folder listing that changes the entries in the list counts as a change of the view. Test: files
  `a_listing_that_changes_the_entries_shown_asks_for_their_metadata_first`.
- Shelf or tab changes touch the resume debounce; a tick 2 s later saves it; Quit saves at once. Tests:
  resume `the_place_is_saved_after_a_pause_and_restored`, `open_groups_are_restored`.
- The record keeps every collection's place, clamped to its list (one that shrank while another was
  shown). Test: shelf `the_resume_record_keeps_a_place_past_its_shrunk_list`.

## Lists and presses (nav/)

`Scroll{offset, visible, anchor}` per list. Scrolling anchors on the top row's item; settle keeps
anchored lists on their item and every list within its ends; Focus scrolls just enough. Tests:
`rows_under_the_view_stay_when_the_list_changes`, `a_list_changed_by_any_event_stays_within_its_ends`,
`the_pads_focus_on_a_row_shows_it`. Presses resolve by item key: rows tested; items
`a_dock_press_acts_on_the_entry_it_came_down_on`, `a_collection_press_acts_on_the_collection_it_came_down_on`,
`an_output_press_chooses_the_output_it_came_down_on`.

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
  needed`, `a_network_that_needs_a_login_says_so`, `the_switch_toggles_the_radio_and_a_cancelled_password_joins_nothing`. A reading keeps the join's busy
  mark until it shows the join, the network connected or an error other than the one from before the
  join (a reading sent before the command still has that one): `a_join_stays_busy_until_a_reading_
  answers_it`. The login needed stays until the next attempt, the system's own error or that network
  connected: `a_login_needed_stays_until_the_next_attempt`.
- Bluetooth: a tap connects (pairing) or disconnects; a long press forgets; discovery runs while the
  tab is open. Test: `taps_connect_or_disconnect_and_a_long_press_forgets`. A connect's busy mark is
  kept over readings as a join's is: `a_connection_stays_busy_until_a_reading_answers_it`.

## Levels (levels/)

Volume and mute hold what was set until released; streams, outputs, brightness and per-screen shares;
the slider; the brightness save. Tests: `the_volume_tab_watches_and_sets_levels`, `leaving_the_tab_mid_
drag_lets_readings_in_again`, `brightness_is_the_level_times_each_screens_share`, `the_pad_steps_levels`,
the slider tests. Setting a muted stream's level unmutes it, as the output volume does:
`setting_a_muted_streams_level_unmutes_it`. Dimming takes a third of the brightness, at least 1 but
never more than it was: `dimming_never_raises_the_brightness`. The Mute key shows the volume's notice
only when the mute changed (not while the volume is unknown):
`the_mute_key_shows_the_level_only_when_it_changes_something`. Brightness sent at Start: tests
`the_ui_starting_sends_the_brightness_the_down_rules_the_keymap_and_the_route`.

## Status (status/)

- Battery warning: a notification at 10 % and at 5 % while discharging, each once until it stops
  discharging; none on the charger in bypass mode; urgent at 5 %. Tests: `the_battery_warns_only_while_discharging`,
  `the_battery_warning_says_the_level_and_is_urgent_at_5`.
- Spender menu: opens on an endable row; Quit/ForceKill end a process or queue the entry's act. Test:
  `a_rows_menu_quits_or_kills_what_it_is`. It closes when the tab is left or its row is gone:
  `a_rows_menu_closes_when_the_tab_is_left_or_the_row_goes`.
- Time pages: regions, a region's zones, set, back; reset on leaving the tab (`leaving_the_tab_resets_it_to_its_main_page`); NTP and stepping the
  clock by hand. Tests: `regions_then_their_zones_…`, `the_clock_set_by_the_network_or_by_hand…`.

## Sync's resends (sync.rs)

| Resend | Raised by | Re-sends | Test |
|---|---|---|---|
| Start | `Event::Start` | keymap, pad route (and Start sends dock save, brightness, DownRules) | watches tested; tests `the_ui_starting_sends_the_brightness_the_down_rules_the_keymap_and_the_route` |
| Resumed | `Event::Resumed` | Apply | tests `back_from_sleep_applies_the_settings_again_and_wakes_the_screens` |
| RefreshCalibrated | `Event::RefreshCalibrated` | Outputs | tests `a_refresh_calibration_ending_sets_the_bottom_screens_power_again` |
| FetchFirst | Fetching, Library | FetchFirst | tests `while_metadata_is_fetched_shelfs_view_goes_first`, `a_library_read_while_fetching_asks_for_shelfs_view_first_again` |

## Interactions

| From | Route | To | Test |
|---|---|---|---|
| shelf play, Continue, Open in, Pin | queued `Event::Entry` | session, dock | shelf tests |
| launch of a group / the Device tool | `Session::launch` returns what to do instead | shelf, Device view | `a_group_started_…`, device tests |
| plays ended | settle fills a reused buffer, then `shelf.record_play` | shelf | tests `a_launchs_start_and_end…`; shelf `a_play_recorded_keeps_shelfs_place_and_focus` |
| a game starting | settle returns it | bottom (`unwake`) | `dark_for_a_game_…` |
| notices | `&mut NoticeSlot` | bottom | per area |
| settings in force | parameters to launch and background; sync Apply | session, system | settings tests |
| refresh rate | sync `Refresh(hz)`: a launch's rate from its launch, the settings in force's when applied, whichever came last; sent when it changes | UI | tests `the_refresh_rate_follows_a_launch_at_once_then_the_settings_in_force` |
| screens' power | sync `Outputs{top, bottom}`: top is the screens on (idle), bottom the second screen as set; sent when either changes or on a resend. The UI turns the screens off and on over Wayland for idle and passes a change of the bottom one alone (or a resend) to sway; how it is carried out is checked on the device | UI, system | tests `the_screens_power_is_sent_when_it_changes`, idle and screen tests |
| program in force | settle → pad mouse; sync Apply, Keymap, PadRoute (keymap before route: the pad thread takes the game's keymap before it has the pad) | pad, system | `a_game_coming_in_front_gets_its_settings_then_its_keymap_then_the_pad` |
| front, tab, listening, dock move | `reconcile_pad` | pad | pad tests |
| windows down, program, battery, refresh, editor | `power()` for the bottom screen | bottom | screen tests |
| idle stage | sync: dim and Outputs; status watch | levels, system | idle test |
| tabs | sync watches; `left_tab` closes the dock menu, releases levels, resets the time page, closes the battery menu | dock, levels, status | per tab; `leaving_the_tab_resets_it_to_its_main_page` |
| keymap chosen/deleted/made own | queued `KeymapEvent` → prefs | settings | keymap tests |
| device view, files, keymaps naming | queued `EditOnKeyboard` / editors | bottom | per area |
| Library read | problems notice, shelf reload, pausable runners, runner keymaps, land rules, dock pins, FetchFirst | bottom, session, keymap, settings, dock | tests `new_library_problems_are_said_once`, `a_library_read_updates_the_landing_rules_and_the_runners_paused_together`, `a_library_read_while_fetching_asks_for_shelfs_view_first_again`; shelf `listings_outlive_…`, `a_library_read_keeps_what_is_typed`; settings `a_runner_gets_its_keymap_once`; dock `pins_of_entries_gone_…` |
| battery tab quit | queued `Event::Entry` | session | `a_rows_menu_quits_…` |
| any event | settle `follow_lists` | every scroll | `a_list_changed_by_any_event_…` |

Effects answered by events: Launch → Units / LaunchFailed; Adopt → Units; EndRun / EndProcesses → Units,
Windows, Tray; Window commands → Windows; Tray Activate → Windows; FindIcon → Icon; CalibrateSticks →
Sticks; CalibrateRefresh → RefreshCalibrated; CalibrateGyro → GyroCalibrated; Account → AccountChanged and
Account; AskAwake → KeptAwake; Sleep → Busy or Resumed; Screenshot → Screenshot; PadRoute(Listen) →
Listened; Watch → readings. Freeze, RunnerCommand, Enqueue and saves are not answered.

## Invariants (for the randomized event-sequence test)

After every event (checked by `random.rs`, `random_event_sequences_keep_the_invariants`):

1. **Sync converges:** running sync again emits nothing.
2. **What was sent mirrors the state:** the last PadRoute is `pad().route()`; the last Keymap is the
   keymap in force; Status is watched iff the screens are on; Account iff the UI is up and the Device
   view is open; the last Apply is `(program, in_force)`; the last Outputs is (screens on, bottom on);
   the last Refresh is the rate of the last Launch or Apply.
3. **Pad route:** consistent with claimant, listening and dock move; reconciling again changes nothing;
   the focus ring shows only when the route is Ui, or after a hold on shelf's selected entry opened its
   actions with the pad elsewhere, until the next touch hides it (the ring is what shows the actions are
   open, S24; the focus may move meanwhile, as when the Device view opens).
4. **Session:** live window ids and running ids unique; at most one selected top window and one window placed
   below; remembered selections belong to their top groups; keyboard focus is absent or names
   a live window; at most one launch waiting per entry; no launching instance listed in
   units; Ours ⇒ its scope is listed; Found ⇒ not; program in force is running and is the selected top
   window's owner when it has one; quitting ⇒ running and not paused; paused ⇒ Ours; suspended ⇒ paused.
5. **Dock:** ids unique; every running entry is on it; every item is pinned, running or being moved.
6. **Shelf:** the cursor within the list; `selected` is the entry at the cursor; the collection index in
   range; every entry listed matches the filter.
7. **Lists:** every scroll offset within its ends.
8. **Bottom:** dimmed ⇒ no windows shown; the chosen tab differs from the tab only on Keyboard; the editor
   shown is the shown tab's; the Touchpad tab ⇒ pointing.
9. **Keymaps:** the open screen's keymap exists; its layer exists; every keymap id in the settings exists.
10. **Settings:** no empty override; no global-only setting in overrides.
11. **Effect stream across events:** Freeze alternates per slice, starting frozen, while a scope of it is
    listed (a slice whose scopes are all gone is gone, and starts thawed again); no second EndRun (not
    forced) for a key until it is quit again or launched; at most one RecordPlay per entry per event, and
    only for an entry that ran before it; no Launch for an entry already launching (one given up in the
    same event may be launched again, as a Steam game whose window is there is).

The generator draws events valid for the current state: taps, holds and presses on the targets shown
(shelf's only while shelf is in front, the Device view's while it is open, the tab's), Focus only while
the route is Ui, shelf's pad events only while the pad is on shelf, a touch's HideFocus before its press,
Listened only while listening, and results only after their requests (a scope or LaunchFailed after
Launch, a scope ending after EndRun or by itself, windows closing after Close or by themselves, the
calibrations', account's and screenshot's answers, Busy or Resumed after Sleep). A small model of the
compositor and systemd follows the effects (focus, moves, windows down). The library holds a Steam game
(ff6), an app handed over to another program (desktop:firefox), an enqueue runner's album, a browsed file,
a group (the series), the Device tool, and any program may open several windows; stray windows, a
landed Steam game's or app's window, and a stray program's tray icon come and go. Clocks advance mostly
by 0-3 s, sometimes by up to 400 s, and now and then by 15-25 minutes to sleep.

It runs 8 fixed seeds × 4000 events with the other tests (about 1.5 s in the debug build). More seeds:
`RANDOM_SEEDS=2000 cargo test -p clamshell-state --release random` runs seeds 1 to 2000,
`RANDOM_SEED=x` one seed, and `RANDOM_EVENTS=n` sets the events per seed. A failure prints the seed, the
event's number, its effects and the 20 events before it. 3000 seeds × 5000 events passed after the fixes
of S50 and S51.

## Suspected problems

Session and dock:
- S1. A tray-only program that exits by itself shows "didn't start" (`session/running.rs:261`): `seen` only
  means it had a window. Fixed: state/session: a program with a tray icon has started.
- S2. A scope of an entry not in the library (window-found, tray-found, browsed) is never re-adopted after
  a restart (`running.rs:222`); its windows and tray are skipped as ours, so it runs with no dock tile and
  can't be quit, and a Frozen hold for it is dropped so its slice stays frozen. Fixed: state/session: a found program's scope is found again after a restart.
- S3. After a tray program's scope is adopted, its tray menu is missing until the tray thread re-reads the
  icons (`tray.rs:122`). Fixed: state/session: a tray icon stays with its adopted program and gives no pid.
- S5. Show when every window is moved down focuses an empty top workspace (`running.rs:488`). Fixed: state/session: Show of an entry whose windows are all below shows them there.
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
- S10. After the account password editor saves or cancels, the screen stays on Keyboard (`lib.rs:1025`),
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
- S37. The Mute key shows its notice when the volume is unknown and nothing changes (`lib.rs:1058`). Fixed: state/levels: the Mute key shows its notice only when the mute changed.
- S38. The resume record can leave out a collection whose stored place is past its list after a relayout
  (`shelf/mod.rs:552`). Fixed: state/shelf: the resume record keeps a place past its shrunk list.
- S39. An entry known by its app id (a Steam game) whose launch's scope ends while its window is open
  runs on by its windows, but its play is then recorded from when the scope ended, not from its launch
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
- S8. A launch waiting for its scope counts as starting, so its runner isn't paused again.
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
- S50. A scope listed runs its entry whatever launch of it waits; the waiting launch joins it when its
  own scope comes.
- S51. A quitting entry can't be suspended; the dock menu's Suspend does nothing for it.

### In-process window presentation (compositor/)

Window limits, role and optional colour description are facts. Changing them preserves selection and
placement. Normal game windows fill their viewport with aspect-preserving fitting; normal apps use
native scale. Fixed-size windows, dialogs and utility/splash roles are centred at native scale and
shrink only when necessary. Rendering and pointer input share the same viewport, origin and scale;
black around the client does not activate the shelf behind it. Colour metadata is reserved for future
colour management; the host does not advertise HDR.

An external client's first window selects itself. A first window of our own scope waits for foreground
launch intent; a later window may select itself only when that program is shown. Dialogs may select
themselves. Tests: `compositor_opening_focus_respects_launch_intent_and_shown_program`, the presentation
policy tests, metadata-change and Home-tap tests in session/windows, and the randomized state test.
The headless compositor protocol test checks first configure, fixed-size child placement and close;
geometry tests check native sizing, fitting and inverse coordinates. `tests/nested-compositor.py` checks
real client pixels across top, bottom, return and Home. The remaining protocols are pending.

### Compositor touch routing (compositor/touch)

A contact starts on a client surface, a UI screen, a Home arrow, or black. Client contacts retain their
surface and inverse geometry from down until up; each slot has its own target, so both screens can be
touched simultaneously. Motion across screens keeps that target. UI contacts use the existing Core
press/move/release path on their initial screen. Arrows switch once on down and consume the contact;
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
