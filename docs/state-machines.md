# State machines

What each feature's state is, how it moves, how features interact, and what pins each of these in tests.
`AGENTS.md` asks that a change to a feature updates its part here in the same change.

Notation: `state --event / condition--> state [effects]`. **Test:** the test that asserts the outcome
(file in `crates/state/src` unless the area names another), or **GAP** (nothing asserts it). The problems
found when this was first written (S1 to S51, all fixed), the decisions taken on them, and the notes kept
while the compositor was built are in `history/state-machines-notes.md`.

## Session: launches (session/)

### Window lifecycle and placement

`Event::Window` applies one change; `WindowBatch` applies one dispatch of the X11 window manager (a
window's facts and its title) before settling once.
State commits placement and selection during `apply`. A compositor reads `Session::place`,
`top_window`, `bottom_window` and `keyboard_focus` immediately after it returns. Home groups
(`Window.group`, a `Group`: an entry's by its key, holding all its launches' windows, or a window's own
for a window of no entry) are not physical output locations: a window below retains its home group. A
window of no entry has none until the settle after the event that opened it gives it one. The session holds the top screen's window (`top`, none while shelf is in front) and the bottom
screen's one slot (`bottom`), each once, and each group's selected window while it is hidden; keyboard
focus is independent and may
belong to a bottom client or ClamShell's UI (`None`).

`Opened` and `Changed` carry the window's facts (`WindowFacts`: id, app id, title, parent, limits,
role, colour, unit and processes), never its placement; the session's record of a window is those facts
with its group and screen. On `Opened` the state resolves the home from a known parent, library app id,
owner or launch unit, and decides whether the window comes in front (`Session::opening_focus`, below),
before returning. A known parent's bottom placement is inherited. `Changed` updates only identity (app id, unit, process ancestry and parent);
it cannot undo placement or change title. `Title` is independent. The compositor sends each change as it
happens (Opened, Closed, Changed, Title, KeyboardFocus, and Activate once a token is checked); a later
fact cannot overwrite a newer state decision.

