# thor (working name)

Linux on the AYN Thor: ClamShell, the program that runs the device's session, plus the setup around it
(Arch Linux ARM, packages built on the device). Planning lives in the GPD repo's `docs/plans.md` for now
(`~/Code/gpdwin`); it moves here once the first parts run.

## ClamShell

`clamshell/` is ClamShell: one process with one in-memory state for shelf (the game list on the top screen),
the second screen (bar, modes, the selected or running game's settings), and later the input path (virtual
pad, mouse emulation, touchpad) and power policy. Rust, a Cargo workspace:

| Crate | What |
|---|---|
| `crates/core` | the state and its rules: events in, dirty screens and effects out; no I/O, unit-tested |
| `crates/policy` | threads that watch the system and send events, and carry out effects (sway, systemd, NetworkManager, evdev/uinput, sysfs) |
| `crates/library` | the game library (today: demo games) |
| `crates/ui` | Wayland (smithay-client-toolkit), CPU drawing (tiny-skia, fontdue), the views, pad navigation; the font built in (Noto Sans Regular, SIL Open Font License 1.1: `crates/ui/fonts/OFL.txt`), with fontconfig for characters it lacks |
| `src/main.rs` | wires them together |

No panics by construction: `unwrap`, `expect`, `panic!` and slice indexing are denied outside tests.

**Test the launcher under sway**: `clamshell/tests/nested-sway.sh` (after `cargo build --release`) runs a
headless sway with the program inside, a test library and test apps, drives it through its control pipe
(`CLAMSHELL_CONTROL`: lines like `tap app 1`, `long game`, `tap plus 1`, `tap launch`; also `mode wifi`,
`tap network 1`, `type text`, `tap save`, `slide volume 280 330 [hold]`, `home`), checks sway's tree
and systemd's units, and cleans up after itself; about a minute, nothing shows on the desktop.
`CLAMSHELL_STEAM=off` leaves Steam's games out.

