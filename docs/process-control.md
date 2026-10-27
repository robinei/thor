# Process control: what runs, and who decides

How ClamShell keeps track of what runs on the Thor, and what it does with it: starting, pausing and
ending programs, and showing what they cost. Then what was measured and tried, and what is still open.

## Aim

On the Thor ClamShell is the session, not a launcher on top of one. What it can manage it does: what
starts, what keeps running, what is paused, what is ended. That control is worth handing to the user only
if it is

- complete: nothing the user would count as a program runs without ClamShell knowing it and showing it;
- reliable: Quit and Force kill end it, and a rule such as "pause in the background" holds.

Linux programs, unlike Android apps, take no part in this: none expects to be paused or ended by the
system, none saves its state when asked. What ClamShell can do to a program is cooperative (close its
windows, SIGTERM), blunt (SIGKILL), or transparent: freezing it (the cgroup freezer, systemd's
FreezeUnit and ThawUnit).

## Modes

- Device session (`clamshell --drm`, on the screens): ClamShell is the session and manages everything
  it can.
- Guest (nested in a desktop's session, `clamshell --nested`; `tools/nested-session.sh` runs it as the
  user unit `clamshell-nested.service`): stopping the unit stops what ClamShell launched (its slices,
  under `app-graphical-clamshell.slice`, by the unit's ExecStopPost). A window in ClamShell's compositor
  is that session's, so it is found and its program adopted as on the device. What stays the device's
  own: the tray (on a desktop its icons are the desktop's programs': shown, never moved or ended), sleep,
  and the hardware keys.

ClamShell needs a systemd user manager: every launch is a service of its own.

On the device the session is ClamShell's compositor as the user service `clamshell.service` (`clamshell
--drm`, in `session.slice`). The first console logs in by itself (getty@tty1's autologin), and the login's
profile runs `thor-session` on tty1 unless the service is already active. `thor-session` gives the user
manager and the bus the login's environment (`dbus-update-activation-environment --systemd --all`; its
`XDG_SESSION_ID` is how ClamShell's libseat takes the seat) and starts the service, waiting on it through its
restarts (systemd's after a failure, or by hand) until it has stopped with nothing queued. The service is
`Type=notify` with a 30 s watchdog and `Restart=on-failure`: ClamShell says it is ready once it listens and
has given the user manager and the bus `WAYLAND_DISPLAY` and `DISPLAY`, which brings up
`graphical-session.target` (bound to the service) and udiskie with it. As the session ends, `thor-session`
takes the displays out of the user manager's environment and stops `app-graphical.slice` (every launch) and
`background-graphical.slice`; then the profile logs out and the autologin starts a new session. The
session is named `ClamShell` (`XDG_CURRENT_DESKTOP`; xdg-desktop-portal takes GTK's portals,
`clamshell-portals.conf`). Under a game that takes every core the session keeps up: `session.slice` has four
times `app.slice`'s CPU weight, and ClamShell asks rtkit for its compositor thread at nice -15 (not
real-time: the kernel kills a real-time thread that runs past `RLIMIT_RTTIME`, and the compositor draws the
screens on the CPU). `systemctl --user status clamshell` shows what runs (ClamShell's `STATUS=`: entries,
windows, Xwayland), and its journal lines carry their priority (`journalctl --user -u clamshell -p err`). Unless it
can't go on: ended 3 times within 2 minutes, the session isn't started again; ClamShell failing 4 times within
a minute (watchdog restarts too), systemd gives up. Either way the console stays on tty1, saying why (the last
lines of ClamShell's journal) and how to go on (the Thor has no keyboard: holding the power button turns it
off, and turned on it tries again; with a keyboard, `exit` does; ssh works). A deliberate restart of ClamShell
(maintenance, the VM checks) clears its count first (`systemctl --user reset-failed`). ClamShell is the
compositor, so a restart closes the session's apps. Tried in the VM when the session was sway under uwsm:
ClamShell made to exit at once, and sway made to fail at start; each ended on the console saying why, and came
back once put right.

## Who started what, and what ClamShell does with it

| Started by | Examples | Where it runs | ClamShell |
|---|---|---|---|
| ClamShell | apps, games, emulators, files opened, Steam | a service per launch, in the entry's slice (`app-graphical-clamshell-run-<key>.slice`, in `app-graphical.slice`) | its own |
| Steam | its games, whether ClamShell or Steam's window started them | moved from Steam's scope into the game's own | its own, once moved |
| Anything else, a program with a window or a tray icon | another program's child, a desktop launcher's, one started from a terminal | moved from its scope or unit (an app scope, a login session's) into one of the entry's | its own, once moved (device session) |
| A systemd service | portals, the polkit agent, PipeWire, D-Bus activated services | its service | not touched: its windows are no entries |
| The session | ClamShell, udiskie | units in `session.slice` (`clamshell.service`; nested, `clamshell-nested.service`) | the session's own: shown in the battery tab, not endable |
| Android | Waydroid's apps | the container (`lxc.payload.waydroid`) | paused and stopped through the Android runner |
| The system | NetworkManager, bluetoothd, sshd, the Waydroid container service | `system.slice` | not touched |

## Entries and how long they run

What runs is shown and controlled as entries (the dock's): one per program, each with its group of windows.

- A launch of ClamShell's is a transient service of the user manager (`systemd-run --user`,
  `clamshell-run-<key>-<secs>-<n>.service`): started once its program runs (`Type=exec`), running as long
  as any of its processes does (`ExitType=cgroup`, as a launcher script's game outlives it), its output in
  the journal as the unit's (`journalctl --user -u <unit>`), in its entry's slice. Restarting ClamShell
  doesn't end it, though a program with a window ends with its compositor. Its windows, found by the unit,
  are the entry's group; an entry may run several (another tab each). An entry runs while systemd
  lists a unit of it, from the user manager's signals: its launch's unit appearing starts it, the last one
  gone ends it, its play recorded from its start to then (launches side by side are one play; a video's
  place read then, as mpv left it). The launcher only starts launches, and says so of one systemd didn't
  start (its program couldn't run). A ClamShell started later finds the launches that run by their units,
  and records their play from the start time in the units' names.
- A launch that hands its app over to a program already running (the Android runner's, to the Waydroid
  session) and ends: the entry runs on by its windows, found by their app id (`waydroid.<package>`),
  until the last one closes (or none came within 15 s).
- A Steam game: Steam starts it under its launcher process (`reaper SteamLaunch AppId=<id>`, in Steam's
  unit). The launch's own service holds a follower (`clamshell --follow-steam <id>`): it moves that
  launcher and what it started into the service, and any later one for the game (a fresh Steam runs
  Proton's install scripts first under one launcher, then the game under another), as long as one runs
  or did within 10 s (the first within 2 minutes: Steam may start or update first); then it ends, and
  the service with the game's last process. So the service, and the entry, last as long as the game, also
  across a ClamShell restarted. A game started from Steam's own window is found by its window
  (`steam_app_<id>`; the app id says whose it is, though the process is in Steam's unit), which makes it
  a launch: its follower finds it running and moves it in the same way. Steam's Running state and its Stop are unaffected: Steam goes
  by its launcher process, not its cgroup.
- A Heroic or Lutris game, launched so that the launcher isn't seen and runs as an entry of its own. A
  Heroic game is launched as a launch of Heroic's entry, in the background: Heroic's command with
  `--no-gui --no-sandbox` and the game's URL, which starts Heroic without its window if it doesn't run and
  hands it the game, which runs in Heroic's unit. A Lutris game's shortcut runs Lutris without its window,
  which runs the game in the launch's unit and ends with it; when Lutris already runs, it takes the game,
  which runs in Lutris's unit, and the launch ends. The game's windows are known by the mark the launcher
  gives its processes (see runners.md), waited for up to 2 minutes, and the entry runs by them as a
  handed-over app does. A game started in the launcher's own window starts its entry the same way.
- A program ClamShell didn't start is found by its tray icon, or by a window no entry has; anything with
  a window is on the dock. Its entry: the library's desktop entry named as the icon's id or the window's
  app id, or whose `StartupWMClass` that is, else one of its own (`tray:…`, `window:<app id>`). Its windows are its group (shown if
  one was in front). In the device session its process and those it started are moved into a scope of
  the entry's, named as a launch's service is but `.scope`, since a service can't take in processes that
  already run (systemd's StartTransientUnit with their PIDs, or AttachProcessesToUnit into one there
  already: the units are delegated, which a user manager requires for that); once systemd lists it, the
  entry is a launch like any other, its windows found by the scope. Not moved: a process a systemd
  service runs (the service would lose it), or one that also has another entry's windows (a browser
  showing a file opened from the Files tab: moving it would take the other's windows too); such an entry
  runs while its icon or a window is there, and Quit only closes its windows. A window a systemd service
  opened (a portal's file chooser, the polkit password prompt) is no entry: it belongs to the program that
  asked for it, and is gone in seconds.

## Ending

| Entry | Quit | Force kill |
|---|---|---|
| A launch (and anything moved into a unit of ours) | its windows asked to close (a "save changes?" window keeps it until answered); then, if it still runs, its slice stopped (SIGTERM, SIGKILL after systemd's timeout) | its slice killed |
| Handed over (Android) | its windows closed; the runner's `stop` once they are gone (`am force-stop`, its tasks removed) | the runner's `stop` at once |
| Found, not moved | its windows closed; then its process asked to end (SIGTERM), in the device session | its process killed (device session) |
| Found, sharing a process | its windows closed | nothing |

An entry is thawed before anything is asked of it (closed, quit, killed, shown).

## In the background

A setting with the settings' levels (All, a runner, an item): what an entry does while none of its
windows is shown.

- Pauses games (the default): Pauses hidden for a game, Runs hidden for the rest. A game is an entry that
  is no app, film, file or group: a desktop file in the Game category is an app if it is also an
  emulator, package manager, network program or utility (Steam, Lutris, Bottles), or one of a short list
  of launchers and game tools that say only Game (Heroic, Faugus, itch, GOverlay and the like); their
  games are entries of their own. Resolved per entry after the levels, so All set to Runs hidden runs
  games too. As consoles do; one that minds (online play) is set to run.
- Runs hidden.
- Pauses hidden: its slice frozen once it has windows and none has been shown for 10 s; thawed when one
  is shown, or before it is shown, closed, quit or killed. Not while it is quitting (frozen, it couldn't
  end), nor once its windows are gone (closed from outside, as Steam's Stop does: thawed, to end). A
  freeze waits while the slice works without drawing (over half a core, its processes' GPU under 5 %,
  looked at for 2 s at a time): loading or compiling shaders, which the user switched away to wait for;
  a game hidden but drawing on uses the GPU and is frozen. Showing it again cancels a waiting freeze.
- Quits closed: ended once it has had a window and has none left.

Suspend (an entry's dock menu, for one in a slice of ours): its slice frozen now, whatever its setting,
its windows hidden; the tile says Suspended. It stays frozen until shown (Resume in its menu, or a tap
on its tile), quit or killed.

A ClamShell started again (an update, a crash) leaves our slices as it finds them: one found frozen
(suspended, or paused hidden by the one before) is held as suspended, so a suspended game stays so. A slice is
thawed before a launch into it (systemd starts no unit in a frozen slice). Tried in the nested session when it ran
under sway (before 2026-10-26): Foot suspended, ClamShell restarted in the same sway, still frozen, resumed
with its window.

Android's apps share one container, so the Android runner is paused as a whole (its `pause` and `resume`,
Waydroid's container service freezing and thawing the container, no root): set to Pauses hidden, once
none of its apps has been shown, or is starting, for the grace. Android's own suspend is turned off at
setup, so that this setting decides. Its windows survive being hidden through ClamShell's Wayland
filter (see android.md).

## Start with ClamShell

In an entry's dock menu (saved): once ClamShell knows what runs (systemd's first list of its units),
each such entry that doesn't run is launched, not shown. What a restarted ClamShell
finds running isn't started twice.

## What the user sees

- The dock: every running entry, with its tray menu if it has one, Quit and Force kill, and what it
  costs now (memory, CPU, GPU).
- The battery tab's list of what is using the machine: CPU, GPU and resident memory per group (an
  entry's launches; an Android app, by its package, from the container's processes; Android's own
  processes; another program by name; the system), idle groups holding 100 MB or more too. A row of a
  program that isn't the session's own (ClamShell, udiskie, PipeWire, the bus: `session.slice`)
  has Quit and Force kill. The GPU time of another user's processes (Android's, the system's) can't be
  read: a dash.

## ClamShell's own wakeups

Measured in the VM, idle, shelf in front, per thread over 20 s (voluntary context switches): about 40 a
second at first. Threads that wait on files (a device, inotify) couldn't also wait on their command
channel, so they woke on a timer to look at it; and devices and mounts were looked for on timers.
Now a worker that waits on files has a bell (an eventfd its commands ring, `worker::Bell`), mounts are
watched through /proc/self/mounts (POLLPRI) and devices through inotify on /dev/input:

| Thread | Before | Now |
|---|---|---|
| pad (no controller) | 10/s (a rescan every second) | none; a controller plugged in is grabbed in about 0.1 s |
| browse | 2.5/s, 0.5 ms CPU/s (mounts and the folder every 400 ms) | none |
| frames (MangoHud's logs) | 2/s | none without a game; gone since (the compositor measures frames) |
| input watch | 2/s (a rescan every 5 s) | none |
| tray | 4/s | its signals only |

Left: the main thread (the events it is sent: the clock's tick every second, the metrics); metrics
(about 1.5 ms of CPU a reading, the GPU's load walking every process's DRM clients): every second while
the sysinfo or battery tab can be seen (open, ClamShell's view on the bottom screen, the screens on),
every five seconds otherwise, the graphs' history kept, coarser. Every tab's watch (Wi-Fi, Bluetooth,
audio, time) stops the same way while its tab can't be seen. D-Bus is libdbus without async: each thread
that talks to a service has its own connection and waits on it itself (before, zbus' executor threads
woke about 8 times a second between them).

## Measured and tried

`tools/inventory.sh [SECONDS]` (read-only, on the device or in the VM): the running units, and per
cgroup its memory now and its CPU time over the window (30 s). In the VM (4 GiB, no GPU), idle, shelf
in front, the machine used 653 MiB (measured when the session was sway under uwsm, before 2026-10-26) of 3.9 GiB: ClamShell 89 MiB and 134 ms of CPU in 30 s, journald 33
MiB, the Waydroid container service 30 MiB, the login session (sway and the mako notification daemon, both gone since) 19 MiB, PipeWire's three 16
MiB, the rest under 10 MiB each. With the Waydroid session up and Calculator open: 1855 MiB used, the
container 2.6 GiB (counting the page cache charged to it) in 89 processes. That inventory also found an
adb server started by hand, in sway's login session, listed nowhere, which ClamShell's Android runner
quietly depended on: since then the runner's adb server is its own, on a port of its own (5039).

On the desktop (AMD GPU, PipeWire, Steam), in the nested session as it was under sway (before 2026-10-26):

- Frozen with systemctl for 10 s, a native Steam game (Super Meat Boy) froze in 37 ms and thawed in
  5 ms; its sound stream stayed open; it went on drawing. RetroArch (snes9x) the same.
- The Resident Evil 6 benchmark (Proton Experimental): all its processes in its own scope, also with
  Steam freshly started (its install scripts first); frozen 2 minutes behind shelf with no CPU used,
  drawing on once shown; with no setting (Pauses games) frozen 12 s after Home; stopped from Steam's own
  Stop while frozen behind Steam's window (Steam closed its window, ClamShell thawed it, it ended within
  2 s, Steam showed it stopped); started from Steam's window, moved into its own scope and ended by Quit.
- A program started through sway, not ClamShell (foot): on the dock, in its own group, in front.

In the VM, as the device's session (`tools/vm/vm-checks.py`, the process control checks): a game
launched from shelf runs as a service of its own and Quit ends it with one play recorded; a game behind
shelf is frozen after the grace and thawed when shown; foot started from an app scope and from a login
session (as over ssh) is found and moved into a scope of ours; a tray program (a Python
StatusNotifierItem) is moved too; the clipboard's text outlives the program that set it.

## Open

On the Thor (see thor-gameplan.md's device checks):

1. The inventory in a normal session (with Steam, with the Waydroid session, with a game): memory,
   CPU, and wakeups (powertop) per cgroup; idle power with and without Steam and Waydroid.
2. Freezing on Freedreno and through FEX: native and Proton games frozen for minutes, sound, the
   controller, the GPU after; whether a game hidden behind shelf still draws; whether the thresholds tell
   loading and shader compiling from drawing; a frozen game's memory going to zram.
3. A game with online play frozen: whether it is dropped (then it is set to run).
4. ClamShell's own wakeups when idle.
5. After some days of use: whether anything the dock doesn't show was wanted, or anything it shows
   wasn't.
