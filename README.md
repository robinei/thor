# thor (working name)

Linux on the AYN Thor: ClamShell, the program that runs the device's session, plus the setup around it
(Arch Linux ARM, packages built on the device). What to do when the device arrives, and what is left to
test on it: `docs/thor-gameplan.md`.

## ClamShell

`clamshell/` is ClamShell: one process with one in-memory state for shelf (the game list on the top
screen), the second screen (the bar and its tabs), the input path (the controller, the pad mouse, the
touchpad, the on-screen keyboard), and the games' power settings. Rust, a Cargo workspace:

| Crate | What |
|---|---|
| `crates/core` | the state and its rules: events in, dirty screens and effects out; no I/O, unit-tested |
| `crates/policy` | threads that watch the system and send events, and carry out effects (sway, systemd, NetworkManager, BlueZ, evdev/uinput, sysfs); the game library from disk (Pegasus' game dirs, Steam, play time, favourites) |
| `crates/library` | the game library's model (games, collections, their order, the demo games) and the parts of Pegasus' and Steam's formats that need no disk; no dependencies |
| `crates/ui` | Wayland (smithay-client-toolkit), CPU drawing (tiny-skia, fontdue), the views, pad navigation; the font built in (Noto Sans Regular, SIL Open Font License 1.1: `crates/ui/fonts/OFL.txt`), with fontconfig for characters it lacks |
| `src/` | the program around them: the command line, the environment, starting the threads, the test pipe, the snapshots |

No panics by construction: `unwrap`, `expect`, `panic!` and slice indexing are denied outside tests.

**Test the dock under sway**: `clamshell/tests/nested-sway.sh` (after `cargo build --release`) runs a
headless sway with the program inside, a test library and test apps, drives it through its control pipe
(`CLAMSHELL_CONTROL`: lines like `tap app 1`, `long game`, `tap plus 1`, `tap launch`; also `tab wifi`,
`tap network 1`, `type text`, `tap save`, `slide volume 280 330 [hold]`, `home`), checks sway's tree and
systemd's units, and cleans up after itself; about a minute, nothing shows on the desktop.
`CLAMSHELL_STEAM=off` leaves Steam's games out.

**Snapshots in the tests**: `cargo test` also draws the `--snapshot` files and compares their hashes
with `clamshell/tests/snapshots.txt`. After a change meant to change what is drawn (look at the files
first), write the new hashes with `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`.

**A virtual Thor** (`tools/vm.py`, Python, standard library only): a QEMU/KVM machine with the device's
session: two screens at the Thor's sizes (sway on real DRM outputs, the second forced on through the
kernel's `video=` option), the host mouse as an absolute pointer per screen window, a multitouch screen
on the bottom one, a fake battery (`test_power`), Wi-Fi through NetworkManager with two test networks on
simulated radios (`mac80211_hwsim`, hostapd, dnsmasq: ThorTest, WPA2, password `thortest123`, and
ThorOpen), uinput for our virtual devices, and ClamShell as a user service started by sway with a control
pipe (`$XDG_RUNTIME_DIR/clamshell-control`), run from this repository (shared read-only: build on the
host, `tools/vm.py restart`). `tools/vm.py image` (downloads Arch's cloud image), `run [--headless]
[--pad VENDOR:PRODUCT]` (the first start provisions it with cloud-init, a few minutes), `ssh [command]`,
`sync` (session files), `restart`, `wake` (from `systemctl suspend`), `reset`. No Thor GPU,
cpufreq/devfreq or fan in it, and no audio device. Suspend and resume work once at most: after a resume
the machine soon stops.

**Input and devices from the host** (the x86_64 machine): `tools/vm.py touch tap X Y`, `hold X Y
SECONDS`, `down X Y`, `move X Y`, `up`, `slide X1 Y X2 [STEPS] [--seconds S] [--hold]` drive the bottom
screen's touchscreen through QEMU's QMP socket (`input-send-event`, multitouch), in that screen's pixels
(1240x1080, the bar's icons at y 45), each with `--slot N` for another finger; `pointer X Y [--head 0|1]
[--click left]` and `key a shift+b ret` (QEMU's key names) do the pointers and the keyboard. `tools/vm.py
pad plug` runs the `fake_pad` example (the host's build: `cargo build --release -p clamshell-policy
--examples`) in the machine as a user service reading commands from a pipe, which ClamShell grabs as the
controller; `pad do 'tap a, dpad down, sleep 100, stick left 1 0, press a, release a'` drives it (buttons
a b x y lb rb view menu home ls rs back, `trigger left|right V`, `gyro X Y Z` in degrees per second: the fake
pad has a motion sensor beside it), `pad unplug` stops it. `tools/vm.py
bluetooth` (again after each boot) builds BlueZ's `btvirt` emulator in the machine once (Arch doesn't
ship it) and starts two virtual controllers: bluetoothd keeps the first (`MaxControllers=1`), the second
is a test device, ThorTestDevice (`tools/vm-bt-device.py`: discoverable, accepts "just works" pairing,
answers with an SDP record and the HID channels), which can be found, paired, connected and forgotten.

**Checks in the machine**: `tools/vm-checks.py [CHECK ...]` (`--list`), with the machine running, does
what a user would with these inputs and checks what came of it in ClamShell's journal and saved files,
sway's workspaces, the input devices (evtest, libinput), NetworkManager, BlueZ, PipeWire and screenshots:
taps, holds and slides on the bar (volume through a PipeWire null sink, brightness through a fake
backlight), a slide while a second finger taps a tab, forgetting a Wi-Fi network by a long press,
scrolling shelf's list (the test library has more games than fit) and the Wi-Fi list (the simulated
radios loaded again with eight more test networks for it) and tapping a row after, the
touchpad tab, the pad on shelf (D-pad, A, Home), View, the pad mouse, typing with the pad, a held A on an
on-screen key when View moves the pad away, unplugging the controller during a game, Home then Back as the game's Guide, a keymap made
through its screens and the pad (A to Ctrl+X, the right stick and the gyro while LB is held to the
pointer) read back from the virtual devices in a game, the gyro's speed slid up on the edit screen while
the game runs, then Default passing everything through, a keymap with a Back layer (A to Ctrl+X, Ctrl+Y
while Back is held, Back let go while A is held), a keymap with a hold, a double and a toggle (A tapped and
held, B once and twice, Y on and off), and Bluetooth. For
them ClamShell runs with a test library, the fake backlight and its own state directory (a runtime
drop-in for its service, removed afterwards; `--keep` leaves it). It prints ok/FAIL lines and a count;
the exit status is the number of failures. About three minutes.

**Run on a desktop** (any Wayland compositor with xdg-shell):

    cd clamshell
    cargo run                         # two windows at half the Thor's size: top and bottom
    cargo run -- --snapshot /tmp      # top.png and bottom.png at the Thor's size, no Wayland
    cargo test --workspace
    cargo clippy --workspace --all-targets

On a desktop, keys stand in for the pad (not on the device, where keys reaching shelf are typing): arrows
move focus (across both screens, as if the bottom one were below the top one), Enter or Space is A, Tab
View, Backspace B, Q and E the shoulders, F Y. Clicks and touches act on release; held 450 ms or more
they are a long press. The Keyboard tab is a PC layout (US) with an Fn layer shown as small corner labels
(F1-F12, arrows on w/a/s/d, and PgUp Home Ins / PgDn End Del on i o p / k l Backspace: a PC's six-key
block with its outer columns swapped): Shift, Ctrl, Alt, Super (Fn+Alt, which leaves room for a wider
space bar) and Fn apply to the next key on a tap, or to every key typed while one is held (two thumbs, as
on a real keyboard), and the keys show their effect meanwhile. Caps Lock is its own key, next to Tab,
with a light. On the Thor (sway), the bottom view is a layer-shell surface on output `DSI-1`
(`CLAMSHELL_BOTTOM_OUTPUT` to change it).

**The Touchpad tab** grabs the second screen's touchscreen (`CLAMSHELL_TOUCHSCREEN`: a /dev/input path or
part of its name; default: the first direct-touch device) and forwards fingers to a virtual touchpad that
sway's libinput handles like a laptop's; fingers that start on the bar stay taps on it. Without a
touchscreen (a desktop) the tab says so. To try it on a desktop with a fake touchscreen (your pointer
will move):

    cargo run --release -p clamshell-policy --example fake_touchscreen &
    CLAMSHELL_TOUCHSCREEN="clamshell fake touchscreen" CLAMSHELL_START_MODE=touchpad cargo run --release

**The Keyboard tab types through a virtual keyboard** (`clamshell keyboard`, uinput) in US key positions.
The compositor must use the US layout for it; in sway: `input "0:0:clamshell_keyboard" xkb_layout us`
(check the identifier with `swaymsg -t get_inputs`). On a desktop with another layout, letters, digits
and named keys come out right but punctuation follows that layout. To type into other apps on a desktop,
run with `CLAMSHELL_BOTTOM_PANEL=1`: the bottom view is then a panel in the bottom right corner that
never takes the keyboard focus (as on the device), not a window.

**A text field brings up the keyboard**: ClamShell is the compositor's input method (input-method-v2;
sway relays the text-input-v3 that GTK, Qt, Firefox and foot use), so when an app's text field wants
typing the keyboard tab opens, and when the field is done (after 0.4 s, so moving between fields doesn't
bounce it) the tab before it comes back, unless a tab was chosen meanwhile. Not while a game is in front:
SDL asks for text input whether or not the game needs it. With another input method running (fcitx, ibus)
the compositor gives the seat to that one and this does nothing.

**The bar**: the second screen's tabs, spread evenly: Settings, Dock, Keyboard, Touchpad, Sysinfo, then
Volume and Brightness near the middle, then Bluetooth, Wi-Fi and Battery, whose icons show their level or
status; past a faint line, the clock and the pad mouse switch. The tab last chosen comes back after a
restart (not Touchpad, which grabs the touchscreen). A tap on volume or brightness opens its tab; a
finger held there for 0.2 s, or moved sideways off it, doesn't (also with the Touchpad tab open, where
the touchpad thread decides the same way): it turns the bar's row into a slider whose current level is
under the finger: moving sets the level as it goes, lifting leaves it there (the icons sit near the
middle; the track's length is what fits on both sides for the level under the finger, at most half the
screen, about 33 mm on the Thor). Sliding the volume unmutes. Volume goes through `wpctl` (PipeWire's
default sink), brightness to every backlight through logind (`CLAMSHELL_BACKLIGHTS`: another folder laid
out like `/sys/class/backlight`, for a desktop without one). Every icon always shows, dim without a
reading (a slider then starts in the middle), so a desktop shows everything the device does.

**Lists** (shelf's games, Wi-Fi's networks, Bluetooth's devices, the apps playing, the backlights, the
keymaps) scroll: a finger moved up or down on one drags it (past 12 pixels: the row it came down on is
let go without a tap), and let go while moving it flings on, slowing down, to a stop or an end; shelf then
settles on a whole row. A finger held still is a tap or a long press as anywhere. With the pad, up and down
go row by row and the list scrolls just enough to show the focused one. A thin dim bar at the right edge
shows where in the list the view is, only while it has more rows than fit. The place is the state's, in
rows; a list that changes under the view keeps the row at its top where it is (a network vanishing above
doesn't move the ones under the finger), unless the view is at the very top, where new rows show.

**Volume** (tab): the output volume with its mute button; the outputs, where a tap sends the sound
(`wpctl set-default`); then each app playing (PipeWire's output streams, read with `pw-dump` every second
while open) with its own level and mute button. **Brightness** (tab): the level (the bar's), then each
backlight's share of it: a screen is at the level times its share, so the bar dims and brightens the
screens together and keeps them as they are to each other. Both are ClamShell's (the kernel only knows
each panel's result), saved in `~/.local/state/clamshell/brightness` and applied at start. A level row is
set where the finger is on it and follows the finger; with the pad, left and right step it by 5. A list
that changes under a finger (a scan, a device connecting, an app that stops playing) doesn't redirect it:
a press acts on the item it came down on, or on nothing once that is gone.

**Wi-Fi** (tap the Wi-Fi icon; lit while open): through NetworkManager on the system bus. The networks in
range (rescanned every 10 s while open), the connected and saved ones first, with a lock on those that
need a password and the radio's switch on top. A tap joins: a saved network straight away, a new one with
a password typed on the on-screen keyboard (WPA2/WPA3 personal, WEP); a long press forgets a saved
network. A new connection that fails to join is deleted again, and the reason shows ("wrong password?",
no address), judged from the connection NetworkManager made for the join (not the device, which also goes
through leaving the network it was on). Enterprise networks (a login) aren't supported.

**Bluetooth** (tap the Bluetooth icon; lit while open): through BlueZ on the system bus. The devices, the
connected and paired ones first, each with its kind's icon (headphones, controller, keyboard, mouse,
phone), and the radio's switch on top; while open it looks for new devices. A tap connects a paired
device, pairs a new one (pair, trust, connect: headphones and controllers need no code; a pairing agent
is registered only for that) or disconnects a connected one; a long press forgets a paired one.
Connecting and pairing run on a worker of their own, so the list and the radio's switch don't wait for
them. PipeWire moves the sound to headphones on its own.

**Battery** (tap the battery icon; lit while open): the level, charging or discharging with the drain and
the time left, beside three settings tiles (as in the settings grid; a tap steps one): the charge limit
(full, 90, 80 %), when the screens go off without input (1, 2, 5, 10 min, always on) and when the device
sleeps (5, 15, 30, 60 min, never); the charge over the last day from UPower (charging in green); then what is using the machine now, the busiest
first, as many as fit: CPU time per process and GPU time per DRM client (`fdinfo`, the render engine's, as the sysinfo tab
counts it), grouped by our scopes
(an app instance, the game), else by program for the user's processes, else as System. The battery only
reports its total drain, so these are shares of the work, not watts.

**The model.** Shelf is home: where games are chosen, and where Home goes. The second screen controls
what is in front. The dock holds the apps at hand and shows what runs. Games run side by side, each on a
workspace of its own (one shelf started is named like its scope, `clamshell-game-<secs>-<n>`; one Steam's
client started, a `steam_app_<id>` window of a library game, gets `clamshell-game-steam-<id>` and its
windows are moved there). *The game* is the running game last in front: its settings are in force. A game
Steam starts becomes the game, and any other keeps running; its play ends, and is recorded, when its last
window closes. In shelf, activating a running game shows it; with another running, shelf offers "keep it
running" or "close it" (then starts this one once that one has ended). What is in front (shelf, a game,
another app) decides where the pad goes, what Home does, and what the settings page edits: the game's
settings, shelf's selected game's, or with another app in front, all games'.

**Dock** (the grid icon): the running games first (the game marked with a dot when several run; a tap
shows one, a long press quits it), or while none runs a "Start game" tile that goes to shelf, so the apps
keep their places unless several games run; then the apps. Each start of an app is an instance: a systemd
scope (`clamshell-app-<key>-<secs>-<n>`) and a sway workspace of the same name, on the main screen. A tap
starts an app, shows its most recent instance when it runs, or its next instance when one of its own is
in front; "1/3" shows which. A long press closes the instance it is on (its windows, politely, as their
close buttons would); on the game tile it quits the game (its windows closed, its scope killed if it
hasn't gone 5 s later). A green "+" on a running app marked `multiple` starts another instance. Running
apps show their icon in full colour (drawn icons green), idle ones grey.

How it stays right: systemd says what runs (the scopes, followed over D-Bus: an instance lives as long as
any of its processes, and survives a restart of the program). sway places windows: launches go through
its `exec` aimed at the instance's workspace (by a job file and `clamshell --run`, so no argument passes
through sway's command parser), which places the launch's first window there before it is shown; a new
instance or game is shown when that window appears (its empty workspace never is; a newer start takes
over). sway only places a launch's first window, so later ones (a second window, a program started from
its terminal) are moved to their instance's workspace by their process's scope, the kernel's record of
where they come from. sway rules set at start: shelf's window to `clamshell-shelf`, Steam's games
(`steam_app_…`) to `clamshell-game`, both kept on the main screen, and windows opening on the second
screen (behind the dock) to the main screen. An emptied workspace of ours in front shows shelf.

Apps come from `~/.config/clamshell/launcher.json` (read at start); without it Steam (its normal client:
store, downloads, friends; Big Picture would be a second home screen) and a terminal (foot); `clamshell
--launcher-example` prints a file to start from. An app is a `desktop` file (giving name, command and
icon) and/or a `name` with a `command` (program and arguments, no shell); `icon` picks a drawn icon
(`shelf`, `gamepad`, `terminal`, `browser`, `files`, `music`, `video`, `settings`) over the desktop
file's, and with neither the initial shows. Without sway there is no window list: taps only start apps. A
broken file shows its error under the tiles.

**Settings** (the sliders icon, first in the bar): a 4×4 grid of tiles, each an icon over its value. A
tap steps to the next value, a long press goes back to the default. Tiles sit on black; an icon's colour
says what its value does: grey stock, green saves power, amber costs power, blue changes how the game
looks or plays. Two tabs: *All games*, the defaults, and the game in context (the running one, else
shelf's selection), whose own choices override the defaults one by one (a dot on the tile; values taken
from the defaults are dimmed); on a game's tab its last tile is Reset. The device's own settings (the charge limit, when the screens go
off and the device sleeps) are on the Battery tab, not here: they aren't a game's.

| Tile | Does |
|---|---|
| Power | Battery / Balanced / Performance: CPU and GPU frequency caps (the Thor has no TDP setting) |
| FPS cap, HUD | MangoHud's limiter and overlay (off, FPS only, full), through the game's own MangoHud config file, reloaded live |
| Screen | the second screen dark while the game runs, until tapped |
| Cores | the little cluster only (the others offline) |
| Mouse | the pad mouse turned on at launch, off at the end |
| Keymap | the keymap the game's controller goes through (its name; Default: as on the pad); a tap opens the keymaps (below) |
| CPU, GPU | a frequency cap in percent, instead of the profile's |
| Env | extra environment variables (`KEY=value`, typed on the on-screen keyboard); a game's come after the defaults' |
| Refresh | the main screen's refresh rate: the nearest its modes have (through the compositor's output management protocol) |
| Upscale, Fan | saved, marked "later": not applied yet |
| Rumble | games' rumble on the controller's motors (where it has them) while the game has the pad: off, low, mid or full |

**Keymaps** remap the controller for games: named, shared between games, saved in `keymaps.txt` next to
`game-settings.txt` (a `keymap ID NAME` line, then a line per binding, `INPUT = OUTPUT [option=value]`, e.g.
`a = key ctrl+x turbo=12`, `a hold = key esc holdtime=60`, `b double = mouse left toggle`,
`rstick = pointer speed=75% deadzone=10`, `gyro = pointer hold=lt`; a `layer back` line
starts the Back layer's bindings; lines not understood are skipped, and a layer not known). Each keymap has
two layers: the main one, and one in force while the Thor's Back button is held (its inputs without a
binding there do what the main layer says; Back itself never reaches the game). The first start makes a
"Nintendo layout" keymap (A and B, X and Y swapped), an ordinary one. The Keymap tile opens the picker
over the settings: Default, then the keymaps with how many games use each; a tap chooses one for the scope
being edited, Edit (or a long press: rename, duplicate, delete; deleting one in use asks first and puts its
games back on Default) opens it, New names one (the editor) and opens it. The edit screen shows one layer
(Main or Back held, switched at the top right, by the Back button drawn on the pad, or by pressing Back on
the pad): the Thor's controls in two clusters at the screen's edges, each input the layer changes with a
label, blue where it maps to something else, grey where it is the input as on the pad, only tuned. An
input is selected by touch or by pressing (or pushing past half) it on the pad, which listens there
instead of moving the focus (Home leaves; Home is never mapped); the panel between the label columns
then shows, for a button or a direction, Press, Hold and Double tabs (each actuation's binding of its own;
a dot under one set), what it does in the layer, the ways to remap it as icons (only those that fit it):
a control picked on the drawing (tapped, or pressed on the pad; the rest fade), keys typed on the
keyboard, the mouse's parts on a drawing of one (for motion, the pointer), as before (the main layer: as
on the pad; the Back layer: as in Main; a hold or double: none), nothing. Each closes as soon as something
is chosen; Cancel gives up; selecting the input again reopens the way it is mapped. Then its parameters: a stick's deadzone, full point, curve and X/Y
inversion (and speed on the pointer), a trigger's deadzone and full point (and threshold on a button), the
gyro's speed, inversion and what holds it on (picked on the drawing too), a hold's time, and on anything
on or off turbo (a slider from Off to 30 a second) and toggle (a switch). Only an input with a hold or a
double waits to decide: held past the hold's time it is a hold, pressed again within 300 ms a double,
else a press, sent as a 60 ms tap once that is known (the Press tab says so); one without acts at once.
Turbo beats while its actuation is on (a hold's from the hold's time, a double's from the second press);
a toggle flips when its actuation starts. A trigger has Pull and Full pull tabs instead: its full pull is
a second stage beside the pull (which stays analog, or a button past its threshold), something on or off
past its own threshold (95 % unless set), e.g. `rt full = mouse left threshold=95`. The defaults are the same for every input and target but a
stick on the pointer, which has a deadzone of 15 % and a curve of 50 % (the distance past it squared, as
the pointer moved before); the panel shows the values that apply, and a new target takes its own defaults
for the values not set. A stick's deadzone is the keymap's only one (a game that needs another uses a
keymap of its own; the old per-game Deadzone setting is gone, its `deadzone=` key ignored). A value is held (or slid) for the bar's slider, which sets
it as the finger moves (the game gets it at once); the panel's parameters are touch only, since the pad
listens on that screen. A long press on an input sets it back; "No changes" clears the layer shown, once
confirmed. What an input may map to fits its kind: a button, D-pad direction or stick direction to a pad
button (one of the list, or the next pressed on the pad), a key chord (typed on the keyboard: modifiers
latch, the next key completes it; Done takes the modifiers alone), a mouse button or a wheel step; a stick
to a stick, the pointer, four keys typed in turn (WASD) or the D-pad; a trigger to a trigger, or past its
threshold to anything a button maps to; the gyro to the pointer or a stick, always or while a chosen button
or trigger is held (which keeps its own mapping), off by default. Speeds go from 0 to 100 (files with the old steps 1-5 load as 0, 25, 50, 75,
100). An input without a binding does what it does on the pad. Only on the game's route: ClamShell's
screens, typing and the pad mouse see the plain pad. The pad thread applies a stick's or trigger's tuning
to its values, then maps the controller's state to what every output should be and sends what changed:
the game's virtual pad, keys and mouse buttons on the pad's virtual mouse (it has a keyboard's keys; held
while the button is, repeated by the compositor), the pointer; so leaving the game, losing the controller,
a keymap change or Back let go mid-press leaves nothing held, and chords press their modifiers first and
let them go last. **The gyro** is read from a motion sensor next to the controller (an evdev device with
INPUT_PROP_ACCELEROMETER and the gyro on ABS_RX/RY/RZ, as the kernel's controller drivers make: the one
paired with the controller by name or port, or the only one). Its offset at rest is learnt whenever the
device lies still for 1.5 s and taken off every sample, and on request ("Calibrate gyro" on the edit
screen: held still for a second; a notice says whether it took). To the pointer it is integrated (yaw to x,
pitch to y) with sub-pixel remainders kept, to a stick scaled and clamped. A binding's parameters:
acceleration (faster turns go further, up to three times at its "fastest at"), tightening (turns slower
than it are scaled down toward nothing, against tremor, without a hard edge), turning by tilting (roll
turns as well as yaw), where a stick starts (past a game's own stick deadzone), and "off while held"
(its hold turns it off instead of on, to lift and re-centre). The Thor's own sensor is still unknown: it
plugs in at the same place (`pad/motion.rs`).

Saved in `game-settings.txt` next to `stats.db`, applied at a game's launch and at once while it runs.
Frequency caps, cores and the charge limit need write access to cpufreq's `scaling_max_freq`, CPUs'
`online`, devfreq's `max_freq` and the battery's `charge_control_end_threshold` (a udev rule on the
device; on the desktop they're skipped, with one warning); they are written on a thread of their own, so
drawing never waits for a core going offline.

**Sysinfo** shows CPU, GPU, memory, temperatures, power, per-core load, and a running game's FPS and
frame time from MangoHud: games started with `clamshell --mangohud-env` in their environment log (overlay
invisible) to a folder in RAM that ClamShell follows. For a Steam game, as launch options: `env
$(clamshell --mangohud-env) %command%`; by hand: `env $(clamshell --mangohud-env) vkcube`.

**The controller** (any Xbox-style pad; `CLAMSHELL_GAMEPAD`: path or part of its name) is grabbed for
good; games see a virtual Xbox 360 pad that is always there. The **pad mouse** toggle (mouse icon in the
bar) routes the controller to a virtual mouse instead (left stick pointer, right stick scroll, A/RT left
click, B right, X middle, D-pad arrows) while the virtual pad shows a neutral controller. Otherwise the
pad goes where what is in front says: with **shelf in front, ClamShell owns it** (D-pad or left stick
move focus, repeating while held, across both screens; A acts; B cancels the editor; Y favourite; LB/RB
the previous or next tab on top, the bar's tabs below; View/Select moves focus to the other screen, back
to where it was there); with the on-screen keyboard open over an app it **types** (D-pad arrows, A Enter,
B Backspace); over any other app it drives the second screen; with a game in front, the game has it,
through the keymap in force (see Keymaps).
Outside a game, **View moves the pad between the second screen and what else wants it**: the pad mouse
when on, typing when the keyboard is open (on the second screen the D-pad then picks keys and A presses
them). With nothing else wanting it, View does nothing and the pad stays on the second screen; if it is
in use there when the keyboard comes up, it stays there too. In a game View is the game's (unless the pad
mouse is on). The virtual pad shows a neutral controller whenever the game doesn't have the pad, and
stays when the controller is unplugged. The **home button** (BTN_MODE) is ClamShell's, never the games':
from anything it goes to shelf, and from shelf back to what was in front before (the game, an app). (So
Steam's overlay doesn't get it.) Test with a fake pad: `cargo run --release -p clamshell-policy --example
fake_pad &` then `CLAMSHELL_GAMEPAD="clamshell fake pad" CLAMSHELL_PAD_MOUSE=1 cargo run --release`.

**Idle**: without input the screens dim (30 s before they go off), go off, and after longer the device
sleeps (times in the Battery tab). Input is read from the input devices themselves (all of them, without
grabbing any; the session user needs the `input` group), so the controller counts as much as touch; the
compositor's idle report (ext-idle-notify) is an extra where it has one, through which a video player that
asks to keep the screen on holds idle off. Any input brings the screens back; the tap or button that does
only wakes them. The screens go off through the compositor's output power protocol. Sleep happens only in
the device's own session (`CLAMSHELL_SESSION=device`, which the image's service sets: a desktop run never
suspends its host) and never while a game is in front; it goes through logind, suspend-then-hibernate
where the system can.

**The device's keys and `clamshell ctl`**: in the device's session, ClamShell takes the volume and
brightness keys of the device's small button devices (gpio-keys; never a full keyboard's, which are the
desktop's) and shows the new level under the bar for a moment. `clamshell ctl volume up|down|mute`,
`brightness up|down` and `screenshot` do the same from a script or a key binding (through the control pipe,
`$XDG_RUNTIME_DIR/clamshell-control`). A screenshot (the pad's Home with RB, or `ctl screenshot`, which the
image binds to a keyboard's Print) is both screens in one PNG in `~/Pictures/Screenshots`, taken by grim;
"Screenshot saved" shows under the bar. Home on its own acts when it is let go, so it can start a
combination: held with RB, a screenshot (a table of such system shortcuts, `pad/route.rs`); held, then
Back with a game in front, the game's Guide button while both are held (Steam's overlay; buttons pressed
meanwhile go to the game, for Steam's Guide chords); held with any other button, that button goes where it
would and letting Home go isn't Home. Back on its own is the keymap's Back layer in a game and back on
ClamShell's screens (closes the editor, goes back one keymap screen; on the edit screen it shows the other
layer). Back's key code is assumed to be KEY_BACK on the controller until the device test.

**Sleep**: the power button and the lid put the device to sleep through logind (the image's
`logind.conf.d/thor.conf`): suspend, then hibernate after 30 minutes (`sleep.conf.d/thor.conf`), or
suspend only when hibernate isn't possible. The swap file for hibernate (16 GiB) and the resume
parameters are made on the card's first boot.
ClamShell notices waking from the clocks (CLOCK_BOOTTIME runs during sleep, CLOCK_MONOTONIC doesn't) and
applies the settings in force again.

Licence: MIT.
