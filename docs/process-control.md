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

- Device session (`CLAMSHELL_SESSION=device`): ClamShell is the session and manages everything it can.
- Guest (nested in a desktop's session): ClamShell manages what it started. The desktop's programs (its
  tray icons, windows it didn't start) are shown on the dock at most, never moved or ended.

ClamShell needs a systemd user manager: every launch runs in a scope of its own.

On the device the session is sway through uwsm (`uwsm start -- sway` from the console's login): sway
runs as `wayland-wm@sway.service`; uwsm reaches `graphical-session.target` once sway has put its
environment in the user manager's (`uwsm finalize` in sway's config); ClamShell, mako and udiskie are
user services of that target, in uwsm's `session-graphical.slice`, started once it is reached and
stopped with it. As the session ends (sway exits or crashes, `uwsm stop`), uwsm stops the app slices
first, so every launch (in `app-graphical.slice`) ends before sway does; then the console's autologin
starts a new session.

## Who started what, and what ClamShell does with it

| Started by | Examples | Where it runs | ClamShell |
|---|---|---|---|
| ClamShell | apps, games, emulators, files opened, Steam | a scope per launch, in the entry's slice (`app-graphical-clamshell-run-<key>.slice`, in uwsm's app slice) | its own |
| Steam | its games, whether ClamShell or Steam's window started them | moved from Steam's scope into the game's own | its own, once moved |
| Anything else, a program with a window or a tray icon | a program from sway's config, another program's child, a desktop launcher's | moved from its scope or unit (an app scope, sway's own unit under uwsm, a login session's) into one of the entry's | its own, once moved (device session) |
| A systemd service | portals, the polkit agent, PipeWire, D-Bus activated services | its service | not touched: its windows are no entries |
| The session | sway, ClamShell, mako, udiskie | under uwsm, units in `session.slice` (in a desktop's session, its login scope) | the session's own: shown in the battery tab, not endable |
| Android | Waydroid's apps | the container (`lxc.payload.waydroid`) | paused and stopped through the Android runner |
| The system | NetworkManager, bluetoothd, sshd, the Waydroid container service | `system.slice` | not touched |

## Entries and how long they run

What runs is shown and controlled as entries (the dock's): one per program, each with a workspace.

- A launch of ClamShell's runs in a scope of its own (`systemd-run --user --scope`, through sway's exec
  so its first window opens on the entry's workspace; restarting ClamShell doesn't end it), in its
  entry's slice; an entry may run several (another tab each). An entry runs while systemd lists a scope
  of it, from the user manager's signals: its launch's scope appearing starts it, the last one gone ends
  it, its play recorded from its start to then (launches side by side are one play; a video's place read
  then, as mpv left it). The launcher only starts launches, and says so of one whose scope never came. A
  ClamShell started later finds the launches that run by their scopes, and records their play from the
  start time in the scopes' names.
- A launch that hands its app over to a program already running (the Android runner's, to the Waydroid
  session) and ends: the entry runs on by its windows, found by their app id (`waydroid.<package>`),
  until the last one closes (or none came within 15 s).
- A Steam game: Steam starts it under its launcher process (`reaper SteamLaunch AppId=<id>`, in Steam's
  scope). The launch's own scope holds a follower (`clamshell --follow-steam <id>`): it moves that
  launcher and what it started into the scope, and any later one for the game (a fresh Steam runs
  Proton's install scripts first under one launcher, then the game under another), as long as one runs
  or did within 10 s (the first within 2 minutes: Steam may start or update first); then it ends, and
  the scope with the game's last process. So the scope, and the entry, last as long as the game, also
  across a ClamShell restarted. A game started from Steam's own window is found by its window
  (`steam_app_<id>`; the app id says whose it is, though the process is in Steam's scope), which makes it
  a launch: its follower finds it running and moves it in the same way. Steam's Running state and its Stop are unaffected: Steam goes
  by its launcher process, not its cgroup.
- A program ClamShell didn't start is found by its tray icon, or by a window no entry has; anything with
  a window is on the dock. Its entry: the library's desktop entry named as the icon's id or the window's
  app id, else one of its own (`tray:…`, `window:<app id>`). Its windows go to its workspace (shown if
  one was in front). In the device session its process and those it started are moved into a scope of
  the entry's (systemd's StartTransientUnit with their PIDs, or AttachProcessesToUnit into one there
  already: the scopes are delegated, which a user manager requires for that); once systemd lists it, the
  entry is a launch like any other, its windows found by the scope. Not moved: a process a systemd
  service runs (the service would lose it), or one that also has another entry's windows (a browser
  showing a file opened from the Files tab: moving it would take the other's windows too); such an entry
  runs while its icon or a window is there, and Quit only closes its windows. A window a systemd service
  opened (a portal's file chooser, the polkit password prompt) is no entry: it belongs to the program that
  asked for it, and is gone in seconds.

## Ending

| Entry | Quit | Force kill |
|---|---|---|
| A launch (and anything moved into a scope of ours) | its windows asked to close (a "save changes?" window keeps it until answered); then, if it still runs, its slice stopped (SIGTERM, SIGKILL after systemd's timeout) | its slice killed |
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
(suspended, or paused hidden by the one before) is held as suspended, so a suspended game stays so. A
slice is thawed before a launch into it (systemd starts no scope in a frozen slice), and a scope whose
sway is gone has its slice thawed before it is stopped. Tried in the nested session: Foot suspended,
ClamShell restarted in the same sway, still frozen, resumed with its window.

Android's apps share one container, so the Android runner is paused as a whole (its `pause` and `resume`,
Waydroid's container service freezing and thawing the container, no root): set to Pauses hidden, once
none of its apps has been shown, or is starting, for the grace. Android's own suspend is turned off at
setup, so that this setting decides. Its windows survive being hidden through ClamShell's Wayland
filter (see android.md).

## Start with ClamShell

In an entry's dock menu (saved): once ClamShell knows what runs (systemd's first list of its scopes),
each such entry that doesn't run is launched on its own workspace, not shown. What a restarted ClamShell
finds running isn't started twice.

## What the user sees

- The dock: every running entry, with its tray menu if it has one, Quit and Force kill, and what it
  costs now (memory, CPU, GPU).
- The battery tab's list of what is using the machine: CPU, GPU and resident memory per group (an
  entry's launches; an Android app, by its package, from the container's processes; Android's own
  processes; another program by name; the system), idle groups holding 100 MB or more too. A row of a
  program that isn't the session's own (ClamShell, sway and what its config starts, PipeWire, the bus)
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
| frames (MangoHud's logs) | 2/s | none without a game |
| input watch | 2/s (a rescan every 5 s) | none |
| tray | 4/s | its signals only |

Left: the main thread (the events it is sent: the clock's tick every second, the metrics); metrics
(about 1.5 ms of CPU a reading, the GPU's load walking every process's DRM clients): every second while
the sysinfo or battery tab can be seen (open, ClamShell's view on the bottom screen, the screens on),
every five seconds otherwise, the graphs' history kept, coarser. Every tab's watch (Wi-Fi, Bluetooth,
audio, time) stops the same way while its tab can't be seen. zbus' executor threads (the `blocking`
crate): about 8 wakeups a second between them, no CPU, idle threads waking every 500 ms.

## Measured and tried

`tools/inventory.sh [SECONDS]` (read-only, on the device or in the VM): the running units, and per
cgroup its memory now and its CPU time over the window (30 s). In the VM (4 GiB, no GPU), idle, shelf
in front, the machine used 653 MiB of 3.9 GiB: ClamShell 89 MiB and 134 ms of CPU in 30 s, journald 33
MiB, the Waydroid container service 30 MiB, the login session (sway, mako) 19 MiB, PipeWire's three 16
MiB, the rest under 10 MiB each. With the Waydroid session up and Calculator open: 1855 MiB used, the
container 2.6 GiB (counting the page cache charged to it) in 89 processes. That inventory also found an
adb server started by hand, in sway's login session, listed nowhere, which ClamShell's Android runner
quietly depended on: since then the runner's adb server is its own, on a port of its own (5039).

On the desktop (AMD GPU, PipeWire, Steam), in the nested session:

- Frozen with systemctl for 10 s, a native Steam game (Super Meat Boy) froze in 37 ms and thawed in
  5 ms; its sound stream stayed open; it went on drawing. RetroArch (snes9x) the same.
- The Resident Evil 6 benchmark (Proton Experimental): all its processes in its own scope, also with
  Steam freshly started (its install scripts first); frozen 2 minutes behind shelf with no CPU used,
  drawing on once shown; with no setting (Pauses games) frozen 12 s after Home; stopped from Steam's own
  Stop while frozen behind Steam's window (Steam closed its window, ClamShell thawed it, it ended within
  2 s, Steam showed it stopped); started from Steam's window, moved into its own scope and ended by Quit.
- A program started through sway, not ClamShell (foot): on the dock, on its workspace, in front.

In the VM, as the device's session (`tools/vm/vm-checks.py`, the process control checks): a game
launched from shelf runs in a scope of its own and Quit ends it with one play recorded; a game behind
shelf is frozen after the grace and thawed when shown; foot started from an app scope and from sway's
exec (sway's unit under uwsm; the login session's scope before) is found and moved into a scope of ours; a tray program (a Python
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
