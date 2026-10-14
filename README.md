# thor (working name)

Linux on the AYN Thor: a system program for its two screens, its pad and its power, plus the setup around
it (Arch Linux ARM, packages built on the device). Planning lives in the GPD repo's `docs/plans.md` for now
(`~/Code/gpdwin`); it moves here once the first parts run.

## sysprog (working name)

`sysprog/` is the system program: one process with one in-memory state for shelf (the game list on the top
screen), the second screen (bar, modes, the selected or running game's settings), and later the input
path (virtual pad, mouse emulation, touchpad) and power policy. Rust, a Cargo workspace:

| Crate | What |
|---|---|
| `crates/core` | the state and its rules: events in, dirty screens and effects out; no I/O, unit-tested |
| `crates/policy` | threads that watch the system and send events (today: the clock) |
| `crates/library` | the game library (today: demo games) |
| `crates/ui` | Wayland (smithay-client-toolkit), CPU drawing (tiny-skia, fontdue), the views, pad navigation; the font built in (Noto Sans Regular, SIL Open Font License 1.1: `crates/ui/fonts/OFL.txt`), with fontconfig for characters it lacks |
| `src/main.rs` | wires them together |

No panics by construction: `unwrap`, `expect`, `panic!` and slice indexing are denied outside tests.

**Run on a desktop** (any Wayland compositor with xdg-shell):

    cd sysprog
    cargo run                         # two windows at half the Thor's size: top and bottom
    cargo run -- --snapshot /tmp      # top.png and bottom.png at the Thor's size, no Wayland
    cargo test --workspace
    cargo clippy --workspace --all-targets

Keys stand in for the pad: arrows move focus (across both screens, as if the bottom one were below the
top one), Enter or Space is A, Escape quits. Clicks and touches act on release; held 450 ms or more
they are a long press. The keyboard mode is a PC layout (US) with an Fn layer shown as small corner labels
(F1-F12, Del, arrows on i/j/k/l, Home/End, PgUp/PgDn, Ins): Shift, Ctrl, Alt and Fn apply to the next
key on a tap and lock on a long press. On the Thor (sway), the bottom view
is a layer-shell surface on output `DSI-1` (`SYSPROG_BOTTOM_OUTPUT` to change it).

**Touchpad mode** grabs the second screen's touchscreen (`SYSPROG_TOUCHSCREEN`: a /dev/input path or
part of its name; default: the first direct-touch device) and forwards fingers to a virtual touchpad
that sway's libinput handles like a laptop's; fingers that start on the bar stay taps on it. To try it
on a desktop with a fake touchscreen (your pointer will move):

    cargo run --release -p sysprog-policy --example fake_touchscreen &
    SYSPROG_TOUCHSCREEN="sysprog fake touchscreen" SYSPROG_START_MODE=touchpad cargo run --release

**The keyboard mode types through a virtual keyboard** (`sysprog keyboard`, uinput) in US key
positions. The compositor must use the US layout for it; in sway:
`input "0:0:sysprog_keyboard" xkb_layout us` (check the identifier with `swaymsg -t get_inputs`). On a
desktop with another layout, letters, digits and named keys come out right but punctuation follows that
layout. To type into other apps on a desktop, run with `SYSPROG_BOTTOM_PANEL=1`: the bottom view is then a
panel in the bottom right corner that never takes the keyboard focus (as on the device), not a window.

**Launcher** (the grid icon): the running game first (a "No game" placeholder otherwise, so tiles never
move), then the apps. A tap starts an app, or goes to its most recent window when it runs, or to its next
window when it already is in front; "1/3" shows which of its windows you are on. A long press closes the
window the tile is on, politely (as its close button would); on the game tile it quits the game (its
windows closed, its scope killed if it hasn't gone 5 s later). A "+" in the corner of a running app marked
`multiple` starts another instance. Running apps show their icon in full colour (drawn icons green), idle ones
grey. Apps come from `~/.config/sysprog/launcher.json` (read at start); without it Shelf, Steam (Big
Picture) and a terminal (foot); `sysprog --launcher-example` prints a file to start from. An app is a
`desktop` file (giving name, command, icon and window class) and/or a `name` with a `command` (program and
arguments, no shell) or a `builtin` (`shelf`: brings shelf's window to the front); `icon` picks a drawn
icon (`shelf`, `gamepad`, `terminal`, `browser`, `files`, `music`, `video`, `settings`) over the desktop
file's, and with neither the initial shows; `app_id` names its windows if nothing else does. Apps start
from the home directory in their own systemd scope (`sysprog-app-<key>-…`, so their windows are known),
and outlive a restart of the system program. Windows come from sway's IPC (`SWAYSOCK`); other compositors
have no window list, so there taps only start apps. A broken file shows its error under the tiles.

**Settings** (the sliders icon, first in the bar): a 4×4 grid of tiles, each an icon over its value. A
tap steps to the next value, a long press goes back to the default. Tiles sit on black; an icon's colour says
what its value does: grey stock, green saves power, amber costs power, blue changes how the game looks or
plays. Two tabs: *All games*, the defaults,
and the game in context (the running one, else shelf's selection), whose own choices override the
defaults one by one (a dot on the tile; values taken from the defaults are dimmed); its last tile is
Reset, where *All games* has the charge limit instead.

| Tile | Does |
|---|---|
| Power | Battery / Balanced / Performance: CPU and GPU frequency caps (the Thor has no TDP setting) |
| FPS cap, HUD | MangoHud's limiter and overlay (off, FPS only, full), through the game's own MangoHud config file, reloaded live |
| Screen | the second screen dark while the game runs, until tapped |
| Cores | the little cluster only (the others offline) |
| Mouse | the pad mouse turned on at launch, off at the end |
| Buttons, Deadzone | Nintendo layout (A/B, X/Y swapped) and a stick deadzone, in the virtual pad |
| CPU, GPU | a frequency cap in percent, instead of the profile's |
| Env | extra environment variables (`KEY=value`, typed on the on-screen keyboard); a game's come after the defaults' |
| Charge | charging stops at 90 or 80 % (*All games* only) |
| Refresh, Upscale, Fan, Rumble | saved, marked "later": not applied yet |

Saved in `game-settings.txt` next to `stats.db`, applied at a game's launch and at once while it runs.
Frequency caps, cores and the charge limit need write access to cpufreq's `scaling_max_freq`, CPUs'
`online`, devfreq's `max_freq` and the battery's `charge_control_end_threshold` (a udev rule on the
device; on the desktop they're skipped, with one warning). The bottom screen keeps the last mode chosen
(sysinfo at first), also across restarts.

**Sysinfo mode** shows CPU, GPU, memory, temperatures, power, per-core load, and a running game's FPS and
frame time from MangoHud: games started with `sysprog --mangohud-env` in their environment log (overlay
invisible) to a folder in RAM that the system program follows. For a Steam game, as launch options:
`env $(sysprog --mangohud-env) %command%`; by hand: `env $(sysprog --mangohud-env) vkcube`.

**The controller** (any Xbox-style pad; `SYSPROG_GAMEPAD`: path or part of its name) is grabbed for good;
games see a virtual Xbox 360 pad that is always there. The **pad mouse** toggle (mouse icon in the bar)
routes the controller to a virtual mouse instead (left stick pointer, right stick scroll, A/RT left
click, B right, X middle, D-pad arrows) while the virtual pad shows a neutral controller. Test with a fake
pad: `cargo run --release -p sysprog-policy --example fake_pad &` then
`SYSPROG_GAMEPAD="sysprog fake pad" SYSPROG_PAD_MOUSE=1 cargo run --release`.

Licence: MIT.