Placing is the session's own: the steps in the Effects columns here and below (Focus, ShowShelf, Move,
Down, Up, Return) are its window commands (`WindowCommand`), applied to the state during
`apply`, not effects sent out. The only window effect is closing (`Compositor::Close`). Shelf is not a
window: it is in front when no top window is shown, and ShowShelf deselects the top windows.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| absent | Opened | present, home assigned synchronously, in front if its opening focus says so | Move before exact Focus when needed | windows `lifecycle_open_and_close_follow_launch_ownership`, `a_window_opens_in_front_as_its_opening_focus_says`, `a_launchs_window_and_its_rule_are_placed_before_its_unit_is_listed` |
| present | duplicate Opened | unchanged | — | windows `lifecycle_messages_keep_one_window_and_ignore_late_messages` |
| present | Title | updated title | matching bottom rule sends Down once | windows `lifecycle_title_applies_a_bottom_rule_once` |
| present | Changed | updated identity, same place and selection; `x11` kept from Opened | ownership rules may assign its home | windows `stale_identity_facts_cannot_undo_a_placement`, `changed_facts_keep_a_window_x11` |
| present | Closed | absent, out of the bottom slot; a dialog in front brings back its parent, another window its group's; else the recent top group, shelf last | exact Focus or ShowShelf when a group empties | windows `lifecycle_close_falls_back_once_and_retains_the_other_windows`, `a_dialogs_close_brings_back_its_parent`, `closing_the_window_below_empties_its_slot`; front tests |
| absent | late Closed, Title, Changed, Activate or keyboard focus | unchanged | — | windows `lifecycle_messages_keep_one_window_and_ignore_late_messages`, `backend_changes_keep_the_placement`, `keyboard_focus_is_independent_of_both_screens_selected_content` |
| present on top | validated Activate | selected now | thaw/resume owner before exact Focus or ShowShelf | windows `activation_selects_the_window_on_its_screen`, `activation_thaws_a_suspended_owner_before_selecting_its_window` |
| present below | validated Activate | bottom shows this window now (the windows' tab opens) | thaw/resume owner, keyboard focus to it | same |
| either screen selected | KeyboardFocus(Some(live id)) or None | keyboard target changes, content stays | — | windows `keyboard_focus_is_independent_of_both_screens_selected_content` |
| top, regardless of keyboard focus | Down, including a remembered rule | previous bottom returned to its home without activation; requested window below; same-program next top or recent group/shelf selected; the keyboard to the top window | Return before Down, then exact top Focus/fallback | bottom `moving_another_window_down_swaps_the_single_bottom_slot`; windows `displacing_another_programs_bottom_window_does_not_activate_it`, `moving_the_top_window_does_not_require_keyboard_focus_there`, `a_window_sent_down_gives_the_keyboard_to_the_top_window` |
| below, shown or covered | Up | top at retained home, selected now; bottom shows ClamShell | Up to explicit home | bottom `moving_another_window_down_swaps_the_single_bottom_slot` |
| front launch waiting | Home or another explicit selection | foreground intent cancelled | navigation's own effects | windows `home_cancels_a_launchs_late_request_to_take_the_front`, `activating_another_window_cancels_a_launchs_late_request_to_take_the_front` |

Remembering a new bottom rule leaves existing sibling placements alone, including other launches of
the same app (`remembering_a_bottom_rule_does_not_move_existing_siblings_of_other_launches`). Subsequent
matching windows use the rule and displace the current bottom occupant.

The foreground launch intent (`front_launch`) and two-second replacement history are domain rules:
an explicitly requested launch may show its first window, and a splash may be replaced without losing
its place. When the window in front closes and its program still runs, the intent is set again for that
program for `REWAIT_SECS` (30), so its next window comes in front however late, as Steam's window after
its update dialog (`a_running_programs_next_window_is_shown_after_its_window_in_front_closed`). The
compositor reads the placement they lead to from the state after `apply`.

At most one window is shown on top and one placed below, since each is one slot. Live ids are unique,
the slots name live windows and never the same one, remembered selections belong to their home groups,
and keyboard focus names a live window or none. The randomized
event-sequence test checks these after every event, including lifecycle, activation and keyboard focus.
The allocation test includes repeated lifecycle events, Home-held changes, controller focus and activation with live bottom rules.

The compositor's tests check placement before the first configure with real clients
(`xdg_clients_are_placed_before_configure_and_follow_state_without_a_gpu`), and
`tests/nested-compositor.py` moves between the screens with real programs; the VM suite checks session
adoption, input routes and services. A dialog takes its parent's group; a dialog of the window below
shows on the top screen while its parent stays below
(`a_dialog_of_a_window_below_shows_on_top_while_its_parent_stays_below`).

The settle steps that follow the session (units, windows, window rules, what is shown, background,
quitting, dock) run in the order `State::settle`'s doc lists, with the test that pins each order that
matters.

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
| Launching | `Units` lists its instance | Running(Ours) | second screen may go dark again | running `a_launch_runs_while_a_unit_…`; tests `…start_and_end_come_to_the_same_in_any_order` |
| Launching | LaunchFailed, browsed file or app-id launch | Running(Windows, unseen, until now+15 s) | — | running `a_file_handed_to_another_program_…` (browsed); `a_failed_launch_known_by_app_id_waits_for_its_window` |
| Launching | LaunchFailed otherwise | Idle | NotStarted notice | tests `a_launch_that_didnt_start_says_so` |
| Launching | no answer from the launcher in 30 s | Idle | NotStarted notice; `front_launch` cleared | running `a_launch_never_answered_is_given_up` |
| Idle | LaunchFailed of no launch waiting (a late answer to one given up; the failure names its launch, so a newer launch of the entry waits on) | Idle | nothing | running `a_launch_failure_after_it_was_given_up_says_nothing`, `a_late_failure_leaves_a_newer_launch_of_the_entry_waiting` |
| Idle | `Units` lists a unit of a library entry (an earlier ClamShell) | Running(Ours) | — | running `entries_started_before_clamshell_…` |
| Idle | `Units` lists a scope of a key not in the library, a window or tray icon in it | Running(Ours), as found, named by the name whose key is the scope's (until a window or icon comes, nothing) | — | found `a_found_program_in_our_scope_is_found_again_after_a_restart` |
| Idle | a window with a library app id (not Steam) | Running(Windows, seen) | Focus once in its group | running `an_app_handed_over_…` |
| Idle | a Steam game's window by app id | Launching, launched before the windows are followed, so its window isn't found as a program | as a launch | running `a_game_steam_started_is_followed…`; tests `a_steam_game_found_by_its_window_is_launched_not_found_as_a_program` |
| Idle | a stray window of no entry | Running(Found) | Move, Focus if focused, FindIcon, Adopt (own pid) | found `a_window_of_no_entry_…` and its siblings |
| Idle | a tray icon not in a unit of ours (device session) | Running(Found{tray}) | Move/Focus, Adopt | tray `a_program_with_a_tray_icon_runs…` |
| Running(Found) | its scope listed | Running(Ours) (pid dropped, tray key kept) | — | tray test; tray `an_adopted_tray_program_keeps_its_menu` |
| Running(Found) | icon gone and no windows | Idle, no play recorded | — | tray test |
| Running(Ours) | unit gone, app id and windows (or just started) | Running(Windows) since its launch | — | running `a_game_known_by_app_id_runs_on_by_its_windows_after_its_unit` |
| Running(Ours) | unit gone otherwise | Idle | `RecordPlay`; NotStarted if it never had a window or tray icon and isn't quitting | tests `a_launch_that_ends_without_a_window_didnt_start`; tray `a_program_with_only_a_tray_icon_started` |
| Running(Windows, unseen) | a new stray window before `until` | Running(Windows, seen) | Move, Focus | running `a_file_handed_…` |
| Running(Windows, unseen) | past `until` | Idle, no play | — | running `a_file_handed_…` |
| Running(Windows, seen) | no windows left | Running(Windows, seen) until `REOPEN_SECS` later, then Idle; a window again in the meantime keeps the run (a program replacing its only window) | `RecordPlay` from `since`, at the end | running `an_app_handed_over_…`, `a_program_replacing_its_only_window_runs_on` |
| Running | Show / Play | same | thaw, runner resumed, Focus; else tray Activate and `front_launch`; else NoWindow notice | background `pauses_hidden_thawed_…`; dock `running_entries_join_the_dock…` |
| Running | Show when every window is moved down | same | thaw, runner resumed; the bottom screen shows them (the windows' tab opens) | running `show_of_an_entry_whose_windows_are_all_below_shows_them_there` |
| Running | Quit | Quitting | thaw, Close(windows) | running `quit_closes_the_windows_then_ends_…` |
| Quitting | windows gone, still runs | Quitting(ended) | EndRun, or EndProcesses (found, own pid) | same |
| Quitting | no longer runs | removed | runner stop, unless the end ran it (once) | same; running `a_quit_that_ends_an_entry_runs_its_runners_stop_once` |
| Quitting | its windows still there 10 s after they were asked to close (a close refused) | Running, not quitting: paused hidden again, not ended when its windows close later | — | running `a_quit_refused_is_given_up` |
| Running | ForceKill | Quitting(ended), so that nothing quits it again while it still runs (Quits when closed) | thaw, EndRun{force} or EndProcesses{force} | same; background `a_force_killed_entry_isnt_quit_again_by_quits_closed`, `force_kill_thaws_a_paused_entry_then_ends_it` |
| not running | Quit / ForceKill | unchanged | nothing | running `quit_and_force_kill_of_an_entry_not_running_do_nothing` |

### Steam's interface (system/launch/steam.rs, packages/thor-session)

Steam is started in its controller interface, `-gamepadui`, every way it starts: its packaged desktop
entry and steam:// links run `steam -gamepadui %U`, and a Steam game from shelf runs `steam -gamepadui
-silent -applaunch ID`, as a launch of Steam's desktop entry when the client isn't running (with only
`WAYLAND_DISPLAY` and `DISPLAY` from the game's environment), or handed to the running client; `-silent`
keeps a cold start from showing the client's window.

    no client --Launch(Steam game)--> the client started as Steam's own launch [the argv above]; the game's unit runs `clamshell --follow-steam ID`
    client running --Launch(Steam game)--> the request handed to the client; the follower started

The state's side is pinned by `steams_windows_open_behind_a_steam_games_launch`,
`a_game_steam_started_is_followed_as_a_launch_beside_the_others`,
`a_steam_game_shelf_started_ends_by_the_launcher_not_its_windows`,
`a_steam_games_window_is_the_games_though_it_runs_in_steams_scope`,
`a_steam_game_found_by_its_window_is_launched_not_found_as_a_program`, and the system's
`a_steam_game_is_known_by_its_app_id`. The `-gamepadui` argument and the interface it gives are checked on
the device.

### Front and Home

| Trigger | Result | Test |
|---|---|---|
| settle: the front group has no window left | Focus the most recent top group, else ShowShelf; remember it 2 s | front `a_closed_window_goes_back_…` |
| settle: within 2 s a window reappears there | Focus it | front `a_window_in_front_replaced_at_once_…` |
| settle: `front_launch` and its window arrived | Focus, cleared | tests `a_launchs_first_window_is_shown_before_…` |
| Home, shelf not in front | ShowShelf; cancel foreground launch intent | front `home_always_shows_shelf_and_repeated_home_stays_there` |
| Home, shelf in front | stays; close Device view first if open | same; device `home_closes_the_device_view_first` |
| WindowPrevious / WindowNext | wrap within the selected top program, skipping the bottom slot; absent/single/shelf do nothing | windows `top_window_switching_wraps_within_the_program_and_skips_the_bottom_slot` |
| HomeHeld(bool) | the button's state, by which the overlay draws the window arrows (with another top window); a new hold starts unused, so the HomeTap after a Home-held arrow is taken by it; repeated values do nothing | windows `pad_activity_returns_keyboard_focus_to_top_without_moving_windows` |
| PadActive | keyboard focus returns to selected top content; placement stays | same |
| settle: program in force | the focused top window's owner, else the kept one if it runs, else the last running | running `the_settings_follow_the_front`, `the_program_in_force_falls_back_to_the_last_started` |

### Background (per entry and per runner)

State: `Pauses{unused, paused: Hidden | Held}`.

| From | Condition / event | To | Effects | Test |
|---|---|---|---|---|
| Running | set to pause, windows, none visible, not quitting, 10 s | Paused(Hidden) | Freeze on | background `pauses_hidden_…` |
| Paused(Hidden) | shown, no windows, quitting, setting changed | Running | Freeze off | background tests, `a_paused_game_set_to_run_hidden_is_thawed_at_once`; shown again as the front window closes, in the same event: tests `a_paused_game_shown_when_the_front_closes_is_thawed_at_once` |
| any | Suspend | Held | ShowShelf if visible; Freeze on | background `a_suspended_entry_stays_frozen_…`; dock `the_menus_autostart_and_suspend` |
| Held | shown after its windows were seen hidden (one suspended in view stays frozen while its window is still shown), Show, Quit, ForceKill | Running | Freeze off | same |
| — | `Frozen(keys)` at start | Held (kept while its unit is listed, until its entry runs) | — | same; found `a_found_program_in_our_scope_is_found_again_after_a_restart` |
| Running | set to quit when closed, seen, no windows | Quitting | as Quit | background `quits_closed_ends_once_…` |
| runner | none shown, launching or starting for 10 s | runner paused | RunnerCommand(pause) | background `a_runner_paused_together…`, `a_runner_isnt_paused_while_a_launch_waits_for_its_unit` |

Freeze and RunnerCommand have no acknowledgement; the state assumes they worked.

### Tray, autostart, enqueue

- Tray: `set_tray` does nothing when unchanged; icons whose unit is ours give their menu to that entry,
  and an adopted entry keeps the key of the icon it was found by; an icon of a running Found entry sets
  its tray key, not its pid. Test: tray `a_program_with_a_tray_icon_runs…`,
  `a_guest_in_a_desktops_session_adopts_none…`, `an_adopted_tray_program_keeps_its_menu`,
  `a_tray_icon_gives_no_process_to_an_entry_found_in_another_programs`.
- Autostart: once, at the first settle after units are known and the library is read, launching
  unshown what isn't running. Tests: autostart `chosen_entries_start_once_…`,
  `chosen_entries_wait_for_the_library`.
- Enqueue: to the runner's running instance whose group was in front most recently; without one, a
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
| — | Library read or start (with the library read) | entries gone from the library and not running dropped; the menu or move of one ends | `Save(Dock)` | dock `pins_of_entries_gone_…`, `the_menu_and_the_move_end_with_their_entry`, `pins_wait_for_the_library_before_any_is_dropped` |

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

### View, power and dim (bottom/screen.rs, session/below.rs)

State: `screen{off, idle_off}`, `used_at`, `dimmed`, `woken`; the window moved down is shown in its own
tab, `Tab::Windows` (the bar's rightmost button), with `before_windows` the tab it was opened from and
`left` a tab left as a window moved down, settled after the window rules as any tab left (`left_tab`).
Which window is below, the rules that send windows there (`window_rules`) and the windows they sent
(`sent_down`) are the session's (session/below.rs): a move down, by hand or by a rule, tells the
bottom screen to open the windows' tab and turn on (`Bottom::show_below`).

| Transition | Test |
|---|---|
| Home + right stick press: to the windows' tab, or from it back to the tab it was opened from; with nothing moved down the tab says how to move a window there | `a_window_moved_down_is_shown_there_and_remembered`, `the_windows_tab_with_nothing_below_shows_no_window` |
| a window moved down (by hand or by its rule), from any tab: the windows' tab opens, the screen turns on | `moving_a_window_down_from_another_tab_opens_the_windows_tab_with_the_screen_on` |
| the windows' tab button: lit while open, standing out while the front program's window is below another tab, dim with nothing below | `the_windows_tab_shows_the_window_below_and_marks_one_hidden_by_another_tab` |
| WindowDown: the selected top window moves down without needing keyboard focus; the old bottom window returns to its group, the screen shows it, a rule is added (notice, `Save(WindowRules)`) | same; `a_programs_only_window_is_remembered_by_its_app_id_alone`; `window_down_with_no_window_in_front_does_nothing` |
| WindowUp: the window below goes back, its rule is forgotten | same; `moving_another_window_down_swaps_the_single_bottom_slot` |
| settle: a window matching a rule moves down once, after the windows are followed (a found program's window too, in the event that found it); the tab it leaves is settled in the same event | `a_rule_sends_the_window_down_when_its_program_opens_it_again`; tests `a_found_programs_window_goes_down_by_its_rule_as_it_is_found`, `the_tab_a_rule_leaves_is_settled_in_the_same_event` |
| dock tap or BottomWindows → windows shown (the windows' tab), the screen turned on if it was off by hand | running `show_of_an_entry_whose_windows_are_all_below_shows_them_there` |
| Home+Start: wake if off by itself (dark for a game too, which sets `woken` as a tap does), else toggle off | `the_bottom_screen_turns_off_and_on`, `home_start_brings_back_…`, `…off_by_hand_and_by_itself_at_once`, `home_start_wakes_a_screen_dark_for_a_game` |
| settle: off by itself when on battery, not refreshing, not set to stay on, no windows shown, no editor, and dark in a game or untouched 4 min | `on_battery_the_untouched_…` and siblings; `a_refresh_calibration_or_an_editor_keeps_it_on` |
| settle: dimmed after 60 s untouched, unless windows are shown | `the_bottom_screen_dims_untouched_and_lights_when_used` |
| Wake tap sets `woken`; a game starting resets it | `dark_for_a_game_…`, `a_game_can_start_the_pad_mouse_…`, `a_game_starting_lets_a_woken_screen_go_dark_again`; touch or pad focus don't wake a dark screen (kept by decision, S16 in the history notes) |

## Idle and sleep (idle.rs)

State: `stage: Awake < Dimmed < Off < Asleep`, compositor activity, `last_input`, `woke_at`,
`sleep_not_before`. The stage is recomputed every settle from the time idle and the policy.

| Transition | Test |
|---|---|
| Awake → Dimmed → Off → Asleep by time; effects through sync: dim brightness, `Outputs{top: false}`, status watch off, `Sleep` (after `Outputs`) | `the_state_dims_turns_the_screens_off_and_back_and_sleeps_only_when_allowed`; tests `the_screens_go_off_before_the_device_sleeps` |
| any input, KeptAwake while awake (one after the dim is ignored), Resumed or a running refresh → Awake: undim, `Outputs{top: true}` | same; `off_by_itself_as_the_screens_go_off_…`, `a_running_refresh_calibration_keeps_idle_awake`, `a_late_kept_awake_leaves_the_screens_dimmed` |
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
| the first password sent → confirming (the row says to press Volume Up) until AccountChanged, whatever it says; the helper sets it only once Volume Up is pressed on a physical device (`clamshell device confirm`, VM check `confirm_key`) | `a_first_password_waits_for_volume_up_until_the_helper_answers` |
| Ssh with a password → `Account(Ssh)`; further taps do nothing until AccountChanged, at most 30 s | `remote_login_needs_a_password`, `remote_login_waits_for_its_answer_before_another_tap`, `remote_login_is_asked_again_after_30_s_without_an_answer` |
| Sticks → `CalibrateSticks`; progress; done saves or fails; further taps do nothing until done, or 30 s without progress | `a_sticks_calibration_shows_its_progress_…`, `the_sticks_calibration_waits_for_its_end_before_another_tap`, `the_sticks_calibration_is_asked_again_after_30_s_without_word` |
| Gyro → notice + `CalibrateGyro`; result notice; without one the notice goes after 30 s | `the_gyros_hold_still_shows_until_it_answers_at_most_30_s` |
| Refresh → `CalibrateRefresh` once at a time, the bottom screen off (`Outputs`) while it runs; `CalibrationRate(hz)` from the tool (`clamshell ctl calibration rate HZ`) → `Refresh(hz)` while it runs, nothing otherwise; result → the bottom screen's power and the rate given back (`Outputs`, `Refresh(0)`), one turned off by hand meanwhile staying off | `the_refresh_calibration_runs_once_at_a_time_…`; tests `a_refresh_calibration_sets_the_screens_and_then_gives_them_back` |

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
`losing_the_controller_releases_home_for_the_state`. Away from the game, Home's press gives the mapping
in force the right stick at rest (`home_away_from_the_game_centres_the_right_stick_for_the_mapping`). The
D-pad pressed with Home held is Home's, as any button is
(`home_with_the_dpad_is_a_combination_the_game_doesnt_see`). After dropped events, `resync` makes up Home's
press before the other buttons' and its release after theirs
(`a_resync_presses_home_first_and_lets_it_go_last`). The activity watch counts a button press or an axis
past 4096 (`game_activity_is_an_axis_past_a_threshold`). The VM `pad_windows` check exercises real evdev
routing, tab switching and bottom-slot swaps.

Home + right stick press switches to the windows' tab and back; Home + Start controls bottom power; Home + R1
takes a screenshot; Home + B closes or quits. Select and the D-pad don't move windows or switch the bottom
view. Tests: `home_chords_use_the_right_stick_and_not_select_or_the_dpad`, and the B and R1 chord tests. The UI's overlay over the top screen's window draws the window arrows while Home is held
(`Id::WindowArrow`); a press on one sends the same `Device` event as the pad's window keys, as it comes down.

## Keymaps (keymap/)

Screens: the picker's (List, Menu, Load, SaveAs, Overwrite, Delete: `profiles::picker`, shared by every
kind of profile, the keymaps' own through `Owner`), then Edit, Pick, Holds, Mouse, Layers, Capture,
LayerPicker, LayerMenu; Undo per keymap since the edit screen opened. One Back for all of them
(`Id::Profile(Keymap, Back)`), so the pad's focus stays on it between the edit screen and the picker.

| Transition | Test |
|---|---|
| Keymap tile → List; a row → chosen for the level edited | `the_picker_chooses_a_keymap_for_the_scope_edited` |
| Own → made, settings saved then keymaps | `making_a_games_own_keymap_saves_the_settings_then_the_keymaps` |
| Menu: rename, duplicate, load into own, save as, delete (asks if in use; settings forget it: all games back to Default if it was theirs, and who chose it chooses Default) | `rename_duplicate_and_delete_…`, `a_named_keymap_is_loaded_…`, `a_games_own_keymap_is_saved_…`; settings `forgetting_a_keymap_sets_who_chose_it_to_default` |
| Edit: select, bind, map to (pick, capture, mouse, layers), toggles, slides, undo, layers | `a_new_keymap_is_named_edited_and_mapped`, `targets_offered_fit_the_input`, `the_hold_and_double_tabs_…`, `parameters_slide_…`, layer tests, `undo_goes_back_…` |
| Calibrate from Edit; MakeOwn from Edit; Back from Load and the pickers; an empty name | `calibrate_on_the_edit_screen_calibrates_the_gyro`, `make_own_on_a_shared_keymaps_edit_screen`, `back_goes_up_from_load_and_the_pickers`, `an_empty_name_makes_nothing` |
| Listened only while the route is Listen | `the_keymap_screens_stay_with_their_tab_and_listen_only_in_view` |

## Tone curves (curve/)

Screens: the picker's (`profiles::picker`, as for keymaps; Default colours is the default, "Unchanged"; its hint says what a curve does: "Maps the game's SDR or HDR to the screen's"), then Edit
(a slider per parameter). One Back for all of them (`Id::Profile(Curve, Back)`). The curve chosen is
`Setting::Curve` (an id, 0 Off), per game or for all; the compositor applies it to the game's picture.

| Transition | Test |
|---|---|
| Curve tile → List; a row → chosen for the level edited; Off → 0 | `the_tile_opens_the_picker_and_a_row_chooses_a_curve_for_the_game` |
| List: Make own → own curve copied from the one in force, Edit opens, chosen; settings saved before curves | `a_games_own_curve_is_made_from_the_one_in_force_and_opens_on_its_sliders` |
| Edit: slide start/to → the curve changes as it moves (the compositor reads it each frame); end → saved once if moved | `a_slider_changes_the_curve_as_it_moves_and_saves_once_when_lifted` |
| Edit --Back--> List --Back--> closed | `back_goes_from_the_sliders_to_the_picker_and_out` |
| Menu: Delete in use → asks; confirmed → removed, its games back on Off | `deleting_a_curve_in_use_asks_and_puts_its_games_back_on_off` |
| List: New → named → Edit, a curve that changes nothing | `a_new_curve_is_named_and_changes_nothing_until_edited` |

The mapping (`Curve::map`, mirrored by the compositor's shader): black point, shadow contrast, decode,
highlights above the start expanded into the curve's share of the screen's room above white
(`Curve::peak_in`) with a smooth join, saturation around luminance; on a screen within its headroom
(`Curve::map_on`); HDR content's light through `curve::map_hdr`, its highlights compressed from the knee
into the screen's peak, its peak as declared or as the program's "HDR content peak" setting sets it
(`State::window_hdr_peak`), edited on the edit screen's last row under a caption saying where its value is
read from or that it is written to the level the picker was opened from: a slider (`Control::Setting`,
`State::setting_stop`) whose first stop, Default, writes nothing there
(`the_hdr_content_peak_is_the_games_setting_slid_from_default_and_outlives_its_curve`); tested in
`curve::tests`. The edit screen's plot follows `State::curve_plot`: the
game's window's content and its screen's description (`Reading::Screens`), and behind the curve the
picture's histogram: while the edit screen is open the compositor draws the plotted window's picture
small, its luminance as the content's signal level (SDR's, or PQ's with its white at the reference),
reads it back four times a second and counts it into 64 bins, without the black bars a game draws at
its edges and the blended line beside each (`colour::histogram`, `Reading::Histogram`,
for `State::histogram_window`). The plot draws the levels along the bottom, each bar filled with the grey
of the light the curve gives it (black to paper white the darker half of the greys, paper white to the
screen's peak the lighter) but for its left quarter, the grey of its light as the content is, so a bar
lighter or darker on the right than on the left is brightened or darkened. There are no axis lines. The
curve is orange and the content as it is blue, to stand out from the greys. The bars' edges are on
whole pixels, and the screen's black is labelled below them. `tests/picture-scaling.py` checks the histogram is drawn for the test
pattern, and `curve-edit-histogram.png` shows one. The randomized test taps the
curve screens and slides their parameters.

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
| Unread at start (`Init::library` is `None`; the binary reads it on a thread): shelf empty, no "nothing found" said; the tab restored at once; the place, and the focus left nowhere, as the library comes; a resume saved meanwhile is the place it goes back to | resume `before_the_library_is_read_the_place_waits_for_it`; views `an_unread_library_isnt_said_to_be_empty` |
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
- Shelf or tab changes touch the resume debounce; a tick 2 s later saves it; Quit (SIGTERM or SIGINT
  read by the event loop, or nested's window closed) saves it and a brightness change still waiting at
  once, then ends ClamShell (`quit_saves_what_waits_then_ends`). Tests:
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
| tile tap cycles the value at the level edited (global-only settings always global); a profile tile's (Keymap, tone curve) steps through Default or Default colours, the level's own, then the named ones | `games_override_the_global_settings`, `a_profile_tile_steps_through_its_profiles_and_its_picker_resets` |
| tile long press opens its menu (`Prefs::menu`, list `Values`): its values, the one in force marked; a row sets it and closes; Reset (last, when the level chooses it itself, or All differs from stock) goes back to the level below's (`Panel::inherited_value`); Back closes. A profile tile's long press opens its picker, which ends with the same Reset | `a_tiles_menu_picks_a_value_and_resets_to_the_level_below`, `a_profile_tile_steps_…` |
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
`the_mute_key_shows_the_level_only_when_it_changes_something`. The brightness is sync's
(`Levels::wanted_brightness`): ClamShell's level; dimmed, a third of the known level; back from dimmed,
the level read before (a reading taken while dimmed is ignored); nothing while ClamShell neither set a
level nor touched the backlights. A level set while dimmed is sent dimmed. Tests:
`what_the_outside_starts_without_goes_out_at_the_first_event`,
`the_screens_come_back_from_dimming_at_the_brightness_they_had`, `a_level_set_while_dimmed_is_sent_dimmed`.

## Status (status/)

- Battery warning: a notification of ClamShell's own (`NotificationId::Battery`, see Notifications) at
  10 % and at 5 % while discharging, each once until it stops discharging; none on the charger in bypass
  mode; critical at 5 %; each replaces the one before. Tests: `the_battery_warns_only_while_discharging`,
  `the_battery_warning_says_the_level_and_is_critical_at_5`.
- Spender menu: opens on an endable row; Quit/ForceKill end a process or queue the entry's act. Test:
  `a_rows_menu_quits_or_kills_what_it_is`. It closes when the tab is left or its row is gone:
  `a_rows_menu_closes_when_the_tab_is_left_or_the_row_goes`.
- Time pages: regions, a region's zones, set, back; reset on leaving the tab (`leaving_the_tab_resets_it_to_its_main_page`); NTP and stepping the
  clock by hand. Tests: `regions_then_their_zones_…`, `the_clock_set_by_the_network_or_by_hand…`.

## Notifications (notifications/, apply/notifying.rs)

ClamShell is the session's notification server. A program's notification comes from the bus thread
(`system/session/notifications.rs`) as `Notification(Notified)`, with the id the thread gave it; the
battery warnings are ClamShell's own (`NotificationId::Battery`) and tell the bus nothing. They are held
by id, newest first, at most 32. Each is waiting, or seen since `t` while the bottom screen is lit
(`State::bottom_lit`: on, not refreshing, the screens not off by idle). How long it shows
(`Notification::lasts`): the request's timeout in whole seconds rounded up, 5 s for -1, until dismissed
for 0 or a critical one.

| Transition | Test |
|---|---|
| timeouts: -1 → 5 s, 0 → never, 1500 ms → 2 s, critical → never | `the_time_a_notification_shows_follows_its_request` |
| `--Notified(n)--> held, newest, waiting`; one with an id held replaces it and is the newest | `notifications_are_held_newest_first_and_one_sent_again_replaces_its_own` |
| settle step 15: `waiting --lit--> seen since now`; `seen since t --now ≥ t + lasts--> gone [NotificationClosed(id, 1)]` | `a_notification_waits_for_the_bottom_screen_and_then_shows_its_time` |
| `seen --not lit (off by hand or by itself, screens off)--> waiting` (its time starts again) | `its_time_starts_again_when_the_screen_is_lit_again`, `the_screens_going_off_by_idle_hold_its_time_too` |
| `--Closed(id)--> gone [NotificationClosed(id, 3)]`; an id not held, or closed already, does nothing | `a_program_closes_its_own_and_an_unknown_id_closes_nothing` |
| `--Tapped(id)--> gone [ActionInvoked(id, key), NotificationClosed(id, 2)]`, the action only if it has a default action; the order is pinned (a client may end on the close) | `a_tap_invokes_the_default_action_then_dismisses` |
| `--Dismissed(id)--> gone [NotificationClosed(id, 2)]` (the close mark) | `the_close_mark_only_dismisses` |
| Tapped or Dismissed on dark screens, or within 1 s of their waking → only wakes them | `a_tap_on_dark_screens_only_wakes_them` |
| `--Notified--> past 32: the oldest gone [NotificationClosed(id, 4)]` | `past_the_most_the_oldest_is_closed` |
| ClamShell's own (Battery) close without an effect | status `the_battery_warning_says_the_level_and_is_critical_at_5` |

Interactions: a critical notification wakes the screens as input does (`idle.own_input`) and turns the
bottom screen on (`Bottom::light`: off by hand or by itself, or dark for a game), routed through
`State::notify`; an ordinary one waits (tests
`a_critical_notification_wakes_the_screens_and_turns_the_bottom_screen_on`,
`a_critical_notification_lights_a_screen_dark_for_a_game`). A tap on one is input and a use of the
bottom screen. Taps reach the state from the bottom screen's overlay as the press comes down
(`Core::press_overlay`), not through the pointer's presses; the overlay draws them, and so offers them to
touch, only while the bottom screen is lit (ui
`the_newest_three_show_below_the_bar_each_with_its_close_mark`).

What the bus thread does (the name, `Notify` answered with the id, `replaces_id`, the signals) is checked
by its parsing tests, a live test by hand (`a_notify_call_is_answered_and_sent_on`), and the nested
compositor test, which sends them with `gdbus`, closes one and taps another.

## Sync's resends (sync.rs)

| Resend | Raised by | Re-sends | Test |
|---|---|---|---|
| Resumed | `Event::Resumed` | Apply | tests `back_from_sleep_applies_the_settings_again_and_wakes_the_screens` |
| FetchFirst | Fetching, Library | FetchFirst | tests `while_metadata_is_fetched_shelfs_view_goes_first`, `a_library_read_while_fetching_asks_for_shelfs_view_first_again` |
| Restarted(thread) | `Event::Restarted` | what that thread was sent (below) | tests in `apply/restart.rs` |

At start only what the outside starts with counts as sent (`Sent::new`): the settings in force
applied, the screens on at their own rate, the bar's status watched. So the brightness, the keymap,
the pad's route and a refresh rate the settings want go out at the first event (tests
`what_the_outside_starts_without_goes_out_at_the_first_event`); Start sends none of them again.

The main screen's refresh rate needs no resend: sync sends `Wanted::refresh` whenever it differs from
what was sent (the calibration's rate, else the rate of the entry launched last while its window is yet
to show: until the program in force changes, it has a window, it is gone, or `LAUNCH_WAIT` has passed;
else the settings'. Tests `the_refresh_rate_follows_a_launch_at_once_then_the_settings_in_force`,
`the_refresh_rate_goes_back_when_a_launch_fails`, `the_refresh_rate_goes_back_when_a_launch_shows_no_window`,
`a_refresh_calibration_sets_the_screens_and_then_gives_them_back`).

### A system thread started again (apply/restart.rs)

A system thread that panics is started again by `os::worker::restarted` and the wiring sends
`Event::Restarted(thread)` (tested end to end in the system crate's wiring,
`a_thread_started_again_after_a_panic_tells_the_state`). What the thread was carrying out is lost; what it was sent and still stands is
sent again, or what waits on it ends. A thread that panics five times in a row, each soon after it
started (as when what it is sent again reproduces the panic), is given up, logged at error; its commands
then go nowhere, logged once (system crate, `a_thread_that_keeps_panicking_soon_after_starting_is_given_up`):

- `idle --Restarted(Pad)--> [Keymap, PadRoute, PadActivity, Apply]` (the pad keeps its virtual devices
  through the restart; tested `a_restarted_pad_is_sent_its_keymap_route_and_settings`)
- `--Restarted(Tuning)--> [Apply]` (tested `a_restarted_tuning_thread_is_sent_the_settings_in_force`)
- `--Restarted(Mixer | Battery | Metrics | Status | Wifi | Bluetooth)--> [Watch(tab, true), Watch(Status, ..), Watch(Account, ..)]`
  (tested `a_restarted_reader_is_sent_the_watches`; each thread with the tab it serves,
  `each_restarted_reader_is_sent_its_tabs_watch`; the account's, `a_restarted_system_thread_is_sent_the_account_watch`)
- `Asleep --Restarted(System)--> [watches]`, the sleep taken as refused (Busy): Sleep again after
  `SLEEP_RETRY` if still asleep; awake, only the watches (tested
  `a_restarted_system_thread_is_asked_to_sleep_again_after_the_retry`)
- `--Restarted(Brightness)--> [Brightness]` if ClamShell set a level, dimmed if idle dimmed it (tested
  `a_restarted_brightness_thread_is_sent_the_level_clamshell_set`)
- `--Restarted(Store)--> [Save of everything saved]` (tested `a_restarted_store_is_sent_everything_saved`)
- `files running --Restarted(Files)--> the oldest operation done, failed [notice]`; the rest run (tested
  `a_restarted_files_thread_fails_the_operation_it_had_and_browse_lists_again`)
- `--Restarted(Browse)--> [Browse(Open), Browse(InUse)]` (same test)
- `--Restarted(Icons)--> [FindIcon for each entry found by its window]` (tested
  `a_restarted_icons_thread_is_asked_again`)
- `--Restarted(Freezer)--> [Freeze(slice, paused) for each slice of ours]` (tested
  `a_restarted_freezer_is_sent_each_slice_as_it_is`)
- `--Restarted(Notifications)--> nothing`: the notifications are the state's, and the thread's ids come
  from a counter that outlives it (tested `a_restarted_notifications_thread_is_sent_nothing_and_they_stay`)
- Other threads: nothing; `apply/restart.rs` says why for each. Short-lived threads the state waits on (a
  launch, the refresh calibration, an account change) answer with a failure if they panic
  (`os::worker::or_on_panic`). The randomized test restarts random threads.

## Windows in the compositor (compositor/)

The compositor's side of the windows: `windows.rs` (facts, configure, the presentation cache, titles),
`handlers.rs` (the xdg-shell and commit callbacks), `geometry.rs` (one transform for drawing and input),
`input.rs` (`hit`, `ui_at`), `render.rs`, `server.rs` (its turn, `frame`, `reject_client`), `clients.rs`,
`trace.rs`; in the state, `presentation/` (the presentation policy) and `Session::opening_focus`.
They run the same way under the nested host (`nested.rs`) and the DRM host (`drm.rs`). The state's side of
the same windows is under Session, "Window lifecycle and placement".

### Lifecycle and facts (windows.rs, handlers.rs)

A Wayland toplevel is a `ClientWindow`, opened and mapped or not. Its facts are its app id, title, parent,
size limits, role (Normal, or Dialog through xdg-dialog), colour, unit and processes; `x11` is set only by
the X11 adapter.

    none --get_toplevel--> unopened [id given]
    none --a second toplevel on the same wl_surface--> none [client ended, reason logged]
    unopened --commit--> opened [Opened(facts); the first configure after the state has placed it]
    opened --commit with a buffer--> mapped
    opened --app id, parent or dialog hint changed--> opened [Changed(facts)]
    opened --commit with changed limits or colour--> opened [Changed(facts)]
    mapped --commit with a null buffer--> unopened [Closed(id); a pending title dropped]
    unopened (after an unmap) --commit--> opened [Opened(facts), the same id]
    any --toplevel destroyed--> none [Closed(id)]

Changed keeps placement, selection, keyboard focus, title and `x11`.

Tests: `xdg_clients_are_placed_before_configure_and_follow_state_without_a_gpu` (opened before the first
buffer, configured at 960×540, its unit identified, unmapped and remapped under the same id, a fixed-size
child with its parent, close); `a_toplevel_marked_a_dialog_has_the_dialog_role`; in the state
`presentation_facts_and_colour_do_not_change_placement_or_selection`, `backend_changes_keep_the_placement`,
`stale_identity_facts_cannot_undo_a_placement`, `changed_facts_keep_a_window_x11`,
`lifecycle_messages_keep_one_window_and_ignore_late_messages`.

### Opening focus (session/mod.rs)

On Opened the state decides whether the window comes in front (`Session::opening_focus`).

| Condition at Opened | Result | Test |
|---|---|---|
| a launch waits for its window | in front only if it is that launch's (in its unit, of its entry by app id or process, or a dialog of one); others open behind | `steams_windows_open_behind_a_steam_games_launch` |
| it has a parent | in front if the parent's program is shown | `opening_focus_respects_launch_intent_and_shown_program` |
| not in a unit of ours | in front | same; `a_window_opens_in_front_as_its_opening_focus_says` |
| the first window of a unit of ours | behind (it waits for the launch's intent) | same |
| a later window of a unit of ours whose program is shown | in front, with the keyboard | same |

A dialog of a window on the bottom screen is placed on the top screen
(`a_dialog_of_a_window_below_shows_on_top_while_its_parent_stays_below`; the compositor test above checks it
with a real fixed-size child).

### Presentation and geometry (state/presentation/, geometry.rs)

The state gives each window a presentation: a layout (Fullscreen or Centred) and a scale (Fit, Integer,
Stretch, Native). With the Size setting at Default, a main window (no parent, role Normal) fills the
screen: a game's at Fit, any other at Native. Dialogs, utility and splash windows, and windows of a fixed
size that is none of the screen's resolutions or Xwayland's modes are Centred at Native; a fixed size equal
to one of those fills the screen. Size at Fit, Integer or Stretch applies to main windows only.

The compositor configures a fixed-size window at its size, a Fullscreen one at the screen's area or at its
game's resolution, and any other at its own size clipped to the area; Fullscreen sets the xdg fullscreen
state, and keyboard focus sets Activated. Drawing and input use one `Geometry` (viewport, origin, scale)
per window. A scaled Fullscreen window is fitted by its picture (the surface `picture_surface` picks, such
as Wine's game subsurface), in the picture's own shape when that differs from its place by more than 2 %.
Native never scales up. A press on the black around a top window reaches neither the window nor shelf
behind it.

    window --role or limits Changed--> the same place and selection, a new presentation [configure]
    window --moved below or above--> a new area [configure to it; until the client answers, its old buffer is fitted]

Tests: in the state `dialogs_and_fixed_windows_are_centred_and_games_fit_ignored_sizes`,
`a_game_window_fixed_at_its_screens_mode_fills_the_screen`,
`a_size_shows_main_windows_as_it_says_whatever_their_size_but_not_dialogs`; geometry
`fitting_preserves_aspect_and_uses_the_same_inverse_for_input`,
`native_size_is_sharp_and_oversized_fixed_windows_scale_down`,
`a_fullscreen_game_at_a_lower_resolution_is_configured_to_it_and_scaled_up`,
`a_mode_stretched_over_the_screen_keeps_its_shape`,
`integer_scaling_shows_the_largest_whole_multiple_of_the_picture`,
`a_picture_in_part_of_its_window_is_what_is_fitted`; the compositor's lifecycle test above (a client that
stops acknowledging configure is fitted below at once). The fullscreen and activated states, and the black
around a window, are checked by `tests/nested-compositor.py` (real pixels above, below, on return and at
Home; X11 fixed-size and transient windows centred on black). `tests/picture-scaling.py` checks that test
games (Wine's subsurface and viewport, SDL 3 on Wayland and on X11, at the desktop's size and at a lower
mode) fill the top screen at each Resolution, Scaling, anti-aliasing and Sharpen setting.

### The presentation cache (windows.rs)

Each window keeps what the state says of it (`Shown`: presentation, clarity, curve, HDR peak, frame rate
cap). `sync_windows` reads it again only when Core's state revision changed, which every applied event
does, the UI's own included; frames, configures and hit tests read the cached values.

    cached at revision r --an event applied--> stale --sync_windows--> cached at r+1

Test: `xdg_clients_are_placed_before_configure_and_follow_state_without_a_gpu` (ten repeated syncs keep the
revision; a role changed to Utility gives Centred, back to Normal Fullscreen).

### Titles (windows.rs, server.rs)

A title request of an opened Wayland toplevel marks it dirty. `Server::frame`, run once per composed frame
by both hosts (and on each frame turn on DRM with both screens off), sends one Title per dirty window whose
title differs from the state's. A title set before Opened comes with Opened's facts.

    clean --set_title--> dirty
    dirty --frame, the title differs--> clean [Title]
    dirty --frame, the same title--> clean
    dirty --unmap--> clean

Test: `xdg_clients_are_placed_before_configure_and_follow_state_without_a_gpu` sends 101 titles in one
dispatch and then 100 identical ones, and finds one Title record in the event ring and the last title in
the state.

### Viewports (protocols.rs, handlers.rs)

Both hosts offer wp_viewporter. A source rectangle crops the buffer after its transform and scale; a
destination sets the surface's logical size, which presentation and hit testing use. Destroying the
viewport restores the buffer's size at the next commit. An invalid request is a protocol error that ends
its client. Viewports add no facts to the state.

    buffer size --a destination committed--> destination size
    destination size --viewport destroyed, commit--> buffer size

Test: `viewport_destination_drives_geometry_and_destroy_restores_buffer_size` (a 640×480 buffer at scale 2,
a cropped source, a 600×400 destination, then 320×240 after destroy; the centre's hit coordinates; one
state window throughout). A game's picture subsurface stretched by a viewport:
`a_picture_in_part_of_its_window_is_what_is_fitted` and `tests/picture-scaling.py`.

### Window colour facts (state/colour.rs, compositor/colour/)

A window's colour is the parametric description committed through wp_color_management on the surface
holding its picture, else on its toplevel: primaries (named or CIE xy), transfer function, luminances,
and optionally the mastering primaries and luminance, MaxCLL and MaxFALL. The DRM host describes each
screen by the device profile's panel colour, with unknown luminance left unset; the nested host by its
host's preferred description, as PQ when the host takes HDR. The picture's pipeline is in `picture.md`.

    window --a commit with a new description--> the same placement, selection and focus [Changed(facts)]
    window --Changed with the same facts--> unchanged

Tests: in the state `presentation_facts_and_colour_do_not_change_placement_or_selection` (repeated facts
too), and the randomized test (window fact changes include colour); in the compositor
`parametric_descriptions_are_double_buffered_and_preferred_information_is_available`,
`an_invalid_colour_request_disconnects_its_client_without_ending_the_server`,
`each_screen_is_described_as_its_panel_and_the_top_is_preferred`,
`clients_are_told_the_hosts_headroom_not_the_contents_volume`, and the conversion tests in
`colour/math.rs`. `tests/nested-compositor.py` checks that a surface's colour reaches the window's facts in
the exported event ring, and PQ, linear and SDR pixels from an EGL client.

### A client's failure (server.rs, render.rs, clients.rs)

    client --a duplicate toplevel role, or a popup that can't be tracked or configured--> ended [protocol error, reason logged]
    client --its buffer fails to import while drawing--> ended [reason logged; the frame goes on]
    client --a dmabuf fails to import, or more than 64 imports pending--> buffer rejected [logged; invalid_format ends the client]
    client --protocol error--> gone [warn]; --disconnects--> gone [debug]

Other clients' windows and the event loop go on. Under the nested host a renderer or host window error
ends the session (there is nothing left to show on); on DRM a failed compose is logged and the loop goes
on.

Tests: `a_protocol_error_disconnects_only_its_client` (a duplicate role on a second connection; the healthy
client's title still changes afterwards), `a_rejected_dmabuf_disconnects_only_its_client`. A failed shm or
EGL import while drawing has no unit test.

### The window event ring and its replay (trace.rs)

Core's observer records events before they are applied and can't change the state. The ring keeps the last
256 window inputs: window events (a `WindowBatch` as one record), Home, HomeTap, HomeHeld, the window
chords (down, up, previous, next, close, quit, bottom view), and PadActive when it gives the keyboard back
to the top window. Each record has a sequence number, both clocks, and the selection before it (top,
bottom, keyboard, the number of windows).

    n < 256 records --a window input--> n+1
    256 records --a window input--> 256 [the oldest dropped]
    any --another event--> unchanged
    any --`clamshell ctl window events FILE`--> unchanged [DumpWindowEvents: the ring, the final selection and the windows, written as JSON at mode 0600 by a worker; success or failure logged]

`Recording::replay` applies the recorded clocks and inputs to a state fixture with the same library, rules
and launch context, checks each selection before an input and the final one, and reports the first
mismatch by its sequence number. A ring that wrapped needs a fixture as it was at its first record.

Tests: `recorded_window_batches_and_chords_replay_through_state` (a batch as one record, the JSON's round
trip, mode 0600, a write failure, a mismatch, replayed windows, title and focus),
`the_ring_keeps_only_the_last_256_window_inputs`,
`dumping_window_events_is_one_backend_effect_without_changing_windows`,
`the_window_event_dump_command_keeps_the_requested_path`; the randomized test generates the event.
`tests/nested-compositor.py` exports the live ring and checks it (at most 256 records, window inputs, an
empty final selection, X11 title changes and colour facts in it); the VM suite reads the windows from the
same export.

## Input in the compositor (compositor/, system/input/pad/)

Both hosts pass Smithay's input events to `Server::input` (`input.rs`): the nested host its winit window's
(absolute pointer positions, each touch event ending a frame), the DRM host libinput's on the seat
(relative mouse motion, libinput's own touch frames). An absolute device's position is placed in the scene
by its host: the nested window covers the whole scene; on DRM a touchscreen or tablet goes on the screen the
device profile names it on (an unnamed touchscreen on the profile's catch-all, anything else on the top
screen), turned back from the panel's mounting. DRM turns on tapping for ClamShell's virtual touchpad.
Every input event counts as the compositor's input for idle.

Tests: `a_turned_panels_corners_reach_the_screens_corners`,
`an_input_device_is_on_the_screen_the_profile_names_it_on`,
`a_device_is_named_by_its_ids_and_its_name_with_underscores`. The VM suite (DRM host) drives a virtio
multitouch screen and checks `touchpad` and `keyboard_points` (a tap clicks, a finger moves the pointer).

### Touch contacts (touch.rs)

Up to 32 contacts, each a slot with a target fixed at its down: a client surface (with the window's
geometry and the surface's origin), a UI screen, or nothing. Each slot keeps its own surface, so both
screens can be touched at once. Every down, motion and up hides the cursor; a down also dismisses menus
outside the window touched (see Popup menus).

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| none | down on a target of the screen's overlay as last drawn (`Core::press_overlay`) | ignored | the target acts once: a Home arrow switches the window, a notification is tapped, its close mark dismisses it | `touching_a_home_arrow_switches_once_and_consumes_home_release`, `touching_a_notification_taps_it_and_its_close_mark_dismisses_it` |
| none | down on a mapped, visible window's surface (not the black around it) | client | `WindowEvent::Activate`; wl_touch down in surface coordinates | `touch_contacts_keep_independent_surfaces_and_native_coordinates` |
| none | down on the bottom screen's UI, the bar over a window below included | UI (bottom) | Core press; keyboard focus kept | same; `the_bottom_keyboard_commits_text_and_sends_shortcuts_without_stealing_focus` |
| none | down on the top screen's UI (no top window) | UI (top) | `KeyboardFocus(None)`; Core press | GAP |
| none | down on black | ignored | — | `touch_contacts_…` |
| any | down on a slot already active | every contact cancelled, then the down as above | as cancel | GAP |
| any | down with 32 contacts active | unchanged | ignored (debug) | GAP |
| client | motion | client | wl_touch motion through the transform of its down, across screens too | `touch_contacts_…` |
| UI | motion | UI | Core move, in its first screen's coordinates | VM `two_fingers` |
| client or UI | up | none | wl_touch up, or Core release | `touch_contacts_…` |
| ignored, or an unknown slot | motion, up | unchanged | — | `touch_contacts_…`, `touching_a_home_arrow_…` |
| any | the host's cancel; the nested window losing focus; a resize; a client contact's window hidden or unmapped or its surface gone (checked at each window sync) | none | Core cancels the UI's presses; each client contact's position is sent again, then wl_touch cancel, so a finger that hasn't moved is cancelled too | `touch_contacts_…` (cancel, Home hiding the client), `x11_popups_share_parent_scaling_keep_tooltips_passive_and_cancel_dismissed_contacts`; the nested focus loss and libinput's cancel: GAP |

A wl_touch frame goes out only when something was sent since the last one (`touching_a_home_arrow_…`
checks that none is left pending). Real touchscreens: the VM suite (`two_fingers`: a slide goes on while
another finger taps a tab) and the device.

### Keys and text input (input.rs, keyboard.rs)

A key from a keyboard passes the compositor's filter before the focused client sees it, on both hosts:

| Key | Effect | Test |
|---|---|---|
| F12 down, up | `HomeHeld(true)`, `HomeHeld(false)` and `HomeTap`; not passed on | GAP (by hand) |
| Ctrl+Alt+F1 to F12 | DRM switches to that VT; nested ignores it; not passed on | on the device |
| F11, F10 down | `Device(WindowDown)`, `Device(WindowUp)`; not passed on | GAP |
| another key, with no keyboard focus in the state | to ClamShell's screens as the pad (`focus::desktop_key`: arrows, Enter, ...); not passed on | `desktop_keys_stand_for_the_pad` (the table) |
| another key | to the focused surface | the nested X11 focus checks |

Text input is Smithay's text-input-v3; the compositor is the seat's input method, so no input method
process is needed.

    inactive --the focused client enables and commits--> active [TextInput(true); the state raises the keyboard tab]
    active --disabled, or the active text input destroyed--> inactive [TextInput(false); the state goes back from the tab]
    any --the seat's keyboard focus changes--> [TextInput(false)]

The state's side: `a_text_field_brings_up_the_keyboard_and_back`,
`a_text_field_done_leaves_an_editor_on_the_keyboard_open`,
`a_text_field_doesnt_take_the_screen_from_an_open_editor`,
`a_keyboard_a_text_field_brought_up_isnt_saved_as_the_tab`.

An on-screen key goes to an open editor, the keymap's capture or shelf's find; otherwise it becomes
`Compositor::Key(stroke)`, which `Server::stroke` carries out:

- no client has the seat's keyboard: nothing;
- a printable character without Ctrl, Alt or Super, with a text input active: `commit_string` of the
  (shifted) character, then `done`;
- otherwise: the modifiers down (Ctrl, Alt, Super, Shift), the key down and up, the modifiers up in
  reverse order, from a keyboard source of its own, so a key held on a real keyboard stays held; these keys
  skip the filter above, so an on-screen F12 reaches the client;
- a modifier alone, Caps, F13 and above, or a key with no keycode: nothing (debug).

Touching the bottom screen's UI keeps the client's keyboard focus. Test:
`the_bottom_keyboard_commits_text_and_sends_shortcuts_without_stealing_focus` (commit strings and serials,
modifier order, keys before enable and after disable, a physical Shift kept, F12 delivered, the tab given
back, silence after Home). `tests/nested-compositor.py`: an X11 client receives whole strokes, a no-input
client none; the VM suite's `keyboard_points` types into foot.

### Idle and inhibitors (idle.rs)

The state: the last input's time, whether an inhibitor is in effect, and the last report. An
idle-inhibit-v1 inhibitor is in effect while its surface has a buffer and its root (through subsurface and
popup parents) is a mapped, visible window. Each inhibitor counts on its own; those of dead surfaces are
dropped. It is checked each turn of the loop, and only changes go to the state's idle machine (Idle and
sleep, above); the loop sets no wake of its own for the 10 s, so quiet is reported at the first turn after
them.

    input --10 s (NOTIFY_AFTER) without input, no inhibitor in effect--> quiet [Idle(CompositorQuiet)]
    quiet --input from the host--> input [Idle(CompositorInput)]
    quiet --an inhibitor comes into effect--> input [Idle(CompositorInput)]
    inhibited --the last inhibitor in effect destroyed, hidden, unmapped or dead--> input, the 10 s counted from now

Tests: `quiet_input_and_inhibition_report_only_transitions_and_restart_the_timeout`,
`inhibitors_count_individually_and_only_mapped_visible_surfaces_keep_idle_off`, and the state's
`the_compositor_holds_idle_off_while_it_sees_input_or_an_app_asks`. The screens' power follows the state's
`Compositor::Outputs`: DRM turns them off and on
(`a_screen_turned_on_is_drawn_whole_and_one_switched_has_no_frame_queued`); the nested host ignores it.

### The pad's activity watch (state/lib.rs, sync.rs; system/input/pad/)

The watch is armed while there is a top window without keyboard focus. Sync sends `PadActivity(watch)`
when it changes; the pad's supervisor keeps the value while the controller is gone and gives it to a new
router when one comes.

    disarmed --keyboard focus leaves the top window--> armed [PadActivity(true)]
    armed (router) --route Game; a button down or an axis past ±4096, outside Home's chords--> disarmed in the router [PadActive]
    any --pad input on ClamShell's screens--> [PadActive]
    state: PadActive with the top window unfocused --> the top window focused [PadActivity(false)]; otherwise nothing

Tests: `game_activity_is_reported_once_per_keyboard_focus_loss`, `game_activity_is_an_axis_past_a_threshold`,
`pad_activity_returns_keyboard_focus_to_top_without_moving_windows`,
`keyboard_focus_is_independent_of_both_screens_selected_content`, and the randomized test (the watch is
"the top window unfocused" after every event). Arming again after the controller comes back: GAP.

### Popup menus (popup.rs)

A menu belongs to its root toplevel and never becomes a window of the state. The state: at most one menu
(its window and grab chain). X11's menus are under X11.

| From | Event / condition | To | Effects |
|---|---|---|---|
| none | a grab on this seat, the root keyboard-focused and visible | menu | the pointer grab installed before the keyboard grab (replacing a parent's pointer grab releases its keyboard grab, so the other order would release the submenu's); keyboard focus to the popup |
| none | a grab on another seat, or the root unfocused or hidden | none | `popup_done` |
| none | the root missing or not a window of ours; a popup that can't be tracked or grabbed | none | client ended, logged |
| none | the seat holds another grab | none | chain dismissed |
| menu | a submenu's grab | menu | the same chain; focus to the submenu |
| menu | a submenu destroyed | menu | focus back to its parent popup |
| menu | the last popup destroyed | none | focus back to the toplevel |
| menu | a press (touch, or a first pointer button) outside the root window | none | dismissed, grabs released, then the press routed |
| menu | Home; keyboard focus to another window; the root hidden, unmapped or destroyed | none | dismissed, grabs released; Home still clears focus and shows shelf |

Test: `menus_keep_popup_focus_and_dismiss_on_outside_touch_home_or_client_destroy` (hit coordinates and
focus, an outside touch, Home, a submenu keeping its live keyboard grab, which pins the order, and giving
focus back to its parent, the parent unmapped and mapped again, one window in the state throughout and none
while unmapped, a menu releasing a pointer lock). Another seat, a conflicting grab and focus moved to
another window: GAP.

### The cursor (cursor.rs)

    hidden --pointer motion (absolute or relative)--> visible [the loop woken at the 3 s deadline]
    visible --3 s without motion--> hidden
    visible --touch down, motion or up; the nested window losing focus--> hidden

It starts hidden. Its image (`effective_cursor`): none while the pointer is locked; the client's request
(wl_pointer.set_cursor or cursor-shape-v1: Named, a theme image; Hidden, nothing; Surface, the client's
cursor tree at its hotspot, with frame callbacks) while a client has pointer focus and the pointer is over a
window (or a button grab holds a visible one); the theme's default otherwise (ClamShell's UI, a window
Home hid), and for a dead cursor surface. A cursor surface whose import fails ends its client and the
default is drawn. Smithay ignores a request with an older enter serial. Theme images come from
`XCURSOR_THEME` at the size nearest `XCURSOR_SIZE` (24 by default), loaded once per icon; a missing icon
tries "default", then a built-in arrow (logged). Animation follows the frames' delays and wraps, skipping
zero-delay frames (all zero: the first frame). Both hosts draw it above everything with the same code; the
nested host hides its window's own cursor.

Tests: `motion_shows_the_cursor_and_touch_or_three_seconds_without_motion_hides_it`,
`cursor_animation_wraps_and_zero_delay_images_do_not_stall_it`,
`cursor_requests_keep_hotspots_ignore_old_serials_and_release_on_home` (named, hidden, a surface with its
hotspot, an old serial ignored, touch hiding it, Home, the cursor surface destroyed).
`tests/nested-compositor.py`, with a virtual pointer in its sway: the cursor drawn into the scene, the
damage at its old and new places, the themed cursor after leaving a client, hidden after 3 s and shown
again by motion.

### Xdg activation (activation.rs)

A token is tracked only if it carries a serial of this seat no older than the keyboard's last enter, the
state's keyboard focus is a mapped window of the requesting client, which also has the seat's keyboard,
and its source surface, if any, is that client's. Otherwise the client still gets a token, which grants
nothing.

    tracked --5 s old--> removed at the next turn of the loop
    tracked --the seat's keyboard focus becomes none (Home, shelf)--> every token removed
    tracked --an activation request--> removed; if it was under 5 s old, the state has a keyboard focus and the surface is a window of the state: WindowEvent::Activate and a window sync

Activation follows the state's ordinary selection: it never moves a window between the screens, and Home
settled in the same dispatch wins. The token's app id is not used. A refusal is not a protocol error.

Test: `activation_tokens_require_focus_expire_are_single_use_and_cannot_undo_home` (missing and old serials,
another connection borrowing the serial, Home before the seat's sync at creation and at redemption,
success, reuse, expiry before pruning, an unknown surface, activating a window below without moving it, an
unmapped target); the state's activation tests pin the effects.

### Relative pointer and constraints (pointer.rs)

The relative-pointer and pointer-constraints globals exist only when the host gives relative motion: DRM
does (libinput's mice, the virtual touchpad); the nested host doesn't, since winit in the pinned Smithay
gives absolute motion only.

- Relative motion, while captured or on DRM: a relative event to the surface with pointer focus, both
  deltas divided by the window's scale; the cursor moves by it unless locked, kept within a confinement.
- Relative motion on an absolute host, not captured: to the client only.
- Absolute motion: ignored while captured; otherwise it places the cursor.

Motion never changes the state. Capture is settled each frame:

    free --relative host, host focused, no menu (xdg or X11), the pointer over a live surface with pointer focus of the keyboard-focused window, a constraint whose region holds the pointer, the host takes the mode--> locked or confined [constraint activated]
    free --the host refuses the mode--> free [a fatal protocol error to that client, flushed; the session goes on]
    locked or confined --host focus lost, a menu opened, the surface dead, keyboard focus moved, the window hidden or unmapped, its geometry changed, the constraint destroyed or inactive, the region no longer holding the pointer--> free [constraint deactivated; a persistent one may activate again, a one-shot one not]
    captured --the host can't release it--> session ends (logged)

Locked, the cursor stays put and hidden; confined, a step outside keeps whichever axis stays inside.
Cursor position hints are not applied. DRM captures by itself and never refuses.

Tests: `relative_motion_and_constraints_follow_geometry_home_regions_and_host_focus` (a free cursor moved
by relative motion, scaled deltas, 100 events without a state revision, a locked cursor fixed and hidden, a
persistent lock again after remapping, confinement, Home, region commits, one-shot, host focus lost, a
refused capture, the exact sequence of host modes), `a_host_that_cannot_release_capture_ends_the_unusable_session`,
and the popup test (a menu releases a lock). `tests/nested-compositor.py` checks that the nested host offers
no capture. A real mouse's relative motion: on the device.

## Frames, buffers, screenshots and the clipboard (compositor/)

Two hosts show the scene: the DRM host (`drm.rs`) on the device's screens, and the nested host
(`nested.rs`), a winit window on a desktop that redraws at each of the desktop compositor's frames. The
protocol tests (`tests.rs`, `tests/*.rs`) run real clients over a socket pair against a `Server`;
`tests/nested-compositor.py` runs the nested host in a headless sway, with software rendering, or with
`--gpu` on GLES and hardware EGL (vkcube needed too).

### Frame pacing (pacing.rs, frames.rs, drm.rs)

Each window has its `Frames`: `fresh` (a new buffer committed to its root surface and not yet shown), a
ring of the times of its last 256 frames shown, and, under a frame rate cap, when its next frame callback
is due. Each surface notes the commit drawn into the frame queued for its screen and the commit last
shown; the nested host notes both right after a submission, the DRM host the first when it queues and the
second at the flip.

The cap is the `FpsCap` setting of the window's running entry, when not 0 (`State::window_fps_cap`). A
capped launch gets `MESA_VK_WSI_PRESENT_MODE=fifo` and `vblank_mode=3`, so the game waits for the callbacks
the compositor paces (`a_windows_fps_cap_is_the_one_set_for_its_program`,
`a_capped_game_is_launched_presenting_in_fifo`).

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| shown | a new buffer committed to the window's root surface | fresh | | `only_new_buffers_count_once_shown` |
| fresh | a screen shows the window (DRM: its flip; nested: the submission) | shown | its time into the ring | same |
| shown | shown again with nothing new | shown | no frame counted | same |
| uncapped, mapped and visible | frame callbacks for its screen (DRM: after a flip, an idle answer, or held callbacks due; nested: each redraw) | | callbacks sent | the protocol tests; `tests/nested-compositor.py` |
| capped, waiting for a callback or on a FIFO barrier | due within a quarter refresh | | sent; next due a cap's interval later, or from now if more than one behind | `a_cap_paces_frame_callbacks_on_the_screens_refreshes` (40 fps at 60 Hz: two refreshes, then one) |
| capped, waiting | not due yet | held | the loop woken when due | same; `held_callbacks_go_once_due_and_wake_the_loop_until_then` |
| FIFO barrier set | callbacks sent, the surface's commit queued but not shown | set | | `a_barrier_goes_once_its_frame_is_shown_and_a_timed_commit_at_its_time` |
| FIFO barrier set | callbacks sent, its commit shown or never drawn | released | barrier signalled; the client's held commits apply; DRM composes again at once | same; `commits_let_through_after_composing_are_composed_at_once` |
| timed commit (`wp_commit_timing`) | its time no later than when the frame being composed is shown | applied | | the barrier test |
| timed commit | later | held | DRM wakes a refresh before it, or at the next frame if within a refresh | same; `a_timed_commit_wakes_the_loop_a_refresh_before_it_or_at_the_next_frame` |
| timed commit of a capped window | any time | applied | the cap paces it instead | `a_capped_windows_timed_commit_goes_ahead_at_once` |

A frame is shown, on DRM, at the next refresh after the top screen's last flip, a refresh later if a frame
is already queued (`a_frame_is_shown_at_the_next_refresh_after_the_last_flip`); nested, a host refresh from
now.

Each DRM screen, each turn of the loop (`drm.rs`, `step`, `screen_turn`):

    pending --any turn--> pending [no callbacks, frame or wake]                     a_queued_frame_waits_for_its_flip_without_turning_the_loop
    pending --flip--> idle [frame shown, feedback, frame callbacks]                 on the device; the VM suite draws through it
    pending --no flip for 1 s--> dirty [buffers reset, warn]                        a_frame_without_its_flip_for_a_second_is_lost
    idle --its part of the scene changed, screen on--> pending [presented, commits answered]   a_changed_screen_shows_a_frame_if_on_and_an_idle_one_answers_at_most_once_a_refresh
    idle --commits unanswered, a refresh since the last answer--> idle [frame callbacks]       same
    on --the state turns it off--> off [nothing pending]                            a_screen_turned_on_is_drawn_whole_and_one_switched_has_no_frame_queued
    off --the state turns it on--> dirty [buffers reset]                            same
    both off --turn--> both off [no compose unless a screenshot waits; dmabuf imports still run]   no unit test
    any --the seat resumed (back from another VT)--> dirty [pending cleared, buffers reset, the whole scene drawn]   on the device

The frame rate: at each turn of the server, for the mapped window in front on the top screen, the frames
shown in the last second and their mean spacing (with two or more) go to the state as `Reading::Frames`
when they change, every 500 ms while frames come, and as none once none came for a second, so the FPS tile
shows no game rather than 0 (`the_rate_is_the_last_seconds_frames_and_their_spacing`,
`the_ring_keeps_the_newest_frames`, `the_top_windows_new_buffers_shown_are_its_frame_rate`,
`the_frame_rate_is_kept_apart_from_the_metrics`). `tests/picture-scaling.py`, where Wine is installed,
changes a Wine-on-Wayland Vulkan game's mode through the FIFO path and checks that its window survives.

### Buffers and dmabuf imports (dmabuf.rs, handlers.rs)

Both hosts offer dmabuf as their renderer allows:

| Renderer | Offered |
|---|---|
| no dmabuf formats | no global |
| a software EGL device | no global: its import formats don't promise that native allocations can be sampled, and a deferred import can fail inside Mesa after EGL accepted it |
| a render device, feedback built | version 4 with default feedback: the main device and its format table, higher-precision formats kept, no scanout tranche |
| no render device, or the feedback failed to build (logged) | version 3 with the format list |

The DRM host also offers `linux-drm-syncobj-v1` when the GPU can wait on a timeline point through an
eventfd, and logs that only implicit fences are used otherwise. A commit with a new dmabuf waits until the
buffer is ready: its acquire point signalled, or without one its implicit fence readable; if the wait
can't be set up, a warning is logged and the commit applies at once (nested `--gpu` exercises the implicit
fences; explicit sync is checked on the device).

    create --fewer than 64 imports pending for the client--> queued
    create --64 pending--> rejected [warn; invalid_format ends the client]                     no unit test
    queued --the next compose (nested redraw; DRM compose, or each turn with both screens off), import succeeds--> created [created sent once]
    queued --import fails--> rejected [warn; invalid_format ends that client only]
    queued --the server dropped--> failed                                                      no unit test

Tests: `an_accepted_import_completes_once_and_an_empty_frame_has_no_import_work` (a renderer boundary that
succeeds: one buffer created, version 3 without a device, no work on an empty queue);
`a_rejected_dmabuf_disconnects_only_its_client` (version 3's modifier events and version 4's feedback with
a 10-bit format, the main device and the format table; one call at a failing boundary; the requesting
client ended; a healthy window's later title applies). `tests/nested-compositor.py` checks that the
software host offers no `zwp_linux_dmabuf_v1`, and with `--gpu` that the EGL client and vkcube create
dmabufs and vkcube gets its feedback; a real GPU import is checked only there.

### Presentation feedback (pacing.rs)

`wp_presentation` uses CLOCK_MONOTONIC. A feedback request belongs to the surface's next commit.

    committed --the window's screen showed the surface (nested submission or DRM flip), window mapped and visible--> presented [its screen as the output; sent once]
    committed --window hidden or covered, or the surface not in the frame's render states--> committed (kept)
    committed --a newer commit with feedback--> discarded
    committed --surface destroyed--> discarded

Drawing alone completes nothing; a failed nested submission ends the host before any feedback is sent.

| Host | Time | Refresh | Sequence | Flags |
|---|---|---|---|---|
| nested | the monotonic clock just after a successful swap (winit gives no display time) | unknown | 0 | none |
| DRM | the flip event's time (the clock's if the event has none) | the screen's frame interval | the flip's | vsync, hw_clock, hw_completion |

Test: `feedback_follows_submission_visibility_superseding_and_surface_destruction` (the nested form: clock
id, exact times, the output for a toplevel and its synchronised subsurface, nothing with no render states,
completion once, held at Home and completed on activation, superseded and destroyed feedback discarded).
`tests/nested-compositor.py` runs `examples/feedback-client` on both hosts' modes: a frame with a new buffer
and an unchanged one get ordered monotonic feedback with refresh 0, sequence 0 and no flags. The DRM flip's
feedback is checked on the device.

### Screenshots (server.rs, colour/render.rs; system/display/screenshot.rs)

The screenshot key (Home+R1, or `clamshell ctl screenshot`) gives `Effect::Compositor(Screenshot)`, which
the compositor keeps.

    idle --Screenshot--> requested
    requested --Screenshot--> requested (one capture)
    requested --the next nested redraw, or the next DRM compose (with both screens off too)--> idle [capture to Handles::save_screenshot]

The capture renders the composed scene of both screens through an sRGB output pass into an 8-bit RGBA
texture and reads it back with the top row first; the nested host then begins an empty frame to restore
its window's EGL target before the swap. A failed readback is passed on as the error (and logged by the
nested host). The system worker writes `~/Pictures/Screenshots/clamshell-YYYYMMDD-HHMMSS.png` (UTC),
making the folder if needed, and answers `Event::Screenshot(Ok(path))` or `Err(reason)`; the state shows
"Screenshot saved" or "Screenshot failed".

Tests: `a_screenshot_is_asked_for_and_its_result_shown`,
`screenshots_are_consumed_by_the_host_instead_of_the_parent_capture_tool` (coalescing, and the effect kept
by the host), `captured_pixels_are_saved_with_their_size_and_row_order`,
`invalid_captures_and_unwritable_folders_return_errors`, `timestamps_are_utc_dates`.
`tests/nested-compositor.py` saves one in an isolated home, checks it is a whole 1580×540 PNG, and that
clients still draw afterwards (which catches a lost EGL window binding). The DRM capture is checked on the
device.

### The clipboard (handlers.rs, windows.rs; system/session/clipboard.rs)

Smithay keeps the data-device, primary-selection and ext-data-control objects and the transfers between
them. The client with keyboard focus gets the clipboard and primary offers; with no surface focused, both
are cleared. The keeper is a system thread, an `ext_data_control` client connected to the compositor's
socket by its path, which keeps the last text copied, offers it itself while the clipboard is its, and
saves it in `$XDG_RUNTIME_DIR/clamshell/compositor/<socket>/clipboard` (mode 0600, gone at reboot); the
process's environment is not changed. Who may read the clipboard is in `security.md`.

    start --saved text up to 1 MiB--> kept [Reading::Clipboard; offered if the clipboard is empty]
    any --a text offer (the best of five text types, no x-kde-passwordManagerHint), read up to 1 MiB, UTF-8, different--> kept [file written, Reading::Clipboard, the primary selection set to it]
    any --a text offer over 1 MiB or not UTF-8--> unchanged
    kept --a non-text or password manager's offer--> none [file removed, Reading::Clipboard(None)]
    kept --clipboard emptied (its program quit)--> kept [its own offer becomes the clipboard]
    own offer --cancelled (replaced)--> kept [own offer dropped]

`clamshell --clipboard get|set` connects through `WAYLAND_DISPLAY`; `set` offers its text until it is
taken, at most 5 s.

Tests: `the_best_text_type_is_taken_and_a_password_managers_copy_is_none`,
`a_copied_text_is_kept_up_to_its_limit_and_only_as_utf8`. `tests/nested-compositor.py` checks the command's
set and get, the text kept after its source quits, the saved file under `compositor/<socket>/`, the desktop's
clipboard unchanged, foot's OSC 52 copy through wl_data_device, the primary selection read back by foot,
and the copy outliving foot; the offers following keyboard focus have no unit test, and foot's copy and
paste there depend on them. The VM suite's `clipboard` check runs on the DRM host. The X11 side is under
X11.

### Real clients (tests/nested-compositor.py)

`examples/egl-client`, a winit Wayland EGL client that clears to one colour, beside foot: on top, moved
below and back, ended with exit 0 by `clamshell ctl window close` (Home+B), foot left on top. With `--gpu`,
`vkcube --wsi wayland`: changing frames on top, still presenting with SMAA on (its edges blended), below
after the resize, and back on top, and exit 0 on close. Home, the clipboard and the window event ring are
checked after both. Screenshots and the clients' `WAYLAND_DEBUG` logs stay in the directory it prints.

## X11 (compositor/x11/)

Xwayland runs on demand behind one display socket, and ClamShell is its window manager over a private
connection. Managed X11 windows reach the state as the same facts as Wayland ones; menus, tooltips and
selections stay in the adapter. `tests/nested-compositor.py` drives real X11 clients (`examples/x11-client`,
`x11-selection`, `x11-copy`) on the software and GPU hosts; its checks are named here by what they say. Test
files: mod `x11/mod.rs`, transport `x11/transport.rs`, wm `x11/wm.rs`, properties `x11/properties.rs`,
popup `tests/x11_popup_tests.rs`, send `x11/selection/send/tests.rs`, receive
`x11/selection/receive/transfer/tests.rs`.

### Xwayland (x11/mod.rs, x11/socket.rs)

One reserved display socket (`/tmp/.X11-unix/X<n>` and its lock file, the first free n below 128; a lock
whose owner is dead is taken over, any other taken slot skipped) and at most one Xwayland. The display is
given to launches as DISPLAY, and on the device to the user manager and the bus too; the compositor's own
environment is not changed. The compositor never accepts an X11 connection: Xwayland gets the listener
(`-listenfd`) and accepts it. Its environment holds only PATH, XDG_RUNTIME_DIR and WAYLAND_SOCKET, and its
four private descriptors (Wayland socket, WM socket, display number pipe, listener) are passed only in the
child's exec setup (`private_descriptors_are_inherited_only_by_the_selected_child`).

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| dormant | the listener readable | starting | spawn Xwayland; insert its Wayland client | mod `dormant_xwayland_has_no_child_or_deadline_and_a_connection_wakes_its_listener`; nested "Wayland-only use leaves Xwayland dormant", "X11 client rendered on top" |
| starting | spawn fails | dormant | logged; again after 1 s while the connection waits | GAP |
| starting | display number (5 s, at most 16 bytes), X11 setup, atoms, X-Resource 1.2, Composite | connected | the hidden owner window; SubstructureRedirect on the root; Composite redirect; WM_S0 and _NET_WM_CM_S0 claimed; the RandR mode sizes sent once (`Reading::XwaylandModes`; no RandR is logged) | every nested X11 check depends on it |
| starting or connected | a transport error; a reply, focus time or selection time past its deadline; BadAccess (another WM); the WM_S0 anchor naming another owner; an extension missing | stopping | logged; its windows closed in the state; the selection bridge cleared; its Wayland client ended; SIGKILL | the transport tests below; wm `outstanding_deadlines_stale_replies_and_menu_ownership_follow_window_lifetimes` (a reply's deadline) |
| any | pidfd readable, the child gone | dormant | reaped without waiting; its windows closed; its client ended; the listener armed again | nested "crashed Xwayland is reaped through pidfd readiness", "crash recovery waits for another X11 connection" |
| dormant after an exit | the next X11 connection | starting | the replacement on the same display | nested "another client starts a replacement Xwayland", "Xwayland starts again on demand", "Wayland survives Xwayland restart" |
| any | the compositor ends | — | the child killed and reaped on a thread of its own; only this instance's socket and lock removed | mod `dormant_…` |

Dormant, or connected and idle, it has no deadline and no periodic wake (nested "Wayland still moves while
idle Xwayland is stopped": an idle Xwayland under SIGSTOP is not restarted).

### The X11 transport (x11/transport.rs)

The WM's connection is a non-blocking socket; x11rb supplies the wire types and sequence tracking, never
I/O. Readiness of the socket, the display pipe, the listener and the pidfd are event loop sources; write
interest is there only while bytes wait. Each turn does at most 64 writes, 64 setup reads, 64 packet reads
of up to 4096 bytes and 64 events. Packets and the outgoing queue are capped at 1 MiB, a packet's length
checked before its payload is read. Partial setup, packets and writes keep their bytes. Deadlines: setup
3 s; a queue that started waiting must empty within 3 s; each reply 3 s. At most 256 of the WM's replies
are outstanding: property queries wait for room, any other request past that stops Xwayland.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| handshake | partial setup | handshake | bytes kept | every transport test's fixture |
| handshake | 3 s without setup, or EOF | stopping | | transport `setup_timeout_eof_and_oversized_packets_end_only_the_connection` |
| connected | a packet over 1 MiB | stopping | nothing allocated | same |
| connected | a partial reply | connected | polled once whole | transport `partial_replies_are_polled_and_idle_connections_never_probe` |
| connected, idle | turns | connected | no traffic, no deadline | same |
| writing | the peer not reading | writing | WouldBlock keeps the bytes; past 1 MiB refused; 3 s: stopping | transport `full_write_buffers_keep_the_queue_bounded_and_still_return_to_the_loop` |
| connected | 32,768 requests since the last reply | anchoring | GetSelectionOwner(WM_S0), so the 16-bit sequence can't wrap unanchored; the reply must name the WM | transport `sequence_anchors_follow_actual_traffic_across_wire_wrap`; the owner check: GAP |

### X11 windows (x11/wm.rs, x11/properties.rs, x11/shell.rs)

The WM follows the root's children it didn't create, at most 128; the client of one more is ended
(KillClient). Each gets a model id never used again, so a late reply for a destroyed window can't change
one that took its XID. Override-redirect windows are menus (below); input-only windows are mapped as asked
and never reach the state. The facts of the others: app id, WM_CLASS's class; title, _NET_WM_NAME (UTF-8),
else WM_NAME (Latin-1); parent, WM_TRANSIENT_FOR while that window is open; limits, WM_NORMAL_HINTS' sizes;
role, dialog, splash or utility from _NET_WM_WINDOW_TYPE, and dialog for WM_TRANSIENT_FOR without a type;
process, X-Resource's client PID, never _NET_WM_PID or Xwayland's own credentials (the unit and processes
follow from it as for Wayland). Properties are read up to 4 KiB, their type and format checked; one absent
or malformed clears its fact.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| — | CreateNotify on the root | known | PropertyChange selected; no queries | wm `outstanding_…` |
| known | MapRequest | querying | attributes, nine properties, the PID (waiting for room under the 256) | same |
| querying | every reply | placed | Opened; ConfigureWindow (the presentation's geometry; _NET_WM_STATE fullscreen or empty; WM_STATE Normal); MapWindow; then focus | wm `placement_is_committed_and_configured_before_mapping_and_focus` (the request order; nothing again when unchanged) |
| querying or placed | an error reply to its query | removed | Closed if open | GAP |
| placed | WL_SURFACE_SERIAL (two 32-bit halves) and the surface committed with that serial through `xwayland_shell_v1` | drawable | the surface bound once; drawn, hit and paced as a Wayland one. A serial is never reused, unlike a surface's id; serial 0 is a protocol error | wm `a_window_is_paired_with_the_surface_committed_with_its_serial`; `a_committed_serial_pairs_its_surface_once_and_only_xwayland_sees_the_shell` |
| placed | PropertyNotify of a property it reads | placed | the property read again (once more if it changes meanwhile); changes held to the frame, then Changed and Title (one batch when both), the state's unit and processes kept | wm `placement_…`; nested "X11 property changes enter the ordinary window-event ring" |
| placed | ConfigureRequest | placed | the size asked for kept and configured again; position and stacking ignored | GAP |
| placed, fullscreen | _XWAYLAND_RANDR_EMU_MONITOR_RECTS gives a mode's size | placed | configured at that size; Xwayland scales it to the screen | GAP |
| placed | the state closes it | placed | WM_DELETE_WINDOW if offered, else KillClient | nested "X11 client receives WM_DELETE_WINDOW"; KillClient: GAP |
| placed or drawable | UnmapNotify | known | Closed; the surface detached; its menus dismissed | nested "unmapping X11 restores the Wayland client" |
| known, was open | MapRequest | placed | Opened with the same id | nested "the same X11 window maps again" |
| any | DestroyNotify | removed | Closed if open; menus dismissed | GAP |

The facts: properties `property_fixtures_translate_to_the_same_protocol_neutral_facts`
(`fixtures/x11-properties.json`) and `focus_protocols_and_input_hints_are_validated_and_removed`; nested
"XRes reports the app, not Xwayland", "fixed-size X11 content is centred on black", "X11 transient maps
before its parent", "a transient stays centred even before its parent maps".

### X11 focus (x11/wm.rs)

X11's focus follows the state's keyboard focus among open X11 windows, and is None when a Wayland window or
nothing has it. The server ignores a focus request older than the last focus change, which an active client
may have made itself, so each change takes a fresh server time
([ICCCM](https://xorg.freedesktop.org/releases/current/doc/xorg-docs/icccm/icccm.html#input_focus), 2.1 for
the append).

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| settled | the keyboard focus changes | stamping | an empty append to _THOR_TIMESTAMP on the hidden window | wm `placement_…` |
| stamping | its PropertyNotify (time t) | settled | SetInputFocus (the window if WM_HINTS' input is true or absent, else None) at t; WM_TAKE_FOCUS at t if offered; _NET_ACTIVE_WINDOW | same (passive, locally active, globally active and no-input; the same nonzero t in both; a new time per change) |
| stamping | no PropertyNotify within 3 s | stopping | | GAP |
| stamping | the focus back where X11 has it before t comes | settled | t dropped | GAP |
| settled | the focused window's WM_HINTS or WM_PROTOCOLS read again | stamping | focus sent again | GAP |

Nested: "active X11 client receives a timestamped focus offer" (the fixture fails on time 0), each focus
model's policy and key strokes, "no-input clients receive no keys", "Home removes X11 keyboard focus",
"remapping restores X11 keyboard focus", "touching the bottom parent restores its keyboard focus".

### X11 menus and tooltips (x11/menus.rs, popup.rs, surface.rs)

Override-redirect windows are drawn as popups of a managed parent and never reach the state; a tooltip
(_NET_WM_WINDOW_TYPE_TOOLTIP) takes no input. The owner is found by following WM_TRANSIENT_FOR through
mapped override-redirect ancestors to a managed window; without it, the focused managed window (for a
tooltip, the one under the pointer, else the focused one) with the same X-Resource PID. A missing or
unmapped ancestor, or a cycle (the walk is bounded by the number of windows), gives no owner. A shown
popup's owner stays until it unmaps. Popups draw and hit-test with their parent's scale and screen clip.
While a popup that takes input is shown, pointer capture is released and not granted (GAP). A touch on a
popup that is detached cancels the touch sequence.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| known | MapNotify, its replies in, a buffer, its owner open and visible (and focused, for a menu) | shown | attached where it is relative to the parent | wm `outstanding_…` (the same PID, the owner under the pointer, a fixed owner, an ancestor unmapped, cycles); nested "X11 menu is drawn over its parent", "X11 menu receives local pointer coordinates", "X11 submenu is drawn above the parent menu", "a parentless menu belongs to the focused client with the same XRes PID", "a menu can extend to its screen edge", "menu pixels cannot cross into the other screen", "X11 menu stays local to a bottom-screen parent", "X11 tooltip is drawn" |
| known | MapNotify without an owner | known | UnmapWindow | wm `outstanding_…` (no owner); the unmap: GAP |
| shown menu | a press outside its window's popups | dismissed | detached; UnmapWindow | popup `x11_popups_share_parent_scaling_keep_tooltips_passive_and_cancel_dismissed_contacts`; nested "an outside press dismisses the X11 menu chain" |
| shown menu | its parent loses the keyboard | dismissed | the same | nested "Home dismisses a menu even while its bottom parent remains visible" |
| shown | the parent's geometry changes, or it is hidden | dismissed | the same | popup `x11_popups_…`; nested "moving a parent dismisses its old menu" |
| shown | an ancestor menu or the parent unmapped or destroyed | dismissed | the same | nested "unmapping a menu dismisses its submenu" |
| shown | its surface destroyed | dismissed | the same | nested "destroying X11 popups removes their pixels" |
| shown tooltip | a press, a focus change | shown | input goes through to the parent | popup `x11_popups_…`; nested "X11 tooltip does not intercept pointer input" |

### Wayland selections to X11 (x11/selection/mod.rs, send.rs)

The seat's selection callbacks keep, for CLIPBOARD and PRIMARY each, a revision and up to 64 MIME types of
at most 255 bytes (longer names dropped); nothing reaches the state. An offer imported from X11 is never
exported back. UTF8_STRING, TEXT and STRING are offered when the source has text/plain;charset=utf-8 or
text/plain. Replies and the timestamp have 3 s; a miss stops Xwayland.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| any | a new seat revision, not imported | preparing | the selection's transfers cancelled; an empty selection releases ownership at its acquisition time; InternAtom for each type | send `late_atom_replies_cannot_restore_an_old_generation_and_missing_replies_have_a_deadline`, `clearing_a_source_releases_ownership_at_its_acquisition_time_and_removes_read_watches` |
| preparing | an atom reply of an older revision | preparing | ignored | send `late_atom_replies_…` |
| preparing | every atom, at least one format | awaiting time | TIMESTAMP replaced on the hidden window | send `server_timestamp_precedes_ownership_and_stale_clears_cannot_cancel_a_new_offer` |
| awaiting time | its PropertyNotify, the same revision | owned | SetSelectionOwner at that time | same |
| awaiting time | a time asked for an older revision | preparing | asked again | same |
| owned | SelectionClear at or after the acquisition time | lost | transfers cancelled; formats forgotten; an unchanged revision never claims it again | same; nested "a new X11 owner is never reclaimed without a new Wayland copy" |
| owned | a TARGETS or TIMESTAMP request | owned | the property, then SelectionNotify | nested |
| owned | a stale revision, an unknown target (MULTIPLE too), the source refusing, the requestor's property busy, 8 transfers running | owned | SelectionNotify None | send `unsupported_duplicate_excess_stale_and_missing_source_requests_are_refused`; nested "unsupported targets are refused" |
| owned | a supported request (property None means the target) | reading | the source into a non-blocking pipe; the requestor watched; 5 s deadline | nested "None property falls back to the requested target" |
| reading | EOF under 16 KiB | done | ChangeProperty, then SelectionNotify; pipe closed | send `small_and_empty_transfers_set_the_property_before_notifying_and_close_the_pipe` |
| reading | 16 KiB buffered | incremental | INCR, SelectionNotify | send `incr_waits_for_each_delete_keeps_one_bounded_chunk_and_finishes_with_zero_length` |
| incremental | PropertyNotify Delete | permitted | deadline renewed | same |
| permitted | data buffered | incremental | one ChangeProperty of up to 16 KiB | same |
| permitted | EOF, nothing buffered | done | a zero-length property | same |
| reading or incremental | 5 s without progress, a read error, the revision replaced, SelectionClear, Xwayland stopped | cancelled | before INCR SelectionNotify None; after it nothing (never a false end) | send `timeout_and_replacement_cancel_only_their_transfers_without_a_fake_incr_eof`; nested "a reader that stops acknowledging times out", "a timed-out transfer does not stop the clipboard" |
| any | the requestor destroyed | removed | | nested "a dead requestor does not disrupt another reader" |

A transfer reads at most four times 4 KiB per turn, and has a read watch only while its buffer has room and
the pipe hasn't ended. STRING is UTF-8 turned into Latin-1, with `?` for what can't be written; a scalar
split between reads (up to 3 bytes) is kept and counts in the 16 KiB (send
`latin1_translation_preserves_split_scalars_and_bounds_incomplete_input`,
`split_invalid_utf8_at_a_chunk_boundary_cannot_exceed_the_payload_budget`). Nested: X11 reads the
Wayland clipboard and primary selection, slow INCR reads of both, a UTF-8 scalar across an INCR chunk's
edge, STRING in Latin-1, the desktop's clipboard untouched.

### X11 selections to Wayland (x11/selection/receive.rs, receive/transfer.rs)

Once connected, the adapter asks for XFixes 2 (without it, logged, nothing is imported), subscribes to
CLIPBOARD's and PRIMARY's owner changes, and asks each's owner once; nothing polls. Each owner change starts
a generation, which requests and replies carry, so an older one can't change a newer one. Each conversion
has a fresh input-only requestor and `_THOR_SELECTION`. Replies have 3 s (a miss stops Xwayland), at most
256 outstanding.

| From | Event / condition | To | Effects | Test |
|---|---|---|---|---|
| any | the owner becomes an X11 client (notified, or the first reply for the current generation) | discovering | the selection's conversions cancelled; ConvertSelection(TARGETS) at the owner's time; the seat's offer kept meanwhile | receive `replacement_owner_cancels_old_requests_without_publishing_an_empty_selection_between_offers`, `conversion_and_mailbox_limits_close_only_new_readers_and_old_owner_snapshots_are_ignored` |
| discovering | TARGETS read (up to 4 KiB, INCR allowed) | naming | UTF8_STRING, else STRING (Latin-1), as text/plain;charset=utf-8 and text/plain; GetAtomName for up to 62 others | receive `offers_wait_for_names_are_tagged_and_do_not_echo_or_clear_newer_wayland_selections` |
| naming | every name | offered | names up to 255 bytes with a `/` (or x-kde-passwordManagerHint) kept; the seat's selection set, with its generation | same; nested "Wayland reads an X11-owned clipboard" |
| discovering or naming | no usable format | none | this selection's imported offer cleared | GAP |
| discovering, naming or offered | the Wayland selection changes | none | conversions cancelled; Wayland's selection kept | GAP |
| offered | the owner none, or the WM itself | none | the seat's selection cleared only if still this offer | receive `replacement_owner_…` |
| offered | the export's turn | offered | never exported back | receive `offers_wait_…`; nested "the imported offer is not echoed back as a new X11 owner" |
| offered | Xwayland stops | none | only this offer cleared; waiting readers closed | receive `offers_wait_…`, `conversion_and_mailbox_…`; nested "keeper survives destruction of the imported X11 offer", "a replacement adapter imports a fresh X11 generation" |
| offered | a reader for a live type and generation, fewer than 8 waiting and 8 running (both selections together) | converting | ConvertSelection; otherwise the reader's fd closed | receive `conversion_and_mailbox_…` |
| converting | SelectionNotify for its selection, target and property | reading | GetProperty of a slice (16 KiB; Latin-1 8 KiB) | receive `incr_deletes_only_after_backpressure_and_all_property_slices_have_drained` |
| converting | refused, or not what was asked | done | requestor destroyed, pipe closed | receive `refusal_timeout_dead_reader_and_late_reply_do_not_affect_a_replacement`; nested "a refused or malformed transfer closes only its reader" |
| reading | INCR at offset 0 | chunk | DeleteProperty | receive `incr_deletes_…` |
| reading | a wrong type or format, too large, a malformed INCR | done | | receive `normal_latin1_empty_and_invalid_properties_finish_only_their_own_conversion`; a malformed INCR: GAP |
| reading | a valid value | draining | Latin-1 turned into UTF-8 | same |
| draining | the pipe full | draining | a write watch; no reads, no DeleteProperty | receive `incr_deletes_…` |
| draining | all written, more in the property | reading | GetProperty at the next offset | same; nested "large ordinary X11 properties are sliced without early deletion" |
| draining | all of an INCR chunk written | chunk | DeleteProperty | same |
| chunk | PropertyNotify NewValue | reading | | same |
| draining | a whole property done, or a zero-length INCR chunk | done | DeleteProperty, the requestor destroyed, pipe closed | same |
| any | 5 s without progress, the reader gone, the owner changed, a newer Wayland selection | done | | receive `refusal_timeout_…`; nested "a stalled Wayland reader is cancelled", "a failed reader does not disrupt another transfer" |

Nested also: large CLIPBOARD and PRIMARY imports through INCR, arbitrary MIME bytes under the pipe's
backpressure, a new owner replacing one whose TARGETS were pending, an old owner dying without clearing its
replacement, Latin-1 STRING into UTF-8, the keeper keeping the text after the X11 owner quits, X11's copies
kept from the desktop's clipboard.

## Device facts (crates/device, system/profile.rs, compositor/drm.rs)

What a profile holds and why is in `device-profiles.md`. This part is how the profile is chosen, which
wins between what is detected and the profile's hints, and the machines that depend on them.

### Identification (device/lib.rs `Profile::identify`, system/profile.rs, fan/main.rs)

Every `clamshell` command and `clamshell-fan` identify the device once, at start, by the same pure
function; `State` gets the profile in `Init` (`State::profile`).

| Transition | Test |
|---|---|
| `start --CLAMSHELL_DEVICE thor, vm, vm-hdr or generic--> that profile` (detection skipped) | device `identity_uses_exact_compatible_entries_dmi_and_an_explicit_override` |
| `start --the device tree's compatible has the whole entry "ayn,thor"--> thor` (a longer entry such as `ayn,thor-other` doesn't match) | same |
| `start --DMI's sys_vendor "QEMU" or product_name "QEMU Virtual Machine"--> vm` | same |
| `start --otherwise--> generic` | same; system `kernel_identity_is_read_from_the_supplied_root_and_unknown_overrides_fail` |
| `start --an unknown CLAMSHELL_DEVICE--> exit [the error names it]` | both |
| `start --an identity file missing--> read as empty`; unreadable otherwise: ClamShell logs it and reads it as empty, the fan daemon exits 1 | the system test (missing files); the fan daemon: GAP |
| `generic --clamshell --drm--> exit [no connector for the top screen]`, as for a named connector absent or not connected (`drm.rs` `choose`) | GAP; the VM suite runs the DRM session with the vm profile |

### What is detected, and the profile's hints

What the hardware reports wins; the profile's value is used only when nothing is reported; with neither,
the fact stays unknown and its UI is dimmed, not hidden.

| Fact | Precedence | Test |
|---|---|---|
| the screens (`drm.rs` `choose`) | the connector the profile names; the mode the kernel set, else the preferred one, else the first; turned by the profile's rotation | the VM suite (Virtual-1 and -2); on the device (DSI-2, DSI-1) |
| the second screen's density, for the virtual touchpad (`drm.rs` `bottom_density`, `system/input/touchpad`) | the connector's millimetres, turned, else the profile's hint (the Thor's only, 16.5 px/mm), else none and no virtual touchpad; nested, the hint only | compositor `the_second_screens_density_is_its_panels_or_else_the_profiles`, device `only_a_known_physical_panel_supplies_a_density_hint`, touchpad `both_reported_axes_are_kept_and_unknown_resolution_is_not_guessed`; VM `touchpad` |
| a backlight's label (`levels` `backlight_label`) | the backlights found; one named as the profile's top or bottom hint reads "Top screen" or "Bottom screen", any other its kernel name | levels `the_thors_backlights_by_their_screens` |
| an absolute input device's screen (`drm.rs` `screen_of`) | the profile's entry for it; else, for a touchscreen, the profile's catch-all; else the top screen | compositor `an_input_device_is_on_the_screen_the_profile_names_it_on` |
| the auxiliary button (`system/input/pad/device.rs`) | an evdev device of the profile's name and key, looked for when a controller opens; its presses become the auxiliary role; without the hint, none | device `physical_back_codes_are_normalised_and_missing_auxiliary_controls_stay_absent`; VM `pad_guide` |
| Back | the profile's physical code becomes the Back role, in events and on resynchronisation, and is kept off the virtual pad | system `control_normalisation_preserves_other_keys_and_axis_codes`, device `physical_back_codes_…` |
| X and Y | the controller's driver read from its sysfs link; a driver in the profile's `letter_code_drivers` swaps them | device `button_conventions_are_facts_about_the_detected_driver`, system `x_and_y_swap_between_letters_and_places` |
| motion (`pad/mod.rs` `find_motion`) | the controller's evdev motion sensor, else the profile's sensor hub (QRTR service, rate, clock, axes), else none; looked for again after waking | device `only_the_physical_profile_claims_a_sensor_hub`, sensor hub `the_thors_frame_is_turned_a_quarter`; VM `gyro_sleep` (evdev); the hub on the device |
| the panels' colour (`drm.rs`, `nested.rs`) | DRM: the profile's description of each screen; nested: the host's, once its colour management answers, else sRGB | device `panel_facts_preserve_the_documented_headroom_without_claiming_a_ten_bit_link`, `tests/device_profiles.rs` `panel_queries_follow_the_selected_profile`; which one is used: GAP |
| the canvas (`src/session.rs`, `server.rs` `resize`) | DRM: the screens' modes, turned; nested: the profile's preview size; a host resize keeps the screens' width ratio, at least 4×4 and 2 px a screen | compositor `requested_preview_geometry_sets_the_initial_canvas_and_resize_ratio` |

### Refresh calibration without the profile's entry (system/display/refresh.rs)

The Device view's Refresh row is under Device view. Without `refresh_calibration` in the profile:

| Transition | Test |
|---|---|
| the row reads Unavailable, dimmed, still tappable | GAP (the snapshots use the Thor's profile) |
| `Idle --tap--> Running [CalibrateRefresh, the bottom screen off]`; no program is started, and `RefreshCalibrated(Err)` answers it: `Done(Err)` | refresh `a_profile_without_calibration_reports_unavailable_without_running_a_tool`, device `the_refresh_calibration_runs_once_at_a_time_and_says_how_it_went` |
| `thor-panel-calibration` takes its facts from `clamshell device panel` (rates, attributes, step, tolerances) and exits saying there is no panel to calibrate when they are null | `panel_queries_follow_the_selected_profile`; the frame length search on the device |

### Charging (system/power/tuning.rs)

The tuning thread chooses how to hold the limit once, at start, and finds each power supply's attributes
and the OEM switches again at each settings application and every 30 s while the Charge limit is under
100.

| Transition | Test |
|---|---|
| `start --CLAMSHELL_CHARGE_LIMIT current, threshold or firmware--> that way` | `detected_controls_take_precedence_unless_the_profile_records_a_firmware_quirk` |
| `start --the profile's prefer_current (the Thor: its firmware ignores the threshold)--> current`; otherwise `detected` | same |
| `detected --the supply has charge_control_end_threshold--> threshold [the limit as end threshold, current at its most]`; `--the threshold gone--> current` | same |
| `current`: threshold 100; current 0 at or over the limit, the most again 5 below | `a_charge_limit_holds_at_the_limit_and_charges_again_five_below` |
| `firmware --both switches there (named by the profile), limit 80--> [the 80 % switch on, charging on, current not held]`; switches missing or another limit: held as current | `firmware_switches_require_both_detected_attributes_and_use_the_supplied_names` |
| `any --Bypass (0)--> [current 0, threshold 100; with the firmware's switches, charging off]` | same |
| `any --the session ends--> [limit 100: threshold 100, current at its most, switches reset]` | same |

Whether the firmware honours a write is checked on the device.

### Display modes and the Refresh tile (state/device/display.rs, state/apply/settings_tab.rs, compositor/drm.rs)

`DisplayModes` holds at most 32 sorted, unique rates (1 to 1000 Hz) and the rate at start; the profile gives
none.

| Transition | Test |
|---|---|
| `unknown --DisplayModes at the DRM host's start (the top connector's rates at its current size, the start rate)--> known`; nested sends none and stays unknown | `rates_are_bounded_sorted_deduplicated_and_never_invented`; the DRM report: the VM suite, the device |
| `any --DisplayModes--> replaced`; the saved Refresh unchanged; effects only if the rate wanted changes | `refresh_taps_follow_announced_modes_and_reports_never_change_saved_choices`; the randomized test; `allocations_per_event` |
| Refresh tap: 0 ("as is"), then each rate: `--tap--> the next`; `--a value not listed, or no rates--> 0` [Save(Settings); Apply and Compositor::Refresh when they change] | `refresh_taps_follow_…` |
| the rate wanted: a running calibration's, else the last launch's settings until its window shows, else those in force; Refresh 0 under a frame cap the start rate isn't a multiple of: the highest rate the cap divides (the tile says "for the cap") | `a_cap_as_is_gets_a_rate_it_divides`, `what_the_outside_starts_without_goes_out_at_the_first_event`; the randomized test |
| the compositor: `Refresh(0)`, the start mode; `Refresh(hz)`, the nearest mode of the top screen's size | compositor `the_mode_for_a_refresh_is_the_nearest_or_the_screens_own` |
| no rates: the tile dimmed, still tappable | GAP |

### The fan daemon without a policy (fan/main.rs, packages/clamshell/clamshell-fan.service)

| Transition | Test |
|---|---|
| `start --the profile has no fan--> exit 0` (with `--safe` too); `Restart=on-failure` leaves it stopped | device `unsupported_profiles_do_not_invent_a_curve_and_known_curves_are_ordered`; the exit itself: GAP |
| `start --identification fails--> exit 1`, restarted every 2 s | GAP |
| `start --a policy--> running` on the profile's hwmon and curves | `waits_for_the_fan_then_takes_it_and_sets_it`, `a_different_board_policy_changes_interpolation_and_fallback_without_device_checks`, `stopping_leaves_it_at_the_safe_speed`; on the device |

The Fan tile is dimmed when the profile has no fan (GAP).

## Interactions

| From | Route | To | Test |
|---|---|---|---|
| shelf play, Continue, Open in, Pin | queued `Event::Entry` | session, dock | shelf tests |
| launch of a group / the Device tool | `Session::launch` returns what to do instead | shelf, Device view | `a_group_started_…`, device tests |
| plays ended | settle fills a reused buffer, then `shelf.record_play` | shelf | tests `a_launchs_start_and_end…`; shelf `a_play_recorded_keeps_shelfs_place_and_focus` |
| a game starting | settle returns it | bottom (`unwake`) | `dark_for_a_game_…` |
| notices | `&mut NoticeSlot` | bottom | per area |
| settings in force | parameters to launch and background; sync Apply | session, system | settings tests |
| refresh rate | sync `Refresh(hz)`: a launch's rate from its launch, the settings in force's when applied, whichever came last (a refresh calibration's while it runs); sent when it changes | compositor | tests `the_refresh_rate_follows_a_launch_at_once_then_the_settings_in_force` |
| screens' power | sync `Outputs{top, bottom}`: top is the screens on (idle), bottom the second screen as set (and off while a refresh calibration runs); sent when either changes. The compositor turns the screens off and on (DRM); how it is carried out is checked on the device | compositor | tests `the_screens_power_is_sent_when_it_changes`, idle and screen tests |
| program in force | settle → pad mouse; sync Apply, Keymap, PadRoute (keymap before route: the pad thread takes the game's keymap before it has the pad) | pad, system | `a_game_coming_in_front_gets_its_settings_then_its_keymap_then_the_pad`, `a_keymap_and_route_changed_in_one_event_are_sent_keymap_first` |
| front, tab, listening, dock move | `reconcile_pad` | pad | pad tests |
| windows down, program, battery, refresh, editor | `power()` for the bottom screen | bottom | screen tests |
| idle stage | sync: dim and Outputs; status watch | levels, system | idle test |
| tabs | sync watches; `left_tab` closes the dock menu, releases levels, resets the time page, closes the battery menu | dock, levels, status | per tab; `leaving_the_tab_resets_it_to_its_main_page` |
| profile (keymap) chosen/deleted/made own | queued `Event::Profile(kind, ProfileEvent)` → prefs, the kind's setting | settings | keymap tests |
| device view, files, keymaps naming | queued `EditOnKeyboard` / editors | bottom | per area |
| Library read | problems notice, shelf reload, pausable runners, runner keymaps, dock pins, FetchFirst | bottom, session, keymap, settings, dock | tests `new_library_problems_are_said_once`, `a_library_read_updates_the_runners_paused_together`, `a_library_read_while_fetching_asks_for_shelfs_view_first_again`; shelf `listings_outlive_…`, `a_library_read_keeps_what_is_typed`; settings `a_runner_gets_its_keymap_once`; dock `pins_of_entries_gone_…` |
| battery tab quit | queued `Event::Entry` | session | `a_rows_menu_quits_…` |
| any event | settle `follow_lists` | every scroll | `a_list_changed_by_any_event_…` |
| battery warning | `Sys::battery_warning` returns it, `State::notify` takes it | notifications | status battery tests |
| critical notification | `State::notify`: `idle.own_input`, `Bottom::light` | idle, bottom | notification tests |
| bottom screen lit | settle step 15 reads `bottom_lit()` after idle and power | notifications | `a_notification_waits_for_the_bottom_screen_…` |

Effects answered by events: Launch → Units / LaunchFailed; Adopt → Units; EndRun / EndProcesses → Units,
Windows, Tray; Close → Windows; Tray Activate → Windows; FindIcon → Icon; CalibrateSticks →
Sticks; CalibrateRefresh → RefreshCalibrated; CalibrateGyro → GyroCalibrated; Account → AccountChanged and
Account; AskAwake → KeptAwake; Sleep → Busy or Resumed; Screenshot → Screenshot; PadRoute(Listen) →
Listened; Watch → readings. Freeze, RunnerCommand, Enqueue, saves, NotificationClosed and ActionInvoked
are not answered.

## Invariants (for the randomized event-sequence test)

After every event (checked by `random.rs`, `random_event_sequences_keep_the_invariants`):

1. **Sync converges:** running sync again emits nothing.
2. **What was sent mirrors the state:** the last PadRoute is `pad().route()`; the last Keymap is the
   keymap in force; Status is watched iff the screens are on; Account iff the UI is up and the Device
   view is open; the last Apply is `(program, in_force)`; the last Outputs is (screens on, bottom on);
   the last Refresh is the calibration's rate while it runs, else the rate of the entry launched last
   while its window is yet to show, else the rate of the settings in force, at the modes known.
   Once ClamShell set a brightness level, the last Brightness is that level, or a third of it (at
   least 1) while dimmed.
   At most one tab's watch is on, and it is the open tab's (or the battery's, for the dock's menu).
3. **Pad route:** consistent with claimant, listening and dock move; reconciling again changes nothing;
   the focus ring shows only when the route is Ui, or after a hold on shelf's selected entry opened its
   actions with the pad elsewhere, until the next touch hides it (the ring is what shows the actions are
   open, S24 in the history notes; the focus may move meanwhile, as when the Device view opens).
4. **Session:** live window ids and running ids unique; at most one selected top window and one window placed
   below; remembered selections belong to their top groups; keyboard focus is absent or names
   a live window; at most one launch waiting per entry; no launching instance listed in
   units; Ours ⇒ a unit of it is listed; Found ⇒ not; program in force is running and is the selected top
   window's owner when it has one; quitting ⇒ running and not paused; paused ⇒ Ours; suspended ⇒ paused;
   at most one selection per group; every window has a group once the event is settled, except one of
   a program still starting (its app id launching).
5. **Dock:** ids unique; every running entry is on it; every item is pinned, running or being moved.
6. **Shelf:** the cursor within the list; `selected` is the entry at the cursor; the collection index in
   range; every entry listed matches the filter.
7. **Lists:** every scroll offset within its ends.
8. **Bottom:** dimmed ⇒ no windows shown; the chosen tab differs from the tab only on Keyboard; the editor
   shown is the shown tab's; the Touchpad tab ⇒ pointing.
9. **Keymaps:** the open screen's keymap exists; its layer exists; every keymap id in the settings exists.
10. **Settings:** no empty override; no global-only setting in overrides.
11. **Effect stream across events:** Freeze alternates per slice, starting frozen, while a unit of it is
    listed (a slice whose units are all gone is gone, and starts thawed again), except after the freezer
    is started again (`Event::Restarted`), when every slice is sent as it should be; each slice of ours is
    frozen as last sent exactly while its entry is paused; no second EndRun (not
    forced) for a key until it is quit again or launched; at most one RecordPlay per entry per event, and
    only for an entry that ran before it; no Launch for an entry already launching (one given up in the
    same event may be launched again, as a Steam game whose window is there is).
12. **Notifications:** ids unique; none past its time; a time runs only while the bottom screen is lit;
    every NotificationClosed names one held before the event and gone after it, once; an ActionInvoked
    names one held with an action and is followed at once by its NotificationClosed.

The generator draws events valid for the current state: taps, holds and presses on the targets shown
(shelf's only while shelf is in front, the Device view's while it is open, the tab's), Focus only while
the route is Ui, shelf's pad events only while the pad is on shelf, a touch's HideFocus before its press,
Listened only while listening, notifications sent (any urgency, timeout and action), closed by their
program, tapped and dismissed (mostly ones held), and results only after their requests (a unit or
LaunchFailed after Launch, a unit ending after EndRun or by itself, windows closing after Close or by
themselves, the calibrations', account's and screenshot's answers, Busy or Resumed after Sleep). A small model of the
compositor and systemd follows the effects (focus, moves, windows down). The library holds a Steam game
(ff6), an app handed over to another program (desktop:firefox), an enqueue runner's album, a browsed file,
a group (the series), the Device tool, and any program may open several windows; stray windows, a
landed Steam game's or app's window, and a stray program's tray icon come and go. Clocks advance mostly
by 0-3 s, sometimes by up to 400 s, and now and then by 15-25 minutes to sleep.

It runs 8 fixed seeds × 4000 events with the other tests (about 1.5 s in the debug build). More seeds:
`RANDOM_SEEDS=2000 cargo test -p clamshell-state --release random` runs seeds 1 to 2000,
`RANDOM_SEED=x` one seed, and `RANDOM_EVENTS=n` sets the events per seed. A failure prints the seed, the
event's number, its effects and the 20 events before it. 3000 seeds × 5000 events passed after the fixes
of S50 and S51 (history notes).