**A virtual Thor** (`tools/vm.py`, Python, standard library only): a QEMU/KVM machine with the device's
session: two screens at the Thor's sizes (sway on real DRM outputs, the second forced on through the
kernel's `video=` option), the host mouse as an absolute pointer per screen window, a multitouch screen on
the bottom one, a fake battery (`test_power`), Wi-Fi through NetworkManager with two test networks on
simulated radios (`mac80211_hwsim`, hostapd, dnsmasq: ThorTest, WPA2, password `thortest123`, and ThorOpen),
uinput for our virtual devices, and ClamShell as a user service started by sway with a control pipe
(`$XDG_RUNTIME_DIR/clamshell-control`), run from this repository (shared read-only: build on the host,
`tools/vm.py restart`). `tools/vm.py image` (downloads Arch's cloud image), `run [--headless] [--pad
VENDOR:PRODUCT]` (the first start provisions it with cloud-init, a few minutes), `ssh [command]`, `sync`
(session files), `restart`, `wake` (from `systemctl suspend`), `reset`. No Thor GPU, cpufreq/devfreq or fan
in it, and no audio device. Suspend and resume work once at most: after a resume the machine soon stops.

**Run on a desktop** (any Wayland compositor with xdg-shell):

    cd clamshell
    cargo run                         # two windows at half the Thor's size: top and bottom
    cargo run -- --snapshot /tmp      # top.png and bottom.png at the Thor's size, no Wayland
    cargo test --workspace
    cargo clippy --workspace --all-targets

Keys stand in for the pad: arrows move focus (across both screens, as if the bottom one were below the
top one), Enter or Space is A, Escape quits. Clicks and touches act on release; held 450 ms or more
they are a long press. The keyboard mode is a PC layout (US) with an Fn layer shown as small corner labels
(F1-F12, Del, arrows on i/j/k/l, Home/End, PgUp/PgDn, Ins): Shift, Ctrl, Alt and Fn apply to the next
key on a tap and lock on a long press. On the Thor (sway), the bottom view
is a layer-shell surface on output `DSI-1` (`CLAMSHELL_BOTTOM_OUTPUT` to change it).

**Touchpad mode** grabs the second screen's touchscreen (`CLAMSHELL_TOUCHSCREEN`: a /dev/input path or
part of its name; default: the first direct-touch device) and forwards fingers to a virtual touchpad
that sway's libinput handles like a laptop's; fingers that start on the bar stay taps on it. To try it
on a desktop with a fake touchscreen (your pointer will move):

    cargo run --release -p clamshell-policy --example fake_touchscreen &
    CLAMSHELL_TOUCHSCREEN="clamshell fake touchscreen" CLAMSHELL_START_MODE=touchpad cargo run --release

**The keyboard mode types through a virtual keyboard** (`clamshell keyboard`, uinput) in US key
positions. The compositor must use the US layout for it; in sway:
`input "0:0:clamshell_keyboard" xkb_layout us` (check the identifier with `swaymsg -t get_inputs`). On a
desktop with another layout, letters, digits and named keys come out right but punctuation follows that
layout. To type into other apps on a desktop, run with `CLAMSHELL_BOTTOM_PANEL=1`: the bottom view is then a
panel in the bottom right corner that never takes the keyboard focus (as on the device), not a window.

**The bar**: the modes on the left; volume and brightness in the middle; the pad mouse, Wi-Fi, the battery
and the clock on the right. A finger on volume or brightness turns the bar's row into a slider whose
current level is under the finger: moving sets the level as it goes, lifting leaves it there (the icons
sit mid-screen so the track fits on both sides; it is about 31 mm long on the Thor). A tap on volume
mutes or unmutes; sliding it unmutes. Volume goes through `wpctl` (PipeWire's default sink), brightness to
every backlight through logind (`CLAMSHELL_BACKLIGHTS`: another folder laid out like
`/sys/class/backlight`, for a desktop without one). Every icon always shows, dim without a reading (a
slider then starts in the middle), so a desktop shows everything the device does.

**Wi-Fi** (tap the Wi-Fi icon; lit while open, a second tap goes back): through NetworkManager on the
system bus. The networks in range (rescanned every 10 s while open), the connected and saved ones first,
with a lock on those that need a password and the radio's switch on top. A tap joins: a saved network
straight away, a new one with a password typed on the on-screen keyboard (WPA2/WPA3 personal, WEP); a long
press forgets a saved network. A new connection that fails to join is deleted again, and the reason shows
("wrong password?", no address). Enterprise networks (a login) aren't supported.

**Battery** (tap the battery icon; lit while open, a second tap goes back): the level, charging or
discharging with the drain and the time left, and the charge limit as a button (full, 90, 80 %, the same
setting as its tile); the charge over the last day from UPower (charging in green); then what is using the
machine now, the busiest first, as many as fit: CPU time per process and GPU time per DRM client
(`fdinfo`), grouped by our scopes (an app instance, the game), else by program for the user's processes,
else as System. The battery only reports its total drain, so these are shares of the work, not watts.

**Launcher** (the grid icon): the running game first (a "No game" placeholder otherwise, so tiles never
move), then the apps. Each start of an app is an instance: a systemd scope (`clamshell-app-<key>-<secs>-<n>`)
and a sway workspace of the same name, on the main screen. A tap starts an app, shows its most recent
instance when it runs, or its next instance when one of its own is in front; "1/3" shows which. A long
press closes the instance it is on (its windows, politely, as their close buttons would); on the game tile
it quits the game (its windows closed, its scope killed if it hasn't gone 5 s later). A green "+" on a
running app marked `multiple` starts another instance. Running apps show their icon in full colour (drawn
icons green), idle ones grey.

How it stays right: systemd says what runs (the scopes, followed over D-Bus: an instance lives as long as
any of its processes, and survives a restart of the program). sway places windows: launches go through its
`exec` aimed at the instance's workspace (by a job file and `clamshell --run`, so no argument passes through
sway's command parser), which places the launch's first window there before it is shown; a new instance or
game is shown when that window appears (its empty workspace never is; a newer start takes over). sway only
places a launch's first window, so later ones (a second window, a program started from its terminal) are
moved to their instance's workspace by their process's scope, the kernel's record of where they come from.
sway rules set at start: shelf's window to `clamshell-shelf`, Steam's games (`steam_app_…`) to
`clamshell-game`, both kept on the main screen, and windows opening on the second screen (behind the
launcher) to the main screen. An emptied workspace of ours in front shows shelf.

Apps come from `~/.config/clamshell/launcher.json` (read at start); without it Shelf, Steam (Big Picture) and
a terminal (foot); `clamshell --launcher-example` prints a file to start from. An app is a `desktop` file
(giving name, command and icon) and/or a `name` with a `command` (program and arguments, no shell) or a
`builtin` (`shelf`); `icon` picks a drawn icon (`shelf`, `gamepad`, `terminal`, `browser`, `files`,
`music`, `video`, `settings`) over the desktop file's, and with neither the initial shows. Without sway
there is no window list: taps only start apps. A broken file shows its error under the tiles.

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
frame time from MangoHud: games started with `clamshell --mangohud-env` in their environment log (overlay
invisible) to a folder in RAM that ClamShell follows. For a Steam game, as launch options:
`env $(clamshell --mangohud-env) %command%`; by hand: `env $(clamshell --mangohud-env) vkcube`.

**The controller** (any Xbox-style pad; `CLAMSHELL_GAMEPAD`: path or part of its name) is grabbed for good;
games see a virtual Xbox 360 pad that is always there. The **pad mouse** toggle (mouse icon in the bar)
routes the controller to a virtual mouse instead (left stick pointer, right stick scroll, A/RT left
click, B right, X middle, D-pad arrows) while the virtual pad shows a neutral controller. The **home
button** (BTN_MODE) is ClamShell's, never the games': from shelf it goes to the running game, from anything
else to shelf. (So Steam's overlay doesn't get it.) Test with a fake
pad: `cargo run --release -p clamshell-policy --example fake_pad &` then
`CLAMSHELL_GAMEPAD="clamshell fake pad" CLAMSHELL_PAD_MOUSE=1 cargo run --release`.

**Sleep**: the power button and the lid suspend through logind (the image's `logind.conf.d/thor.conf`).
ClamShell notices waking from the clocks (CLOCK_BOOTTIME runs during sleep, CLOCK_MONOTONIC doesn't) and
applies the settings in force again.

Licence: MIT.
