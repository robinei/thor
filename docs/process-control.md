# Process control: what runs, and who decides

A design note, not a description of what is built. It records the aim, what starts processes on the
Thor, the mechanisms available for each kind of process, and what has to be measured on the device
before any policy is chosen.

## Aim

On the Thor, ClamShell is the session, not a launcher on top of one. Whatever it can manage it should:
what starts, what keeps running, what is paused, what is ended. That control is only worth handing to
the user if it is

- complete: nothing the user would count as a program runs without ClamShell knowing it and showing it;
- reliable: Quit and Force kill end it, and a rule such as "not in the background" holds.

Linux programs, unlike Android apps, don't take part in this: none of them expects to be paused or
ended by the system and none saves its state when asked. What ClamShell can do to a program is
cooperative (close its windows, SIGTERM) or blunt (SIGKILL), and one thing is transparent: freezing it
(the cgroup freezer).

## Modes

- Device session (`CLAMSHELL_SESSION=device`): ClamShell owns the session and manages everything it
  can.
- Guest (nested in a desktop's session): ClamShell manages only what it started. The desktop's
  programs (its tray icons, its services) are the desktop's: shown at most, never adopted or ended.

Each control decides by the mode rather than by special cases.

## What starts processes

| Source | Examples | Where they run | ClamShell's |
|---|---|---|---|
| System services | NetworkManager, bluetoothd, sshd, waydroid-container | `system.slice` | No: the device's plumbing |
| User services | PipeWire, WirePlumber, portals, D-Bus activated services | `user@.service/session.slice`, `app.slice` | No |
| The login session | sway, and what sway's config starts: mako, udiskie | `session-N.scope` | No: plumbing |
| ClamShell's launches | apps, emulators, files opened, Steam (when a game starts it) | `clamshell-run-<key>.slice`, a scope per launch | Yes |
| Launchers inside launches | Steam's games, Lutris', a terminal's commands | the launcher's scope | Partly: counted as the launcher |
| Android | Waydroid's apps | `lxc.payload.waydroid`, outside both | No: Android's own |

Everything that escapes ClamShell's scopes today does so by being started by something else: sway's
config, the user manager (D-Bus activation, units started by scripts), a program already running
(`xdg-open` handing over), or Android.

## Mechanisms, per kind of process

- Native programs in ClamShell's scopes: everything systemd offers a unit. Stop (ends the slice), kill,
  freeze and thaw (FreezeUnit, ThawUnit), CPU weight and quota, memory limits, IO weight; and the
  accounting (memory.current, cpu.stat) to show what each costs.
- Launchers' children: a Steam game runs in Steam's scope (since Steam is a ClamShell launch), so it is
  counted, frozen or stopped together with Steam. systemd can move running processes into a unit of
  their own (AttachProcessesToUnit, for the user manager's scopes): a game Steam starts could get its own
  scope, found by Steam's launcher process (`reaper SteamLaunch AppId=<id>`), which ClamShell already
  watches. Not tried.
- Android: the container's processes are in a cgroup of their own outside systemd's trees. Android
  manages its apps itself (background limits, standby buckets, battery optimisation); ClamShell can only
  ask it, through adb (`am force-stop`, `cmd appops`, `am set-standby-bucket`), or freeze or stop the
  whole container.
- Programs found by their tray icon (started outside ClamShell): their processes, by process id; the
  same signals, no cgroup of their own.

## What the user sees

The dock lists every running entry: anything ClamShell runs is visible there and can be quit. On
Android a program may only run in the background with a notification saying so (a foreground
service); the dock plays that part here. Whether that is enough control, or whether rules are wanted
on top of it, is for use to show.

## Possible controls (none decided)

- Starts with ClamShell, per entry: Steam silently at session start (no cold start before the first
  game), the Waydroid session.
- In the background, per entry (the settings' levels: All, runner, item):
  - Ends: its scope stopped when its last window closes (Steam closing its window would end it);
  - Runs: as now;
  - Pauses: frozen while not in front, thawed when shown. A console's suspended games: switching
    between several is instant, and a paused one costs no CPU. "In front" needs defining with two
    screens (a program on the bottom screen is visible) and with sound (music playing in the background).
- The rest of what runs: a Tools page listing what is in no entry (user services, the login session's
  processes, Android), with what each costs, and Quit and Force kill where that is safe.

## A session as the VM runs it

`tools/inventory.sh [SECONDS]` (read-only, on the device or in the VM): the running units, and per
cgroup its memory now and its CPU time over the window (30 s). In the VM (4 GiB, no GPU), idle, shelf in front:

| Cgroup | Memory MiB | CPU ms / 30 s | Processes |
|---|---|---|---|
| clamshell.service | 89 | 134 | 1 |
| system.slice/systemd-journald | 33 | 0 | 1 |
| system.slice/waydroid-container | 30 | 0 | 1 |
| session-2.scope (sway, mako, a stray adb server) | 19 | 0 | 3 |
| NetworkManager | 14 | 1 | 1 |
| pipewire, pipewire-pulse, wireplumber | 16 | 11 | 3 |
| everything else | under 10 each | 0 to 3 | |

Machine: 653 MiB used of 3.9 GiB.

With the Waydroid session up and Calculator open:

| Cgroup | Memory MiB | CPU ms / 30 s | Processes |
|---|---|---|---|
| lxc.payload.waydroid (Android) | 2631 | 69 | 89 |
| clamshell.service | 79 | 175 | 1 |
| session-2.scope | 59 | 4 | 3 |
| waydroid-container | 37 | 0 | 2 |
| waydroid-session.service | 21 | 0 | 1 |

Machine: 1855 MiB used. The container's 2.6 GiB counts the page cache charged to it, not only Android's
own memory.

What it showed:

- An adb server started by hand during testing sat in sway's login session, listed nowhere: and
  ClamShell's Android runner used it, because the adb service unit it starts found the port taken and
  ended. One stray process, unseen, and the managed path quietly depending on it.
- ClamShell itself idles at about 5 ms of CPU a second (shelf in front, nothing changing). Worth finding
  out what wakes it, on the device.
- Android is a few hundred megabytes of its own at least, and nearly 90 processes, while one small app
  is open.

## To find out on the Thor

1. The same inventory in a normal session: with Steam, with the Waydroid session, with a game; memory,
   CPU, and wakeups (powertop) per cgroup.
2. Idle power with and without Steam and Waydroid running.
3. Freezing a game's scope and thawing it: two or three native games and an emulator, with Turnip and
   PipeWire. Sound during and after, the GPU after, the controller, online games' connections, how long
   thawing takes.
4. Steam with a game frozen, or moved into a scope of its own: whether it notices, times out, or loses
   the game (play time, cloud sync at exit).
5. Waydroid in the background: what Android keeps running, and freezing the container as a whole.
6. ClamShell's own wakeups when idle.
7. After some days of use: whether anything the dock doesn't show was wanted, or anything it shows
   wasn't.

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
(about 1.5 ms of CPU a reading, the GPU's load walking every process's DRM clients: every second while
the sysinfo or battery tab can be seen (open, ClamShell's view on the bottom screen, the screens on),
every five seconds otherwise, the graphs' history kept, coarser. Every tab's watch (Wi-Fi, Bluetooth,
audio, time) stops the same way while its tab can't be seen); zbus' executor threads (the `blocking`
crate: about 8 wakeups a second between them, no CPU, idle threads waking every 500 ms).

## Tried on the desktop (AMD GPU, PipeWire, Steam)

- A Steam game (Super Meat Boy, native) moved into a scope of its own while it ran (its launcher,
  `reaper`, and its processes, by AttachProcesses' kin: a transient scope given their PIDs): Steam didn't
  notice, and recorded the game's end as usual when the scope was stopped.
- Frozen (systemctl freeze, the cgroup freezer) for 10 s: frozen in 37 ms, thawed in 5 ms; its sound
  stream stayed open (not corked: PipeWire gets nothing meanwhile); the game went on after, drawing.
- ClamShell launching it (the nested session, Steam already running): the game ran in the launch's own
  scope, in the entry's slice; stopping that scope ended it, Steam recording the end.

Still to try: a game with online play, and all of it on the Thor (Freedreno, FEX).

- RetroArch (snes9x) frozen for 10 s the same way: alive after, its sound stream open, drawing on.
- In the background, live in the nested session: the SNES runner set to Pauses hidden, a game launched,
  hidden behind shelf (Home): its slice frozen after the grace, thawed as it was shown again (it went
  on), and Quit ended it.
- A Proton game (the Resident Evil 6 benchmark, Proton Experimental) on the desktop, in the nested
  session: frozen 2 minutes behind shelf, no CPU used meanwhile, drawing on once shown. Stopped from
  Steam's own Stop while frozen behind Steam's window: Steam closed its window, ClamShell thawed it, it
  ended within 2 s and Steam showed it stopped.

## Done

- The modes above: in a desktop's session ClamShell adopts nothing it didn't start (its tray programs
  stay the desktop's; an icon of ClamShell's own launch still gives its entry a menu).
- A Steam game ClamShell launches runs in the launch's own scope (moved there once Steam's launcher for
  it runs, and any later one for it as long as it runs: a fresh Steam may run Proton's install scripts
  first, under a launcher of their own, then the game): counted, frozen and ended apart from Steam. It
  counts as ended once no launcher of Steam's for it has run for 10 s. Steam's Stop and its Running
  state work as before (Steam finds the game by its launcher process, not by its cgroup).
- In the background, a setting (All, a runner, an item): Runs hidden (default), Pauses hidden (its slice
  frozen once it has windows and none has been shown for 3 s; thawed when shown, before Quit, Force kill
  or being shown, and when its windows are gone: one closed from outside, as Steam's Stop does, could
  otherwise not end; not paused while quitting), Quits closed (ended once it has had a window and has
  none left). A slice left frozen (by a ClamShell before, which a new one doesn't know paused it) is
  thawed when ClamShell starts and before a launch into it: systemd starts no scope in a frozen slice. Only launches of
  ClamShell's have a slice to freeze or stop; programs found by their windows or tray icons run on.
- A launch found running when ClamShell starts (started by a ClamShell before) has its play recorded
  when its scopes are gone, from the start time in its scope's name.
- Start with ClamShell, in an entry's dock menu (saved): once ClamShell knows what runs (systemd's
  first list of its scopes), each such entry that doesn't run is launched on its own workspace, not
  shown. What a restarted ClamShell finds running isn't started twice.
- The battery tab's list of what is using the machine: CPU, GPU and resident memory per group (an entry's
  launches, another program by name, the system); idle groups holding 100 MB or more listed too. A row
  of a program that isn't the session's own (ClamShell, sway and what its config starts, PipeWire, the
  bus) opens a menu: Quit and Force kill (an entry as from the dock; another program's processes
  signalled). An entry's dock menu shows what it costs.
- Android in the background: its apps share one container, frozen and thawed whole through Waydroid's
  container service (no root), by the Android runner's `pause` and `resume`; Pauses hidden set for the
  runner freezes it once none of its apps has been shown for the grace. Android also suspends itself once
  its display is off (Waydroid then freezes the container). Its windows are kept as they are hidden and shown by
  ClamShell's Wayland filter (see android.md).
- The Android runner's adb server is ClamShell's own, on a port of its own (5039), not whatever server
  is on adb's usual port.
